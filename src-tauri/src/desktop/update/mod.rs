pub(crate) mod consent;
mod package;
mod schedule;
mod state;
mod transport;

use crate::{desktop::lifecycle, terminal};
use schedule::Schedule;
use serde::Deserialize;
use serde_json::Value;
use state::{Offer, Phase, State, Status, Updates};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime, Url};
use tauri_plugin_updater::{Update, UpdaterExt};

pub(super) const INSTALLER: &str = "https://github.com/reformo/tugboat/releases/latest";
pub(crate) const RELEASE_BUILD_NUMBER: Option<&str> = option_env!("TUGBOAT_RELEASE_BUILD_NUMBER");
const STABLE_FEED: &str = "https://github.com/reformo/tugboat/releases/latest/download/latest.json";
const TARGET: &str = "darwin-aarch64";

pub(crate) fn supported(app: &AppHandle) -> bool {
    supported_identity(app, tauri::is_dev())
}

fn supported_identity<R: Runtime>(app: &AppHandle<R>, development: bool) -> bool {
    production_identity(development, &app.config().identifier)
        && cfg!(target_arch = "aarch64")
        && app
            .config()
            .plugins
            .0
            .get("updater")
            .and_then(|config| config.get("pubkey"))
            .and_then(Value::as_str)
            .is_some_and(|key| !key.trim().is_empty())
}

fn production_identity(development: bool, identifier: &str) -> bool {
    !development && identifier == "sh.brig.desktop"
}

pub(crate) fn configure(app: &AppHandle) -> tauri::Result<()> {
    configure_identity(app, tauri::is_dev())
}

fn configure_identity<R: Runtime>(app: &AppHandle<R>, development: bool) -> tauri::Result<()> {
    let enabled = supported_identity(app, development);
    if enabled {
        app.plugin(tauri_plugin_updater::Builder::new().target(TARGET).build())?;
    }
    app.manage(Updates(Mutex::new(State::new(enabled))));
    Ok(())
}

pub(crate) fn start(app: &AppHandle) {
    static START: Once = Once::new();
    if !supported(app) {
        return;
    }
    START.call_once(|| {
        let app = app.clone();
        std::thread::spawn(move || loop {
            if !lifecycle::is_closing() {
                check(&app, false);
            }
            std::thread::sleep(Duration::from_secs(60));
        });
    });
}

fn schedule_file<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|path| path.join("app-updates.json"))
        .map_err(|error| error.to_string())
}

fn feed<R: Runtime>(app: &AppHandle<R>) -> Result<Url, String> {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|config| config.get("endpoints"))
        .and_then(Value::as_array)
        .and_then(|endpoints| endpoints.first())
        .and_then(Value::as_str)
        .and_then(|address| Url::parse(address).ok())
        .filter(secure_url)
        .ok_or_else(|| "The app update feed configuration is invalid.".into())
}

fn secure_url(address: &Url) -> bool {
    let secure = address.scheme() == "https";
    #[cfg(test)]
    let secure = secure || transport::loopback_fixture(address);
    secure
        && address.host_str().is_some()
        && address.username().is_empty()
        && address.password().is_none()
        && address.fragment().is_none()
}

#[derive(Deserialize)]
struct Manifest {
    version: semver::Version,
    notes: String,
    platforms: Platforms,
}

#[derive(Deserialize)]
struct Platforms {
    #[serde(rename = "darwin-aarch64")]
    mac: Artifact,
}

#[derive(Deserialize)]
struct Artifact {
    url: Url,
    signature: String,
}

fn is_newer(current: &semver::Version, candidate: &semver::Version) -> bool {
    candidate.pre.is_empty() && candidate.cmp_precedence(current).is_gt()
}

fn read_manifest(raw: &Value, endpoint: &Url) -> Result<Manifest, String> {
    let manifest: Manifest = serde_json::from_value(raw.clone())
        .map_err(|_| "The update feed is missing required release information.")?;
    let artifact = &manifest.platforms.mac;
    let allowed = if endpoint.as_str() == STABLE_FEED {
        artifact.url.origin() == endpoint.origin()
            && artifact.url.query().is_none()
            && artifact
                .url
                .path()
                .strip_prefix("/reformo/tugboat/releases/download/")
                .and_then(|path| path.strip_suffix("/Tugboat.app.tar.gz"))
                .is_some_and(|tag| !tag.is_empty() && !tag.contains('/'))
    } else {
        artifact.url.origin() == endpoint.origin()
    };
    if !secure_url(&artifact.url)
        || !allowed
        || artifact.signature.trim().is_empty()
        || manifest.notes.trim().is_empty()
        || manifest.notes.len() > 64 * 1024
    {
        return Err("The update feed contains invalid release information.".into());
    }
    Ok(manifest)
}

fn update_from_manifest<R: Runtime>(
    app: &AppHandle<R>,
    endpoint: &Url,
    raw: Value,
) -> Result<Option<Update>, String> {
    let manifest = read_manifest(&raw, endpoint)?;
    if !is_newer(&app.package_info().version, &manifest.version) {
        return Ok(None);
    }
    app.updater_builder()
        .version_comparator(|current, release| is_newer(&current, &release.version))
        .build()
        .and_then(|updater| updater.restore_update(raw))
        .map_err(|_| "Could not read the update release information.".into())
}

