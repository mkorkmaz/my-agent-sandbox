mod account;
mod credentials;
mod guest;
mod http;
mod store;
use account::Runtime;
pub use account::Status;
pub(crate) use credentials::Credentials;
use std::sync::Arc;
use store::{Account, Session};
use tauri::{AppHandle, Manager};

#[derive(Default)]
pub struct SharedClaude(pub(crate) Arc<tauri::async_runtime::Mutex<Runtime>>);

pub(crate) fn prepare_provider_login(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<SharedClaude>();
    let (_, session) = state.0.blocking_lock().load(app)?;
    account::require_idle(app, &session)
}

pub(crate) fn credentials(
    response: &serde_json::Value,
    cancellation: &std::sync::atomic::AtomicBool,
) -> Result<Credentials, String> {
    Credentials::from_response(response.clone(), http::profile(response, cancellation)?)
}

pub(crate) fn connect_provider(
    app: &AppHandle,
    records: &crate::storage::data_location::RecordsDirectory,
    credentials: Credentials,
) -> Result<(), String> {
    let state = app.state::<SharedClaude>();
    let mut runtime = state.0.blocking_lock();
    let (store, mut session) = runtime.load(app)?;
    store.verify_records(records)?;
    account::require_idle(app, &session)?;
    session.connect(credentials)?;
    runtime.save(records.clone(), &store, &session)?;
    let mut status = account::status(&session);
    if account::publish(app, &session).is_err() {
        status.message.push_str(" A project copy could not be updated. Stop and reopen that environment before starting its next chat.");
    }
    account::announce(app, &status);
    Ok(())
}

pub(crate) fn ensure(
    app: &AppHandle,
    checked: &crate::environment::CheckedProject,
) -> Result<Status, String> {
    crate::providers::provider_login::require_idle(
        app,
        crate::providers::provider_login::Provider::Claude,
    )?;
    let state = app.state::<SharedClaude>();
    let result = account::ensure(app, &mut state.0.blocking_lock(), checked);
    result
}

pub(crate) fn ensure_locked(
    app: &AppHandle,
    runtime: &mut Runtime,
    checked: &crate::environment::CheckedProject,
) -> Result<Status, String> {
    crate::providers::provider_login::require_idle(
        app,
        crate::providers::provider_login::Provider::Claude,
    )?;
    account::ensure(app, runtime, checked)
}

#[tauri::command]
pub async fn shared_claude_status(app: AppHandle) -> Result<Status, String> {
    let mut runtime = app.state::<SharedClaude>().0.clone().lock_owned().await;
    tauri::async_runtime::spawn_blocking(move || {
        let (_, session) = runtime.load(&app)?;
        Ok(account::status(&session))
    })
    .await
    .map_err(|_| "Could not read the shared Claude account.")?
}

#[tauri::command]
pub async fn retry_shared_claude(app: AppHandle) -> Result<Status, String> {
    crate::providers::provider_login::require_idle(
        &app,
        crate::providers::provider_login::Provider::Claude,
    )?;
    let mut runtime = app.state::<SharedClaude>().0.clone().lock_owned().await;
    tauri::async_runtime::spawn_blocking(move || {
        let (store, mut session) = runtime.load(&app)?;
        let result = account::refresh(&app, &mut runtime, &store, &mut session, true);
        let status = account::status(&session);
        account::announce(&app, &status);
        result?;
        Ok(status)
    })
    .await
    .map_err(|_| "Could not renew the shared Claude account.")?
}

fn cleanup(app: &AppHandle, session: &mut Session) -> Result<usize, String> {
    let mut remaining = Vec::new();
    for binding in session.bindings.drain(..) {
        let result = (|| {
            let checked = crate::environment::checked_environment(
                app,
                &binding.project_id,
                Some(&binding.environment_id),
                None,
            )?;
            if checked.project.stage == crate::projects::ProjectStage::EnvironmentReady {
                guest::remove(&checked)?;
            }
            Ok::<_, String>(())
        })();
        if result.is_err() {
            remaining.push(binding);
        }
    }
    session.bindings = remaining;
    Ok(session.bindings.len())
}

