use super::session_state::AccountState;
use super::renewal::{CredentialWrite, RenewalRetry};
use super::{login_phase::LoginPhase, login_process::LoginProcess};
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
    login: LoginProcess,
    retry: RenewalRetry,
    error: Option<String>,
}

#[derive(Default)]
pub struct SharedChatgpt(pub Arc<tauri::async_runtime::Mutex<Runtime>>);

impl Runtime {
    pub fn attach_login_process(&mut self, nonce: String, identifier: u64) -> Result<(), String> {
        self.login.attach(nonce, identifier)
    }

    pub fn record_login_exit(&mut self, nonce: &str, success: bool) {
        self.login.record(nonce, success);
    }

    pub fn recovery_success(&self, pending: &PendingLogin) -> bool {
        pending.phase() == LoginPhase::Imported
            || self.login.result().is_some_and(|(nonce, success)| nonce == pending.nonce && success)
    }
    pub(crate) fn reconnect_records(
        &mut self,
        records: &data_location::RecordsDirectory,
    ) -> Result<(), String> {
        records.verify()?;
        let CredentialWrite::Pending { records: previous, .. } = &mut self.pending_write else {
            return Ok(());
        };
        if previous.id() != records.id() {
            return Err("The pending ChatGPT write belongs to a different records dataset.".into());
        }
        *previous = records.clone();
        // Login results and staging paths belong to their existing guest, not this folder.
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
    runtime.pending_write = CredentialWrite::Pending { records, session: session.clone() };
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
    if !force
        && runtime.retry.waiting(Instant::now())
    {
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

pub fn begin_login(
    app: &AppHandle,
    runtime: &mut Runtime,
    checked: &CheckedProject,
) -> Result<(String, String), String> {
    checked.project.require_codex()?;
    let (store, mut session) = load(app, runtime)?;
    if session.pending_login.is_some() {
        return Err("A ChatGPT sign-in is already in progress.".into());
    }
    session.generation_after(3)?;
    require_idle(app, &session, false)?;
    guest::require_idle(checked, false)?;
    guest::publish(&store, &mut session)?;
    let mut random = [0u8; 16];
    if unsafe { libc::getentropy(random.as_mut_ptr().cast(), random.len()) } != 0 {
        return Err("Could not create the ChatGPT sign-in transaction.".into());
    }
    let nonce = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let (directory, guest_path) = guest::login_directory(checked, &nonce)?;
    session.generation = session
        .generation
        .checked_add(1)
        .ok_or("The account record generation is invalid.")?;
    session.pending_login = Some(PendingLogin {
        project_id: checked.project.id.clone(),
        nonce: nonce.clone(),
        identity: guest::identity(&directory)?,
        directory,
        imported: false,
        cancelled: false,
    });
    if let Err(error) = store.save(&session) {
        guest::remove_login_directory(session.pending_login.as_ref().unwrap())?;
        return Err(error);
    }
    runtime.login = LoginProcess::Idle;
    Ok((nonce, guest_path))
}

pub fn finish_login(
    app: &AppHandle,
    runtime: &mut Runtime,
    project_id: &str,
    nonce: &str,
    success: bool,
) -> Result<Status, String> {
    let (store, mut session) = load(app, runtime)?;
    let Some(pending) = session.pending_login.clone() else {
        return Ok(status(&session));
    };
    if pending.nonce != nonce || pending.project_id != project_id {
        return Ok(status(&session));
    }
    let checked = if success && pending.phase() == LoginPhase::Pending {
        require_idle(app, &session, true)?;
        Some(environment::checked_project(app, project_id, None)?)
    } else {
        None
    };
    complete_login(
        &store,
        &mut session,
        project_id,
        nonce,
        success,
        checked.as_ref(),
    )?;
    runtime.login = LoginProcess::Idle;
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
    success: bool,
    checked: Option<&CheckedProject>,
) -> Result<(), String> {
    session.validate()?;
    let Some(mut pending) = session.pending_login.clone() else {
        return Ok(());
    };
    if pending.nonce != nonce || pending.project_id != project_id {
        return Ok(());
    }
    let changes = if success && pending.phase() == LoginPhase::Pending { 2 } else { 1 };
    session.generation_after(changes)?;
    if success && pending.phase() == LoginPhase::Pending {
        let checked = checked.ok_or(
            "Reconnect the sign-in project's environment to finish connecting the shared account.",
        )?;
        if checked.project.id != project_id
            || guest::identity(&pending.directory)? != pending.identity
        {
            return Err("The temporary ChatGPT sign-in directory changed.".into());
        }
        let credentials = guest::read(&pending.directory)?
            .ok_or("The ChatGPT sign-in did not save credentials.")?;
        let directory = guest::directory(checked)?;
        guest::bind(session, checked, directory)?;
        session.connect(credentials)?;
        pending.import()?;
        session.pending_login = Some(pending.clone());
        store.save(session)?;
    }
    if pending.phase() == LoginPhase::Imported {
        guest::publish(store, session)?;
    }
    if !success && pending.phase() != LoginPhase::Imported {
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

pub fn cancel_login(app: &AppHandle, runtime: &mut Runtime) -> Result<Option<u64>, String> {
    let (store, mut session) = load(app, runtime)?;
    let Some(pending) = &mut session.pending_login else {
        return Ok(None);
    };
    if pending.phase() == LoginPhase::Imported {
        return Err(
            "The new account is saved. Select Retry connection to finish connecting it.".into(),
        );
    }
    pending.cancel()?;
    let nonce = pending.nonce.clone();
    // Save cancellation before stopping the process. Restart must not adopt its late auth file.
    session.generation = session
        .generation
        .checked_add(1)
        .ok_or("The account record generation is invalid.")?;
    store.save(&session)?;
    let identifier = runtime
        .login
        .process()
        .filter(|(registered, _)| *registered == nonce.as_str())
        .and_then(|(_, identifier)| {
            terminal::terminal_snapshot(identifier)
                .filter(|process| {
                    process.agent_kind == Some(crate::agents::AgentKind::Codex)
                        && process.purpose == "login"
                        && process.running
                })
                .map(|_| identifier)
        });
    if identifier.is_none() {
        let pending = session.pending_login.as_ref().unwrap();
        drop(store);
        finish_login(app, runtime, &pending.project_id, &pending.nonce, false)?;
    }
    Ok(identifier)
}

pub fn read_status(app: &AppHandle, runtime: &mut Runtime) -> Result<Status, String> {
    let (_, session) = load(app, runtime)?;
    if runtime.login.process().is_some_and(|(_, id)| {
        terminal::terminal_snapshot(id).is_none_or(|process| !process.running)
    }) {
        runtime.login.clear_process();
    }
    if let Some(pending) = &session.pending_login {
        if runtime.login.process().is_none() {
            let success = runtime.recovery_success(pending);
            let interrupted = pending.phase() != LoginPhase::Imported && runtime.login.result().is_none();
            let mut response =
                finish_login(app, runtime, &pending.project_id, &pending.nonce, success)?;
            if interrupted {
                response.message = "The interrupted ChatGPT sign-in was cancelled. Your previous shared account was preserved. Sign in again in Settings if needed.".into();
            }
            return Ok(response);
        }
    }
    if session.credentials.is_none() {
        return Ok(candidate_status(&session, &candidates(app)?));
    }
    Ok(status(&session))
}

pub fn flush(app: &AppHandle, runtime: &mut Runtime) -> Result<(), String> {
    if matches!(&runtime.pending_write, CredentialWrite::Clean) && runtime.login.result().is_none() {
        return Ok(());
    }
    load(app, runtime)?;
    if runtime.login.result().is_some() {
        read_status(app, runtime)?;
    }
    if runtime.login.result().is_some() {
        return Err("Finish saving the ChatGPT sign-in before closing Tugboat.".into());
    }
    Ok(())
}
