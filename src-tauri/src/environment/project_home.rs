use crate::{platform::brig::Brig, projects::Project};
use serde::Deserialize;
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, Read},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
struct SessionMounts {
    home: Option<PathBuf>,
    project: Option<PathBuf>,
}

pub(crate) fn remove(brig: &Brig, project: &Project, approved: &str) -> Result<(), String> {
    if !super::managed_profile::shared(project) {
        return Err("Only a Tugboat-owned project guest home can be removed.".into());
    }
    let home = brig.shared_project_home(project)?;
    if Path::new(approved) != home || overlaps(&home, &project.path) {
        return Err("The guest home changed after review.".into());
    }
    let metadata = match fs::symlink_metadata(&home) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || fs::canonicalize(&home).ok().as_deref() != Some(&home)
    {
        return Err("The project's guest home does not have verified ownership.".into());
    }
    let index = brig.state_directory()?.join("sessions.json");
    for mounts in sessions(&index)?.into_values() {
        for path in [mounts.home, mounts.project].into_iter().flatten() {
            let resolved = match fs::canonicalize(&path) {
                Ok(path) => path,
                Err(error) if error.kind() == io::ErrorKind::NotFound => path.clone(),
                Err(error) => {
                    return Err(format!("Could not verify a Brig session mount: {error}"))
                }
            };
            if overlaps(&home, &path) || overlaps(&home, &resolved) {
                return Err("A Brig session still uses the project guest home.".into());
            }
        }
    }
    fs::remove_dir_all(home)
        .map_err(|error| format!("Could not remove the project guest home: {error}"))
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn sessions(index: &Path) -> Result<HashMap<String, SessionMounts>, String> {
    const MAX_BYTES: u64 = 1024 * 1024;
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(index)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => return Err(error.to_string()),
    };
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
        || metadata.len() > MAX_BYTES
    {
        return Err(
            "The Brig session inventory must be a private file owned by the current user.".into(),
        );
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("The Brig session inventory exceeds its size limit.".into());
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| "Could not verify the Brig session inventory.".into())
}
