use crate::{command, host, storage};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Default)]
pub struct Installation(pub Mutex<Option<Arc<AtomicBool>>>);

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    completed: Vec<String>,
    current: Option<String>,
    message: Option<String>,
}

fn install(app: &AppHandle, cancel: &AtomicBool) -> Result<Progress, String> {
    let brew = host::discover("brew", None)
        .ok_or("Homebrew was not found. Install it from brew.sh first.")?;
    let file = app
        .path()
        .app_config_dir()
        .map_err(|error| error.to_string())?
        .join("installation.json");
    install_steps(&brew, &file, cancel, |progress| {
        app.emit("installation-progress", progress)
            .map_err(|error| error.to_string())
    })
}

fn install_steps(
    brew: &Path,
    file: &Path,
    cancel: &AtomicBool,
    emit: impl Fn(&Progress) -> Result<(), String>,
) -> Result<Progress, String> {
    let mut progress: Progress = storage::read(file)?.unwrap_or_default();
    let steps: [(&str, &[&str]); 4] = [
        ("update", &["update"]),
        ("tap", &["tap", "brig-sh/brig"]),
        ("trust", &["trust", "brig-sh/brig"]),
        ("install", &["install", "--cask", "brig"]),
    ];
    for (name, arguments) in steps {
        if progress.completed.iter().any(|step| step == name) {
            continue;
        }
        progress.current = Some(name.into());
        progress.message = None;
        storage::write(file, &progress)?;
        emit(&progress)?;
        let output = command::run(
            brew,
            &arguments
                .iter()
                .map(|argument| (*argument).to_owned())
                .collect::<Vec<_>>(),
            None,
            &command::environment(),
            Duration::from_secs(900),
            cancel,
        )?;
        if output.cancelled
            || output.timed_out
            || output.exit_code != Some(0)
            || output.output_exceeded
        {
            progress.message = Some(format!(
                "Homebrew {name} failed. Restart installation to retry this step."
            ));
            storage::write(file, &progress)?;
            return Ok(progress);
        }
        progress.completed.push(name.into());
        storage::write(file, &progress)?;
    }
    progress.current = None;
    progress.message = Some("Brig installation finished. Run the Mac check again.".into());
    storage::write(file, &progress)?;
    Ok(progress)
}

#[tauri::command]
pub async fn install_brig(
    app: AppHandle,
    state: State<'_, Installation>,
) -> Result<Progress, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut current = state.0.lock().map_err(|error| error.to_string())?;
        if current.is_some() {
            return Err("Installation is already running.".into());
        }
        *current = Some(cancel.clone());
    }
    let result = tauri::async_runtime::spawn_blocking(move || install(&app, &cancel))
        .await
        .map_err(|error| error.to_string());
    *state.0.lock().map_err(|error| error.to_string())? = None;
    result?
}

#[tauri::command]
pub fn cancel_installation(state: State<'_, Installation>) -> Result<(), String> {
    if let Some(cancel) = state.0.lock().map_err(|error| error.to_string())?.as_ref() {
        cancel.store(true, Ordering::SeqCst);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn interrupted_installation_retries_the_failed_step_and_keeps_completed_steps() {
        let directory =
            std::env::temp_dir().join(format!("brig-install-resume-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let executable = directory.join("test brew");
        fs::write(&executable, "#!/bin/sh\nroot=${0%/*}\nprintf '%s\\n' \"$*\" >> \"$root/calls\"\nif test \"$1\" = tap && ! test -e \"$root/retry\"; then : > \"$root/retry\"; exit 2; fi\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let file = directory.join("progress.json");
        let cancel = AtomicBool::new(false);
        let first = install_steps(&executable, &file, &cancel, |_| Ok(())).unwrap();
        assert_eq!(first.completed, ["update"]);
        assert_eq!(first.current.as_deref(), Some("tap"));
        assert!(first.message.unwrap().contains("retry this step"));
        let resumed = install_steps(&executable, &file, &cancel, |_| Ok(())).unwrap();
        assert_eq!(resumed.completed, ["update", "tap", "trust", "install"]);
        assert!(resumed.current.is_none());
        assert_eq!(
            fs::read_to_string(directory.join("calls")).unwrap(),
            "update\ntap brig-sh/brig\ntap brig-sh/brig\ntrust brig-sh/brig\ninstall --cask brig\n"
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
