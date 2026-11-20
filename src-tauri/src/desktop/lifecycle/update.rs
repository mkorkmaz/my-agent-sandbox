use super::install_location::InstallLocation;
use super::{close_storage, drain, relaunch, SHUTDOWN};
use crate::storage::data_location;
use std::path::PathBuf;
use std::sync::MutexGuard;
use tauri::AppHandle;

pub(crate) struct InstallFailure {
    pub message: String,
    pub preserved_bundle: Option<PathBuf>,
}

pub(crate) enum UpdateFailure {
    Shutdown(String),
    Recovery {
        message: String,
        installed_version: Option<String>,
        can_reopen: bool,
    },
}

pub(crate) fn check_update_location() -> Result<(), String> {
    InstallLocation::current()?.require_writable()
}

// Call while holding the terminal registry lock, after checking the approved sessions.
// Terminal admission takes the same lock before checking the shutdown state.
pub(crate) fn begin_update(
    app: &AppHandle,
    version: String,
    install: impl FnOnce() -> Result<(), InstallFailure> + Send + 'static,
    on_failure: impl FnOnce(UpdateFailure) + Send + 'static,
) -> Result<(), String> {
    let location = InstallLocation::current()?;
    location.require_writable()?;
    if !state().begin_update() {
        return Err("Tugboat is already closing. Try the update again when closing stops.".into());
    }
    let app = app.clone();
    let worker = std::thread::Builder::new()
        .name("app-update".into())
        .spawn(move || run_update(app, location, &version, install, on_failure));
    if let Err(error) = worker {
        state().abort();
        return Err(format!("Could not start the app update worker. {error}"));
    }
    Ok(())
}

pub(crate) fn reopen_after_update(app: &AppHandle) -> Result<(), String> {
    let location = state().reopen_update()?;
    let records = data_location::records_directory(app).ok();
    let records_id = records.as_ref().map_or("", |records| records.id());
    let result = relaunch(app, &location, None, records_id);
    if result.is_err() {
        state().report_update_recovery(Some(location));
    }
    result
}

fn state() -> MutexGuard<'static, super::ShutdownState> {
    SHUTDOWN.lock().unwrap_or_else(|error| error.into_inner())
}

fn run_update(
    app: AppHandle,
    location: InstallLocation,
    version: &str,
    install: impl FnOnce() -> Result<(), InstallFailure>,
    on_failure: impl FnOnce(UpdateFailure),
) {
    let result = finish_update(
        || {
            drain(&app)?;
            let records = data_location::records_directory(&app)?;
            close_storage(&app)?;
            state().install_update()?;
            Ok(records)
        },
        install,
        |records| relaunch(&app, &location, Some(version), records.id()),
        |message, preserved_bundle| recover(&app, &location, message, preserved_bundle),
    );
    match result {
        Ok(()) => {}
        Err(failure @ UpdateFailure::Shutdown(_)) => {
            state().abort();
            on_failure(failure);
        }
        Err(failure @ UpdateFailure::Recovery { .. }) => on_failure(failure),
    }
}

fn finish_update<Records>(
    close: impl FnOnce() -> Result<Records, String>,
    install: impl FnOnce() -> Result<(), InstallFailure>,
    relaunch: impl FnOnce(Records) -> Result<(), String>,
    recover: impl FnOnce(String, Option<PathBuf>) -> UpdateFailure,
) -> Result<(), UpdateFailure> {
    let records = close().map_err(UpdateFailure::Shutdown)?;
    if let Err(failure) = install() {
        return Err(recover(
            format!("Could not install the Tugboat update. {}", failure.message),
            failure.preserved_bundle,
        ));
    }
    relaunch(records).map_err(|error| recover(format!("Could not restart Tugboat. {error}"), None))
}

fn recover(
    app: &AppHandle,
    location: &InstallLocation,
    mut message: String,
    preserved_bundle: Option<PathBuf>,
) -> UpdateFailure {
    let identifier = &app.config().identifier;
    let installed_version = location.installed_version(identifier).ok();
    let mut reopen = installed_version.as_ref().map(|_| location.clone());
    if reopen.is_none() {
        let previous = app.package_info().version.to_string();
        reopen = preserved_bundle
            .and_then(|path| location.preserved_at(&path).ok())
            .filter(|backup| {
                backup
                    .installed_version(identifier)
                    .is_ok_and(|version| version == previous)
            });
        if reopen.is_some() {
            message.push_str(&format!(
                " You can reopen the preserved Tugboat {previous} app."
            ));
        }
    }
    let can_reopen = reopen.is_some();
    state().report_update_recovery(reopen);
    UpdateFailure::Recovery {
        message,
        installed_version,
        can_reopen,
    }
}

fn relaunch(
    app: &AppHandle,
    location: &InstallLocation,
    expected_version: Option<&str>,
    records_id: &str,
) -> Result<(), String> {
    let mut supervisor =
        location.relaunch(&app.config().identifier, expected_version, records_id)?;
    if let Err(error) = state().allow_update_exit() {
        relaunch::cancel(&mut supervisor);
        return Err(error);
    }
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tests;
