use crate::projects::{EnvironmentId, Project};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

type AccountKey = (String, EnvironmentId);

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    SignedIn,
    SignedOut,
    SigningIn,
    EnvironmentStopped,
    Unknown,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub project_id: String,
    pub environment_id: EnvironmentId,
    pub state: State,
    pub message: String,
    pub terminal_identifier: Option<u64>,
}

impl Status {
    pub fn new(
        project: &Project,
        state: State,
        message: &str,
        terminal_identifier: Option<u64>,
    ) -> Self {
        Self {
            project_id: project.id.clone(),
            environment_id: project.environment_id.clone(),
            state,
            message: message.into(),
            terminal_identifier,
        }
    }

    pub fn verified(project: &Project, signed_in: bool) -> Self {
        if signed_in {
            return Self::new(
                project,
                State::SignedIn,
                "Signed in to Claude in this environment.",
                None,
            );
        }
        Self::new(
            project,
            State::SignedOut,
            "Sign in to Claude in this environment.",
            None,
        )
    }
}

struct PendingLogin {
    identifier: u64,
    fingerprint: String,
}

struct Entry {
    status: Status,
    pending: Option<PendingLogin>,
}

#[derive(Default)]
pub struct Accounts {
    entries: Mutex<HashMap<AccountKey, Entry>>,
    operations: Mutex<HashMap<AccountKey, Arc<tauri::async_runtime::Mutex<()>>>>,
}

impl Accounts {
    pub fn operation(
        &self,
        project: &Project,
    ) -> Result<Arc<tauri::async_runtime::Mutex<()>>, String> {
        Ok(self
            .operations
            .lock()
            .map_err(|_| "Could not lock this Claude account.")?
            .entry((project.id.clone(), project.environment_id.clone()))
            .or_default()
            .clone())
    }

    pub fn pending(&self, project: &Project) -> Result<Option<Status>, String> {
        Ok(self
            .entries
            .lock()
            .map_err(|_| "Could not read this Claude account.")?
            .get(&(project.id.clone(), project.environment_id.clone()))
            .filter(|entry| entry.pending.is_some())
            .map(|entry| entry.status.clone()))
    }

    pub fn begin(
        &self,
        project: &Project,
        identifier: u64,
        fingerprint: &str,
    ) -> Result<Status, String> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| "Could not record Claude sign-in.")?;
        let key = (project.id.clone(), project.environment_id.clone());
        if entries
            .get(&key)
            .is_some_and(|entry| entry.pending.is_some())
        {
            return Err("Claude sign-in is already running in this environment.".into());
        }
        let status = Status::new(
            project,
            State::SigningIn,
            "Complete Claude sign-in in this environment's terminal.",
            Some(identifier),
        );
        entries.insert(
            key,
            Entry {
                status: status.clone(),
                pending: Some(PendingLogin {
                    identifier,
                    fingerprint: fingerprint.into(),
                }),
            },
        );
        Ok(status)
    }

    pub fn pending_fingerprint(
        &self,
        project: &Project,
        identifier: u64,
    ) -> Result<Option<String>, String> {
        Ok(self
            .entries
            .lock()
            .map_err(|_| "Could not read Claude sign-in.")?
            .get(&(project.id.clone(), project.environment_id.clone()))
            .and_then(|entry| entry.pending.as_ref())
            .filter(|pending| pending.identifier == identifier)
            .map(|pending| pending.fingerprint.clone()))
    }

    pub fn finish(
        &self,
        project: &Project,
        identifier: u64,
        result: Result<bool, String>,
    ) -> Result<Option<Status>, String> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| "Could not complete Claude sign-in.")?;
        let Some(entry) = entries.get_mut(&(project.id.clone(), project.environment_id.clone()))
        else {
            return Ok(None);
        };
        if entry
            .pending
            .as_ref()
            .is_none_or(|pending| pending.identifier != identifier)
        {
            return Ok(None);
        }
        entry.pending = None;
        entry.status = match result {
            Ok(signed_in) => Status::verified(project, signed_in),
            Err(message) => Status::new(project, State::Unknown, &message, Some(identifier)),
        };
        Ok(Some(entry.status.clone()))
    }

    pub fn invalidate(&self, project: &Project) -> Result<Status, String> {
        let status = Status::new(
            project,
            State::EnvironmentStopped,
            "Prepare this Claude environment before checking its account.",
            None,
        );
        self.entries
            .lock()
            .map_err(|_| "Could not clear this Claude account state.")?
            .insert(
                (project.id.clone(), project.environment_id.clone()),
                Entry {
                    status: status.clone(),
                    pending: None,
                },
            );
        Ok(status)
    }

    pub fn publish(&self, project: &Project, status: Status) -> Result<Status, String> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| "Could not update this Claude account.")?;
        let key = (project.id.clone(), project.environment_id.clone());
        if let Some(entry) = entries.get(&key).filter(|entry| entry.pending.is_some()) {
            return Ok(entry.status.clone());
        }
        entries.insert(
            key,
            Entry {
                status: status.clone(),
                pending: None,
            },
        );
        Ok(status)
    }
}
