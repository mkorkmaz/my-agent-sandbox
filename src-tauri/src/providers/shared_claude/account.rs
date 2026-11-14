use super::{
    credentials::Credentials,
    guest, http,
    store::{Account, Binding, Session, Store},
};
use crate::{
    environment::CheckedProject, projects::Registry, storage::data_location::RecordsDirectory,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

#[derive(Default)]
pub(crate) struct Runtime {
    pending_write: Option<(RecordsDirectory, Session)>,
    retry: crate::providers::shared_chatgpt::RenewalRetry,
}

impl Runtime {
    pub(super) fn load(&mut self, app: &AppHandle) -> Result<(Store, Session), String> {
        let store = Store::for_app(app)?;
        self.restore(&store)?;
        let session = store.load()?;
        Ok((store, session))
    }
    pub(super) fn restore(&mut self, store: &Store) -> Result<(), String> {
        let Some((records, session)) = &self.pending_write else {
            return Ok(());
        };
        store.verify_records(records)?;
        store.save(session)?;
        self.pending_write = None;
        Ok(())
    }
    pub(super) fn save(
        &mut self,
        records: RecordsDirectory,
        store: &Store,
        session: &Session,
    ) -> Result<(), String> {
        store.verify_records(&records)?;
        self.pending_write = Some((records, session.clone()));
        self.restore(store)
    }
    pub(super) fn reconnect_records(&mut self, records: &RecordsDirectory) -> Result<(), String> {
        let Some((previous, _)) = &mut self.pending_write else {
            return Ok(());
        };
        records.verify()?;
        if previous.id() != records.id() {
            return Err("The pending Claude write belongs to another records dataset.".into());
        }
        *previous = records.clone();
        Ok(())
    }
    pub(super) fn flush(&mut self, app: &AppHandle) -> Result<(), String> {
        if self.pending_write.is_none() {
            return Ok(());
        }
        self.load(app).map(|_| ())
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub generation: u64,
    pub state: &'static str,
    pub email: Option<String>,
    pub message: String,
}

pub(super) fn status(session: &Session) -> Status {
    let (state, message) = match &session.account {
        Account::Disconnected => (
            "signed-out",
            "Sign in once to use your Claude account in every project.",
        ),
        Account::Connected(_) => (
            "signed-in",
            "Your shared Claude account is available to all projects.",
        ),
        Account::NeedsSignIn(_) => (
            "needs-sign-in",
            "The shared Claude account needs sign-in. Sign in again in Settings.",
        ),
    };
    Status {
        generation: session.generation,
        state,
        email: session
            .account
            .credentials()
            .and_then(|credentials| credentials.email.clone()),
        message: message.into(),
    }
}

pub(super) fn announce(app: &AppHandle, status: &Status) {
    if app.emit("shared-claude-changed", status).is_err() {
        eprintln!("Could not deliver a shared Claude account update.");
    }
}

pub(super) fn require_idle(app: &AppHandle, session: &Session) -> Result<(), String> {
    let registry = Registry::load(&crate::projects::file(app)?)?;
    if crate::terminal::claude_in_use() {
        return Err(
            "End Claude chats and guest terminals before changing the shared Claude account."
                .into(),
        );
    }
    for binding in &session.bindings {
        if registry
            .environment(&binding.project_id, &binding.environment_id)
            .is_err()
        {
            continue;
        }
        let checked = crate::environment::checked_environment(
            app,
            &binding.project_id,
            Some(&binding.environment_id),
            None,
        )?;
        guest::require_idle(&checked)?;
    }
    Ok(())
}

pub(super) fn ensure(
    app: &AppHandle,
    runtime: &mut Runtime,
    checked: &CheckedProject,
) -> Result<Status, String> {
    if checked.project.agent_kind != crate::agents::AgentKind::ClaudeCode {
        return Err("Select a Claude environment first.".into());
    }
    if checked.settings.network == crate::desktop::settings::Network::Offline {
        return Err("This Claude environment is offline. Enable isolated networking before connecting the shared account.".into());
    }
    let (store, mut session) = runtime.load(app)?;
    refresh_with(
        runtime,
        &store,
        &mut session,
        false,
        crate::storage::data_location::records_directory(app)?,
        http::refresh,
    )?;
    let Account::Connected(credentials) = &session.account else {
        return Ok(status(&session));
    };
    let bytes = serde_json::to_vec(&credentials.guest())
        .map_err(|_| "Could not prepare the Claude access copy.")?;
    if !session.bindings.iter().any(|binding| {
        binding.environment_id == checked.project.environment_id
            && binding.project_id == checked.project.id
    }) {
        if session.bindings.len() >= 256 {
            return Err("Too many Claude environments use this account. Remove an unused environment first.".into());
        }
        session.bindings.push(Binding {
            project_id: checked.project.id.clone(),
            environment_id: checked.project.environment_id.clone(),
        });
        store.save(&session)?;
    }
    guest::publish(checked, bytes)?;
    let response = status(&session);
    announce(app, &response);
    Ok(response)
}

pub(super) fn refresh(
    app: &AppHandle,
    runtime: &mut Runtime,
    store: &Store,
    session: &mut Session,
    force: bool,
) -> Result<(), String> {
    refresh_with(
        runtime,
        store,
        session,
        force,
        crate::storage::data_location::records_directory(app)?,
        http::refresh,
    )?;
    publish(app, session)
}

pub(super) fn refresh_with(
    runtime: &mut Runtime,
    store: &Store,
    session: &mut Session,
    force: bool,
    records: RecordsDirectory,
    request: impl FnOnce(&Credentials) -> Result<Credentials, http::Failure>,
) -> Result<(), String> {
    let credentials = match &session.account {
        Account::Connected(credentials) => credentials,
        Account::NeedsSignIn(_) => return Err(http::Failure::SignIn.message().into()),
        Account::Disconnected => return Ok(()),
    };
    if !force && credentials.expires_at > chrono::Utc::now().timestamp_millis() + 600_000 {
        return Ok(());
    }
    if !force && runtime.retry.exhausted() {
        return Err("Claude renewal stopped after five failed attempts. Select Check connection in Settings, Providers.".into());
    }
    if !force && runtime.retry.waiting(std::time::Instant::now()) {
        return Err("Claude renewal is waiting to retry. Check connection in Settings.".into());
    }
    store.verify_records(&records)?;
    session
        .generation
        .checked_add(1)
        .ok_or("The Claude account generation is invalid.")?;
    match request(credentials) {
        Ok(credentials) => {
            session.connect(credentials)?;
            runtime.save(records, store, session)?;
            runtime.retry.succeeded();
            Ok(())
        }
        Err(failure) => {
            runtime.retry.failed(std::time::Instant::now());
            if failure.needs_sign_in() {
                session.require_sign_in()?;
                runtime.save(records, store, session)?;
            }
            Err(failure.message().into())
        }
    }
}

pub(super) fn publish(app: &AppHandle, session: &Session) -> Result<(), String> {
    let Account::Connected(credentials) = &session.account else {
        return Ok(());
    };
    let registry = Registry::load(&crate::projects::file(app)?)?;
    let mut failed = 0;
    for binding in &session.bindings {
        if registry
            .environment(&binding.project_id, &binding.environment_id)
            .is_err()
        {
            continue;
        }
        let result = (|| {
            let checked = crate::environment::checked_environment(
                app,
                &binding.project_id,
                Some(&binding.environment_id),
                None,
            )?;
            if checked.project.stage == crate::projects::ProjectStage::EnvironmentReady
                && checked.settings.network != crate::desktop::settings::Network::Offline
            {
                guest::publish(
                    &checked,
                    serde_json::to_vec(&credentials.guest())
                        .map_err(|_| "Could not prepare Claude sign-in.")?,
                )?;
            }
            Ok::<_, String>(())
        })();
        if result.is_err() {
            failed += 1;
        }
    }
    if failed > 0 {
        return Err("Some Claude environment copies could not be updated. Reopen those environments and retry their chats.".into());
    }
    Ok(())
}
