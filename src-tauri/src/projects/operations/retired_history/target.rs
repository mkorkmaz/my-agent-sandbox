use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::Path,
};

#[derive(Clone, Copy)]
pub(super) enum EntryKind {
    File,
    Database,
    Folder,
}

#[derive(Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    device: u64,
    inode: u64,
    created: std::time::SystemTime,
    file_stamp: Option<(u64, i64, i64, i64, i64)>,
}

impl Identity {
    pub(super) fn folder(path: &Path) -> Result<Self, String> {
        observe(path, EntryKind::Folder)?
            .ok_or_else(|| "The reviewed folder is unavailable.".into())
    }

    fn read(metadata: &fs::Metadata) -> Result<Self, String> {
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            created: metadata.created().map_err(|error| error.to_string())?,
            file_stamp: metadata.is_file().then(|| {
                (
                    metadata.len(),
                    metadata.mtime(),
                    metadata.mtime_nsec(),
                    metadata.ctime(),
                    metadata.ctime_nsec(),
                )
            }),
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Target {
    directory: Identity,
    identity: Identity,
}

impl Target {
    pub(super) fn verify_absence(path: &Path) -> Result<(), String> {
        for ancestor in path.ancestors() {
            match fs::symlink_metadata(ancestor) {
                Ok(_) if ancestor == path => {
                    return Err(
                        "A retired file appeared after review. Review the reset again.".into(),
                    )
                }
                Ok(metadata)
                    if metadata.is_dir()
                        && fs::canonicalize(ancestor).map_err(|error| error.to_string())?
                            == ancestor =>
                {
                    return Ok(())
                }
                Ok(_) => {
                    return Err(
                        "The retired data folder contains a symbolic link or a file.".into(),
                    )
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.to_string()),
            }
        }
        Err("The retired data folder is unavailable.".into())
    }

    pub(super) fn capture(path: &Path, kind: EntryKind) -> Result<Option<Self>, String> {
        let Some(mut identity) = observe(path, kind)? else {
            Self::verify_absence(path)?;
            return Ok(None);
        };
        if matches!(kind, EntryKind::Database) {
            identity.file_stamp = None;
        }
        let directory = Identity::read(
            &open_parent(path)?
                .metadata()
                .map_err(|error| error.to_string())?,
        )?;
        Ok(Some(Self {
            directory,
            identity,
        }))
    }

    pub(super) fn verify_parent(&self, path: &Path) -> Result<File, String> {
        let directory = open_parent(path)?;
        if Identity::read(&directory.metadata().map_err(|error| error.to_string())?)?
            != self.directory
        {
            return Err("The retired data folder changed or was replaced.".into());
        }
        Ok(directory)
    }

    pub(super) fn verify(&self, path: &Path, kind: EntryKind) -> Result<Option<File>, String> {
        let directory = self.verify_parent(path)?;
        let Some(mut identity) = observe(path, kind)? else {
            return Ok(None);
        };
        if matches!(kind, EntryKind::Database) {
            identity.file_stamp = None;
        }
        if identity != self.identity {
            return Err(
                "The retired data changed after review. Its replacement was preserved.".into(),
            );
        }
        Ok(Some(directory))
    }

    pub(super) fn erase(&self, path: &Path, kind: EntryKind) -> Result<(), String> {
        let Some(directory) = self.verify(path, kind)? else {
            return Ok(());
        };
        match kind {
            EntryKind::File | EntryKind::Database => {
                let leaf = path
                    .file_name()
                    .ok_or("The retired file name is missing.")?;
                let leaf = std::ffi::CString::new(leaf.as_bytes())
                    .map_err(|_| "The retired file name is invalid.")?;
                match unsafe { libc::unlinkat(directory.as_raw_fd(), leaf.as_ptr(), 0) } {
                    0 => Ok(()),
                    _ => Err(std::io::Error::last_os_error()),
                }
            }
            EntryKind::Folder => fs::remove_dir_all(path),
        }
        .map_err(|error| error.to_string())?;
        directory.sync_all().map_err(|error| error.to_string())
    }
}

fn open_parent(path: &Path) -> Result<File, String> {
    let directory = path
        .parent()
        .ok_or("The retired data path has no parent.")?;
    if fs::canonicalize(directory).map_err(|error| error.to_string())? != directory {
        return Err("The retired data folder contains a symbolic link.".into());
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(directory)
        .map_err(|error| error.to_string())?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    let observed = fs::symlink_metadata(directory).map_err(|error| error.to_string())?;
    if (metadata.dev(), metadata.ino()) != (observed.dev(), observed.ino())
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o022 != 0
    {
        return Err("The retired data folder is not owned by the current user.".into());
    }
    Ok(file)
}

fn observe(path: &Path, kind: EntryKind) -> Result<Option<Identity>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let kind_matches = match kind {
        EntryKind::File | EntryKind::Database => metadata.is_file() && metadata.nlink() == 1,
        EntryKind::Folder => metadata.is_dir(),
    };
    if !kind_matches || metadata.uid() != unsafe { libc::geteuid() } {
        return Err("The retired data is linked or is not owned by the current user.".into());
    }
    Identity::read(&metadata).map(Some)
}
