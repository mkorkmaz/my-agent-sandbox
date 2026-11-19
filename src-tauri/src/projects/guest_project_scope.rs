use super::{validate_guest_path, Project};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub(crate) struct GuestProjectScope {
    current: PathBuf,
    previous: Vec<PathBuf>,
}

impl GuestProjectScope {
    pub(crate) fn current(&self) -> &Path {
        &self.current
    }

    pub(crate) fn contains(&self, directory: &Path) -> bool {
        directory.starts_with(&self.current)
            || self.previous.iter().any(|path| directory.starts_with(path))
    }
}

impl Project {
    pub(crate) fn guest_project_scope(&self, current: &Path) -> GuestProjectScope {
        GuestProjectScope {
            current: current.into(),
            // Brig v0.5.2 mounts the project at /work/<host folder name>.
            previous: self
                .previous_guest_project_names
                .iter()
                .map(|name| Path::new("/work").join(name))
                .collect(),
        }
    }
}

pub(super) fn valid_folder_name(name: &str) -> bool {
    Path::new(name).file_name() == Some(OsStr::new(name)) && validate_guest_path(name).is_ok()
}

#[cfg(test)]
impl From<PathBuf> for GuestProjectScope {
    fn from(current: PathBuf) -> Self {
        Self {
            current,
            previous: Vec::new(),
        }
    }
}

#[cfg(test)]
impl From<&str> for GuestProjectScope {
    fn from(path: &str) -> Self {
        PathBuf::from(path).into()
    }
}
