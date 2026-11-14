mod state;
use state::{ChangeKind, MarkerState, MovePhase};
use crate::storage::{self, FileLock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, WebviewWindow};
use tauri_plugin_dialog::DialogExt;

const POINTER: &str = "storage-location.json";
const MARKER: &str = ".tugboat-data.json";
const RECORDS: &[&str] = &[
    "settings.json",
    "projects.json",
    "projects.json.v1.backup",
    "settings.json.v1.backup",
    "interface.json",
    "installation.json",
    "conversation-ledgers",
    "usage.sqlite3",
    "boundaries",
    "chatgpt",
    "claude",
    "profiles",
];

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingLocation {
    directory: PathBuf,
    #[serde(rename = "locate")]
    kind: ChangeKind,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Location {
    schema_version: u32,
    directory: PathBuf,
    directory_id: String,
    pending: Option<PendingLocation>,
    previous_directory: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DirectoryMarker {
    schema_version: u32,
    id: String,
    #[serde(rename = "complete")]
    state: MarkerState,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyDirectoryMarker {
    schema_version: u32,
    id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationStatus {
    directory: Option<PathBuf>,
    default_directory: PathBuf,
    pending_directory: Option<PathBuf>,
    available: bool,
    can_change: bool,
    message: Option<String>,
}

struct SavedLocation {
    location: Option<Location>,
    message: Option<String>,
    reconnected: Option<RecordsDirectory>,
}

pub struct DataLocation {
    default_directory: PathBuf,
    pointer: PathBuf,
    saved: Mutex<SavedLocation>,
    // One instance owns the pointer for its lifetime, including shutdown and final usage writes.
    _lease: Option<FileLock>,
}

#[derive(Clone)]
pub struct RecordsDirectory {
    directory: PathBuf,
    id: String,
}

impl RecordsDirectory {
    pub(crate) fn path(&self) -> &Path {
        &self.directory
    }
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    pub(crate) fn schema_version(&self) -> Result<u32, String> {
        self.verify()?;
        Ok(marker(&self.directory)?.schema_version)
    }
    pub(crate) fn activate_sqlite(&self) -> Result<(), String> {
        self.verify()?;
        if self.schema_version()? != 1 {
            return Err(
                "The records folder is already activated. Reopen its verified usage database."
                    .into(),
            );
        }
        let marker = DirectoryMarker {
            schema_version: 2,
            id: self.id.clone(),
            state: MarkerState::Complete,
        };
        let bytes = serde_json::to_vec_pretty(&marker).map_err(|error| error.to_string())?;
        self.verify()?;
        storage::write_bytes_existing(&self.directory.join(MARKER), &bytes)
    }
    #[cfg(test)]
    pub fn fixture(directory: &Path) -> Self {
        Self::fixture_with_id(directory, &"a".repeat(32))
    }
    #[cfg(test)]
    pub(crate) fn fixture_with_id(directory: &Path, id: &str) -> Self {
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|byte| byte.is_ascii_hexdigit()));
        let directory = fs::canonicalize(directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        storage::write(
            &directory.join(MARKER),
            &DirectoryMarker {
                schema_version: 1,
                id: id.into(),
                state: MarkerState::Complete,
            },
        )
        .unwrap();
        Self {
            directory,
            id: id.into(),
        }
    }
    #[cfg(test)]
    pub(crate) fn fixture_existing(directory: &Path, id: &str) -> Self {
        let records = Self {
            directory: fs::canonicalize(directory).unwrap(),
            id: id.into(),
        };
        records.verify().unwrap();
        records
    }
    pub fn verify_at(&self, directory: &Path) -> Result<(), String> {
        self.verify()?;
        if self.directory != directory {
            return Err(
                "The records folder changed during account storage. Reconnect its original folder."
                    .into(),
            );
        }
        Ok(())
    }
    pub fn verify(&self) -> Result<(), String> {
        require_directory(&self.directory, &self.id)
    }
}

fn marker(directory: &Path) -> Result<DirectoryMarker, String> {
    if !directory
        .join(MARKER)
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.is_file())
    {
        return Err(
            "The records folder is unavailable. Connect its drive or locate it in Settings.".into(),
        );
    }
    let bytes = storage::read_regular_bytes(&directory.join(MARKER))?
        .ok_or("The records folder is unavailable. Connect its drive or locate it in Settings.")?;
    let marker: DirectoryMarker = serde_json::from_slice(&bytes)
        .map_err(|_| "The records folder marker could not be read.".to_owned())?;
    if !matches!(marker.schema_version, 1 | 2)
        || marker.id.len() != 32
        || !marker.id.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("The records folder marker is invalid.".into());
    }
    Ok(marker)
}

fn require_directory(directory: &Path, id: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(directory)
        .map_err(|_| "The records folder is unavailable. Reconnect its original drive.")?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || fs::canonicalize(directory).map_err(|error| error.to_string())? != directory
    {
        return Err(
            "The records folder path changed. Locate the original folder in Settings.".into(),
        );
    }
    let saved = marker(directory)?;
    if saved.id != id {
        return Err("A different Tugboat records folder is at this path. Locate the original folder in Settings.".into());
    }
    if saved.state == MarkerState::Incomplete {
        return Err("The records move in this folder is incomplete. Use the original records location and restart Tugboat to finish the move.".into());
    }
    Ok(())
}

// Only the saved location transaction can establish the state of an older marker.
// Arbitrary folders selected through Locate records must still carry an explicit complete flag.
fn upgrade_known_marker(directory: &Path, id: &str, state: MarkerState) -> Result<(), String> {
    let file = directory.join(MARKER);
    if !file
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.is_file())
    {
        return Ok(());
    }
    let Some(bytes) = storage::read_regular_bytes(&file)? else {
        return Ok(());
    };
    if serde_json::from_slice::<DirectoryMarker>(&bytes).is_ok() {
        return Ok(());
    }
    let legacy: LegacyDirectoryMarker = serde_json::from_slice(&bytes)
        .map_err(|_| "The records folder marker could not be read.".to_owned())?;
    if legacy.schema_version != 1 || legacy.id != id {
        return Err("The records folder marker does not match the saved location. The records were preserved.".into());
    }
    storage::write(
        &file,
        &DirectoryMarker {
            schema_version: 1,
            id: legacy.id,
            state,
        },
    )
}

fn create_directory(directory: &Path) -> Result<(), String> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)
        .map_err(|error| error.to_string())
}

fn private_bootstrap_directory(directory: &Path) -> Result<(), String> {
    create_directory(directory)?;
    private_directory(directory, || Ok(()))
}

fn private_records_directory(directory: &Path, id: &str) -> Result<(), String> {
    private_directory(directory, || require_directory(directory, id))
}

fn private_directory(directory: &Path, verify: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    let folder = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(directory)
        .map_err(|_| {
            "The app records folder must be a regular folder owned by the current user."
        })?;
    let metadata = folder.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err("The app records folder must be owned by the current user.".into());
    }
    verify()?;
    let current = fs::symlink_metadata(directory).map_err(|error| error.to_string())?;
    if (current.dev(), current.ino()) != (metadata.dev(), metadata.ino()) {
        return Err("The records folder changed while checking its permissions.".into());
    }
    if metadata.mode() & 0o777 == 0o700 {
        return Ok(());
    }
    folder
        .set_permissions(fs::Permissions::from_mode(0o700))
        .and_then(|_| folder.sync_all())
        .map_err(|error| format!("Could not set private permissions on the records folder. {error}"))?;
    if folder.metadata().map_err(|error| error.to_string())?.mode() & 0o777 != 0o700 {
        return Err("This volume could not apply private permissions to the records folder.".into());
    }
    Ok(())
}

fn prepare_bootstrap_directories(default: &Path, legacy: &Path) -> Result<(), String> {
    private_bootstrap_directory(default)?;
    match fs::symlink_metadata(legacy) {
        Ok(_) => private_bootstrap_directory(legacy),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn is_empty_destination(directory: &Path, default_directory: &Path) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let name = entry.map_err(|error| error.to_string())?.file_name();
        if name == ".DS_Store"
            || (directory == default_directory
                && (name == POINTER || name == format!("{POINTER}.lock").as_str()))
        {
            continue;
        }
        return Err(
            "Choose an empty folder for Tugboat records. Existing files will not be replaced."
                .into(),
        );
    }
    Ok(())
}

