use super::state::{Change, RootEvidence};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, DirBuilder, File, OpenOptions},
    io::{Error, Read, Seek, SeekFrom},
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
        },
    },
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

const UPPER_SIZE: u64 = 16 * 1024 * 1024 * 1024;
const READ_LIMIT: u64 = UPPER_SIZE;
const UNVERIFIED: &str =
    "The writable guest system disk could not be verified. Keep this guest and retry.";

pub(super) fn evidence(upper: &Path) -> Result<RootEvidence, String> {
    if fs::canonicalize(upper).ok().as_deref() != Some(upper) {
        return Err(UNVERIFIED.into());
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&upper)
        .map_err(|_| UNVERIFIED)?;
    let metadata = file.metadata().map_err(|_| UNVERIFIED)?;
    if !metadata.is_file()
        || metadata.len() != UPPER_SIZE
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
    {
        return Err(UNVERIFIED.into());
    }
    let fingerprint = digest(&file, metadata.len()).map_err(|_| UNVERIFIED)?;
    let after = file.metadata().map_err(|_| UNVERIFIED)?;
    if (metadata.mtime(), metadata.mtime_nsec(), metadata.len())
        != (after.mtime(), after.mtime_nsec(), after.len())
    {
        return Err(UNVERIFIED.into());
    }
    Ok(RootEvidence {
        disk: (&metadata).into(),
        fingerprint,
    })
}

fn offset(file: &File, position: u64, whence: i32) -> Result<Option<u64>, Error> {
    let result = unsafe { libc::lseek(file.as_raw_fd(), position as i64, whence) };
    if result >= 0 {
        return Ok(Some(result as u64));
    }
    let error = Error::last_os_error();
    if error.raw_os_error() == Some(libc::ENXIO) {
        return Ok(None);
    }
    Err(error)
}

fn digest(mut file: &File, size: u64) -> Result<String, Error> {
    let mut digest = Sha256::new();
    digest.update(size.to_le_bytes());
    let mut position = 0;
    let mut read: u64 = 0;
    let mut buffer = [0; 64 * 1024];
    while let Some(start) = offset(file, position, libc::SEEK_DATA)? {
        let end = offset(file, start, libc::SEEK_HOLE)?.unwrap_or(size);
        if end > size || end <= start || start < position || read + end - start > READ_LIMIT {
            return Err(Error::other("Unsupported sparse disk extent."));
        }
        digest.update(start.to_le_bytes());
        digest.update(end.to_le_bytes());
        file.seek(SeekFrom::Start(start))?;
        let mut remaining = end - start;
        while remaining > 0 {
            let count = remaining.min(buffer.len() as u64) as usize;
            file.read_exact(&mut buffer[..count])?;
            digest.update(&buffer[..count]);
            remaining -= count as u64;
        }
        read += end - start;
        position = end;
        if position == size {
            break;
        }
    }
    Ok(crate::hash::hex(digest.finalize()))
}

const RECOVERY_DIRECTORY: &str = "tugboat-image-recovery";

pub(super) fn backup_path(store: &Path, sandbox: &str) -> PathBuf {
    store
        .join(RECOVERY_DIRECTORY)
        .join(sandbox)
        .join("original.ext4")
}

fn private_directory(path: &Path) -> Result<(), String> {
    match DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(disk_error(error)),
    }
    let metadata = fs::symlink_metadata(path).map_err(disk_error)?;
    if !metadata.is_dir()
        || metadata.mode() & 0o077 != 0
        || metadata.uid() != unsafe { libc::geteuid() }
        || fs::canonicalize(path).ok().as_deref() != Some(path)
    {
        return Err(UNVERIFIED.into());
    }
    Ok(())
}

pub(super) fn disk_error(error: Error) -> String {
    if error.raw_os_error() == Some(libc::ENOSPC) {
        return "The image operation needs more free disk space.".into();
    }
    format!("Could not protect the guest system disk. {error}")
}

pub(super) fn backup(
    upper: &Path,
    path: &Path,
    expected: &RootEvidence,
) -> Result<RootEvidence, String> {
    let directory = path.parent().ok_or(UNVERIFIED)?;
    private_directory(directory.parent().ok_or(UNVERIFIED)?)?;
    private_directory(directory)?;
    if evidence(upper)? != *expected {
        return Err("The stopped guest changed after review. Check the guest image again.".into());
    }
    if path
        .symlink_metadata()
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    {
        clone_disk(upper, path)?;
    }
    let observed = evidence(path)?;
    if observed.fingerprint != expected.fingerprint {
        return Err(
            "The system disk backup differs from the reviewed guest. The source was preserved."
                .into(),
        );
    }
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(disk_error)?;
    fs::File::open(directory)
        .and_then(|parent| parent.sync_all())
        .map_err(disk_error)?;
    Ok(observed)
}

