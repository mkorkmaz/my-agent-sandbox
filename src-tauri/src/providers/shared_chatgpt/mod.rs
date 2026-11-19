mod account;
mod binding_state;
mod credentials;
mod directory;
mod guest;
mod http;
mod login_phase;
#[cfg(test)]
mod proof;
mod renewal;
mod session_state;
mod store;

pub use account::SharedChatgpt;
pub(crate) use credentials::Credentials;
#[cfg(test)]
pub(crate) use proof::with_access_copy;
pub(crate) use renewal::RenewalRetry;
pub(crate) use store::read_private;

pub(crate) fn prepare_provider_login(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<SharedChatgpt>();
    let mut runtime = state.0.blocking_lock();
    let (_, session) = account::load(app, &mut runtime)?;
    if session.pending_login.is_some() {
        return Err("Complete or cancel the previous ChatGPT sign-in first.".into());
    }
    account::require_idle(app, &session, false)
}

pub(crate) fn connect_provider(
    app: &AppHandle,
    records: &crate::storage::data_location::RecordsDirectory,
    credentials: Credentials,
) -> Result<(), String> {
    let state = app.state::<SharedChatgpt>();
    let mut runtime = state.0.blocking_lock();
    account::connect_provider(app, &mut runtime, records, credentials)
}

use tauri::{AppHandle, Emitter, Manager};

pub fn ensure(
    app: &AppHandle,
    runtime: &mut account::Runtime,
    checked: &crate::environment::CheckedProject,
) -> Result<account::Status, String> {
    account::ensure(app, runtime, checked)
}

pub fn detach_project(
    app: &AppHandle,
    runtime: &mut account::Runtime,
    project_id: &str,
    remove_auth: bool,
) -> Result<bool, String> {
    let (store, mut session) = account::load(app, runtime)?;
    if session
        .pending_login
        .as_ref()
        .is_some_and(|login| login.project_id == project_id)
    {
        return Err("Complete or cancel this project's ChatGPT sign-in first.".into());
    }
    guest::detach(&store, &mut session, project_id, remove_auth)
}

#[tauri::command]
pub async fn shared_chatgpt_status(app: AppHandle) -> Result<account::Status, String> {
    let mut runtime = app.state::<SharedChatgpt>().0.clone().lock_owned().await;
    tauri::async_runtime::spawn_blocking(move || account::read_status(&app, &mut runtime))
        .await
        .map_err(|_| "Could not read the shared ChatGPT account.")?
}

#[tauri::command]
pub async fn sync_project_chatgpt(
    app: AppHandle,
    identifier: String,
) -> Result<account::Status, String> {
    let mut runtime = app.state::<SharedChatgpt>().0.clone().lock_owned().await;
    tauri::async_runtime::spawn_blocking(move || {
        let checked = crate::environment::checked_project(&app, &identifier, None)?;
        if checked.project.stage != crate::projects::ProjectStage::EnvironmentReady {
            return account::read_status(&app, &mut runtime);
        }
        account::ensure(&app, &mut runtime, &checked)
    })
    .await
    .map_err(|_| "Could not connect the shared ChatGPT account.")?
}

#[tauri::command]
pub async fn use_project_chatgpt(
    app: AppHandle,
    identifier: String,
) -> Result<account::Status, String> {
    crate::providers::provider_login::require_idle(
        &app,
        crate::providers::provider_login::Provider::Chatgpt,
    )?;
    let mut runtime = app.state::<SharedChatgpt>().0.clone().lock_owned().await;
    tauri::async_runtime::spawn_blocking(move || {
        let (store, mut session) = account::load(&app, &mut runtime)?;
        if session.pending_login.is_some() {
            return Err("Complete or cancel the current sign-in first.".into());
        }
        account::require_idle(&app, &session, false)?;
        let checked = crate::environment::checked_project(&app, &identifier, None)?;
        let directory = guest::directory(&checked)?;
        let credentials =
            guest::read(&directory)?.ok_or("This project has no saved ChatGPT sign-in.")?;
        account::adopt(&store, &mut session, &checked, credentials)?;
        let response = account::status(&session);
        let _ = app.emit("shared-chatgpt-changed", &response);
        Ok(response)
    })
    .await
    .map_err(|_| "Could not use the saved ChatGPT sign-in.")?
}

#[tauri::command]
pub async fn retry_shared_chatgpt(app: AppHandle) -> Result<account::Status, String> {
    crate::providers::provider_login::require_idle(
        &app,
        crate::providers::provider_login::Provider::Chatgpt,
    )?;
    let mut runtime = app.state::<SharedChatgpt>().0.clone().lock_owned().await;
    tauri::async_runtime::spawn_blocking(move || {
        account::read_status(&app, &mut runtime)?;
        let (store, mut session) = account::load(&app, &mut runtime)?;
        account::refresh(&app, &mut runtime, &store, &mut session, true)?;
        let response = account::status(&session);
        let _ = app.emit("shared-chatgpt-changed", &response);
        Ok(response)
    })
    .await
    .map_err(|_| "Could not renew the shared ChatGPT sign-in.")?
}

