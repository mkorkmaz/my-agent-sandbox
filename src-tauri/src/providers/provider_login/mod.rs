mod callback;
pub(crate) mod http;
pub(crate) mod protocol;
mod state;
mod worker;
use serde::{Deserialize, Serialize};
use state::{Attempt, Flow};
pub use state::{ProviderLogins, Status};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Provider {
    Chatgpt,
    Claude,
}

impl Provider {
    pub(super) fn index(self) -> usize {
        match self {
            Self::Chatgpt => 0,
            Self::Claude => 1,
        }
    }
}

pub fn require_idle(app: &AppHandle, provider: Provider) -> Result<(), String> {
    let state = app.state::<ProviderLogins>();
    let registry = state
        .0
        .lock()
        .map_err(|_| "Could not read provider sign-in.")?;
    if registry.flows[provider.index()].attempt().is_some() {
        return Err("Complete or cancel provider sign-in in Settings first.".into());
    }
    Ok(())
}

pub(super) fn announce(app: &AppHandle, status: &Status) {
    if app.emit("provider-login-changed", status).is_err() {
        eprintln!("Could not deliver a provider sign-in update.");
    }
}

#[tauri::command]
pub fn provider_login_status(app: AppHandle, provider: Provider) -> Result<Status, String> {
    Ok(app
        .state::<ProviderLogins>()
        .0
        .lock()
        .map_err(|_| "Could not read provider sign-in.")?
        .status(provider))
}

#[tauri::command]
pub fn start_provider_login(app: AppHandle, provider: Provider) -> Result<Status, String> {
    if crate::desktop::lifecycle::is_closing() {
        return Err("The app is closing.".into());
    }
    let records = crate::storage::data_location::records_directory(&app)?;
    let attempt = Attempt::new(records)?;
    let state = app.state::<ProviderLogins>();
    let mut registry = state
        .0
        .lock()
        .map_err(|_| "Could not start provider sign-in.")?;
    if registry.flows[provider.index()].attempt().is_some() {
        return Ok(registry.status(provider));
    }
    let status = registry.update(provider, Flow::Preparing(attempt.clone()))?;
    announce(&app, &status);
    drop(registry);
    std::thread::spawn(move || worker::run(app, provider, attempt));
    Ok(status)
}

#[tauri::command]
pub fn cancel_provider_login(app: AppHandle, provider: Provider) -> Result<Status, String> {
    let state = app.state::<ProviderLogins>();
    let mut registry = state
        .0
        .lock()
        .map_err(|_| "Could not cancel provider sign-in.")?;
    registry.flows[provider.index()].cancel()?;
    let status = registry.update(provider, Flow::Idle)?;
    announce(&app, &status);
    Ok(status)
}

pub(super) fn open(address: &str) -> Result<(), String> {
    let result = crate::platform::command::run(
        std::path::Path::new("/usr/bin/open"),
        &["--".into(), address.into()],
        None,
        &crate::platform::command::environment(),
        std::time::Duration::from_secs(10),
        &std::sync::atomic::AtomicBool::new(false),
    )?;
    if result.exit_code != Some(0) || result.timed_out || result.cancelled {
        return Err("Could not open the browser. Retry sign-in in Settings.".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn open_provider_login(app: AppHandle, provider: Provider) -> Result<(), String> {
    let address = provider_login_status(app, provider)?
        .authorization_url
        .ok_or("Start provider sign-in again in Settings.")?;
    tauri::async_runtime::spawn_blocking(move || open(&address))
        .await
        .map_err(|_| "Could not open the provider sign-in page.")?
}

pub fn cancel_all(app: &AppHandle) {
    let state = app.state::<ProviderLogins>();
    if let Ok(mut registry) = state.0.lock() {
        for provider in [Provider::Chatgpt, Provider::Claude] {
            if let Err(message) = registry.flows[provider.index()].cancel() {
                eprintln!("{message}");
                continue;
            }
            if let Ok(status) = registry.update(provider, Flow::Idle) {
                announce(app, &status);
            }
        }
    };
}

pub fn flush(app: &AppHandle) -> Result<(), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
    loop {
        let saving = app
            .state::<ProviderLogins>()
            .0
            .lock()
            .map_err(|_| "Could not check pending provider sign-in.")?
            .flows
            .iter()
            .any(|flow| matches!(flow, Flow::Saving(_)));
        if !saving {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(
                "Wait for provider sign-in to finish saving before closing Tugboat.".into(),
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests;
