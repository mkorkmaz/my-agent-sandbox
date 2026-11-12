use crate::{command, host, storage};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};

#[derive(Default, Clone)]
pub struct Installation {
    current: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    done: Arc<Condvar>,
}

struct FinishInstallation(Installation);
impl Drop for FinishInstallation {
    fn drop(&mut self) {
        *self
            .0
            .current
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        self.0.done.notify_all();
    }
}

impl Installation {
    pub fn running(&self) -> bool {
        self.current.lock().is_ok_and(|current| current.is_some())
    }

    fn begin(&self) -> Result<(Arc<AtomicBool>, FinishInstallation), String> {
        let mut current = self.current.lock().map_err(|error| error.to_string())?;
        if current.is_some() {
            return Err("Installation is already running.".into());
        }
        if crate::lifecycle::is_closing() {
            return Err("The app is closing.".into());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *current = Some(cancel.clone());
        Ok((cancel, FinishInstallation(self.clone())))
    }

    pub fn cancel_and_wait(&self, timeout: Duration) -> Result<(), String> {
        let current = self.current.lock().map_err(|error| error.to_string())?;
        if let Some(cancel) = current.as_ref() {
            cancel.store(true, Ordering::SeqCst);
        }
        let (current, _) = self
            .done
            .wait_timeout_while(current, timeout, |current| current.is_some())
            .map_err(|error| error.to_string())?;
        if current.is_some() {
            return Err("Installation has not stopped yet. Retry closing the app.".into());
        }
        Ok(())
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    completed: Vec<String>,
    current: Option<String>,
    message: Option<String>,
}

fn install(app: &AppHandle, cancel: &AtomicBool) -> Result<Progress, String> {
    let configured = crate::settings::load(&crate::settings::file(app)?)?.cli_path;
    let brew = host::discover("brew", None)
        .ok_or("Homebrew was not found. Install it from brew.sh first.")?;
    let records_directory = crate::data_location::records_directory(app)?;
    let file = crate::data_location::directory(app)?.join("installation.json");
    install_steps(
        &brew,
        &file,
        cancel,
        Some(&records_directory),
        || installed_version(configured.as_deref(), cancel),
        |progress| {
            app.emit("installation-progress", progress)
                .map_err(|error| error.to_string())
        },
    )
}

fn installed_version(
    configured: Option<&str>,
    cancel: &AtomicBool,
) -> Result<Option<String>, String> {
    let Some(brig) = host::discover("brig", configured) else {
        return Ok(None);
    };
    let output = host::query(&brig, &["--json", "version"], cancel)?;
    let version = host::envelope(&output, "Version")?;
    Ok(Some(
        version["data"]["version"]
            .as_str()
            .ok_or("The installed Brig version is missing.")?
            .to_owned(),
    ))
}

fn install_steps(
    brew: &Path,
    file: &Path,
    cancel: &AtomicBool,
    records_directory: Option<&crate::data_location::RecordsDirectory>,
    mut version: impl FnMut() -> Result<Option<String>, String>,
    emit: impl Fn(&Progress) -> Result<(), String>,
) -> Result<Progress, String> {
    let save = |progress: &Progress| -> Result<(), String> {
        if let Some(directory) = records_directory {
            directory.verify()?;
        }
        storage::write(file, progress)
    };
    if let Some(directory) = records_directory {
        directory.verify()?;
    }
    let mut progress: Progress = storage::read(file)?.unwrap_or_default();
    let installed = version()?;
    if installed.as_deref() == Some(host::SUPPORTED_BRIG_VERSION) {
        progress.current = None;
        progress.message = Some(format!(
            "Brig {} is already available. Run the Mac check again.",
            host::SUPPORTED_BRIG_VERSION
        ));
        save(&progress)?;
        return Ok(progress);
    }
    // Old completion describes the previous install, not the CLI currently on disk.
    progress.completed.retain(|step| step != "install");
    let install_arguments = if installed.is_some() {
        &["reinstall", "--cask", "brig"][..]
    } else {
        &["install", "--cask", "brig"][..]
    };
    let steps: [(&str, &[&str]); 4] = [
        ("update", &["update"]),
        ("tap", &["tap", "brig-sh/brig"]),
        ("trust", &["trust", "brig-sh/brig"]),
        ("install", install_arguments),
    ];
    for (name, arguments) in steps {
        if progress.completed.iter().any(|step| step == name) {
            continue;
        }
        progress.current = Some(name.into());
        progress.message = None;
        save(&progress)?;
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
            save(&progress)?;
            return Ok(progress);
        }
        progress.completed.push(name.into());
        save(&progress)?;
    }
    progress.current = Some("verify".into());
    save(&progress)?;
    emit(&progress)?;
    progress.message = Some(match version() {
        Ok(Some(found)) if found == host::SUPPORTED_BRIG_VERSION => {
            progress.current = None;
            format!("Brig {found} installation verified. Run the Mac check again.")
        }
        Ok(Some(found)) => format!("Brig {found} was found, but this app requires {}. Select a compatible CLI path before continuing.", host::SUPPORTED_BRIG_VERSION),
        Ok(None) => "Homebrew finished, but Brig was not found at the configured CLI path. Check the path and run the Mac check again.".into(),
        Err(error) => format!("Homebrew finished, but the installed CLI could not be verified. {error}"),
    });
    save(&progress)?;
    Ok(progress)
}

#[tauri::command]
pub async fn install_brig(
    app: AppHandle,
    state: State<'_, Installation>,
) -> Result<Progress, String> {
    let (cancel, finish) = state.begin()?;
    tauri::async_runtime::spawn_blocking(move || {
        let _finish = finish;
        install(&app, &cancel)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn cancel_installation(state: State<'_, Installation>) -> Result<(), String> {
    if let Some(cancel) = state
        .current
        .lock()
        .map_err(|error| error.to_string())?
        .as_ref()
    {
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
    fn closing_waits_for_installation_to_save_the_retryable_step() {
        let root =
            std::env::temp_dir().join(format!("tugboat-install-close-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let brew = root.join("brew");
        fs::write(&brew, "#!/bin/sh\n/bin/sleep 60\n").unwrap();
        fs::set_permissions(&brew, fs::Permissions::from_mode(0o700)).unwrap();
        let file = root.join("installation.json");
        let installation = Installation::default();
        let (cancel, finish) = installation.begin().unwrap();
        let destination = file.clone();
        let worker = std::thread::spawn(move || {
            let _finish = finish;
            install_steps(&brew, &destination, &cancel, None, || Ok(None), |_| Ok(())).unwrap()
        });
        while !file.is_file() {
            std::thread::yield_now();
        }
        assert!(installation.running());
        installation
            .cancel_and_wait(Duration::from_secs(2))
            .unwrap();
        assert!(!installation.running());
        let result = worker.join().unwrap();
        assert_eq!(result.current.as_deref(), Some("update"));
        assert!(result.completed.is_empty());
        let saved: Progress = storage::read(&file).unwrap().unwrap();
        assert!(saved.message.unwrap().contains("retry"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interrupted_installation_retries_the_failed_step_and_keeps_completed_steps() {
        let directory =
            std::env::temp_dir().join(format!("brig-install-resume-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let executable = directory.join("test brew");
        fs::write(&executable, "#!/bin/sh\nroot=${0%/*}\nprintf '%s\\n' \"$*\" >> \"$root/calls\"\nif test \"$1\" = tap && ! test -e \"$root/retry\"; then : > \"$root/retry\"; exit 2; fi\nif test \"$1\" = install; then : > \"$root/installed\"; fi\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let file = directory.join("progress.json");
        let cancel = AtomicBool::new(false);
        let version = || {
            Ok(directory
                .join("installed")
                .is_file()
                .then(|| host::SUPPORTED_BRIG_VERSION.into()))
        };
        let first = install_steps(&executable, &file, &cancel, None, &version, |_| Ok(())).unwrap();
        assert_eq!(first.completed, ["update"]);
        assert_eq!(first.current.as_deref(), Some("tap"));
        assert!(first.message.unwrap().contains("retry this step"));
        let resumed =
            install_steps(&executable, &file, &cancel, None, &version, |_| Ok(())).unwrap();
        assert_eq!(resumed.completed, ["update", "tap", "trust", "install"]);
        assert!(resumed.current.is_none());
        assert!(resumed.message.unwrap().contains("verified"));
        assert_eq!(
            fs::read_to_string(directory.join("calls")).unwrap(),
            "update\ntap brig-sh/brig\ntap brig-sh/brig\ntrust brig-sh/brig\ninstall --cask brig\n"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn an_old_completed_install_is_reconciled_with_the_cli_and_verified_again() {
        let root = std::env::temp_dir().join(format!("tugboat-reinstall-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let brew = root.join("brew");
        fs::write(&brew, "#!/bin/sh\nroot=${0%/*}\nprintf '%s\\n' \"$*\" >> \"$root/calls\"\n: > \"$root/installed\"\n").unwrap();
        fs::set_permissions(&brew, fs::Permissions::from_mode(0o700)).unwrap();
        let file = root.join("installation.json");
        let completed = || Progress {
            completed: ["update", "tap", "trust", "install"]
                .map(str::to_owned)
                .to_vec(),
            current: None,
            message: None,
        };
        storage::write(&file, &completed()).unwrap();
        let probe = || {
            Ok(root
                .join("installed")
                .is_file()
                .then(|| host::SUPPORTED_BRIG_VERSION.into()))
        };
        let cancel = AtomicBool::new(false);
        for _ in 0..2 {
            let result = install_steps(&brew, &file, &cancel, None, &probe, |_| Ok(())).unwrap();
            assert!(result.current.is_none());
            assert!(result.message.unwrap().contains("verified"));
            fs::remove_file(root.join("installed")).unwrap();
        }
        assert_eq!(
            fs::read_to_string(root.join("calls")).unwrap(),
            "install --cask brig\ninstall --cask brig\n"
        );
        for final_version in [
            Ok(Some("v9.9.9".to_owned())),
            Ok(None),
            Err("Invalid version response".to_owned()),
        ] {
            storage::write(&file, &completed()).unwrap();
            let mut calls = 0;
            let result = install_steps(
                &brew,
                &file,
                &cancel,
                None,
                || {
                    calls += 1;
                    if calls == 1 {
                        Ok(None)
                    } else {
                        final_version.clone()
                    }
                },
                |_| Ok(()),
            )
            .unwrap();
            assert_eq!(result.current.as_deref(), Some("verify"));
            assert!(!result.message.unwrap().contains("installation verified"));
        }
        fs::remove_dir_all(root).unwrap();
    }
}
