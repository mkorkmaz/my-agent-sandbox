mod account;
mod credentials;
mod directory;
mod guest;
mod http;
mod store;

pub use account::SharedChatgpt;

use tauri::{AppHandle, Emitter, Manager};

pub fn ensure(
    app: &AppHandle,
    runtime: &mut account::Runtime,
    checked: &crate::environment::CheckedProject,
) -> Result<account::Status, String> {
    account::ensure(app, runtime, checked)
}

pub fn begin_login(
    app: &AppHandle,
    runtime: &mut account::Runtime,
    checked: &crate::environment::CheckedProject,
) -> Result<(String, String), String> {
    account::begin_login(app, runtime, checked)
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

pub fn finish_login(
    app: &AppHandle,
    project: &str,
    nonce: &str,
    success: bool,
) -> Result<account::Status, String> {
    let state = app.state::<SharedChatgpt>();
    let mut runtime = state.0.blocking_lock();
    runtime.record_login_exit(nonce, success);
    let result = account::finish_login(app, &mut runtime, project, nonce, success);
    if result.is_err() {
        if let Ok((_, session)) = account::load(app, &mut runtime) {
            let _ = app.emit("shared-chatgpt-changed", account::status(&session));
        }
    }
    result
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
        if checked.project.stage != "environment-ready" {
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
    let mut runtime = app.state::<SharedChatgpt>().0.clone().lock_owned().await;
    tauri::async_runtime::spawn_blocking(move || {
        let (store, mut session) = account::load(&app, &mut runtime)?;
        if session.pending_login.is_some() { return Err("Complete or cancel the current ChatGPT sign-in first.".into()); }
        account::require_idle(&app, &session, false)?;
        let revoked = session.credentials.as_ref().is_none_or(http::revoke);
        session.credentials = None; session.disconnected = true; session.reconnect_required = false;
        for binding in &mut session.bindings { binding.pending = false; }
        session.generation = session.generation.checked_add(1).ok_or("The account record generation is invalid.")?;
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

#[tauri::command]
pub async fn cancel_shared_chatgpt_login(app: AppHandle) -> Result<account::Status, String> {
    // Resolve only a terminal registered by this app instance; persisted IDs are never trusted.
    let mut runtime = app.state::<SharedChatgpt>().0.clone().lock_owned().await;
    let handle = app.clone();
    let identifier =
        tauri::async_runtime::spawn_blocking(move || account::cancel_login(&handle, &mut runtime))
            .await
            .map_err(|_| "Could not cancel ChatGPT sign-in.")??;
    if let Some(identifier) = identifier {
        crate::terminal::terminal_close(app.clone(), identifier).await?;
    }
    shared_chatgpt_status(app).await
}

pub fn monitor(app: AppHandle) {
    std::thread::spawn(move || loop {
        for _ in 0..15 {
            if crate::lifecycle::is_closing() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        let state = app.state::<SharedChatgpt>();
        let mut runtime = state.0.blocking_lock();
        if crate::lifecycle::is_closing() {
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
            if session.disconnected {
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
            let defaults = crate::settings::load(&crate::settings::file(&app)?)?;
            let online = registry.projects.iter().any(|project| {
                session
                    .bindings
                    .iter()
                    .any(|binding| binding.project_id == project.id)
                    && project
                        .settings(defaults.clone())
                        .is_ok_and(|settings| settings.network != crate::settings::Network::Offline)
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

pub fn flush(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<SharedChatgpt>();
    let mut runtime = state.0.blocking_lock();
    account::flush(app, &mut runtime)
}

#[cfg(test)]
mod tests;
