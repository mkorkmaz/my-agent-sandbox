use serde::{de::DeserializeOwned, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT_WRITE: AtomicU64 = AtomicU64::new(1);
pub(crate) mod data_location;
mod publication;
pub(crate) mod records_watch;
mod schema;
use publication::{LinkPublication, TemporaryRecord};
pub use schema::RecordSchema;

#[cfg(test)]
mod tests;

// Keep this separate from the atomically replaced data file. Closing the descriptor releases
// the advisory lock even when the process exits during a transaction.
pub struct FileLock {
    _file: File,
}

pub fn lock(file: &Path) -> Result<FileLock, String> {
    let directory = file.parent().ok_or("Invalid storage location.")?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    lock_existing(file)
}

fn lock_existing(file: &Path) -> Result<FileLock, String> {
    let mut name = file.as_os_str().to_os_string();
    name.push(".lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(name)
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(FileLock { _file: file });
        }
        let error = std::io::Error::last_os_error();
        if !matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
        ) {
            return Err(error.to_string());
        }
        if Instant::now() >= deadline {
            return Err(
                "Another Tugboat process is updating this record. Try again shortly.".into(),
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub fn read<T: DeserializeOwned>(file: &Path) -> Result<Option<T>, String> {
    let bytes = match fs::read(file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "Could not read the saved settings. Check the file.".into())
}

pub fn read_regular_bytes(file: &Path) -> Result<Option<Vec<u8>>, String> {
    let mut input = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(file)
    {
        Ok(input) => input,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if !input
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("Saved data is not a regular file.".into());
    }
    let mut bytes = Vec::new();
    input
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(Some(bytes))
}

pub fn v1_backup_path(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_os_string();
    name.push(".v1.backup");
    PathBuf::from(name)
}

pub fn upgrade_record<T: Serialize>(
    file: &Path,
    records: &crate::storage::data_location::RecordsDirectory,
    decode: impl Fn(&[u8]) -> Result<RecordSchema<T>, String>,
) -> Result<bool, String> {
    let directory = file.parent().ok_or("Invalid storage location.")?;
    records.verify_at(directory)?;
    let Some(bytes) = read_regular_bytes(file)? else {
        return Ok(false);
    };
    if matches!(decode(&bytes)?, RecordSchema::Current(_)) {
        return Ok(false);
    }
    let _lock = lock_existing(file)?;
    records.verify_at(directory)?;
    let bytes = read_regular_bytes(file)?.ok_or("The original record is unavailable.")?;
    let RecordSchema::Upgrade(upgraded) = decode(&bytes)? else {
        return Ok(false);
    };
    records.verify_at(directory)?;
    immutable_v1_backup(file)?;
    records.verify_at(directory)?;
    if read_regular_bytes(file)?.as_deref() != Some(bytes.as_slice()) {
        return Err(
            "The original record changed during migration. Its contents were preserved.".into(),
        );
    }
    let serialized = serde_json::to_vec_pretty(&upgraded).map_err(|error| error.to_string())?;
    write_bytes_existing(file, &serialized)?;
    Ok(true)
}

fn regular_input(file: &Path) -> Result<Option<File>, String> {
    let input = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(file)
    {
        Ok(input) => input,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if !input
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("Saved data is not a regular file.".into());
    }
    Ok(Some(input))
}

fn equal_inputs(first: &mut File, second: &mut File) -> Result<bool, String> {
    let mut first_buffer = [0u8; 65_536];
    let mut second_buffer = [0u8; 65_536];
    loop {
        let count = first
            .read(&mut first_buffer)
            .map_err(|error| error.to_string())?;
        match second.read_exact(&mut second_buffer[..count]) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(false),
            Err(error) => return Err(error.to_string()),
        }
        if first_buffer[..count] != second_buffer[..count] {
            return Ok(false);
        }
        if count == 0 {
            return second
                .read(&mut second_buffer[..1])
                .map(|count| count == 0)
                .map_err(|error| error.to_string());
        }
    }
}

fn backup_matches(first: &Path, second: &Path) -> Result<bool, String> {
    let mut first = regular_input(first)?.ok_or("The original saved record is unavailable.")?;
    let Some(mut second) = regular_input(second)? else {
        return Ok(false);
    };
    equal_inputs(&mut first, &mut second)
}

// The caller holds the ledger lock. A hard link publishes the synced temporary
// snapshot atomically without replacing an existing backup.
pub fn immutable_v1_backup(file: &Path) -> Result<(), String> {
    let mut input = regular_input(file)?.ok_or("The original saved record is unavailable.")?;
    let backup = v1_backup_path(file);
    if let Some(mut existing) = regular_input(&backup)? {
        if !equal_inputs(&mut input, &mut existing)? {
            return Err(
                "The immutable v1 backup differs from this record. Both files were preserved."
                    .into(),
            );
        }
        return Ok(());
    }
    let directory = file.parent().ok_or("Invalid storage location.")?;
    let temporary = backup.with_extension(format!(
        "backup-tmp-{}-{}",
        std::process::id(),
        NEXT_WRITE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut staged = TemporaryRecord::new(temporary.clone());
    (|| {
        let mut output = staged.create()?;
        let mut buffer = [0u8; 65_536];
        loop {
            let count = input.read(&mut buffer).map_err(|error| error.to_string())?;
            if count == 0 {
                break;
            }
            output
                .write_all(&buffer[..count])
                .map_err(|error| error.to_string())?;
        }
        staged.sync_file(&output)?;
        if !backup_matches(file, &temporary)? {
            return Err(
                "The original saved record changed during its backup. Both files were preserved."
                    .into(),
            );
        }
        match staged.link(&backup)? {
            LinkPublication::Created => {}
            LinkPublication::Existing => {
                if !backup_matches(&temporary, &backup)? {
                    return Err("The immutable v1 backup differs from this record. Both files were preserved.".into());
                }
            }
        }
        staged.remove()?;
        File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())
    })()
}

pub fn write(file: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    write_bytes(file, &bytes)
}

pub fn write_bytes(file: &Path, bytes: &[u8]) -> Result<(), String> {
    let directory = file.parent().ok_or("Invalid storage location.")?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    write_bytes_existing(file, bytes)
}

pub(crate) fn write_bytes_existing(file: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = file.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        NEXT_WRITE.fetch_add(1, Ordering::Relaxed)
    ));
    write_staged_bytes(file, bytes, temporary)
}

fn write_staged_bytes(file: &Path, bytes: &[u8], temporary: PathBuf) -> Result<(), String> {
    let directory = file.parent().ok_or("Invalid storage location.")?;
    let mut staged = TemporaryRecord::new(temporary);
    let mut output = staged.create()?;
    output.write_all(bytes).map_err(|error| error.to_string())?;
    staged.sync_file(&output)?;
    staged.replace(file)?;
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|error| error.to_string())
}