#[tauri::command]
pub async fn disconnect_shared_claude(app: AppHandle) -> Result<Status, String> {
    crate::providers::provider_login::require_idle(
        &app,
        crate::providers::provider_login::Provider::Claude,
    )?;
    let mut runtime = app.state::<SharedClaude>().0.clone().lock_owned().await;
    tauri::async_runtime::spawn_blocking(move || {
        let (store, mut session) = runtime.load(&app)?;
        account::require_idle(&app, &session)?;
        let revoked = session.account.credentials().is_none_or(http::revoke);
        session.disconnect()?;
        runtime.save(crate::storage::data_location::records_directory(&app)?, &store, &session)?;
        let remaining = cleanup(&app, &mut session)?;
        store.save(&session)?;
        let mut status = account::status(&session);
        status.message = "Signed out of Claude in Tugboat.".into();
        if !revoked { status.message.push_str(" Remote session revocation was not confirmed. Disconnect this session in Claude settings if needed."); }
        if remaining > 0 { status.message.push_str(" Some environment copies are unavailable. Tugboat will clear them when they reconnect."); }
        account::announce(&app, &status);
        Ok(status)
    }).await.map_err(|_| "Could not disconnect the shared Claude account.")?
}

pub fn monitor(app: AppHandle) {
    std::thread::spawn(move || loop {
        for _ in 0..15 {
            if crate::desktop::lifecycle::is_closing() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        if crate::providers::provider_login::require_idle(
            &app,
            crate::providers::provider_login::Provider::Claude,
        )
        .is_err()
        {
            continue;
        }
        if let Err(message) = maintain(&app) {
            use tauri::Emitter;
            let _ = app.emit("shared-claude-error", message);
        }
    });
}

fn maintain(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<SharedClaude>();
    let mut runtime = state.0.blocking_lock();
    if crate::desktop::lifecycle::is_closing() {
        return Ok(());
    }
    let (store, mut session) = runtime.load(app)?;
    if matches!(session.account, Account::Disconnected) {
        cleanup(app, &mut session)?;
        store.save(&session)?;
    }
    let registry = crate::projects::Registry::load(&crate::projects::file(app)?)?;
    let defaults = crate::desktop::settings::load(&crate::desktop::settings::file(app)?)?;
    let online = session.bindings.iter().any(|binding| {
        registry
            .environment(&binding.project_id, &binding.environment_id)
            .and_then(|project| project.settings(defaults.clone()))
            .is_ok_and(|settings| settings.network != crate::desktop::settings::Network::Offline)
    });
    if online {
        account::refresh(app, &mut runtime, &store, &mut session, false)?;
    }
    account::announce(app, &account::status(&session));
    Ok(())
}

pub(crate) fn reconnect_records(
    app: &AppHandle,
    records: &crate::storage::data_location::RecordsDirectory,
) -> Result<(), String> {
    app.state::<SharedClaude>()
        .0
        .blocking_lock()
        .reconnect_records(records)
}

pub fn flush(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<SharedClaude>();
    let result = state.0.blocking_lock().flush(app);
    result
}

pub(crate) fn detach_environment(
    app: &AppHandle,
    brig: &crate::platform::brig::Brig,
    project: &crate::projects::Project,
    remove_auth: bool,
) -> Result<bool, String> {
    let state = app.state::<SharedClaude>();
    let mut runtime = state.0.blocking_lock();
    let (store, mut session) = runtime.load(app)?;
    let owned = session.bindings.iter().any(|binding| {
        binding.environment_id == project.environment_id && binding.project_id == project.id
    });
    if !owned {
        return Ok(false);
    }
    if remove_auth {
        guest::remove_using(brig, project)?;
    }
    session.bindings.retain(|binding| {
        binding.environment_id != project.environment_id || binding.project_id != project.id
    });
    store.save(&session)?;
    Ok(true)
}

pub(crate) fn forget_project(app: &AppHandle, project_id: &str) -> Result<(), String> {
    let state = app.state::<SharedClaude>();
    let mut runtime = state.0.blocking_lock();
    let (store, mut session) = runtime.load(app)?;
    session
        .bindings
        .retain(|binding| binding.project_id != project_id);
    store.save(&session)
}

#[cfg(test)]
mod tests;