fn validate_separate_directory(source: &Path, target: &Path) -> Result<(), String> {
    if source != target && (target.starts_with(source) || source.starts_with(target)) {
        return Err(
            "Choose a folder outside the current records folder and its parent folders.".into(),
        );
    }
    Ok(())
}

pub fn require_private_records(
    records: &Path,
    registry: &crate::projects::Registry,
) -> Result<(), String> {
    if registry.projects.iter().any(|project| {
        let path = fs::canonicalize(&project.path).unwrap_or_else(|_| project.path.clone());
        records.starts_with(path)
    }) {
        return Err("Choose a records folder outside all project folders. Projects must not contain the shared ChatGPT account.".into());
    }
    Ok(())
}

fn copy_record(source: &Path, target: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err(
            "A saved record is a symbolic link. The original records were preserved.".into(),
        );
    }
    if target
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err("The destination contains a symbolic link. Choose an empty folder.".into());
    }
    if metadata.is_dir() {
        create_directory(target)?;
        fs::set_permissions(target, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
        for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            if entry.file_name().to_string_lossy().ends_with(".lock") {
                continue;
            }
            copy_record(&entry.path(), &target.join(entry.file_name()))?;
        }
        File::open(target)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    if !metadata.is_file() {
        return Err(
            "Saved records must be regular files or folders. The original records were preserved."
                .into(),
        );
    }
    if source
        .file_name()
        .is_some_and(|name| name == "usage.sqlite3")
    {
        return crate::usage_store::move_support::copy_closed_file(source, target);
    }
    let bytes = storage::read_regular_bytes(source)?
        .ok_or("A saved record disappeared during the move.")?;
    storage::write_bytes(target, &bytes)?;
    fs::set_permissions(target, fs::Permissions::from_mode(0o600))
        .map_err(|error| error.to_string())?;
    if storage::read_regular_bytes(target)?.as_deref() != Some(bytes.as_slice()) {
        return Err(
            "The copied records could not be verified. The original records were preserved.".into(),
        );
    }
    Ok(())
}

fn remove_copied_record(source: &Path, target: &Path) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.file_type().is_symlink() {
        return Err("An original record changed after copying. It was preserved.".into());
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            if entry.file_name().to_string_lossy().ends_with(".lock") {
                fs::remove_file(entry.path()).map_err(|error| error.to_string())?;
                continue;
            }
            remove_copied_record(&entry.path(), &target.join(entry.file_name()))?;
        }
        fs::remove_dir(source).map_err(|error| error.to_string())?;
        return Ok(());
    }
    if source
        .file_name()
        .is_some_and(|name| name == "usage.sqlite3")
    {
        crate::usage_store::move_support::verify_closed_match(source, target)?;
        return fs::remove_file(source).map_err(|error| error.to_string());
    }
    if storage::read_regular_bytes(source)? != storage::read_regular_bytes(target)? {
        return Err("An original record changed after copying. It was preserved.".into());
    }
    fs::remove_file(source).map_err(|error| error.to_string())
}

