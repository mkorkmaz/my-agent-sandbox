use super::login_phase::LoginPhase;
use super::renewal::{CredentialWrite, RenewalRetry};
use super::session_state::AccountState;
use super::{
    credentials::Credentials,
    guest, http,
    store::{PendingLogin, Session, Store},
};
use crate::{
    environment::{self, CheckedProject},
    projects,
    storage::data_location,
    terminal,
};
use serde::Serialize;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use tauri::{AppHandle, Emitter};

#[derive(Default)]
pub struct Runtime {
    pending_write: CredentialWrite,
    retry: RenewalRetry,
    error: Option<String>,
}

#[derive(Default)]
pub struct SharedChatgpt(pub Arc<tauri::async_runtime::Mutex<Runtime>>);

impl Runtime {
    pub(crate) fn reconnect_records(
        &mut self,
        records: &data_location::RecordsDirectory,
    ) -> Result<(), String> {
        records.verify()?;
        let CredentialWrite::Pending {
            records: previous, ..
        } = &mut self.pending_write
        else {
            return Ok(());
        };
        if previous.id() != records.id() {
            return Err("The pending ChatGPT write belongs to a different records dataset.".into());
        }
        *previous = records.clone();
        // Saved staging paths belong to their existing guest, not this folder.
        Ok(())
    }

