use super::{
    credentials::Credentials,
    guest, http,
    store::{PendingLogin, Session, Store},
};
use crate::{
    data_location,
    environment::{self, CheckedProject},
    projects, terminal,
};
use serde::Serialize;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

#[derive(Default)]
pub struct Runtime {
    pending_write: Option<(data_location::RecordsDirectory, Session)>,
    pub login_process: Option<(String, u64)>,
    login_result: Option<(String, bool)>,
    retry_after: Option<Instant>,
    failures: u32,
    error: Option<String>,
}

#[derive(Default)]
pub struct SharedChatgpt(pub Arc<tauri::async_runtime::Mutex<Runtime>>);

impl Runtime {
    pub fn record_login_exit(&mut self, nonce: &str, success: bool) {
        if self
            .login_process
            .as_ref()
            .is_some_and(|(active, _)| active == nonce)
        {
            self.login_result = Some((nonce.into(), success));
        }
    }

    pub fn recovery_success(&self, pending: &PendingLogin) -> bool {
        pending.imported
            || self
                .login_result
                .as_ref()
                .is_some_and(|(nonce, success)| nonce == &pending.nonce && *success)
    }
    pub fn restore(&mut self, store: &Store) -> Result<(), String> {
        let Some((records, session)) = &self.pending_write else {
            return Ok(());
        };
        store.verify_records(records)?;
        store.save(session)?;
        self.pending_write = None;
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
    pub pending_finalizing: bool,
    pub state: &'static str,
    pub email: Option<String>,
    pub message: String,
    pub candidates: Vec<Candidate>,
}

pub fn status(session: &Session) -> Status {
    let credentials = session.credentials.as_ref();
    let (state, mut message) = match (
        session.pending_login.as_ref(),
        credentials,
        session.reconnect_required,
    ) {
        (Some(_), _, _) => (
            "signing-in",
            "Complete the ChatGPT sign-in in your browser.",
        ),
        (_, Some(_), true) => (
            "needs-sign-in",
            "The shared ChatGPT session is no longer valid. Sign in again.",
        ),
        (_, Some(_), false) => (
            "signed-in",
            "Your shared ChatGPT account is available to all projects.",
        ),
        _ => (
            "signed-out",
            "Sign in once to use your ChatGPT account in every project.",
        ),
    };
    if let Some(login) = &session.pending_login {
        message = match (login.imported, login.cancelled) {
            (true, _) => "Finishing the shared ChatGPT connection…",
            (_, true) => "Finishing cancelled ChatGPT sign-in cleanup…",
            _ => message,
        };
    }
    Status {
        generation: session.generation,
        pending_finalizing: session
            .pending_login
            .as_ref()
            .is_some_and(|login| login.imported),
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
    runtime.pending_write = Some((records, session.clone()));
    store.save(session)?;
    runtime.pending_write = None;
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
        && runtime
            .retry_after
            .is_some_and(|retry| retry > Instant::now())
    {
        return Err(runtime
            .error
            .clone()
            .unwrap_or_else(|| "ChatGPT sign-in renewal is waiting to retry.".into()));
    }
    guest::publish(store, session)?;
    match request(session.credentials.as_ref().unwrap()) {
        Ok(credentials) => {
            session.credentials = Some(credentials);
            session.generation = session
                .generation
                .checked_add(1)
                .ok_or("The account record generation is invalid.")?;
            save_after_request(records, runtime, store, session)?;
            runtime.failures = 0;
            runtime.retry_after = None;
            runtime.error = None;
            guest::publish(store, session)
        }
        Err(error) => {
            if error.requires_sign_in() {
                session.reconnect_required = true;
                session.generation = session
                    .generation
                    .checked_add(1)
                    .ok_or("The account record generation is invalid.")?;
                save_after_request(records, runtime, store, session)?;
            }
            runtime.failures = runtime.failures.saturating_add(1).min(5);
            runtime.retry_after =
                Some(Instant::now() + Duration::from_secs((10u64 << runtime.failures).min(300)));
            runtime.error = Some(error.message());
            Err(error.message())
        }
    }
}

fn candidates(app: &AppHandle) -> Result<Vec<(CheckedProject, Credentials)>, String> {
    let registry = projects::Registry::load(&projects::file(app)?)?;
    let mut results = Vec::new();
    for project in registry.projects {
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
    session.credentials = Some(credentials);
    session.generation = session
        .generation
        .checked_add(1)
        .ok_or("The account record generation is invalid.")?;
    session.disconnected = false;
    session.reconnect_required = false;
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
    let (store, mut session) = load(app, runtime)?;
    if checked.settings.network == crate::settings::Network::Offline {
        return Err("This project is offline. Enable isolated networking in project settings to connect ChatGPT.".into());
    }
    if session.pending_login.is_some() {
        return Ok(status(&session));
    }
    if session.credentials.is_none() && !session.disconnected {
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

pub fn require_idle(app: &AppHandle, session: &Session, skip_login: bool) -> Result<(), String> {
    if terminal::chatgpt_in_use(skip_login) {
        return Err("End running chats, sign-in and guest terminals before changing the shared ChatGPT account.".into());
    }
    for binding in &session.bindings {
        if let Ok(checked) = environment::checked_project(app, &binding.project_id, None) {
            guest::require_idle(&checked, skip_login)?;
        }
    }
    Ok(())
}

pub fn begin_login(
    app: &AppHandle,
    runtime: &mut Runtime,
    checked: &CheckedProject,
) -> Result<(String, String), String> {
    let (store, mut session) = load(app, runtime)?;
    if session.pending_login.is_some() {
        return Err("A ChatGPT sign-in is already in progress.".into());
    }
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
    runtime.login_result = None;
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
    let checked = if success && !pending.cancelled && !pending.imported {
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
    runtime.login_process = None;
    runtime.login_result = None;
    runtime.error = None;
    runtime.retry_after = None;
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
    let Some(mut pending) = session.pending_login.clone() else {
        return Ok(());
    };
    if pending.nonce != nonce || pending.project_id != project_id {
        return Ok(());
    }
    if success && !pending.cancelled && !pending.imported {
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
        session.credentials = Some(credentials);
        session.disconnected = false;
        session.reconnect_required = false;
        pending.imported = true;
        session.pending_login = Some(pending.clone());
        session.generation = session
            .generation
            .checked_add(1)
            .ok_or("The account record generation is invalid.")?;
        store.save(session)?;
    }
    if pending.imported {
        guest::publish(store, session)?;
    }
    if !success && !pending.imported {
        pending.cancelled = true;
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
    if pending.imported {
        return Err(
            "The new account is saved. Select Retry connection to finish connecting it.".into(),
        );
    }
    pending.cancelled = true;
    let nonce = pending.nonce.clone();
    // Save cancellation before stopping the process. Restart must not adopt its late auth file.
    session.generation = session
        .generation
        .checked_add(1)
        .ok_or("The account record generation is invalid.")?;
    store.save(&session)?;
    let identifier = runtime
        .login_process
        .as_ref()
        .filter(|(registered, _)| registered == &nonce)
        .and_then(|(_, identifier)| {
            terminal::terminal_snapshot(*identifier)
                .filter(|process| process.purpose == "login" && process.running)
                .map(|_| *identifier)
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
    if runtime.login_process.as_ref().is_some_and(|(_, id)| {
        terminal::terminal_snapshot(*id).is_none_or(|process| !process.running)
    }) {
        runtime.login_process = None;
    }
    if let Some(pending) = &session.pending_login {
        if runtime.login_process.is_none() {
            let success = runtime.recovery_success(pending);
            let interrupted = !pending.imported && runtime.login_result.is_none();
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
    if runtime.pending_write.is_none() && runtime.login_result.is_none() {
        return Ok(());
    }
    load(app, runtime)?;
    if runtime.login_result.is_some() {
        read_status(app, runtime)?;
    }
    if runtime.login_result.is_some() {
        return Err("Finish saving the ChatGPT sign-in before closing Tugboat.".into());
    }
    Ok(())
}
