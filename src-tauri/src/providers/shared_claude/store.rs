use super::credentials::Credentials;
use crate::{projects::EnvironmentId, storage, storage::data_location::RecordsDirectory};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::PathBuf,
};

const INVALID: &str = "The shared Claude record could not be read. Its contents were preserved.";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Binding {
    pub project_id: String,
    pub environment_id: EnvironmentId,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(
    tag = "state",
    content = "credentials",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub(super) enum Account {
    #[default]
    Disconnected,
    Connected(Credentials),
    NeedsSignIn(Credentials),
}

impl Account {
    pub fn credentials(&self) -> Option<&Credentials> {
        match self {
            Self::Connected(credentials) | Self::NeedsSignIn(credentials) => Some(credentials),
            _ => None,
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Session {
    pub schema_version: u32,
    pub generation: u64,
    pub account: Account,
    pub bindings: Vec<Binding>,
}

impl Session {
    pub fn empty() -> Self {
        Self {
            schema_version: 1,
            ..Self::default()
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 || self.bindings.len() > 256 {
            return Err(INVALID.into());
        }
        if let Some(credentials) = self.account.credentials() {
            credentials.validate()?;
        }
        let mut environments = std::collections::HashSet::new();
        for binding in &self.bindings {
            if binding.project_id.len() != 32
                || !binding
                    .project_id
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
                || binding.environment_id
                    != EnvironmentId::for_agent(
                        &binding.project_id,
                        crate::agents::AgentKind::ClaudeCode,
                    )
                || !environments.insert(binding.environment_id.clone())
            {
                return Err(INVALID.into());
            }
        }
        Ok(())
    }
    pub fn advance(&mut self) -> Result<(), String> {
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or("The Claude account generation is invalid.")?;
        Ok(())
    }
    pub fn connect(&mut self, credentials: Credentials) -> Result<(), String> {
        credentials.validate()?;
        self.advance()?;
        self.account = Account::Connected(credentials);
        Ok(())
    }
    pub fn require_sign_in(&mut self) -> Result<(), String> {
        let credentials = self
            .account
            .credentials()
            .cloned()
            .ok_or("Connect Claude before renewing sign-in.")?;
        self.advance()?;
        self.account = Account::NeedsSignIn(credentials);
        Ok(())
    }
    pub fn disconnect(&mut self) -> Result<(), String> {
        self.advance()?;
        self.account = Account::Disconnected;
        Ok(())
    }
}

pub(super) struct Store {
    records: RecordsDirectory,
    directory: (PathBuf, (u64, u64)),
    _lock: storage::FileLock,
}

impl Store {
    pub fn for_app(app: &tauri::AppHandle) -> Result<Self, String> {
        let records = crate::storage::data_location::records_directory(app)?;
        let registry = crate::projects::Registry::load(&records.path().join("projects.json"))?;
        crate::storage::data_location::require_private_records(records.path(), &registry)?;
        Self::open(records)
    }
    pub fn open(records: RecordsDirectory) -> Result<Self, String> {
        records.verify()?;
        let directory = records.path().join("claude");
        match fs::DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("Could not create the private Claude records folder.".into()),
        }
        let metadata = fs::symlink_metadata(&directory).map_err(|_| INVALID)?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || fs::canonicalize(&directory).ok().as_ref() != Some(&directory)
        {
            return Err(INVALID.into());
        }
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).map_err(|_| INVALID)?;
        let lock = storage::lock(&directory.join("session.json"))?;
        let store = Self {
            records,
            directory: (directory, (metadata.dev(), metadata.ino())),
            _lock: lock,
        };
        store.verify()?;
        Ok(store)
    }
    pub fn verify(&self) -> Result<(), String> {
        self.records.verify()?;
        let metadata = fs::symlink_metadata(&self.directory.0).map_err(|_| INVALID)?;
        if !metadata.is_dir()
            || (metadata.dev(), metadata.ino()) != self.directory.1
            || fs::canonicalize(&self.directory.0).ok().as_ref() != Some(&self.directory.0)
        {
            return Err(INVALID.into());
        }
        Ok(())
    }
    pub fn verify_records(&self, records: &RecordsDirectory) -> Result<(), String> {
        self.verify()?;
        records.verify_at(self.records.path())
    }
    pub fn load(&self) -> Result<Session, String> {
        self.verify()?;
        let Some(bytes) =
            crate::providers::shared_chatgpt::read_private(&self.directory.0.join("session.json"))
                .map_err(|_| INVALID)?
        else {
            return Ok(Session::empty());
        };
        let session: Session = serde_json::from_slice(&bytes).map_err(|_| INVALID)?;
        session.validate()?;
        Ok(session)
    }
    pub fn save(&self, session: &Session) -> Result<(), String> {
        session.validate()?;
        self.load()?;
        self.verify()?;
        storage::write(&self.directory.0.join("session.json"), session).map_err(|_| {
            "Could not save the shared Claude account. Check the records folder and retry.".into()
        })
    }
}