#[cfg(target_os = "macos")]
pub(super) fn clone_disk(source: &Path, target: &Path) -> Result<(), String> {
    let source = std::ffi::CString::new(source.as_os_str().as_bytes()).map_err(|_| UNVERIFIED)?;
    let target = std::ffi::CString::new(target.as_os_str().as_bytes()).map_err(|_| UNVERIFIED)?;
    const CLONE_NOFOLLOW: u32 = 0x01; // Darwin clonefile(2).
    if unsafe { libc::clonefile(source.as_ptr(), target.as_ptr(), CLONE_NOFOLLOW) } != 0 {
        return Err(disk_error(Error::last_os_error()));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub(super) fn clone_disk(_: &Path, _: &Path) -> Result<(), String> {
    Err("Guest image updates require macOS.".into())
}

pub(super) fn clear_backup(path: &Path, expected: &RootEvidence) -> Result<(), String> {
    if path
        .symlink_metadata()
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    {
        return Ok(());
    }
    if evidence(path)? != *expected {
        return Err(
            "The system disk backup changed after review. Its files were preserved.".into(),
        );
    }
    fs::remove_file(path).map_err(disk_error)?;
    Ok(())
}

pub(super) fn disk_query(
    debugfs: &Path,
    upper: &Path,
    environment: &BTreeMap<String, String>,
    command: &str,
    timeout: Duration,
) -> Result<String, String> {
    let path = upper.to_str().ok_or(UNVERIFIED)?;
    let arguments = vec!["-R".into(), command.into(), path.into()];
    let result = crate::platform::command::run(
        debugfs,
        &arguments,
        None,
        environment,
        timeout,
        &AtomicBool::new(false),
    )?;
    if result.exit_code != Some(0)
        || result.timed_out
        || result.cancelled
        || result.output_exceeded
        || result
            .stderr
            .lines()
            .any(|line| !line.is_empty() && !line.starts_with("debugfs "))
    {
        return Err("Could not read the stopped guest file inventory.".into());
    }
    Ok(result.stdout)
}

pub(super) fn inventory(
    debugfs: &Path,
    replay: &super::replay::Disk,
    environment: &BTreeMap<String, String>,
) -> Result<Vec<Change>, String> {
    let start = Instant::now();
    enumerate(|command| {
        let timeout = Duration::from_secs(40)
            .checked_sub(start.elapsed())
            .ok_or("The guest system file inventory timed out.")?;
        disk_query(debugfs, replay.path(), environment, command, timeout)
    })
}

fn enumerate(mut read: impl FnMut(&str) -> Result<String, String>) -> Result<Vec<Change>, String> {
    let mut directories = vec![(None, String::new(), 0)];
    let mut visited = HashSet::new();
    let mut changes = Vec::new();
    while let Some((inode, prefix, depth)) = directories.pop() {
        if depth > 32 || inode.is_some_and(|inode| !visited.insert(inode)) || visited.len() > 512 {
            return Err("The guest system file inventory exceeds its safe traversal limit.".into());
        }
        let arguments =
            inode.map_or_else(|| "ls -p /upper".into(), |inode| format!("ls -p <{inode}>"));
        let listing = read(&arguments)?;
        let mut directory_entries = 0;
        for line in listing.lines().filter(|line| !line.is_empty()) {
            let entry = listing_entry(line)?;
            if matches!(entry.name, "." | "..") {
                directory_entries += 1;
                continue;
            }
            let path = format!("{prefix}/{}", entry.name);
            if path.len() > 4096 || changes.len() >= 4096 {
                return Err("The guest system file inventory exceeds its size limit.".into());
            }
            let kind = match entry.mode & 0o170000 {
                0o040000 => "directory",
                0o100000 => "file",
                0o120000 => "link",
                0o020000 => "device-or-deletion",
                0o010000 | 0o060000 | 0o140000 => "special-file",
                _ => return Err("The guest system disk includes an unknown file type.".into()),
            };
            if kind == "directory" {
                directories.push((Some(entry.inode), path.clone(), depth + 1));
            }
            changes.push(Change { path, kind });
        }
        if directory_entries != 2 {
            return Err("The guest system file listing is incomplete.".into());
        }
    }
    changes.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(changes)
}

struct ListingEntry<'a> {
    inode: u64,
    mode: u32,
    name: &'a str,
}

fn listing_entry(line: &str) -> Result<ListingEntry<'_>, String> {
    let fields: Vec<_> = line.split('/').collect();
    let ["", inode, mode, uid, gid, name, size, ""] = fields.as_slice() else {
        return Err(
            "The guest system disk includes a file name that cannot be displayed exactly.".into(),
        );
    };
    let invalid = || "The guest system disk file listing is invalid.".to_owned();
    let inode = inode.parse::<u64>().map_err(|_| invalid())?;
    let mode = u32::from_str_radix(mode, 8).map_err(|_| invalid())?;
    if inode == 0
        || uid.parse::<u32>().is_err()
        || gid.parse::<u32>().is_err()
        || (!size.is_empty() && size.parse::<u64>().is_err())
        || name.is_empty()
        || name.len() > 255
        || name.contains('\u{fffd}')
        || name.chars().any(|value| value.is_control())
    {
        return Err(invalid());
    }
    Ok(ListingEntry { inode, mode, name })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lists_exact_nested_paths_including_whiteouts_and_links() {
        let changes = enumerate(|request| Ok(match request {
            "ls -p /upper" => "/12/040755/0/0/.//\n/2/040755/0/0/..//\n/13/040755/0/0/dir name//\n/14/020600/0/0/deleted/0/\n",
            "ls -p <13>" => "/13/040755/0/0/.//\n/12/040755/0/0/..//\n/15/120777/0/0/link/9/\n",
            _ => panic!("Unexpected reader command {request}"),
        }.into())).unwrap();
        assert_eq!(
            changes,
            [
                Change {
                    path: "/deleted".into(),
                    kind: "device-or-deletion"
                },
                Change {
                    path: "/dir name".into(),
                    kind: "directory"
                },
                Change {
                    path: "/dir name/link".into(),
                    kind: "link"
                },
            ]
        );
        assert!(listing_entry("/14/0100600/0/0/bad\tname/0/").is_err());
        assert!(enumerate(|_| Ok("/14/100600/0/0/file/0/".into())).is_err());
    }
}