fn finish_move(
    location: &mut Location,
    pointer: &Path,
    default_directory: &Path,
) -> Result<(), String> {
    // A located dataset can be available while the original drive is offline.
    let active = match location.phase() {
        MovePhase::Pending(pending) if pending.kind == ChangeKind::Locate => &pending.directory,
        _ => &location.directory,
    };
    private_records_directory(active, &location.directory_id)?;
    if let MovePhase::Pending(pending) = location.phase() {
        let pending = pending.clone();
        if pending.kind == ChangeKind::Move && location.previous_directory.is_some() {
            return Err("Cancel the pending move and restart Tugboat to finish cleanup of the previous records folder.".into());
        }
        let target = fs::canonicalize(&pending.directory).map_err(|_| {
            "The selected records folder is unavailable. Connect its drive and restart Tugboat."
                .to_owned()
        })?;
        let source = if pending.kind == ChangeKind::Locate {
            &target
        } else {
            &location.directory
        };
        if source.join("chatgpt/session.json").exists() || source.join("claude/session.json").exists() {
            let registry = crate::projects::Registry::load(&source.join("projects.json"))?;
            require_private_records(&target, &registry)?;
        }
        if target == location.directory {
            crate::usage_store::move_support::recover(&RecordsDirectory {
                directory: target.clone(),
                id: location.directory_id.clone(),
            })?;
            location.pending = None;
            storage::write(pointer, location)?;
            return Ok(());
        }
        if pending.kind == ChangeKind::Locate {
            require_directory(&target, &location.directory_id)?;
            crate::usage_store::move_support::recover(&RecordsDirectory {
                directory: target.clone(),
                id: location.directory_id.clone(),
            })?;
        }
        if pending.kind == ChangeKind::Move {
            require_directory(&location.directory, &location.directory_id)?;
            let source_directory =
                fs::canonicalize(&location.directory).map_err(|error| error.to_string())?;
            if source_directory == target {
                return Err("The selected folder already points to the current records. Cancel the folder change in Settings.".into());
            }
            validate_separate_directory(&source_directory, &target)?;
            let source_records = RecordsDirectory {
                directory: source_directory.clone(),
                id: location.directory_id.clone(),
            };
            let source_version = source_records.schema_version()?;
            let database = crate::usage_store::move_support::recover(&source_records)?;
            if !target.join(MARKER).exists() {
                is_empty_destination(&target, default_directory)?;
                private_bootstrap_directory(&target)?;
                storage::write(
                    &target.join(MARKER),
                    &DirectoryMarker {
                        schema_version: source_version,
                        id: location.directory_id.clone(),
                        state: MarkerState::Incomplete,
                    },
                )?;
            }
            upgrade_known_marker(&target, &location.directory_id, MarkerState::Incomplete)?;
            if marker(&target)?.id != location.directory_id
                || marker(&target)?.schema_version != source_version
            {
                return Err("A different Tugboat records folder is at this path. The original records were preserved.".into());
            }
            private_bootstrap_directory(&target)?;
            crate::usage_store::move_support::verify_partial_target(&target, &source_records)?;
            storage::write(
                &target.join(MARKER),
                &DirectoryMarker {
                    schema_version: source_version,
                    id: location.directory_id.clone(),
                    state: MarkerState::Incomplete,
                },
            )?;
            #[cfg(test)]
            move_checkpoint("before-copy", pointer);
            for name in RECORDS {
                let partial = target.join(name);
                match partial.symlink_metadata() {
                    Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(&partial).map_err(|error| error.to_string())?,
                    Ok(metadata) if metadata.is_file() => fs::remove_file(&partial).map_err(|error| error.to_string())?,
                    Ok(_) => return Err("The destination contains an unsupported record. The original records were preserved.".into()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                    Err(error) => return Err(error.to_string()),
                }
                let source = source_directory.join(name);
                if source.symlink_metadata().is_ok() {
                    copy_record(&source, &partial)?;
                }
            }
            crate::usage_store::move_support::verify_copy(
                &source_records,
                &target,
                database.as_ref(),
            )?;
            storage::write(
                &target.join(MARKER),
                &DirectoryMarker {
                    schema_version: source_version,
                    id: location.directory_id.clone(),
                    state: MarkerState::Complete,
                },
            )?;
        }
        #[cfg(test)]
        move_checkpoint("target-complete", pointer);
        let mut committed = location.clone();
        if pending.kind == ChangeKind::Move {
            committed.previous_directory = Some(location.directory.clone());
        }
        committed.directory = target;
        committed.pending = None;
        let committed_result = storage::write(pointer, &committed);
        if committed_result.is_ok()
            || storage::read::<Location>(pointer)?.as_ref() == Some(&committed)
        {
            *location = committed;
        }
        committed_result?;
        #[cfg(test)]
        move_checkpoint("pointer-committed", pointer);
    }
    if let MovePhase::Cleanup(previous) = location.phase() {
        let previous = previous.clone();
        require_directory(&location.directory, &location.directory_id)?;
        crate::usage_store::move_support::recover(&RecordsDirectory {
            directory: location.directory.clone(),
            id: location.directory_id.clone(),
        })?;
        if previous.exists() {
            let old = fs::canonicalize(&previous).map_err(|error| error.to_string())?;
            let active =
                fs::canonicalize(&location.directory).map_err(|error| error.to_string())?;
            if old == active {
                return Err("The original folder now points to the active records. Both locations were preserved.".into());
            }
            validate_separate_directory(&old, &active)?;
        }
        let already_cleaned = previous.is_dir()
            && !previous.join(MARKER).exists()
            && !RECORDS.iter().any(|name| previous.join(name).exists());
        if already_cleaned {
            location.previous_directory = None;
            return storage::write(pointer, location);
        }
        upgrade_known_marker(&previous, &location.directory_id, MarkerState::Incomplete)?;
        if marker(&previous)?.id != location.directory_id {
            return Err(
                "The original records folder changed after copying. It was preserved.".into(),
            );
        }
        let previous_version = marker(&previous)?.schema_version;
        if previous_version != marker(&location.directory)?.schema_version {
            return Err(
                "The records folders use different storage versions. Both were preserved.".into(),
            );
        }
        storage::write(
            &previous.join(MARKER),
            &DirectoryMarker {
                schema_version: previous_version,
                id: location.directory_id.clone(),
                state: MarkerState::Incomplete,
            },
        )?;
        for name in RECORDS {
            remove_copied_record(&previous.join(name), &location.directory.join(name))?;
            let lock = previous.join(format!("{name}.lock"));
            if lock.exists() {
                fs::remove_file(lock).map_err(|error| error.to_string())?;
            }
        }
        fs::remove_file(previous.join(MARKER)).map_err(|error| error.to_string())?;
        location.previous_directory = None;
        storage::write(pointer, location)?;
        // Keep user-owned files and the bootstrap pointer. Remove the old directory only if empty.
        if previous != default_directory
            && fs::read_dir(&previous).is_ok_and(|mut entries| entries.next().is_none())
        {
            fs::remove_dir(previous).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

impl DataLocation {
    fn open(default_directory: PathBuf, legacy_directory: PathBuf) -> Self {
        let lease = prepare_bootstrap_directories(&default_directory, &legacy_directory)
            .and_then(|_| storage::lock(&default_directory.join(POINTER)));
        let default_directory = fs::canonicalize(&default_directory).unwrap_or(default_directory);
        let pointer = default_directory.join(POINTER);
        let mut saved = SavedLocation {
            location: None,
            message: None,
            reconnected: None,
        };
        if let Err(error) = &lease {
            saved.message = Some(error.clone());
        }
        if lease.is_ok() {
            let initialize = || -> Result<Location, String> {
                if let Some(location) = storage::read::<Location>(&pointer)? {
                    if location.schema_version != 1
                        || !location.directory.is_absolute()
                        || location.directory_id.len() != 32
                        || !location
                            .directory_id
                            .bytes()
                            .all(|byte| byte.is_ascii_hexdigit())
                        || location
                            .pending
                            .as_ref()
                            .is_some_and(|pending| !pending.directory.is_absolute())
                        || location
                            .previous_directory
                            .as_ref()
                            .is_some_and(|previous| !previous.is_absolute())
                    {
                        return Err("The saved records location is invalid. Locate your records folder in Settings.".into());
                    }
                    return Ok(location);
                }
                let legacy = RECORDS
                    .iter()
                    .any(|name| legacy_directory.join(name).exists());
                let directory = if legacy {
                    fs::canonicalize(&legacy_directory).map_err(|error| error.to_string())?
                } else {
                    default_directory.clone()
                };
                create_directory(&directory)?;
                let id = if directory.join(MARKER).exists() {
                    marker(&directory)?.id
                } else {
                    let stamp = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_err(|error| error.to_string())?
                        .as_nanos();
                    let id = crate::hash::hex(Sha256::digest(format!(
                            "{}:{}:{stamp}",
                            directory.display(),
                            std::process::id()
                        )))[..32]
                        .to_owned();
                    storage::write(
                        &directory.join(MARKER),
                        &DirectoryMarker {
                            schema_version: 1,
                            id: id.clone(),
                            state: MarkerState::Complete,
                        },
                    )?;
                    id
                };
                let location = Location {
                    schema_version: 1,
                    directory,
                    directory_id: id,
                    pending: legacy.then(|| PendingLocation {
                        directory: default_directory.clone(),
                        kind: ChangeKind::Move,
                    }),
                    previous_directory: None,
                };
                storage::write(&pointer, &location)?;
                Ok(location)
            };
            match initialize() {
                Ok(mut location) => {
                    let restored =
                        upgrade_known_marker(&location.directory, &location.directory_id, MarkerState::Complete)
                            .and_then(|_| finish_move(&mut location, &pointer, &default_directory));
                    if let Err(error) = restored {
                        saved.message = Some(format!("Could not finish moving records. {error}"));
                    }
                    saved.location = Some(location);
                }
                Err(error) => saved.message = Some(error),
            }
        }
        Self {
            default_directory,
            pointer,
            saved: Mutex::new(saved),
            _lease: lease.ok(),
        }
    }

    fn status(&self) -> Result<LocationStatus, String> {
        let saved = self.saved.lock().map_err(|error| error.to_string())?;
        let available = saved.reconnected.as_ref().map_or_else(
            || {
                saved.location.as_ref().is_some_and(|location| {
                    require_directory(&location.directory, &location.directory_id).is_ok()
                })
            },
            |records| records.verify().is_ok(),
        );
        let message = saved.message.clone().or_else(|| (!available).then(|| "The records folder is unavailable. Connect its drive and restart Tugboat, or locate the folder below.".into()));
        Ok(LocationStatus {
            directory: saved
                .reconnected
                .as_ref()
                .map(|records| records.path().to_owned())
                .or_else(|| {
                    saved
                        .location
                        .as_ref()
                        .map(|location| location.directory.clone())
                }),
            default_directory: self.default_directory.clone(),
            pending_directory: saved.location.as_ref().and_then(|location| {
                location
                    .pending
                    .as_ref()
                    .map(|pending| pending.directory.clone())
            }),
            available,
            can_change: self._lease.is_some(),
            message,
        })
    }

    fn pending_reconnect(&self) -> Result<Option<RecordsDirectory>, String> {
        if self._lease.is_none() {
            return Err("Another Tugboat instance owns these records.".into());
        }
        let saved = self.saved.lock().map_err(|error| error.to_string())?;
        let Some(location) = &saved.location else {
            return Ok(None);
        };
        let Some(pending) = location.pending.as_ref().filter(|pending| pending.kind == ChangeKind::Locate) else {
            return Ok(None);
        };
        let records = RecordsDirectory {
            directory: pending.directory.clone(),
            id: location.directory_id.clone(),
        };
        records.verify()?;
        let directory = fs::symlink_metadata(records.path()).map_err(|error| error.to_string())?;
        if !directory.is_dir()
            || directory.file_type().is_symlink()
            || directory.permissions().mode() & 0o077 != 0
            || directory.uid() != unsafe { libc::geteuid() }
        {
            return Err(
                "The located records folder must be private and owned by the current user.".into(),
            );
        }
        let registry = crate::projects::Registry::load(&records.path().join("projects.json"))?;
        require_private_records(records.path(), &registry)?;
        Ok(Some(records))
    }

    fn use_reconnected(&self, records: &RecordsDirectory) -> Result<(), String> {
        records.verify()?;
        let mut saved = self.saved.lock().map_err(|error| error.to_string())?;
        let location = saved
            .location
            .as_ref()
            .ok_or("The saved records location is unavailable.")?;
        let pending = location
            .pending
            .as_ref()
            .filter(|pending| pending.kind == ChangeKind::Locate)
            .ok_or("The pending records reconnection changed. Restart Tugboat to verify it.")?;
        if location.directory_id != records.id() || pending.directory != records.path() {
            return Err("The records reconnection belongs to a different dataset.".into());
        }
        saved.reconnected = Some(records.clone());
        Ok(())
    }

    fn schedule(&self, folder: &Path, locate: bool) -> Result<(), String> {
        let kind = ChangeKind::from(locate);
        if self._lease.is_none() {
            return Err("Another Tugboat instance is using these records. Close it before changing the folder.".into());
        }
        let target = crate::projects::check_folder(folder)?;
        let mut saved = self.saved.lock().map_err(|error| error.to_string())?;
        if saved.reconnected.is_some() {
            return Err("Restart Tugboat to finish reconnecting records before changing their location again.".into());
        }
        if kind == ChangeKind::Move && saved.location.as_ref().is_some_and(|location| location.previous_directory.is_some()) {
            return Err("Reconnect the previous records folder and restart Tugboat to finish cleanup before moving records again.".into());
        }
        if kind == ChangeKind::Locate {
            if saved.location.as_ref().is_some_and(|location| {
                require_directory(&location.directory, &location.directory_id).is_ok()
                    && crate::usage_store::move_support::require_file(&RecordsDirectory {
                        directory: location.directory.clone(),
                        id: location.directory_id.clone(),
                    })
                    .is_ok()
            }) {
                return Err("The current records folder is available. Use Change folder to move its records.".into());
            }
            let marker = marker(&target)?;
            if let Some(location) = saved.location.as_ref() {
                if marker.id != location.directory_id {
                    return Err("This folder contains different Tugboat records. Choose the original records folder.".into());
                }
            }
            private_records_directory(&target, &marker.id)?;
            crate::usage_store::move_support::recover(&RecordsDirectory {
                directory: target.clone(),
                id: marker.id.clone(),
            })?;
            let location = saved.location.get_or_insert(Location {
                schema_version: 1,
                directory: target.clone(),
                directory_id: marker.id,
                pending: None,
                previous_directory: None,
            });
            let mut next = location.clone();
            next.pending = Some(PendingLocation {
                directory: target,
                kind: ChangeKind::Locate,
            });
            storage::write(&self.pointer, &next)?;
            saved.location = Some(next);
            saved.message = None;
            return Ok(());
        }
        let location = saved
            .location
            .as_ref()
            .ok_or("Locate the existing records folder first.")?;
        require_directory(&location.directory, &location.directory_id)?;
        crate::usage_store::move_support::require_file(&RecordsDirectory {
            directory: location.directory.clone(),
            id: location.directory_id.clone(),
        })?;
        validate_separate_directory(&location.directory, &target)?;
        if target != location.directory {
            is_empty_destination(&target, &self.default_directory)?;
        }
        let mut next = location.clone();
        next.pending = (target != location.directory).then_some(PendingLocation {
            directory: target,
            kind: ChangeKind::Move,
        });
        storage::write(&self.pointer, &next)?;
        saved.location = Some(next);
        saved.message = None;
        Ok(())
    }
}

pub fn initialize(app: &AppHandle) -> Result<DataLocation, String> {
    let base = app.path().config_dir().map_err(|error| error.to_string())?;
    Ok(DataLocation::open(
        base.join("Tugboat"),
        app.path()
            .app_config_dir()
            .map_err(|error| error.to_string())?,
    ))
}

pub fn records_directory(app: &AppHandle) -> Result<RecordsDirectory, String> {
    let state = app.state::<DataLocation>();
    if state._lease.is_none() {
        return Err(
            "Another Tugboat instance is using these records. Close it and restart this app."
                .into(),
        );
    }
    let saved = state.saved.lock().map_err(|error| error.to_string())?;
    if let Some(records) = &saved.reconnected {
        records.verify()?;
        return Ok(records.clone());
    }
    let location = saved
        .location
        .as_ref()
        .ok_or("Locate the records folder in Settings before using Tugboat.")?;
    require_directory(&location.directory, &location.directory_id)?;
    Ok(RecordsDirectory {
        directory: location.directory.clone(),
        id: location.directory_id.clone(),
    })
}

pub(crate) fn reconnect_for_shutdown(app: &AppHandle) -> Result<(), String> {
    let location = app.state::<DataLocation>();
    let Some(records) = location.pending_reconnect()? else {
        return Ok(());
    };
    let writer = app
        .state::<crate::usage_store::Runtime>()
        .existing_store()?;
    if writer.is_some() {
        crate::conversations::sql_reconnect::reconnect_storage(app, &records)?;
    }
    if writer.is_none() && records.schema_version()? == 2 {
        crate::usage_store::move_support::recover(&records)?;
    }
    location.use_reconnected(&records)?;
    crate::providers::shared_chatgpt::reconnect_records(app, &records)?;
    crate::providers::shared_claude::reconnect_records(app, &records)
}

pub fn directory(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(records_directory(app)?.directory)
}

pub fn protected_directories(app: &AppHandle) -> Result<Vec<PathBuf>, String> {
    let active = directory(app)?;
    let state = app.state::<DataLocation>();
    let saved = state.saved.lock().map_err(|error| error.to_string())?;
    let mut directories = vec![active];
    if let Some(pending) = saved
        .location
        .as_ref()
        .and_then(|location| location.pending.as_ref())
    {
        directories.push(pending.directory.clone());
    }
    Ok(directories)
}

fn resolved_share(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("The plan contains an invalid host share path.".into());
    }
    match fs::canonicalize(path) {
        Ok(path) => return Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    if fs::symlink_metadata(path).is_ok() {
        return Err("The host share contains an unresolved symbolic link.".into());
    }
    let parent = path.parent().ok_or("Invalid host share location.")?;
    let name = path.file_name().ok_or("Invalid host share name.")?;
    Ok(resolved_share(parent)?.join(name))
}

pub fn require_private_plan_records(
    records: &[PathBuf],
    plan: &serde_json::Value,
) -> Result<(), String> {
    let records = records
        .iter()
        .map(|directory| resolved_share(directory))
        .collect::<Result<Vec<_>, _>>()?;
    for field in ["project", "home"] {
        let share = plan["data"][field]["host"]
            .as_str()
            .ok_or("The plan host share is missing.")?;
        let share = resolved_share(Path::new(share))?;
        require_records_outside_share(&records, &share)?;
    }
    Ok(())
}

fn require_records_outside_share(records: &[PathBuf], share: &Path) -> Result<(), String> {
    if records.iter().any(|directory| directory.starts_with(share)) {
        return Err(
            "Active and pending records folders must remain outside project and guest home shares."
                .into(),
        );
    }
    Ok(())
}

pub fn validate_project_records(
    app: &AppHandle,
    registry: &crate::projects::Registry,
) -> Result<(), String> {
    let state = app.state::<DataLocation>();
    let saved = state.saved.lock().map_err(|error| error.to_string())?;
    let location = saved
        .location
        .as_ref()
        .ok_or("Locate the records folder in Settings first.")?;
    require_private_records(&location.directory, registry)?;
    if let Some(pending) = &location.pending {
        let directory =
            fs::canonicalize(&pending.directory).unwrap_or_else(|_| pending.directory.clone());
        require_private_records(&directory, registry)?;
    }
    Ok(())
}

#[tauri::command]
pub fn storage_location(app: AppHandle) -> Result<LocationStatus, String> {
    let mut status = app.state::<DataLocation>().status()?;
    if let Some(error) = app.state::<crate::usage_store::Runtime>().failure() {
        status.message = Some(error);
    }
    Ok(status)
}

#[tauri::command]
pub async fn choose_storage_folder(window: WebviewWindow) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        window
            .app_handle()
            .dialog()
            .file()
            .set_parent(&window)
            .set_title("Choose Tugboat records folder")
            .blocking_pick_folder()
            .map(|folder| {
                folder
                    .into_path()
                    .map(|path| path.to_string_lossy().into_owned())
            })
            .transpose()
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn change_storage_location(
    app: AppHandle,
    folder: Option<String>,
    locate: bool,
) -> Result<LocationStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<DataLocation>();
        let target = folder
            .map(PathBuf::from)
            .unwrap_or_else(|| state.default_directory.clone());
        let target = crate::projects::check_folder(&target)?;
        let registry_file = directory(&app)
            .map(|root| root.join("projects.json"))
            .unwrap_or_else(|_| target.join("projects.json"));
        let registry = crate::projects::Registry::load(&registry_file)?;
        require_private_records(&target, &registry)?;
        if !locate {
            app.state::<crate::usage_store::Runtime>()
                .wait()?
                .with(|store| store.verify())?;
        }
        state.schedule(&target, locate)?;
        state.status()
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn cancel_storage_move(app: AppHandle) -> Result<LocationStatus, String> {
    let state = app.state::<DataLocation>();
    if state._lease.is_none() {
        return Err(
            "Another Tugboat instance is using these records. Close it before changing the folder."
                .into(),
        );
    }
    let mut saved = state.saved.lock().map_err(|error| error.to_string())?;
    if saved.reconnected.is_some() {
        return Err("Restart Tugboat to finish the verified records reconnection.".into());
    }
    let mut location = saved
        .location
        .clone()
        .ok_or("The records location is unavailable.")?;
    location.pending = None;
    storage::write(&state.pointer, &location)?;
    saved.location = Some(location);
    saved.message = None;
    drop(saved);
    state.status()
}

#[tauri::command]
pub fn restart_for_storage_change(app: AppHandle) {
    crate::desktop::lifecycle::request_restart(&app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn snapshot(directory: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        let mut files = std::collections::BTreeMap::new();
        let mut pending: Vec<_> = RECORDS.iter().map(|name| directory.join(name)).collect();
        while let Some(path) = pending.pop() {
            if path.is_dir() {
                pending.extend(
                    fs::read_dir(path)
                        .unwrap()
                        .map(|entry| entry.unwrap().path()),
                );
                continue;
            }
            if path.is_file() {
                files.insert(
                    path.strip_prefix(directory).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
        files
    }

    fn write_legacy_marker(directory: &Path, id: &str) {
        storage::write(
            &directory.join(MARKER),
            &serde_json::json!({ "schemaVersion": 1, "id": id }),
        )
        .unwrap();
    }

    #[test]
    fn a_committed_legacy_marker_is_upgraded_without_changing_any_records() {
        let (root, default, legacy) = fixture("legacy-marker");
        let location = DataLocation::open(default.clone(), legacy.clone());
        seed(&default);
        let id = marker(&default).unwrap().id;
        let records = snapshot(&default);
        let pointer = fs::read(default.join(POINTER)).unwrap();
        write_legacy_marker(&default, &id);
        assert_eq!(
            marker(&default).err().as_deref(),
            Some("The records folder marker could not be read.")
        );
        drop(location);
        let reopened = DataLocation::open(default.clone(), legacy.clone());
        assert!(reopened.status().unwrap().available);
        assert!(reopened.status().unwrap().message.is_none());
        assert_eq!(marker(&default).unwrap().id, id);
        assert_eq!(marker(&default).unwrap().state, MarkerState::Complete);
        assert_eq!(snapshot(&default), records);
        assert_eq!(fs::read(default.join(POINTER)).unwrap(), pointer);
        let upgraded = fs::read(default.join(MARKER)).unwrap();
        drop(reopened);
        let reopened = DataLocation::open(default.clone(), legacy);
        assert!(reopened.status().unwrap().available);
        assert_eq!(fs::read(default.join(MARKER)).unwrap(), upgraded);
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_transaction_markers_resume_a_pending_copy_and_committed_cleanup() {
        let (root, default, legacy) = fixture("legacy-move-markers");
        let target = root.join("target");
        create_directory(&target).unwrap();
        let location = DataLocation::open(default.clone(), legacy.clone());
        seed(&default);
        let records = snapshot(&default);
        let id = marker(&default).unwrap().id;
        location.schedule(&target, false).unwrap();
        write_legacy_marker(&default, &id);
        write_legacy_marker(&target, &id);
        fs::write(target.join("settings.json"), "partial").unwrap();
        symlink(
            root.join("outside"),
            default.join("conversation-ledgers/link"),
        )
        .unwrap();
        drop(location);
        let interrupted = DataLocation::open(default.clone(), legacy.clone());
        assert!(interrupted.status().unwrap().available);
        assert!(interrupted
            .status()
            .unwrap()
            .message
            .unwrap()
            .contains("symbolic link"));
        assert_eq!(marker(&target).unwrap().state, MarkerState::Incomplete);
        assert!(require_directory(&target, &id).is_err());
        drop(interrupted);
        fs::remove_file(default.join("conversation-ledgers/link")).unwrap();
        for name in RECORDS {
            if default.join(name).exists() {
                copy_record(&default.join(name), &target.join(name)).unwrap();
            }
        }
        write_legacy_marker(&target, &id);
        write_legacy_marker(&default, &id);
        let pointer = default.join(POINTER);
        let mut committed = storage::read::<Location>(&pointer).unwrap().unwrap();
        committed.directory = target.clone();
        committed.pending = None;
        committed.previous_directory = Some(default.clone());
        storage::write(&pointer, &committed).unwrap();
        let completed = DataLocation::open(default.clone(), legacy);
        assert!(completed.status().unwrap().available);
        assert!(completed.status().unwrap().message.is_none());
        assert_eq!(completed.status().unwrap().directory, Some(target.clone()));
        assert_eq!(marker(&target).unwrap().state, MarkerState::Complete);
        assert_eq!(snapshot(&target), records);
        assert!(!default.join("settings.json").exists());
        drop(completed);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_and_explicitly_incomplete_markers_are_never_upgraded() {
        for (index, extra) in [
            serde_json::json!({ "complete": false }),
            serde_json::json!({ "complete": null }),
            serde_json::json!({ "complete": "true" }),
            serde_json::json!({ "schemaVersion": 2 }),
            serde_json::json!({ "id": "f".repeat(32) }),
            serde_json::json!({ "unknown": true }),
        ]
        .into_iter()
        .enumerate()
        {
            let (root, default, legacy) = fixture(&format!("invalid-legacy-marker-{index}"));
            let location = DataLocation::open(default.clone(), legacy.clone());
            seed(&default);
            let mut document =
                serde_json::json!({ "schemaVersion": 1, "id": marker(&default).unwrap().id });
            document
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            storage::write(&default.join(MARKER), &document).unwrap();
            let before = fs::read(default.join(MARKER)).unwrap();
            let records = snapshot(&default);
            drop(location);
            let reopened = DataLocation::open(default.clone(), legacy);
            assert!(!reopened.status().unwrap().available);
            assert_eq!(fs::read(default.join(MARKER)).unwrap(), before);
            assert_eq!(snapshot(&default), records);
            drop(reopened);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn locating_an_unselected_legacy_copy_does_not_assume_it_is_complete() {
        let (root, default, legacy) = fixture("unselected-legacy-marker");
        let target = root.join("drive");
        create_directory(&target).unwrap();
        let location = DataLocation::open(default.clone(), legacy.clone());
        seed(&default);
        location.schedule(&target, false).unwrap();
        drop(location);
        let moved = DataLocation::open(default.clone(), legacy.clone());
        let id = marker(&target).unwrap().id;
        fs::rename(&target, root.join("disconnected")).unwrap();
        drop(moved);
        let offline = DataLocation::open(default.clone(), legacy);
        let partial = root.join("partial-copy");
        create_directory(&partial).unwrap();
        write_legacy_marker(&partial, &id);
        fs::write(partial.join("settings.json"), "partial").unwrap();
        let before = fs::read(default.join(POINTER)).unwrap();
        assert!(offline
            .schedule(&partial, true)
            .unwrap_err()
            .contains("marker could not be read"));
        assert_eq!(fs::read(default.join(POINTER)).unwrap(), before);
        assert_eq!(
            fs::read_to_string(partial.join("settings.json")).unwrap(),
            "partial"
        );
        assert!(!serde_json::from_slice::<serde_json::Value>(
            &fs::read(partial.join(MARKER)).unwrap()
        )
        .unwrap()
        .as_object()
        .unwrap()
        .contains_key("complete"));
        drop(offline);
        fs::remove_dir_all(root).unwrap();
    }

    fn fixture(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("tugboat-data-{name}-{}", std::process::id()));
        create_directory(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        (
            root.clone(),
            root.join("Application Support/Tugboat"),
            root.join("Application Support/sh.brig.desktop"),
        )
    }

    #[test]
    fn active_and_pending_records_must_remain_outside_both_plan_shares_and_their_aliases() {
        let (root, _, _) = fixture("plan-shares");
        create_directory(&root.join("project")).unwrap();
        create_directory(&root.join("guest-home")).unwrap();
        let mut plan = serde_json::json!({"data":{"project":{"host":root.join("project")},"home":{"host":root.join("guest-home")}}});
        let active = root.join("active-records");
        let pending = root.join("pending-records");
        assert!(require_private_plan_records(&[active.clone(), pending.clone()], &plan).is_ok());
        for share in ["project", "guest-home"] {
            let inside = root.join(share).join("records");
            assert!(
                require_private_plan_records(&[inside.clone(), pending.clone()], &plan).is_err()
            );
            assert!(require_private_plan_records(&[active.clone(), inside], &plan).is_err());
        }
        std::os::unix::fs::symlink(root.join("guest-home"), root.join("home-alias")).unwrap();
        plan["data"]["home"]["host"] = root
            .join("home-alias")
            .to_string_lossy()
            .into_owned()
            .into();
        assert!(require_private_plan_records(&[root.join("guest-home/records")], &plan).is_err());
        plan["data"]["home"]["host"] = "relative/home".into();
        assert!(require_private_plan_records(&[active], &plan).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    fn seed(directory: &Path) {
        create_directory(&directory.join("conversation-ledgers")).unwrap();
        create_directory(&directory.join("boundaries")).unwrap();
        create_directory(&directory.join("chatgpt")).unwrap();
        create_directory(&directory.join("profiles")).unwrap();
        for (name, bytes) in [
            ("settings.json", b"{\"profile\":\"codex\"}".as_slice()),
            (
                "projects.json",
                b"{\"schemaVersion\":1,\"projects\":[]}".as_slice(),
            ),
            (
                "projects.json.v1.backup",
                b"original project records\n".as_slice(),
            ),
            (
                "settings.json.v1.backup",
                b"original settings records\n".as_slice(),
            ),
            ("interface.json", b"{\"sidebarWidth\":360}".as_slice()),
            (
                "installation.json",
                b"{\"completed\":[\"update\"]}".as_slice(),
            ),
            (
                "conversation-ledgers/project.json",
                b"{\"saved\":\"usage\"}".as_slice(),
            ),
            (
                "conversation-ledgers/project.json.v1.backup",
                b"immutable\x00original\n".as_slice(),
            ),
            ("boundaries/project.json", b"{\"passed\":true}".as_slice()),
            (
                "profiles/tugboat-claude.yaml",
                b"managed profile".as_slice(),
            ),
            (
                "chatgpt/session.json",
                b"{\"synthetic\":\"private account record\"}".as_slice(),
            ),
        ] {
            fs::write(directory.join(name), bytes).unwrap();
        }
    }

    #[test]
    fn records_moves_keep_both_environment_ledgers_and_migration_backups() {
        let (root, default, legacy) = fixture("agent-environments");
        let location = DataLocation::open(default.clone(), legacy.clone());
        create_directory(&root.join("project")).unwrap();
        create_directory(&root.join("drive")).unwrap();
        let mut registry = crate::projects::Registry::default();
        let project = registry
            .register(&root.join("project"), "saved-codex")
            .unwrap();
        let project_file = default.join("projects.json");
        storage::write(
            &project_file,
            &crate::projects::migration_tests::legacy_registry(&registry),
        )
        .unwrap();
        storage::write(&default.join("settings.json"),&serde_json::json!({"schemaVersion":1,"profile":"saved-codex","allowedHosts":["private.example"]})).unwrap();
        let records = RecordsDirectory {
            directory: default.clone(),
            id: location
                .saved
                .lock()
                .unwrap()
                .location
                .as_ref()
                .unwrap()
                .directory_id
                .clone(),
        };
        crate::desktop::settings::migrate(&default.join("settings.json"), &records).unwrap();
        crate::projects::migrate(&project_file, &records).unwrap();
        registry = crate::projects::Registry::load(&project_file).unwrap();
        let added = registry
            .ensure_environment(
                &project.id,
                crate::agents::AgentKind::ClaudeCode,
                &crate::desktop::settings::Settings::default(),
            )
            .unwrap();
        let claude = registry
            .environment(&project.id, &added.environment_id)
            .unwrap();
        storage::write(&project_file, &registry).unwrap();
        let codex_ledger = project.ledger_file(&default).unwrap();
        let claude_ledger = claude.ledger_file(&default).unwrap();
        storage::write(
            &codex_ledger,
            &serde_json::json!({"saved":"codex","costUsd":0.1234}),
        )
        .unwrap();
        storage::write(
            &claude_ledger,
            &serde_json::json!({"saved":"claude","state":"archived"}),
        )
        .unwrap();
        fs::write(
            storage::v1_backup_path(&codex_ledger),
            b"original immutable ledger",
        )
        .unwrap();
        let relative = [
            PathBuf::from("projects.json"),
            PathBuf::from("settings.json"),
            PathBuf::from("projects.json.v1.backup"),
            PathBuf::from("settings.json.v1.backup"),
            codex_ledger.strip_prefix(&default).unwrap().to_owned(),
            claude_ledger.strip_prefix(&default).unwrap().to_owned(),
            storage::v1_backup_path(&codex_ledger)
                .strip_prefix(&default)
                .unwrap()
                .to_owned(),
        ];
        let originals: Vec<_> = relative
            .iter()
            .map(|path| fs::read(default.join(path)).unwrap())
            .collect();
        let target = root.join("drive");
        location.schedule(&target, false).unwrap();
        drop(location);
        let reopened = DataLocation::open(default.clone(), legacy);
        assert_eq!(reopened.status().unwrap().directory, Some(target.clone()));
        for (path, bytes) in relative.iter().zip(originals) {
            assert_eq!(fs::read(target.join(path)).unwrap(), bytes);
            assert!(!default.join(path).exists());
        }
        assert_eq!(
            crate::projects::Registry::load(&target.join("projects.json"))
                .unwrap()
                .environments(&project.id)
                .unwrap()
                .iter()
                .count(),
            2
        );
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn records_moves_preserve_private_chatgpt_storage_without_a_second_copy() {
        let (root, default, legacy) = fixture("chatgpt-move");
        let target = root.join("drive");
        create_directory(&target).unwrap();
        let location = DataLocation::open(default.clone(), legacy.clone());
        seed(&default);
        let original = fs::read(default.join("chatgpt/session.json")).unwrap();
        location.schedule(&target, false).unwrap();
        drop(location);
        let moved = DataLocation::open(default.clone(), legacy);
        assert_eq!(moved.status().unwrap().directory, Some(target.clone()));
        assert_eq!(
            fs::read(target.join("chatgpt/session.json")).unwrap(),
            original
        );
        assert_eq!(
            fs::metadata(target.join("chatgpt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(target.join("chatgpt/session.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(!default.join("chatgpt").exists());
        drop(moved);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn records_moves_preserve_private_claude_storage_without_a_second_copy() {
        let (root, default, legacy) = fixture("claude-provider-move");
        let target = root.join("drive");
        create_directory(&target).unwrap();
        let location = DataLocation::open(default.clone(), legacy.clone());
        seed(&default);
        create_directory(&default.join("claude")).unwrap();
        let original = br#"{"schemaVersion":1,"generation":3,"account":{"state":"disconnected"},"bindings":[]}"#;
        storage::write_bytes(&default.join("claude/session.json"), original).unwrap();
        location.schedule(&target, false).unwrap();
        drop(location);
        let moved = DataLocation::open(default.clone(), legacy);
        assert_eq!(moved.status().unwrap().directory, Some(target.clone()));
        assert_eq!(fs::read(target.join("claude/session.json")).unwrap(), original);
        assert_eq!(fs::metadata(target.join("claude")).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(target.join("claude/session.json")).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(!default.join("claude").exists());
        drop(moved);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_project_registered_after_scheduling_cannot_receive_the_shared_account_on_restart() {
        let (root, default, legacy) = fixture("private-target");
        let project = root.join("project");
        let target = project.join("records");
        create_directory(&target).unwrap();
        let location = DataLocation::open(default.clone(), legacy.clone());
        seed(&default);
        location.schedule(&target, false).unwrap();
        let mut registry = crate::projects::Registry::load(&default.join("projects.json")).unwrap();
        registry.register(&project, "codex").unwrap();
        storage::write(&default.join("projects.json"), &registry).unwrap();
        drop(location);
        let blocked = DataLocation::open(default.clone(), legacy);
        assert_eq!(blocked.status().unwrap().directory, Some(default.clone()));
        assert!(blocked
            .status()
            .unwrap()
            .message
            .unwrap()
            .contains("outside all project folders"));
        assert!(!target.join("chatgpt").exists());
        assert!(default.join("chatgpt/session.json").exists());
        drop(blocked);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn first_launch_moves_legacy_records_and_preserves_immutable_backups_byte_for_byte() {
        let (root, default, legacy) = fixture("legacy");
        seed(&legacy);
        fs::write(legacy.join("settings.json.lock"), "old lock").unwrap();
        let location = DataLocation::open(default.clone(), legacy.clone());
        let status = location.status().unwrap();
        assert!(status.available);
        assert_eq!(status.directory, Some(default.clone()));
        assert!(status.message.is_none());
        assert_eq!(
            fs::read(default.join("conversation-ledgers/project.json.v1.backup")).unwrap(),
            b"immutable\x00original\n"
        );
        assert_eq!(
            fs::read(default.join("interface.json")).unwrap(),
            b"{\"sidebarWidth\":360}"
        );
        assert!(!legacy.exists());
        assert!(default.join(POINTER).exists());
        drop(location);
        let reopened = DataLocation::open(default, legacy);
        assert!(reopened.status().unwrap().available);
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_move_waits_for_restart_and_leaves_only_the_location_pointer_at_application_support() {
        let (root, default, legacy) = fixture("move");
        let target = root.join("Removable storage/Tugboat records");
        create_directory(&target).unwrap();
        let location = DataLocation::open(default.clone(), legacy.clone());
        seed(&default);
        let before = fs::read(default.join("projects.json")).unwrap();
        location.schedule(&target, false).unwrap();
        assert_eq!(location.status().unwrap().directory, Some(default.clone()));
        assert_eq!(
            location.status().unwrap().pending_directory,
            Some(target.clone())
        );
        assert!(!target.join("projects.json").exists());
        assert_eq!(fs::read(default.join("projects.json")).unwrap(), before);
        drop(location);
        let moved = DataLocation::open(default.clone(), legacy.clone());
        assert_eq!(moved.status().unwrap().directory, Some(target.clone()));
        assert!(moved.status().unwrap().pending_directory.is_none());
        assert_eq!(fs::read(target.join("projects.json")).unwrap(), before);
        assert_eq!(
            fs::read(target.join("conversation-ledgers/project.json.v1.backup")).unwrap(),
            b"immutable\x00original\n"
        );
        let mut remaining: Vec<_> = fs::read_dir(&default)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        remaining.sort();
        assert_eq!(remaining, [POINTER, "storage-location.json.lock"]);
        moved.schedule(&default, false).unwrap();
        drop(moved);
        let restored = DataLocation::open(default.clone(), legacy);
        assert_eq!(restored.status().unwrap().directory, Some(default.clone()));
        assert!(restored.status().unwrap().message.is_none());
        assert_eq!(fs::read(default.join("projects.json")).unwrap(), before);
        assert!(!target.exists());
        drop(restored);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unavailable_destination_keeps_the_original_records_and_retries_on_the_next_launch() {
        let (root, default, legacy) = fixture("offline-target");
        let target = root.join("drive");
        create_directory(&target).unwrap();
        let location = DataLocation::open(default.clone(), legacy.clone());
        seed(&default);
        location.schedule(&target, false).unwrap();
        fs::rename(&target, root.join("disconnected")).unwrap();
        drop(location);
        let deferred = DataLocation::open(default.clone(), legacy.clone());
        let status = deferred.status().unwrap();
        assert!(status.available);
        assert_eq!(status.directory, Some(default.clone()));
        assert!(status.message.unwrap().contains("unavailable"));
        assert!(
            !target.exists(),
            "a missing drive must never be recreated as an empty data folder"
        );
        assert!(default.join("projects.json").exists());
        drop(deferred);
        fs::rename(root.join("disconnected"), &target).unwrap();
        let recovered = DataLocation::open(default, legacy);
        assert_eq!(recovered.status().unwrap().directory, Some(target));
        assert!(recovered.status().unwrap().message.is_none());
        drop(recovered);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_missing_active_drive_never_falls_back_and_can_be_located_at_its_new_path() {
        let (root, default, legacy) = fixture("offline-source");
        let target = root.join("original-drive");
        let reconnected = root.join("renamed-drive");
        create_directory(&target).unwrap();
        let location = DataLocation::open(default.clone(), legacy.clone());
        seed(&default);
        location.schedule(&target, false).unwrap();
        drop(location);
        let moved = DataLocation::open(default.clone(), legacy.clone());
        let id = marker(&target).unwrap().id;
        let guard = RecordsDirectory {
            directory: target.clone(),
            id,
        };
        fs::rename(&target, &reconnected).unwrap();
        assert!(guard.verify().is_err());
        assert!(!target.exists());
        drop(moved);
        let offline = DataLocation::open(default.clone(), legacy.clone());
        assert!(!offline.status().unwrap().available);
        assert!(!default.join("settings.json").exists());
        assert!(!target.exists());
        let partial = root.join("partial-copy");
        create_directory(&partial).unwrap();
        storage::write(
            &partial.join(MARKER),
            &DirectoryMarker {
                schema_version: 1,
                id: marker(&reconnected).unwrap().id,
                state: MarkerState::Incomplete,
            },
        )
        .unwrap();
        fs::write(partial.join("settings.json"), "partial").unwrap();
        let pointer_before = fs::read(default.join(POINTER)).unwrap();
        assert!(offline
            .schedule(&partial, true)
            .unwrap_err()
            .contains("incomplete"));
        assert_eq!(fs::read(default.join(POINTER)).unwrap(), pointer_before);
        offline.schedule(&reconnected, true).unwrap();
        drop(offline);
        let recovered = DataLocation::open(default, legacy);
        assert!(recovered.status().unwrap().available);
        assert_eq!(recovered.status().unwrap().directory, Some(reconnected));
        drop(recovered);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn partial_copies_resume_without_changing_the_original_and_symlinks_are_preserved_on_failure() {
        let (root, default, legacy) = fixture("interrupted");
        let target = root.join("target");
        create_directory(&target).unwrap();
        let location = DataLocation::open(default.clone(), legacy.clone());
        seed(&default);
        location.schedule(&target, false).unwrap();
        let id = marker(&default).unwrap().id;
        storage::write(
            &target.join(MARKER),
            &DirectoryMarker {
                schema_version: 1,
                id,
                state: MarkerState::Incomplete,
            },
        )
        .unwrap();
        fs::write(target.join("settings.json"), b"half-written").unwrap();
        create_directory(&target.join("conversation-ledgers")).unwrap();
        fs::write(
            target.join("conversation-ledgers/stale.json"),
            "stale partial copy",
        )
        .unwrap();
        symlink(
            root.join("unrelated"),
            default.join("conversation-ledgers/link.json"),
        )
        .unwrap();
        drop(location);
        let failed = DataLocation::open(default.clone(), legacy.clone());
        assert_eq!(failed.status().unwrap().directory, Some(default.clone()));
        assert!(failed
            .status()
            .unwrap()
            .message
            .unwrap()
            .contains("symbolic link"));
        assert!(default.join("projects.json").exists());
        assert_eq!(
            fs::read(target.join("settings.json")).unwrap(),
            fs::read(default.join("settings.json")).unwrap()
        );
        drop(failed);
        fs::remove_file(default.join("conversation-ledgers/link.json")).unwrap();
        let completed = DataLocation::open(default.clone(), legacy);
        assert_eq!(completed.status().unwrap().directory, Some(target.clone()));
        assert!(completed.status().unwrap().message.is_none());
        assert!(!target.join("conversation-ledgers/stale.json").exists());
        assert_eq!(
            fs::read(target.join("conversation-ledgers/project.json.v1.backup")).unwrap(),
            b"immutable\x00original\n"
        );
        assert!(!default.join("settings.json").exists());
        drop(completed);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nonempty_nested_or_different_record_destinations_are_rejected_without_switching() {
        let (root, default, legacy) = fixture("invalid-target");
        let location = DataLocation::open(default.clone(), legacy.clone());
        seed(&default);
        let target = root.join("nonempty");
        create_directory(&target).unwrap();
        fs::write(target.join("keep.txt"), "kept").unwrap();
        assert!(location
            .schedule(&target, false)
            .unwrap_err()
            .contains("empty folder"));
        create_directory(&default.join("nested")).unwrap();
        assert!(location.schedule(&default.join("nested"), false).is_err());
        assert!(location.schedule(&root, false).is_err());
        assert!(location.status().unwrap().pending_directory.is_none());
        assert_eq!(fs::read_to_string(target.join("keep.txt")).unwrap(), "kept");
        drop(location);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn committed_moves_finish_cleanup_after_a_restart_but_never_delete_changed_originals() {
        let (root, default, legacy) = fixture("cleanup");
        let target = root.join("target");
        create_directory(&target).unwrap();
        let location = DataLocation::open(default.clone(), legacy.clone());
        seed(&default);
        copy_record(&default.join(MARKER), &target.join(MARKER)).unwrap();
        for name in RECORDS {
            if default.join(name).exists() {
                copy_record(&default.join(name), &target.join(name)).unwrap();
            }
        }
        let mut committed = location.saved.lock().unwrap().location.clone().unwrap();
        committed.directory = target.clone();
        committed.previous_directory = Some(default.clone());
        storage::write(&location.pointer, &committed).unwrap();
        fs::write(default.join("settings.json"), "changed after commit").unwrap();
        drop(location);
        let preserved = DataLocation::open(default.clone(), legacy.clone());
        assert_eq!(preserved.status().unwrap().directory, Some(target.clone()));
        assert!(preserved
            .status()
            .unwrap()
            .message
            .unwrap()
            .contains("changed after copying"));
        assert_eq!(
            fs::read_to_string(default.join("settings.json")).unwrap(),
            "changed after commit"
        );
        let next = root.join("next");
        create_directory(&next).unwrap();
        let pointer_before = fs::read(&preserved.pointer).unwrap();
        assert!(preserved.schedule(&next, false).unwrap_err().contains("cleanup"));
        assert_eq!(fs::read(&preserved.pointer).unwrap(), pointer_before);
        drop(preserved);
        fs::copy(target.join("settings.json"), default.join("settings.json")).unwrap();
        let cleaned = DataLocation::open(default.clone(), legacy);
        assert!(cleaned.status().unwrap().message.is_none());
        assert!(!default.join("settings.json").exists());
        drop(cleaned);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_pointers_and_different_directory_markers_do_not_create_a_new_dataset() {
        let (root, default, legacy) = fixture("pointer");
        create_directory(&default).unwrap();
        fs::write(default.join(POINTER), "not json").unwrap();
        let broken = DataLocation::open(default.clone(), legacy.clone());
        assert!(!broken.status().unwrap().available);
        assert!(!default.join(MARKER).exists());
        drop(broken);
        fs::remove_file(default.join(POINTER)).unwrap();
        let location = DataLocation::open(default.clone(), legacy.clone());
        seed(&default);
        let target = root.join("target");
        create_directory(&target).unwrap();
        location.schedule(&target, false).unwrap();
        storage::write(
            &target.join(MARKER),
            &DirectoryMarker {
                schema_version: 1,
                id: "f".repeat(32),
                state: MarkerState::Incomplete,
            },
        )
        .unwrap();
        drop(location);
        let rejected = DataLocation::open(default.clone(), legacy);
        assert_eq!(rejected.status().unwrap().directory, Some(default));
        assert!(rejected
            .status()
            .unwrap()
            .message
            .unwrap()
            .contains("different Tugboat"));
        assert!(!target.join("settings.json").exists());
        drop(rejected);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn self_moves_and_cleanup_aliases_never_delete_the_active_records() {
        let (root, default, legacy) = fixture("aliases");
        let location = DataLocation::open(default.clone(), legacy.clone());
        seed(&default);
        let mut pending = location.saved.lock().unwrap().location.clone().unwrap();
        pending.pending = Some(PendingLocation {
            directory: default.clone(),
            kind: ChangeKind::Move,
        });
        storage::write(&location.pointer, &pending).unwrap();
        drop(location);
        let preserved = DataLocation::open(default.clone(), legacy.clone());
        assert!(preserved.status().unwrap().pending_directory.is_none());
        assert!(default.join("settings.json").exists());
        let alias = root.join("alias");
        symlink(&default, &alias).unwrap();
        let mut committed = preserved.saved.lock().unwrap().location.clone().unwrap();
        committed.previous_directory = Some(alias.clone());
        storage::write(&preserved.pointer, &committed).unwrap();
        drop(preserved);
        let guarded = DataLocation::open(default.clone(), legacy);
        assert!(guarded.status().unwrap().available);
        assert!(guarded
            .status()
            .unwrap()
            .message
            .unwrap()
            .contains("points to the active records"));
        assert!(default.join("settings.json").exists());
        assert!(default
            .join("conversation-ledgers/project.json.v1.backup")
            .exists());
        drop(guarded);
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
fn move_checkpoint(phase: &str, pointer: &Path) {
    let expected = std::env::var_os("TUGBOAT_SQLITE_MOVE_POINTER");
    if expected.as_deref() == Some(pointer.as_os_str())
        && std::env::var("TUGBOAT_SQLITE_MOVE_PHASE").as_deref() == Ok(phase)
    {
        std::process::exit(87);
    }
}

#[cfg(test)]
mod sqlite_tests;

#[cfg(test)]
mod p06_volume_tests;


#[cfg(test)]
mod review_move {
    use super::*;
    #[test]
    fn recheck_a_new_move_cannot_replace_an_outstanding_cleanup_receipt() {
        let root = std::env::temp_dir().join(format!("tugboat-move-review-{}", std::process::id()));
        create_directory(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let default = root.join("current"); let legacy = root.join("legacy");
        let target = root.join("next"); create_directory(&target).unwrap();
        let first = DataLocation::open(default.clone(),legacy.clone());
        let mut committed = first.saved.lock().unwrap().location.clone().unwrap();
        committed.previous_directory = Some(root.join("unavailable-previous-drive"));
        storage::write(&first.pointer, &committed).unwrap(); drop(first);
        let recovered = DataLocation::open(default,legacy);
        assert!(recovered.saved.lock().unwrap().location.as_ref().unwrap().previous_directory.is_some());
        let scheduled = recovered.schedule(&target,false);
        let next = storage::read::<Location>(&recovered.pointer).unwrap().unwrap();
        let lost = if scheduled.is_ok() {
            drop(recovered);
            let moved = DataLocation::open(root.join("current"),root.join("legacy"));
            let location = moved.saved.lock().unwrap().location.clone().unwrap();
            location.directory == target && location.previous_directory != committed.previous_directory
        } else { false };
        std::fs::remove_dir_all(root).unwrap();
        assert!(!lost, "second commit discarded the first cleanup receipt (pending persisted: {})",next.pending.is_some());
    }
}
