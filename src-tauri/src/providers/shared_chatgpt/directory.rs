use super::store;
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);
const INVALID: &str = "The guest Codex directory is unavailable or points elsewhere.";

// Keep guest operations on the verified directory, even if its path changes during a write.
pub struct Directory(File);
impl Directory {
    pub fn open(path: &Path, expected: Option<(u64, u64)>) -> Result<Self, String> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| INVALID)?;
        let directory = Self(file);
        let metadata = directory.0.metadata().map_err(|_| INVALID)?;
        if metadata.uid() != unsafe { libc::geteuid() }
            || fs::canonicalize(path).ok().as_deref() != Some(path)
            || expected.is_some_and(|identity| identity != (metadata.dev(), metadata.ino()))
        {
            return Err(INVALID.into());
        }
        Ok(directory)
    }

    pub fn identity(&self) -> Result<(u64, u64), String> {
        let metadata = self.0.metadata().map_err(|_| INVALID)?;
        Ok((metadata.dev(), metadata.ino()))
    }

    pub fn read(&self) -> Result<Option<Vec<u8>>, String> {
        let descriptor = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                c"auth.json".as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err("Could not read the project's ChatGPT sign-in.".into());
        }
        store::read_handle(unsafe { File::from_raw_fd(descriptor) }).map(Some)
    }

    pub fn write(&self, bytes: &[u8]) -> Result<(), String> {
        let name = CString::new(format!(
            "auth.tugboat-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
        .unwrap();
        let descriptor = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if descriptor < 0 {
            return Err("Could not prepare the project's ChatGPT sign-in.".into());
        }
        let mut file = unsafe { File::from_raw_fd(descriptor) };
        let result = (|| {
            file.write_all(bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| "Could not save the project's ChatGPT sign-in.")?;
            if unsafe {
                libc::renameat(
                    self.0.as_raw_fd(),
                    name.as_ptr(),
                    self.0.as_raw_fd(),
                    c"auth.json".as_ptr(),
                )
            } != 0
            {
                return Err("Could not apply the project's ChatGPT sign-in.".into());
            }
            self.0
                .sync_all()
                .map_err(|_| "Could not finish saving the project's ChatGPT sign-in.".into())
        })();
        if result.is_err() {
            unsafe {
                libc::unlinkat(self.0.as_raw_fd(), name.as_ptr(), 0);
            }
        }
        result
    }

    pub fn remove(&self) -> Result<(), String> {
        if unsafe { libc::unlinkat(self.0.as_raw_fd(), c"auth.json".as_ptr(), 0) } != 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
                return Ok(());
            }
            return Err("Could not remove the project's managed sign-in copy.".into());
        }
        self.0
            .sync_all()
            .map_err(|_| "Could not finish removing the project's sign-in copy.".into())
    }
}