#[tauri::command]
pub async fn disconnect_shared_chatgpt(app: AppHandle) -> Result<account::Status, String> {
    crate::providers::provider_login::require_idle(
        &app,
        crate::providers::provider_login::Provider::Chatgpt,
    )?;
    let mut runtime = app.state::<SharedChatgpt>().0.clone().lock_owned().await;
    tauri::async_runtime::spawn_blocking(move || {
        let (store, mut session) = account::load(&app, &mut runtime)?;
        if session.pending_login.is_some() { return Err("Complete or cancel the current ChatGPT sign-in first.".into()); }
        account::require_idle(&app, &session, false)?;
        let revoked = session.credentials.as_ref().is_none_or(http::revoke);
        session.disconnect()?;
        store.save(&session)?;
        let remaining = guest::cleanup(&mut session)?;
        store.save(&session)?;
        let mut response = account::status(&session);
        response.message = if revoked { "Signed out of Tugboat.".into() }
            else { "Signed out of Tugboat. Remote session revocation was not confirmed; you can disconnect the session in ChatGPT Settings.".into() };
        if remaining > 0 { response.message.push_str(" Some guest homes are unavailable. Their managed sign-in copies will be removed when they reconnect."); }
        let _ = app.emit("shared-chatgpt-changed", &response);
        Ok(response)
    }).await.map_err(|_| "Could not disconnect the shared ChatGPT account.")?
}

pub fn monitor(app: AppHandle) {
    std::thread::spawn(move || loop {
        for _ in 0..15 {
            if crate::desktop::lifecycle::is_closing() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        let state = app.state::<SharedChatgpt>();
        let mut runtime = state.0.blocking_lock();
        if crate::desktop::lifecycle::is_closing() {
            return;
        }
        let result = (|| {
            let (store, mut session) = account::load(&app, &mut runtime)?;
            if session.pending_login.is_some() {
                drop(store);
                let response = account::read_status(&app, &mut runtime)?;
                let _ = app.emit("shared-chatgpt-changed", response);
                return Ok::<_, String>(());
            }
            if session.account_state() == session_state::AccountState::Disconnected {
                guest::cleanup(&mut session)?;
                store.save(&session)?;
                drop(store);
                let response = account::read_status(&app, &mut runtime)?;
                let _ = app.emit("shared-chatgpt-changed", response);
                return Ok(());
            }
            if session.credentials.is_none() {
                drop(store);
                let response = account::read_status(&app, &mut runtime)?;
                let _ = app.emit("shared-chatgpt-changed", response);
                return Ok(());
            }
            let registry = crate::projects::Registry::load(&crate::projects::file(&app)?)?;
            let defaults = crate::desktop::settings::load(&crate::desktop::settings::file(&app)?)?;
            let online = registry.project_views()?.iter().any(|project| {
                session
                    .bindings
                    .iter()
                    .any(|binding| binding.belongs_to(project))
                    && project.settings(defaults.clone()).is_ok_and(|settings| {
                        settings.network != crate::desktop::settings::Network::Offline
                    })
            });
            if online {
                account::refresh(&app, &mut runtime, &store, &mut session, false)?;
            }
            let _ = app.emit("shared-chatgpt-changed", account::status(&session));
            Ok(())
        })();
        if let Err(message) = result {
            let _ = app.emit("shared-chatgpt-error", message);
        }
    });
}

pub(crate) fn retire_bindings(
    records: &crate::storage::data_location::RecordsDirectory,
    projects: &[crate::projects::Project],
    old_homes: &[(&str, &std::path::Path)],
) -> Result<(), String> {
    use std::{fs, os::unix::fs::MetadataExt};
    let path = records.path().join("chatgpt/session.json");
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Ok(metadata) if metadata.is_file() && metadata.nlink() == 1 => {}
        _ => {
            return Err(
                "The ChatGPT account record is unavailable or linked. Preserve it for review."
                    .into(),
            )
        }
    }
    let store = store::Store::open(records.path())?;
    store.verify_records(records)?;
    let mut session = store.load()?;
    let belongs =
        |binding: &store::Binding| projects.iter().any(|project| binding.belongs_to(project));
    let retired_login = session.pending_login.as_ref().is_some_and(|login| {
        let home = login.directory.parent().and_then(std::path::Path::parent);
        old_homes
            .iter()
            .any(|(project, directory)| *project == login.project_id && home == Some(*directory))
            || session.bindings.iter().any(|binding| {
                binding.project_id == login.project_id
                    && belongs(binding)
                    && home == binding.directory.parent()
            })
    });
    let count = session.bindings.len();
    session.bindings.retain(|binding| !belongs(binding));
    if retired_login {
        session.pending_login = None;
    }
    if retired_login || session.bindings.len() != count {
        store.verify_records(records)?;
        store.save(&session)?;
    }
    Ok(())
}

pub(crate) fn reconnect_records(
    app: &AppHandle,
    records: &crate::storage::data_location::RecordsDirectory,
) -> Result<(), String> {
    app.state::<SharedChatgpt>()
        .0
        .blocking_lock()
        .reconnect_records(records)
}

pub fn flush(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<SharedChatgpt>();
    let mut runtime = state.0.blocking_lock();
    account::flush(app, &mut runtime)
}

#[cfg(test)]
mod tests;
