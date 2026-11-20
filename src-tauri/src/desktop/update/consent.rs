use super::{
    current_status, package, refresh_sessions, reject_offer, schedule_file, transport, Offer,
    Phase, Schedule, State, Status, INSTALLER,
};
use crate::{desktop::lifecycle, terminal};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Runtime};

type BeginInstall<R> = fn(&AppHandle<R>, Offer, Arc<Vec<u8>>) -> Result<(), String>;

#[tauri::command]
pub(crate) async fn app_update_approve(
    app: AppHandle,
    offer_id: u64,
    session_revision: u64,
) -> Result<Status, String> {
    tauri::async_runtime::spawn_blocking(move || {
        approve(
            &app,
            offer_id,
            session_revision,
            lifecycle::check_update_location,
            begin_install,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

pub(super) fn approve<R: Runtime>(
    app: &AppHandle<R>,
    offer_id: u64,
    session_revision: u64,
    check_update_location: impl FnOnce() -> Result<(), String>,
    install: BeginInstall<R>,
) -> Result<Status, String> {
    let mut state = State::lock(app);
    if !state.has_offer(offer_id) || !matches!(state.phase, Phase::Available | Phase::Ready(_, _)) {
        return Ok(current_status(app, &mut state));
    }
    let active = terminal::with_active_update_sessions(|active| active)?;
    state.refresh_sessions(active);
    if state.session_revision != session_revision {
        return Ok(state.publish(app, true));
    }
    if let Err(message) = check_update_location() {
        state.phase = Phase::Failed {
            message,
            installer: true,
        };
        return Ok(state.publish(app, true));
    }
    let offer = state.offer.clone().expect("checked offer");
    let approved: Vec<_> = state
        .sessions
        .iter()
        .map(|session| session.identifier)
        .collect();
    if let Phase::Ready(bytes, _) = &state.phase {
        let bytes = Arc::clone(bytes);
        return Ok(finish_approval(
            app, &mut state, offer, bytes, &approved, install,
        ));
    }
    // A failed or interrupted download needs fresh release information, including after restart.
    Schedule::edit(&schedule_file(app)?, |schedule| schedule.manifest = None)?;
    state.phase = Phase::Downloading(0);
    let status = state.publish(app, false);
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || download(&app, offer, &approved, install));
    Ok(status)
}

fn download<R: Runtime>(
    app: &AppHandle<R>,
    offer: Offer,
    approved: &[u64],
    install: BeginInstall<R>,
) {
    let mut last_progress: Option<Instant> = None;
    let result = transport::fetch(
        &offer.update.download_url,
        transport::ARCHIVE_LIMIT,
        600,
        true,
        |downloaded| {
            if last_progress.is_some_and(|last| last.elapsed() < Duration::from_millis(150)) {
                return;
            }
            last_progress = Some(Instant::now());
            let mut state = State::lock(app);
            if state.has_offer(offer.id) && matches!(state.phase, Phase::Downloading(_)) {
                state.phase = Phase::Downloading(downloaded);
                state.publish(app, false);
            }
        },
    )
    .and_then(|bytes| {
        offer
            .update
            .verify(&bytes)
            .map_err(|_| "The update signature could not be verified.")?;
        package::validate(&bytes, &offer.update.version, &app.config().identifier)?;
        Ok(Arc::new(bytes))
    });
    let mut state = State::lock(app);
    if !state.has_offer(offer.id) || !matches!(state.phase, Phase::Downloading(_)) {
        return;
    }
    match result {
        Ok(bytes) => {
            finish_approval(app, &mut state, offer, bytes, approved, install);
        }
        Err(message) => {
            reject_offer(app, &mut state, message, true);
        }
    }
}

fn finish_approval<R: Runtime>(
    app: &AppHandle<R>,
    state: &mut State,
    offer: Offer,
    bytes: Arc<Vec<u8>>,
    approved: &[u64],
    install: BeginInstall<R>,
) -> Status {
    let result = terminal::with_active_update_sessions(|active| {
        let changed = active.iter().any(|session| !approved.contains(&session.identifier));
        state.refresh_sessions(active);
        if changed {
            return Err("Another session started during the update. Review the sessions before you continue.".into());
        }
        install(app, offer, Arc::clone(&bytes))
    }).and_then(|result| result);
    match result {
        Ok(()) => state.phase = Phase::ShuttingDown,
        Err(message) => state.phase = Phase::Ready(bytes, message),
    }
    let needs_approval = matches!(state.phase, Phase::Ready(_, _));
    state.publish(app, needs_approval)
}

fn begin_install(app: &AppHandle, offer: Offer, bytes: Arc<Vec<u8>>) -> Result<(), String> {
    let package = Arc::clone(&bytes);
    let failure_app = app.clone();
    let offer_id = offer.id;
    lifecycle::begin_update(
        app,
        offer.update.version.clone(),
        move || {
            offer.update.install(package.as_slice()).map_err(|error| {
                let preserved_bundle = match &error {
                    tauri_plugin_updater::Error::PreviousAppNotRestored(path) => Some(path.clone()),
                    _ => None,
                };
                lifecycle::InstallFailure {
                    message: error.to_string(),
                    preserved_bundle,
                }
            })
        },
        move |failure| report_failure(&failure_app, offer_id, bytes, failure),
    )
}

fn report_failure(
    app: &AppHandle,
    offer_id: u64,
    bytes: Arc<Vec<u8>>,
    failure: lifecycle::UpdateFailure,
) {
    let mut state = State::lock(app);
    if !state.has_offer(offer_id) || !matches!(state.phase, Phase::ShuttingDown) {
        return;
    }
    state.phase = match failure {
        lifecycle::UpdateFailure::Shutdown(message) => Phase::Ready(bytes, message),
        lifecycle::UpdateFailure::Recovery {
            message,
            installed_version,
            can_reopen,
        } => {
            state.offer = None;
            Phase::Recovery {
                message,
                version: installed_version,
                can_reopen,
            }
        }
    };
    refresh_sessions(&mut state);
    state.publish(app, true);
}

#[tauri::command]
pub(crate) async fn app_update_reopen(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || lifecycle::reopen_after_update(&app))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn app_update_open_installer() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        // Recovery runs with the ordinary host command gate closed.
        Command::new("/usr/bin/open")
            .arg(INSTALLER)
            .spawn()
            .map(|mut child| {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            })
            .map_err(|error| format!("Could not open the signed installer page. {error}"))
    })
    .await
    .map_err(|error| error.to_string())?
}
