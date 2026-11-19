use crate::platform::brig::Brig;
use serde::Deserialize;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

const MAX_INDEX_BYTES: u64 = 1024 * 1024;

#[derive(Deserialize)]
struct SessionEntry {
    sandbox: Option<String>,
    project: Option<PathBuf>,
    home: Option<PathBuf>,
    #[serde(default)]
    ephemeral: bool,
}

fn index_path(brig: &Brig) -> Option<PathBuf> {
    let environment = &brig.environment;
    if let Some(directory) = environment
        .get("BRIG_STATE_DIR")
        .filter(|value| !value.is_empty())
    {
        return Some(Path::new(directory).join("sessions.json"));
    }
    let home = environment.get("HOME").filter(|value| !value.is_empty())?;
    Some(Path::new(home).join(".brig/sessions.json"))
}

fn decode_entries(input: File) -> Result<HashMap<String, SessionEntry>, String> {
    if !input
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("The Brig session index is not a regular file.".into());
    }
    let mut bytes = Vec::new();
    input
        .take(MAX_INDEX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_INDEX_BYTES {
        return Err("The Brig session index exceeds the size limit.".into());
    }
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

pub(crate) struct GuestHomes {
    pub(crate) directory: PathBuf,
    entries: HashMap<String, SessionEntry>,
}

impl GuestHomes {
    pub(crate) fn read(brig: &Brig) -> Result<Self, String> {
        use std::{fs, os::unix::fs::MetadataExt};
        let path = index_path(brig).ok_or("The Brig state directory is unknown.")?;
        let state = path
            .parent()
            .ok_or("The Brig state directory is unknown.")?;
        match fs::canonicalize(state) {
            Ok(path) if path == state => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let parent = state
                    .parent()
                    .ok_or("The Brig state directory is unknown.")?;
                if fs::canonicalize(parent).ok().as_deref() != Some(parent)
                    || fs::symlink_metadata(state).is_ok()
                {
                    return Err(
                        "The Brig state directory changed or contains a symbolic link.".into(),
                    );
                }
            }
            _ => {
                return Err("The Brig state directory changed or contains a symbolic link.".into())
            }
        }
        let input = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path);
        let entries = match input {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(error) => return Err(error.to_string()),
            Ok(input) => {
                let metadata = input.metadata().map_err(|error| error.to_string())?;
                if metadata.uid() != unsafe { libc::geteuid() }
                    || metadata.mode() & 0o077 != 0
                    || metadata.nlink() != 1
                {
                    return Err(
                        "The Brig session index must be a private file owned by the current user."
                            .into(),
                    );
                }
                decode_entries(input)?
            }
        };
        Ok(Self {
            directory: state.join("homes"),
            entries,
        })
    }

    pub(crate) fn owned_sandbox(
        &self,
        project: &crate::projects::Project,
    ) -> Result<Option<String>, String> {
        if !project.owns_session || project.adopted_managed_profile {
            return Ok(None);
        }
        let Some(entry) = self
            .entries
            .get(&project.reference())
            .filter(|entry| entry.ephemeral)
        else {
            return Ok(None);
        };
        let sandbox = entry
            .sandbox
            .as_deref()
            .filter(|name| {
                name.starts_with("brig-") && Path::new(name).file_name() == Some(OsStr::new(name))
            })
            .ok_or("The Brig session has an invalid sandbox name.")?;
        if entry.project.as_deref() != Some(project.path.as_path())
            || entry.home.as_deref() != Some(self.directory.join(sandbox).as_path())
        {
            return Err("The Brig guest home does not match the retired project.".into());
        }
        Ok(Some(sandbox.into()))
    }

    pub(crate) fn registered_home(&self, project: &crate::projects::Project) -> Option<PathBuf> {
        let saved = self.entries.get(&project.reference())?;
        if saved.project.as_deref() != Some(project.path.as_path()) {
            return None;
        }
        saved
            .home
            .as_ref()
            .filter(|path| {
                path.is_absolute() && !path.components().any(|part| part == Component::ParentDir)
            })
            .cloned()
    }

    pub(crate) fn protect_shared_home(&self, reference: &str, home: &Path) -> Result<(), String> {
        for (key, entry) in &self.entries {
            if key == reference {
                continue;
            }
            for other in [entry.home.as_deref(), entry.project.as_deref()]
                .into_iter()
                .flatten()
            {
                let resolved = match std::fs::canonicalize(other) {
                    Ok(path) => path,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => other.into(),
                    Err(error) => return Err(error.to_string()),
                };
                if [other, resolved.as_path()]
                    .iter()
                    .any(|other| home.starts_with(other) || other.starts_with(home))
                {
                    return Err("Another Brig session uses the retired guest home. Its files were preserved.".into());
                }
            }
        }
        Ok(())
    }

    pub(crate) fn verify_retired_home(
        &self,
        project: &crate::projects::Project,
        sandbox: &str,
    ) -> Result<(), String> {
        if self.entries.contains_key(&project.reference())
            && self.owned_sandbox(project)?.as_deref() != Some(sandbox)
        {
            return Err(
                "The retired Brig session changed after review. Its home was preserved.".into(),
            );
        }
        self.protect_shared_home(&project.reference(), &self.directory.join(sandbox))
    }
}
