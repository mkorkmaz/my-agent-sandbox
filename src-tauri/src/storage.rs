use serde::{de::DeserializeOwned, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT_WRITE: AtomicU64 = AtomicU64::new(1);

// Keep this separate from the atomically replaced data file. Closing the descriptor releases
// the advisory lock even when the process exits during a transaction.
pub struct FileLock {
    _file: File,
}

pub fn lock(file: &Path) -> Result<FileLock, String> {
    let directory = file.parent().ok_or("Invalid storage location.")?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
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
        .custom_flags(libc::O_NOFOLLOW)
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

// The caller holds the ledger lock. A hard link publishes the synced temporary
// snapshot atomically without replacing an existing backup.
pub fn immutable_v1_backup(file: &Path) -> Result<(), String> {
    let bytes = read_regular_bytes(file)?.ok_or("The original usage ledger is unavailable.")?;
    let backup = v1_backup_path(file);
    if let Some(existing) = read_regular_bytes(&backup)? {
        if existing != bytes {
            return Err("The immutable v1 ledger backup differs from this ledger. Both files were preserved.".into());
        }
        return Ok(());
    }
    let directory = file.parent().ok_or("Invalid storage location.")?;
    let temporary = backup.with_extension(format!(
        "backup-tmp-{}-{}",
        std::process::id(),
        NEXT_WRITE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        output
            .write_all(&bytes)
            .and_then(|_| output.sync_all())
            .map_err(|error| error.to_string())?;
        match fs::hard_link(&temporary, &backup) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if read_regular_bytes(&backup)?.as_deref() != Some(bytes.as_slice()) {
                    return Err("The immutable v1 ledger backup differs from this ledger. Both files were preserved.".into());
                }
            }
            Err(error) => return Err(error.to_string()),
        }
        fs::remove_file(&temporary).map_err(|error| error.to_string())?;
        File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub fn write(file: &Path, value: &impl Serialize) -> Result<(), String> {
    let directory = file.parent().ok_or("Invalid storage location.")?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let temporary = file.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        NEXT_WRITE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
        output
            .write_all(&bytes)
            .map_err(|error| error.to_string())?;
        output.sync_all().map_err(|error| error.to_string())?;
        fs::rename(&temporary, file).map_err(|error| error.to_string())?;
        File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
