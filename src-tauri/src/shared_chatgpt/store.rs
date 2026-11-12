use super::credentials::Credentials;
use crate::storage;
use serde::{Deserialize, Serialize};
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const INVALID: &str = "The shared ChatGPT record could not be read. Its contents were preserved.";
const LIMIT: u64 = 1_048_576;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Binding {
    pub project_id: String,
    pub directory: PathBuf,
    pub auth_hash: Option<String>,
    pub access_hash: Option<String>,
    pub pending: bool,
    pub identity: (u64, u64),
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingLogin {
    pub project_id: String,
    pub nonce: String,
    pub directory: PathBuf,
    pub identity: (u64, u64),
    pub imported: bool,
    pub cancelled: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Session {
    pub schema_version: u32,
    pub generation: u64,
    pub credentials: Option<Credentials>,
    pub disconnected: bool,
    pub reconnect_required: bool,
    pub bindings: Vec<Binding>,
    pub pending_login: Option<PendingLogin>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            schema_version: 1,
            generation: 0,
            credentials: None,
            disconnected: false,
            reconnect_required: false,
            bindings: Vec::new(),
            pending_login: None,
        }
    }
}

impl Session {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1
            || self.bindings.len() > 256
            || (self.disconnected && self.credentials.is_some())
        {
            return Err(INVALID.into());
        }
        if let Some(credentials) = &self.credentials {
            credentials.validate().map_err(|_| INVALID)?;
        }
        if let Some(login) = &self.pending_login {
            if (login.imported && login.cancelled)
                || login.project_id.len() != 32
                || !login
                    .project_id
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
                || login.nonce.len() != 32
                || !login.nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
                || !login.directory.is_absolute()
                || login
                    .directory
                    .file_name()
                    .is_none_or(|name| name != login.nonce.as_str())
                || login
                    .directory
                    .parent()
                    .and_then(Path::file_name)
                    .is_none_or(|name| name != ".tugboat-chatgpt-login")
                || login
                    .directory
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir))
            {
                return Err(INVALID.into());
            }
        }
        let mut projects = std::collections::HashSet::new();
        for binding in &self.bindings {
            if binding.project_id.len() != 32
                || !binding
                    .project_id
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
                || !binding.directory.is_absolute()
                || binding
                    .directory
                    .file_name()
                    .is_none_or(|name| name != ".codex")
                || binding
                    .directory
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir))
                || !projects.insert(&binding.project_id)
                || binding.auth_hash.as_ref().is_some_and(|hash| {
                    hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
                || binding.access_hash.as_ref().is_some_and(|hash| {
                    hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            {
                return Err(INVALID.into());
            }
        }
        Ok(())
    }
}

pub fn read_private(file: &Path) -> Result<Option<Vec<u8>>, String> {
    let input = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(file)
    {
        Ok(input) => input,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(INVALID.into()),
    };
    read_handle(input).map(Some)
}

pub fn read_handle(mut input: fs::File) -> Result<Vec<u8>, String> {
    let metadata = input.metadata().map_err(|_| INVALID)?;
    if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() } || metadata.len() > LIMIT
    {
        return Err(INVALID.into());
    }
    let mut bytes = Vec::new();
    input
        .by_ref()
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| INVALID)?;
    if bytes.len() as u64 > LIMIT {
        return Err(INVALID.into());
    }
    Ok(bytes)
}

pub struct Store {
    file: PathBuf,
    root: PathBuf,
    identity: ((u64, u64), (u64, u64)),
    records_guard: Option<crate::data_location::RecordsDirectory>,
    _lock: storage::FileLock,
}

impl Store {
    pub fn for_app(app: &tauri::AppHandle) -> Result<Self, String> {
        let records_guard = crate::data_location::records_directory(app)?;
        let directory = crate::data_location::directory(app)?;
        let registry = crate::projects::Registry::load(&directory.join("projects.json"))?;
        crate::data_location::require_private_records(&directory, &registry)?;
        let mut store = Self::open(&directory)?;
        store.records_guard = Some(records_guard);
        store.verify()?;
        Ok(store)
    }

    pub fn open(records: &Path) -> Result<Self, String> {
        let root = fs::canonicalize(records).map_err(|_| "The records folder is unavailable.")?;
        let metadata = root.metadata().map_err(|_| INVALID)?;
        let root_identity = (metadata.dev(), metadata.ino());
        let directory = root.join("chatgpt");
        match DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("Could not create the private ChatGPT records folder.".into()),
        }
        let handle = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(&directory)
            .map_err(|_| INVALID)?;
        let metadata = handle.metadata().map_err(|_| INVALID)?;
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(INVALID.into());
        }
        let identity = (root_identity, (metadata.dev(), metadata.ino()));
        handle
            .set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(|_| INVALID)?;
        let file = directory.join("session.json");
        let lock = storage::lock(&file).map_err(|_| "Could not lock the shared ChatGPT record.")?;
        Ok(Self {
            file,
            root,
            identity,
            records_guard: None,
            _lock: lock,
        })
    }

    pub fn load(&self) -> Result<Session, String> {
        self.verify()?;
        let Some(bytes) = read_private(&self.file)? else {
            return Ok(Session::default());
        };
        let session: Session = serde_json::from_slice(&bytes).map_err(|_| INVALID)?;
        session.validate()?;
        self.require_private(&session)?;
        Ok(session)
    }

    pub fn save(&self, session: &Session) -> Result<(), String> {
        session.validate()?;
        self.require_private(session)?;
        self.verify()?;
        self.load()?;
        storage::write(&self.file, session)
            .map_err(|_| "Could not save the shared ChatGPT record.".into())
    }

    fn verify(&self) -> Result<(), String> {
        if let Some(records) = &self.records_guard {
            records.verify()?;
        }
        // Do not recreate the records root or write through a replaced private directory.
        let metadata = self
            .root
            .metadata()
            .map_err(|_| "The records folder is unavailable.")?;
        let directory = fs::symlink_metadata(self.file.parent().unwrap()).map_err(|_| INVALID)?;
        if (metadata.dev(), metadata.ino()) != self.identity.0
            || (directory.dev(), directory.ino()) != self.identity.1
            || !directory.is_dir()
            || fs::canonicalize(self.file.parent().unwrap()).map_err(|_| INVALID)?
                != self.root.join("chatgpt")
        {
            return Err("The ChatGPT records folder changed or is unavailable.".into());
        }
        Ok(())
    }

    fn require_private(&self, session: &Session) -> Result<(), String> {
        if session.bindings.iter().any(|binding| {
            binding
                .directory
                .parent()
                .is_some_and(|home| self.root.starts_with(home))
        }) || session
            .pending_login
            .as_ref()
            .and_then(|login| login.directory.parent())
            .and_then(Path::parent)
            .is_some_and(|home| self.root.starts_with(home))
        {
            return Err("The records folder must be outside Brig guest homes.".into());
        }
        Ok(())
    }

    pub fn verify_records(
        &self,
        records: &crate::data_location::RecordsDirectory,
    ) -> Result<(), String> {
        self.verify()?;
        records.verify_at(&self.root)
    }
}
