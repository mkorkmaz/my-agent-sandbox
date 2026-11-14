use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
enum PublicationState {
    Uncreated,
    Writing((u64, u64)),
    Synced((u64, u64)),
    Published((u64, u64)),
    Released,
}

pub(super) enum LinkPublication {
    Created,
    Existing,
}

pub(super) struct TemporaryRecord {
    path: PathBuf,
    state: PublicationState,
}

impl TemporaryRecord {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            state: PublicationState::Uncreated,
        }
    }

    pub fn create(&mut self) -> Result<File, String> {
        if !matches!(self.state, PublicationState::Uncreated) {
            return Err("The temporary record was already created.".into());
        }
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&self.path)
            .map_err(|error| error.to_string())?;
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        self.state = PublicationState::Writing((metadata.dev(), metadata.ino()));
        Ok(file)
    }

    pub fn sync_file(&mut self, file: &File) -> Result<(), String> {
        let PublicationState::Writing(identity) = self.state else {
            return Err("The temporary record is not being written.".into());
        };
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() || (metadata.dev(), metadata.ino()) != identity {
            return Err("The file does not belong to this temporary record.".into());
        }
        file.sync_all().map_err(|error| error.to_string())?;
        self.state = PublicationState::Synced(identity);
        Ok(())
    }

    fn synced(&self) -> Result<(u64, u64), String> {
        let PublicationState::Synced(identity) = self.state else {
            return Err("Sync the temporary record before publication.".into());
        };
        if !self.owns_path(identity) {
            return Err("The temporary saved record changed.".into());
        }
        Ok(identity)
    }

    pub fn replace(&mut self, target: &Path) -> Result<(), String> {
        let identity = self.synced()?;
        fs::rename(&self.path, target).map_err(|error| error.to_string())?;
        self.state = PublicationState::Published(identity);
        Ok(())
    }

    pub fn link(&mut self, target: &Path) -> Result<LinkPublication, String> {
        let identity = self.synced()?;
        match fs::hard_link(&self.path, target) {
            Ok(()) => {
                self.state = PublicationState::Published(identity);
                Ok(LinkPublication::Created)
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Ok(LinkPublication::Existing)
            }
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn remove(&mut self) -> Result<(), String> {
        let identity = self
            .identity()
            .ok_or("The temporary record is not owned.")?;
        if !self.owns_path(identity) {
            return Err("The temporary saved record changed.".into());
        }
        fs::remove_file(&self.path).map_err(|error| error.to_string())?;
        self.state = PublicationState::Released;
        Ok(())
    }

    fn identity(&self) -> Option<(u64, u64)> {
        match self.state {
            PublicationState::Writing(identity)
            | PublicationState::Synced(identity)
            | PublicationState::Published(identity) => Some(identity),
            _ => None,
        }
    }

    fn owns_path(&self, identity: (u64, u64)) -> bool {
        fs::symlink_metadata(&self.path).is_ok_and(|metadata| {
            metadata.is_file() && (metadata.dev(), metadata.ino()) == identity
        })
    }
}

impl Drop for TemporaryRecord {
    fn drop(&mut self) {
        if !self
            .identity()
            .is_some_and(|identity| self.owns_path(identity))
        {
            return;
        }
        if let Err(error) = fs::remove_file(&self.path) {
            eprintln!("Could not remove a temporary saved record: {error}");
        }
    }
}
