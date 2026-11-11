use crate::{
    brig::{self, Brig},
    projects::{self, Project, Projects, Registry},
    settings::{self, Settings},
    storage,
    terminal::{self, Layout},
};
use serde::Serialize;
use serde_json::Value;
use std::process::Command;
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

pub struct CheckedProject {
    pub project: Project,
    pub settings: Settings,
    pub brig: Brig,
    pub plan: Value,
    pub fingerprint: String,
}

pub fn plan_fingerprint(plan: &Value) -> Result<String, String> {
    let mut snapshot = plan.clone();
    // This prose contains transient inspect errors. All executable plan fields remain covered.
    snapshot["data"]
        .as_object_mut()
        .ok_or("Invalid Brig plan.")?
        .remove("network");
    brig::fingerprint(&snapshot)
}

pub fn checked_project(
    app: &AppHandle,
    identifier: &str,
    expected: Option<&str>,
) -> Result<CheckedProject, String> {
    let mut project = Registry::load(&projects::file(app)?)?.project(identifier)?;
    project.require_active()?;
    project.check_path()?;
    project.check_guest_path()?;
    if project.policy_detached {
        return Err("Attach the project policy before preparing the environment.".into());
    }
    if !["ready", "environment-starting", "environment-ready"].contains(&project.stage.as_str()) {
        return Err("Complete project setup first.".into());
    }
    let settings = project.settings(settings::load(&settings::file(app)?)?)?;
    let brig = Brig::new(&settings)?;
    brig.validate_codex_profile(&project.profile)?;
    let policy = project
        .policy
        .as_ref()
        .ok_or("The project policy is missing.")?;
    let document = brig
        .policy(&policy.name)?
        .ok_or("The project policy was removed.")?;
    if policy.fingerprint.as_deref() != Some(brig::fingerprint(&document)?.as_str()) {
        return Err(
            "The policy changed after the plan was shown. Review the current policy and prepare the project again.".into(),
        );
    }
    let plan = brig.plan(&project, &settings)?;
    brig::validate_plan(&plan, &project, &settings)?;
    if project.stage != "environment-starting" {
        project.stage =
            if is_running(&brig, &project, &plan)? && project.environment_verified == Some(true) {
                "environment-ready"
            } else {
                "ready"
            }
            .into();
    }
    let fingerprint = plan_fingerprint(&plan)?;
    if expected.is_some_and(|expected| expected != fingerprint) {
        return Err("The execution plan changed. Review it and start again.".into());
    }
    Ok(CheckedProject {
        project,
        settings,
        brig,
        plan,
        fingerprint,
    })
}

pub(crate) fn is_running(brig: &Brig, project: &Project, plan: &Value) -> Result<bool, String> {
    let list = brig
        .json(&["--json".into(), "ls".into()], "SandboxList", None)
        .map_err(|message| {
            format!("Could not verify the current Brig environment state. {message}")
        })?;
    let sessions = list["data"]["sandboxes"]
        .as_array()
        .ok_or("Could not read the Brig environment inventory.")?;
    let Some(session) = sessions
        .iter()
        .find(|session| session["ref"] == project.reference())
    else {
        return Ok(false);
    };
    if session["state"] == "stopped" {
        return Ok(false);
    }
    if session["state"] != "running" {
        return Err("The Brig environment state could not be verified. Refresh the project before continuing.".into());
    }
    let info = brig.json(
        &["--json".into(), "info".into(), project.reference()],
        "Info",
        None,
    )?;
    let home = plan["data"]["home"]["host"]
        .as_str()
        .ok_or("The guest home is missing from the execution plan.")?;
    if info["data"]["profile"] != project.profile
        || info["data"]["session"] != project.label
        || info["data"]["project"]["host"].as_str() != project.path.to_str()
        || info["data"]["workspace"].as_str() != Some(home)
    {
        return Err(
            "The running Brig environment does not match this project's folder or guest home."
                .into(),
        );
    }
    Ok(true)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    project: Project,
    plan: Value,
    fingerprint: String,
}