    pub fn restore(&mut self, store: &Store) -> Result<(), String> {
        let CredentialWrite::Pending { records, session } = &self.pending_write else {
            return Ok(());
        };
        store.verify_records(records)?;
        store.save(session)?;
        self.pending_write = CredentialWrite::Clean;
        Ok(())
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub project_id: String,
    pub name: String,
    pub email: Option<String>,
    pub path: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub generation: u64,
    pub pending_project_id: Option<String>,
    pub pending_environment_id: Option<crate::projects::EnvironmentId>,
    pub pending_finalizing: bool,
    pub state: &'static str,
    pub email: Option<String>,
    pub message: String,
    pub candidates: Vec<Candidate>,
}

pub fn status(session: &Session) -> Status {
    let credentials = session.credentials.as_ref();
    let (state, mut message) = match session.account_state() {
        AccountState::SigningIn => (
            "signing-in",
            "Complete the ChatGPT sign-in in your browser.",
        ),
        AccountState::NeedsSignIn => (
            "needs-sign-in",
            "The shared ChatGPT session is no longer valid. Sign in again.",
        ),
        AccountState::SignedIn => (
            "signed-in",
            "Your shared ChatGPT account is available to all projects.",
        ),
        _ => (
            "signed-out",
            "Sign in once to use your ChatGPT account in every project.",
        ),
    };
    if let Some(login) = &session.pending_login {
        message = match login.phase() {
            LoginPhase::Imported => "Finishing the shared ChatGPT connection…",
            LoginPhase::Cancelled => "Finishing cancelled ChatGPT sign-in cleanup…",
            _ => message,
        };
    }
    Status {
        generation: session.generation,
        pending_environment_id: session
            .pending_login
            .as_ref()
            .map(PendingLogin::environment_id),
        pending_finalizing: session
            .pending_login
            .as_ref()
            .is_some_and(|login| login.phase() == LoginPhase::Imported),
        pending_project_id: session
            .pending_login
            .as_ref()
            .map(|login| login.project_id.clone()),
        state,
        email: credentials.and_then(Credentials::email),
        message: message.into(),
        candidates: Vec::new(),
    }
}

pub fn load(app: &AppHandle, runtime: &mut Runtime) -> Result<(Store, Session), String> {
    let store = Store::for_app(app)?;
    runtime.restore(&store)?;
    let session = store.load()?;
    Ok((store, session))
}

fn save_after_request(
    records: data_location::RecordsDirectory,
    runtime: &mut Runtime,
    store: &Store,
    session: &Session,
) -> Result<(), String> {
    runtime.pending_write = CredentialWrite::Pending {
        records,
        session: session.clone(),
    };
    store.save(session)?;
    runtime.pending_write = CredentialWrite::Clean;
    Ok(())
}

pub fn refresh(
    app: &AppHandle,
    runtime: &mut Runtime,
    store: &Store,
    session: &mut Session,
    force: bool,
) -> Result<(), String> {
    let records = data_location::records_directory(app)?;
    let result = refresh_with(runtime, store, session, force, records, http::refresh);
    if result.is_err() && session.reconnect_required {
        let _ = app.emit("shared-chatgpt-changed", status(session));
    }
    result
}

pub fn refresh_with(
    runtime: &mut Runtime,
    store: &Store,
    session: &mut Session,
    force: bool,
    records: data_location::RecordsDirectory,
    request: impl FnOnce(&Credentials) -> Result<Credentials, http::Failure>,
) -> Result<(), String> {
    if session.pending_login.is_some() {
        return Err("Complete or cancel the current ChatGPT sign-in first.".into());
    }
    if session.reconnect_required {
        return Err(http::Failure::SignIn.message());
    }
    let Some(credentials) = &session.credentials else {
        return Ok(());
    };
    if !force && credentials.expires_at()? > chrono::Utc::now().timestamp() + 600 {
        return Ok(());
    }
    if !force && runtime.retry.waiting(Instant::now()) {
        return Err(runtime
            .error
            .clone()
            .unwrap_or_else(|| "ChatGPT sign-in renewal is waiting to retry.".into()));
    }
    session.generation_after(1)?;
    guest::publish(store, session)?;
    match request(session.credentials.as_ref().unwrap()) {
        Ok(credentials) => {
            session.connect(credentials)?;
            save_after_request(records, runtime, store, session)?;
            runtime.retry.succeeded();
            runtime.error = None;
            guest::publish(store, session)
        }
        Err(error) => {
            if error.requires_sign_in() {
                session.require_sign_in()?;
                save_after_request(records, runtime, store, session)?;
            }
            runtime.retry.failed(Instant::now());
            runtime.error = Some(error.message());
            Err(error.message())
        }
    }
}

fn candidates(app: &AppHandle) -> Result<Vec<(CheckedProject, Credentials)>, String> {
    let registry = projects::Registry::load(&projects::file(app)?)?;
    let mut results = Vec::new();
    for project in registry.codex_projects()? {
        let Ok(checked) = environment::checked_project(app, &project.id, None) else {
            continue;
        };
        let Ok(directory) = guest::directory(&checked) else {
            continue;
        };
        if let Ok(Some(credentials)) = guest::read(&directory) {
            results.push((checked, credentials));
        }
    }
    Ok(results)
}

fn candidate_status(session: &Session, sources: &[(CheckedProject, Credentials)]) -> Status {
    let mut response = status(session);
    response.candidates = sources
        .iter()
        .map(|(checked, credentials)| Candidate {
            project_id: checked.project.id.clone(),
            name: checked.project.name.clone(),
            email: credentials.email(),
            path: checked.project.path.to_string_lossy().into_owned(),
        })
        .collect();
    if !response.candidates.is_empty() {
        response.message = "Use a saved project's ChatGPT sign-in or sign in with ChatGPT.".into();
    }
    response
}

pub fn adopt(
    store: &Store,
    session: &mut Session,
    checked: &CheckedProject,
    credentials: Credentials,
) -> Result<(), String> {
    guest::require_idle(checked, false)?;
    let directory = guest::directory(checked)?;
    session.connect(credentials)?;
    session.pending_login = None;
    guest::bind(session, checked, directory)?;
    store.save(session)?;
    guest::publish(store, session)
}

pub fn ensure(
    app: &AppHandle,
    runtime: &mut Runtime,
    checked: &CheckedProject,
) -> Result<Status, String> {
    crate::providers::provider_login::require_idle(
        app,
        crate::providers::provider_login::Provider::Chatgpt,
    )?;
    checked.project.require_codex()?;
    let (store, mut session) = load(app, runtime)?;
    if checked.settings.network == crate::desktop::settings::Network::Offline {
        return Err("This project is offline. Enable isolated networking in project settings to connect ChatGPT.".into());
    }
    if session.pending_login.is_some() {
        return Ok(status(&session));
    }
    if session.account_state() == AccountState::Unconnected {
        let sources = candidates(app)?;
        let accounts: HashSet<_> = sources
            .iter()
            .filter_map(|(_, credentials)| credentials.selection_key().ok())
            .collect();
        if accounts.len() > 1 {
            return Ok(candidate_status(&session, &sources));
        }
        if let Some((source, credentials)) = sources.into_iter().next() {
            adopt(&store, &mut session, &source, credentials)?;
        }
    }
    if session.credentials.is_none() {
        return Ok(status(&session));
    }
    if session.reconnect_required {
        return Ok(status(&session));
    }
    let directory = guest::directory(checked)?;
    if !guest::connected(&session, checked, &directory)? {
        guest::require_idle(checked, false)?;
        guest::bind(&mut session, checked, directory)?;
        store.save(&session)?;
    }
    guest::publish(&store, &mut session)?;
    refresh(app, runtime, &store, &mut session, false)?;
    let response = status(&session);
    let _ = app.emit("shared-chatgpt-changed", &response);
    Ok(response)
}

pub(crate) fn connect_provider(
    app: &AppHandle,
    runtime: &mut Runtime,
    records: &data_location::RecordsDirectory,
    credentials: Credentials,
) -> Result<(), String> {
    let (store, mut session) = load(app, runtime)?;
    store.verify_records(records)?;
    if session.pending_login.is_some() {
        return Err("Complete or cancel the previous ChatGPT sign-in first.".into());
    }
    require_idle(app, &session, false)?;
    session.connect(credentials)?;
    save_after_request(records.clone(), runtime, &store, &session)?;
    runtime.retry.succeeded();
    runtime.error = None;
    let mut response = status(&session);
    if guest::publish(&store, &mut session).is_err() {
        response.message.push_str(" A project copy could not be updated. Reopen that environment before starting its next chat.");
    }
    let _ = app.emit("shared-chatgpt-changed", response);
    Ok(())
}

pub fn require_idle(app: &AppHandle, session: &Session, skip_login: bool) -> Result<(), String> {
    if terminal::chatgpt_in_use(skip_login) {
        return Err("End running chats, sign-in and guest terminals before changing the shared ChatGPT account.".into());
    }
    for binding in &session.bindings {
        let Ok(checked) = environment::checked_project(app, &binding.project_id, None) else {
            continue;
        };
        if checked.project.agent_kind != crate::agents::AgentKind::Codex {
            continue;
        }
        guest::require_idle(&checked, skip_login)?;
    }
    Ok(())
}

pub fn finish_login(
    app: &AppHandle,
    runtime: &mut Runtime,
    project_id: &str,
    nonce: &str,
) -> Result<Status, String> {
    let (store, mut session) = load(app, runtime)?;
    let Some(pending) = session.pending_login.clone() else {
        return Ok(status(&session));
    };
    if pending.nonce != nonce || pending.project_id != project_id {
        return Ok(status(&session));
    }
    complete_login(&store, &mut session, project_id, nonce)?;
    runtime.error = None;
    runtime.retry.allow_retry();
    let response = status(&session);
    let _ = app.emit("shared-chatgpt-changed", &response);
    Ok(response)
}

pub fn complete_login(
    store: &Store,
    session: &mut Session,
    project_id: &str,
    nonce: &str,
) -> Result<(), String> {
    session.validate()?;
    let Some(mut pending) = session.pending_login.clone() else {
        return Ok(());
    };
    if pending.nonce != nonce || pending.project_id != project_id {
        return Ok(());
    }
    session.generation_after(1)?;
    if pending.phase() == LoginPhase::Imported {
        guest::publish(store, session)?;
    }
    if pending.phase() != LoginPhase::Imported {
        pending.cancel()?;
        session.pending_login = Some(pending.clone());
        store.save(session)?;
    }
    guest::remove_login_directory(&pending)?;
    session.pending_login = None;
    session.generation = session
        .generation
        .checked_add(1)
        .ok_or("The account record generation is invalid.")?;
    store.save(session)
}

pub fn read_status(app: &AppHandle, runtime: &mut Runtime) -> Result<Status, String> {
    let (_, session) = load(app, runtime)?;
    if let Some(pending) = &session.pending_login {
        let imported = pending.phase() == LoginPhase::Imported;
        let mut response = finish_login(app, runtime, &pending.project_id, &pending.nonce)?;
        if !imported {
            response.message = "The interrupted ChatGPT sign-in was cancelled. Your previous shared account was preserved. Sign in again in Settings if needed.".into();
        }
        return Ok(response);
    }
    if session.credentials.is_none() {
        return Ok(candidate_status(&session, &candidates(app)?));
    }
    Ok(status(&session))
}

pub fn flush(app: &AppHandle, runtime: &mut Runtime) -> Result<(), String> {
    if matches!(&runtime.pending_write, CredentialWrite::Clean) {
        return Ok(());
    }
    load(app, runtime)?;
    Ok(())
}