fn current_status<R: Runtime>(app: &AppHandle<R>, state: &mut State) -> Status {
    refresh_sessions(state);
    state.snapshot(app, false)
}

fn refresh_sessions(state: &mut State) {
    if state.offer.is_some() {
        if let Ok(active) = terminal::with_active_update_sessions(|active| active) {
            state.refresh_sessions(active);
        }
    }
}

fn check<R: Runtime>(app: &AppHandle<R>, manual: bool) -> Status {
    let mut state = State::lock(app);
    if matches!(
        state.phase,
        Phase::Disabled
            | Phase::Checking
            | Phase::Downloading(_)
            | Phase::ShuttingDown
            | Phase::Recovery { .. }
    ) {
        return current_status(app, &mut state);
    }
    let endpoint = feed(app);
    let file = schedule_file(app);
    let checked = endpoint.and_then(|endpoint| {
        let file = file?;
        let timestamp = schedule::now();
        let (claimed, cached) = Schedule::claim_file(&file, timestamp, manual)?;
        Ok((endpoint, file, claimed, cached))
    });
    let (endpoint, file, claimed, cached) = match checked {
        Ok(result) => result,
        Err(error) => return reject_offer(app, &mut state, error, false),
    };
    if !claimed {
        if matches!(state.phase, Phase::Idle) {
            if let Some(update) =
                cached.and_then(|raw| update_from_manifest(app, &endpoint, raw).ok().flatten())
            {
                state.offer = Some(Offer {
                    id: state.revision + 1,
                    update: Arc::new(update),
                });
                state.phase = Phase::Available;
                refresh_sessions(&mut state);
                return state.publish(app, false);
            }
        }
        return current_status(app, &mut state);
    }
    let previous = std::mem::replace(&mut state.phase, Phase::Checking);
    state.publish(app, false);
    drop(state);

    let result = transport::fetch(&endpoint, transport::MANIFEST_LIMIT, 15, false, |_| {})
        .and_then(|bytes| {
            serde_json::from_slice::<Value>(&bytes)
                .map_err(|_| "The update feed returned invalid JSON.".into())
        })
        .and_then(|raw| {
            let update = update_from_manifest(app, &endpoint, raw.clone())?;
            Schedule::edit(&file, |schedule| {
                schedule.manifest = update.as_ref().map(|_| raw)
            })?;
            Ok(update)
        });
    let mut state = State::lock(app);
    let update = match result {
        Ok(update) => update,
        Err(error) if matches!(previous, Phase::Ready(_, _)) => {
            if let Phase::Ready(bytes, _) = previous {
                state.phase = Phase::Ready(bytes, error);
            }
            return state.publish(app, false);
        }
        Err(error) => return reject_offer(app, &mut state, error, false),
    };
    let previous_offer = state.offer.take();
    state.phase = Phase::Current;
    if let Some(update) = update {
        let same = previous_offer.as_ref().is_some_and(|offer| {
            offer.update.version == update.version
                && offer.update.download_url == update.download_url
                && offer.update.signature == update.signature
                && offer.update.body == update.body
        });
        let id = state.revision + 1;
        state.offer = Some(Offer {
            id,
            update: Arc::new(update),
        });
        state.phase = match previous {
            ready @ Phase::Ready(_, _) if same => ready,
            _ => Phase::Available,
        };
        refresh_sessions(&mut state);
    }
    let automatic = !manual && state.offer.is_some();
    state.publish(app, automatic)
}

fn reject_offer<R: Runtime>(
    app: &AppHandle<R>,
    state: &mut State,
    message: String,
    show: bool,
) -> Status {
    state.offer = None;
    state.refresh_sessions(Vec::new());
    state.phase = Phase::Failed {
        message,
        installer: false,
    };
    state.publish(app, show)
}

#[tauri::command]
pub(crate) async fn app_update_status(app: AppHandle) -> Result<Status, String> {
    tauri::async_runtime::spawn_blocking(move || current_status(&app, &mut State::lock(&app)))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn app_update_check(app: AppHandle) -> Result<Status, String> {
    tauri::async_runtime::spawn_blocking(move || check(&app, true))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn app_update_later(app: AppHandle, offer_id: u64) -> Result<Status, String> {
    tauri::async_runtime::spawn_blocking(move || later(&app, offer_id))
        .await
        .map_err(|error| error.to_string())?
}

fn later<R: Runtime>(app: &AppHandle<R>, offer_id: u64) -> Result<Status, String> {
    let mut state = State::lock(app);
    if state.has_offer(offer_id)
        && matches!(
            state.phase,
            Phase::Available | Phase::Ready(_, _) | Phase::Failed { .. }
        )
    {
        Schedule::edit(&schedule_file(app)?, |schedule| {
            schedule.defer(schedule::now())
        })?;
    }
    Ok(state.publish(app, false))
}

#[cfg(test)]
mod tests;