#[tauri::command]
pub async fn preview_project(app: AppHandle, identifier: String) -> Result<Preview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let checked = checked_project(&app, &identifier, None)?;
        Ok(Preview {
            project: checked.project,
            plan: checked.plan,
            fingerprint: checked.fingerprint,
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

pub fn command(checked: &CheckedProject, arguments: Vec<String>) -> Command {
    let mut command = Command::new(&checked.brig.executable);
    command
        .args(arguments)
        .current_dir(&checked.project.path)
        .env_clear()
        .envs(&checked.brig.environment)
        .env("TERM", "xterm-256color");
    command
}

pub fn run_arguments(checked: &CheckedProject, codex_arguments: &[&str]) -> Vec<String> {
    let mut arguments = vec!["run".into()];
    arguments.extend(checked.settings.arguments());
    arguments.extend([
        checked.project.reference(),
        checked.project.path.to_string_lossy().into_owned(),
        "--".into(),
    ]);
    arguments.extend(
        codex_arguments
            .iter()
            .map(|argument| (*argument).to_owned()),
    );
    arguments
}

#[tauri::command]
pub async fn prepare_environment(
    window: WebviewWindow,
    projects: State<'_, Projects>,
    identifier: String,
    fingerprint: String,
    layout: Layout,
) -> Result<u64, String> {
    let app = window.app_handle().clone();
    let preparation_app = app.clone();
    let registry_lock = projects.0.clone();
    let project_id = identifier.clone();
    let terminal_identifier = terminal::reserve_identifier();
    let token = format!(
        "{}-{terminal_identifier}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let pending_token = token.clone();
    let command = tauri::async_runtime::spawn_blocking(move || {
        let _guard = registry_lock.lock().map_err(|error| error.to_string())?;
        if terminal::terminal_sessions(identifier.clone())
            .iter()
            .any(|status| status.purpose == "environment" && status.running)
        {
            return Err("Environment preparation is already running for this project.".into());
        }
        let file = projects::file(&preparation_app)?;
        let _file_guard = storage::lock(&file)?;
        let mut checked = checked_project(&preparation_app, &identifier, Some(&fingerprint))?;
        let command = command(&checked, run_arguments(&checked, &["--version"]));
        checked.project.stage = "environment-starting".into();
        checked.project.environment_verified = Some(false);
        checked.project.environment_attempt = Some(projects::EnvironmentAttempt {
            token: pending_token,
            terminal_identifier,
            completed: false,
            error: None,
        });
        let mut registry = Registry::load(&file)?;
        registry.update(checked.project)?;
        storage::write(&file, &registry)?;
        Ok::<_, String>(command)
    })
    .await
    .map_err(|error| error.to_string())??;
    let completed_app = app.clone();
    let completed_project = project_id.clone();
    let completed_token = token.clone();
    let on_exit = Box::new(move |code| {
        let result = if code == Some(0) {
            Ok(())
        } else {
            Err("Could not verify the guest Codex version. Check the terminal output and prepare the environment again.".into())
        };
        complete_environment(
            &completed_app,
            &completed_project,
            &completed_token,
            terminal_identifier,
            result,
        );
    });
    let result = terminal::start_reserved(
        window,
        project_id.clone(),
        "environment",
        command,
        layout,
        None,
        terminal::Reservation {
            identifier: terminal_identifier,
            on_exit: Some(on_exit),
        },
    )
    .await;
    if let Err(error) = &result {
        complete_environment(
            &app,
            &project_id,
            &token,
            terminal_identifier,
            Err(format!("Could not open the environment terminal. {error}")),
        );
    }
    result
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EnvironmentResult {
    project: Project,
    terminal_identifier: u64,
    message: Option<String>,
}

fn complete_attempt(
    file: &std::path::Path,
    identifier: &str,
    token: &str,
    terminal_identifier: u64,
    process_result: Result<(), String>,
    verify: impl FnOnce() -> Result<(), String>,
) -> Result<Option<EnvironmentResult>, String> {
    let _file_guard = storage::lock(file)?;
    let mut registry = Registry::load(file)?;
    let mut project = registry.project(identifier)?;
    if project.removal_pending() {
        return Ok(None);
    }
    let Some(attempt) = &project.environment_attempt else {
        return Ok(None);
    };
    if attempt.token != token || attempt.terminal_identifier != terminal_identifier {
        return Ok(None);
    }
    if attempt.completed {
        return Ok(Some(EnvironmentResult {
            terminal_identifier,
            message: attempt.error.clone(),
            project,
        }));
    }
    let message = process_result.and_then(|_| verify()).err();
    project.environment_verified = Some(message.is_none());
    project.stage = if message.is_none() {
        "environment-ready"
    } else {
        "ready"
    }
    .into();
    let attempt = project.environment_attempt.as_mut().unwrap();
    attempt.completed = true;
    attempt.error = message.clone();
    registry.update(project.clone())?;
    storage::write(file, &registry)?;
    Ok(Some(EnvironmentResult {
        project,
        terminal_identifier,
        message,
    }))
}

fn complete_environment(
    app: &AppHandle,
    identifier: &str,
    token: &str,
    terminal_identifier: u64,
    process_result: Result<(), String>,
) {
    let result = (|| {
        let projects = app.state::<Projects>();
        let _guard = projects.0.lock().map_err(|error| error.to_string())?;
        complete_attempt(
            &projects::file(app)?,
            identifier,
            token,
            terminal_identifier,
            process_result,
            || {
                let checked = checked_project(app, identifier, None)?;
                if !is_running(&checked.brig, &checked.project, &checked.plan)? {
                    return Err(
                        "The Brig environment did not remain running. Prepare it again.".into(),
                    );
                }
                Ok(())
            },
        )
    })();
    match result {
        Ok(Some(result)) => {
            let _ = app.emit("environment-preparation-finished", result);
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!("Could not complete environment preparation: {error}");
            let _ = app.emit("environment-preparation-error", serde_json::json!({"projectId":identifier,"terminalIdentifier":terminal_identifier,"message":error}));
        }
    }
}

#[tauri::command]
pub async fn finish_environment(
    app: AppHandle,
    projects: State<'_, Projects>,
    identifier: String,
    terminal_identifier: u64,
) -> Result<Project, String> {
    let status = terminal::terminal_snapshot(terminal_identifier)
        .ok_or("The environment terminal record was not found.")?;
    if status.project_id != identifier || status.purpose != "environment" || status.running {
        return Err("Could not verify the guest Codex version. Check the terminal output.".into());
    }
    let lock = projects.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = lock.lock().map_err(|error| error.to_string())?;
        let file = projects::file(&app)?;
        let project = Registry::load(&file)?.project(&identifier)?;
        let attempt = project
            .environment_attempt
            .ok_or("The preparation attempt was not found. Prepare the environment again.")?;
        let process_result = if status.exit_code == Some(0) {
            Ok(())
        } else {
            Err("Could not verify the guest Codex version. Prepare the environment again.".into())
        };
        let result = complete_attempt(
            &file,
            &identifier,
            &attempt.token,
            terminal_identifier,
            process_result,
            || {
                let checked = checked_project(&app, &identifier, None)?;
                if !is_running(&checked.brig, &checked.project, &checked.plan)? {
                    return Err("The Brig environment is not running.".into());
                }
                Ok(())
            },
        )?
        .ok_or("This preparation attempt was superseded by a newer one.")?;
        if let Some(message) = result.message {
            return Err(message);
        }
        Ok(result.project)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparation_completion_uses_its_attempt_and_failed_attempts_can_be_retried() {
        let root = std::env::temp_dir().join(format!(
            "tugboat-environment-attempt-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("projects.json");
        let mut registry = Registry::load(&file).unwrap();
        let mut project = registry.register(&root, "codex").unwrap();
        let set_attempt =
            |registry: &mut Registry, project: &mut Project, token: &str, terminal_identifier| {
                project.stage = "environment-starting".into();
                project.environment_verified = Some(false);
                project.environment_attempt = Some(projects::EnvironmentAttempt {
                    token: token.into(),
                    terminal_identifier,
                    completed: false,
                    error: None,
                });
                registry.update(project.clone()).unwrap();
                storage::write(&file, registry).unwrap();
            };
        set_attempt(&mut registry, &mut project, "first", 10);
        let success = complete_attempt(&file, &project.id, "first", 10, Ok(()), || Ok(()))
            .unwrap()
            .unwrap();
        assert_eq!(success.project.stage, "environment-ready");
        assert_eq!(success.project.environment_verified, Some(true));
        assert!(
            complete_attempt(&file, &project.id, "first", 10, Ok(()), || panic!(
                "Completed attempts must not run verification twice"
            ))
            .unwrap()
            .unwrap()
            .message
            .is_none()
        );
        set_attempt(&mut registry, &mut project, "newer", 20);
        assert!(
            complete_attempt(&file, &project.id, "first", 10, Ok(()), || panic!(
                "Old attempts must not verify the new environment"
            ))
            .unwrap()
            .is_none()
        );
        assert_eq!(
            Registry::load(&file)
                .unwrap()
                .project(&project.id)
                .unwrap()
                .stage,
            "environment-starting"
        );
        let failed = complete_attempt(
            &file,
            &project.id,
            "newer",
            20,
            Err("Codex --version failed".into()),
            || panic!("A failed process cannot verify an environment"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(failed.project.stage, "ready");
        assert_eq!(failed.project.environment_verified, Some(false));
        assert!(failed.message.unwrap().contains("version failed"));
        set_attempt(&mut registry, &mut project, "spawn-failure", 30);
        let spawn_failure = complete_attempt(
            &file,
            &project.id,
            "spawn-failure",
            30,
            Err("Terminal could not open".into()),
            || Ok(()),
        )
        .unwrap()
        .unwrap();
        assert_eq!(spawn_failure.project.stage, "ready");
        set_attempt(&mut registry, &mut project, "retry", 40);
        let retry = complete_attempt(&file, &project.id, "retry", 40, Ok(()), || Ok(()))
            .unwrap()
            .unwrap();
        assert_eq!(retry.project.stage, "environment-ready");
        set_attempt(&mut registry, &mut project, "wrong-runtime", 50);
        let wrong = complete_attempt(&file, &project.id, "wrong-runtime", 50, Ok(()), || {
            Err("Guest home mismatch".into())
        })
        .unwrap()
        .unwrap();
        assert_eq!(wrong.project.stage, "ready");
        assert_eq!(wrong.project.environment_verified, Some(false));
        set_attempt(&mut registry, &mut project, "late", 60);
        project.stage = "removing".into();
        registry.update(project.clone()).unwrap();
        storage::write(&file, &registry).unwrap();
        assert!(
            complete_attempt(&file, &project.id, "late", 60, Ok(()), || panic!(
                "Removal must not be reset by a late terminal result"
            ))
            .unwrap()
            .is_none()
        );
        assert_eq!(
            Registry::load(&file)
                .unwrap()
                .project(&project.id)
                .unwrap()
                .stage,
            "removing"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn current_inventory_distinguishes_running_stopped_missing_and_unreadable_environments() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("tugboat-inventory-{}", std::process::id()));
        std::fs::create_dir_all(root.join("project")).unwrap();
        let project = Registry::load(&root.join("projects.json"))
            .unwrap()
            .register(&root.join("project"), "codex")
            .unwrap();
        let executable = root.join("brig-fixture");
        std::fs::write(&executable, "#!/bin/sh\ncase \"$2\" in\nls) [ \"$TUGBOAT_INVENTORY_FAIL\" != 1 ] || exit 1; printf '%s' \"$TUGBOAT_LIST\";;\ninfo) printf '%s' \"$TUGBOAT_INFO\";;\n*) exit 1;;\nesac\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let inventory = |state| {
            serde_json::json!({"apiVersion":"brig.sh/v1alpha1","kind":"SandboxList","data":{"sandboxes":[{"ref":project.reference(),"state":state}]}}).to_string()
        };
        let info = serde_json::json!({"apiVersion":"brig.sh/v1alpha1","kind":"Info","data":{"profile":project.profile,"session":project.label,"project":{"host":project.path},"workspace":"/fixture/home"}});
        let plan = serde_json::json!({"data":{"home":{"host":"/fixture/home"}}});
        let mut brig = Brig {
            executable,
            environment: crate::command::environment(),
        };
        brig.environment
            .insert("TUGBOAT_LIST".into(), inventory("running"));
        brig.environment
            .insert("TUGBOAT_INFO".into(), info.to_string());
        assert!(is_running(&brig, &project, &plan).unwrap());
        brig.environment
            .insert("TUGBOAT_LIST".into(), inventory("stopped"));
        assert!(!is_running(&brig, &project, &plan).unwrap());
        brig.environment.insert("TUGBOAT_LIST".into(), serde_json::json!({"apiVersion":"brig.sh/v1alpha1","kind":"SandboxList","data":{"sandboxes":[]}}).to_string());
        assert!(!is_running(&brig, &project, &plan).unwrap());
        brig.environment
            .insert("TUGBOAT_INVENTORY_FAIL".into(), "1".into());
        assert!(is_running(&brig, &project, &plan)
            .unwrap_err()
            .contains("Could not verify"));
        brig.environment.remove("TUGBOAT_INVENTORY_FAIL");
        brig.environment
            .insert("TUGBOAT_LIST".into(), inventory("running"));
        let mut wrong = info;
        wrong["data"]["project"]["host"] = "/another/project".into();
        brig.environment
            .insert("TUGBOAT_INFO".into(), wrong.to_string());
        assert!(is_running(&brig, &project, &plan).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn plan_snapshot_ignores_inspection_prose_but_covers_execution_fields() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/brig/v0.5.2/ready.json"))
                .unwrap();
        let mut plan: Value = serde_json::from_str(fixture["stdout"].as_str().unwrap()).unwrap();
        let first = plan_fingerprint(&plan).unwrap();
        plan["data"]["network"] = "runtime inspection became available".into();
        assert_eq!(first, plan_fingerprint(&plan).unwrap());
        for (field, value) in [
            ("project", serde_json::json!({"host":"/other"})),
            ("limits", serde_json::json!({"cpus":1,"mem":1024})),
            ("policies", serde_json::json!(["other"])),
        ] {
            let mut changed = plan.clone();
            changed["data"][field] = value;
            assert_ne!(first, plan_fingerprint(&changed).unwrap());
        }
    }
}
