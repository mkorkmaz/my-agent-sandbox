use crate::{
    desktop::settings::{self, Settings},
    platform::brig::{self, Brig},
    projects::{self, EnvironmentId, Project, Projects, Registry},
    storage,
    terminal::{self, Layout},
};
use serde::Serialize;
use serde_json::Value;
use std::process::Command;
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};
pub(crate) mod boundaries;
mod gate;
pub(crate) mod managed_profile;
#[cfg(test)]
mod plan_tests;
pub(crate) mod preparation;
pub(crate) use gate::EnvironmentOperation;

pub struct CheckedProject {
    pub project: Project,
    pub settings: Settings,
    pub brig: Brig,
    pub plan: Value,
    pub paths: brig::PlanPaths,
    pub fingerprint: String,
}

pub fn plan_fingerprint(
    plan: &Value,
    project: &Project,
    profile: &Value,
) -> Result<String, String> {
    let mut snapshot = plan.clone();
    // This prose contains transient inspect errors. All executable plan fields remain covered.
    snapshot["data"]
        .as_object_mut()
        .ok_or("Invalid Brig plan.")?
        .remove("network");
    brig::fingerprint(&serde_json::json!({
        "projectId":project.id,"environmentId":project.environment_id,"agentKind":project.agent_kind,"profile":profile,"plan":snapshot
    }))
}

pub fn brig_for_project(
    app: &AppHandle,
    project: &Project,
    settings: &Settings,
) -> Result<Brig, String> {
    Brig::new(settings)?.for_project(
        project,
        &crate::storage::data_location::directory(app)?.join("profiles"),
    )
}

pub fn checked_project(
    app: &AppHandle,
    identifier: &str,
    expected: Option<&str>,
) -> Result<CheckedProject, String> {
    checked_environment(app, identifier, None, expected)
}

pub fn checked_environment(
    app: &AppHandle,
    identifier: &str,
    environment_id: Option<&EnvironmentId>,
    expected: Option<&str>,
) -> Result<CheckedProject, String> {
    let registry = Registry::load(&projects::file(app)?)?;
    crate::storage::data_location::validate_project_records(app, &registry)?;
    let project = registry.resolve_environment(identifier, environment_id)?;
    let settings = project.settings(settings::load(&settings::file(app)?)?)?;
    let brig = brig_for_project(app, &project, &settings)?;
    check_environment(
        brig,
        project,
        settings,
        expected,
        &crate::storage::data_location::protected_directories(app)?,
    )
}

pub(crate) fn check_environment(
    brig: Brig,
    mut project: Project,
    settings: Settings,
    expected: Option<&str>,
    private_records: &[std::path::PathBuf],
) -> Result<CheckedProject, String> {
    project.require_active()?;
    project.check_path()?;
    project.check_guest_path()?;
    if project.policy_detached {
        return Err("Attach the project policy before preparing the environment.".into());
    }
    if !project.stage.permits_plan() {
        return Err("Complete project setup first.".into());
    }
    let profile = brig.agent_profile(&project.profile)?;
    project
        .agent_kind
        .validate_profile(&project.profile, &profile)?;
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
    let paths = brig::validate_plan(&plan, &project, &settings)?;
    crate::storage::data_location::require_private_plan_records(private_records, &plan)?;
    let fingerprint = plan_fingerprint(&plan, &project, &profile)?;
    if project.stage != crate::projects::ProjectStage::EnvironmentStarting {
        project.stage = project
            .environment_stage(is_running(&brig, &project, &paths)?, &fingerprint)
            .into();
    } else if project
        .environment_attempt
        .as_ref()
        .and_then(|attempt| attempt.plan_fingerprint.as_deref())
        != Some(&fingerprint)
    {
        project.stage = crate::projects::ProjectStage::Ready;
    }
    if expected.is_some_and(|expected| expected != fingerprint) {
        return Err("The execution plan changed. Review it and start again.".into());
    }
    Ok(CheckedProject {
        project,
        settings,
        brig,
        plan,
        paths,
        fingerprint,
    })
}

pub(crate) fn is_running(
    brig: &Brig,
    project: &Project,
    paths: &brig::PlanPaths,
) -> Result<bool, String> {
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
    if info["data"]["profile"] != project.profile
        || info["data"]["session"] != project.label
        || info["data"]["project"]["host"].as_str() != project.path.to_str()
        || info["data"]["workspace"].as_str() != paths.host_home.to_str()
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
pub async fn preview_project(
    app: AppHandle,
    identifier: String,
    environment_id: Option<EnvironmentId>,
) -> Result<Preview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let checked = checked_environment(&app, &identifier, environment_id.as_ref(), None)?;
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

pub fn run_arguments(checked: &CheckedProject, agent_arguments: &[String]) -> Vec<String> {
    let mut arguments = vec!["run".into()];
    arguments.extend(checked.settings.arguments());
    arguments.extend([
        checked.project.reference(),
        checked.project.path.to_string_lossy().into_owned(),
        "--".into(),
    ]);
    arguments.extend_from_slice(agent_arguments);
    arguments
}

#[tauri::command]
pub async fn prepare_environment(
    window: WebviewWindow,
    projects: State<'_, Projects>,
    identifier: String,
    environment_id: Option<EnvironmentId>,
    fingerprint: String,
    layout: Layout,
) -> Result<u64, String> {
    let app = window.app_handle().clone();
    let preparation_app = app.clone();
    let registry_lock = projects.0.clone();
    let project_id = identifier.clone();
    let environment_id = environment_id
        .unwrap_or_else(|| EnvironmentId::for_agent(&identifier, crate::agents::AgentKind::Codex));
    let _operation = EnvironmentOperation::begin(&environment_id)?;
    let pending_environment = environment_id.clone();
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
    let (command, terminal_target) = tauri::async_runtime::spawn_blocking(move || {
        let _guard = registry_lock.lock().map_err(|error| error.to_string())?;
        if terminal::terminal_sessions(identifier.clone())
            .iter()
            .any(|status| {
                status.purpose == "environment"
                    && status.environment_id.as_ref() == Some(&pending_environment)
                    && status.running
            })
        {
            return Err("Environment preparation is already running for this project.".into());
        }
        let file = projects::file(&preparation_app)?;
        let _file_guard = storage::lock(&file)?;
        let mut checked = checked_environment(
            &preparation_app,
            &identifier,
            Some(&pending_environment),
            Some(&fingerprint),
        )?;
        let home = checked
            .brig
            .environment
            .get("HOME")
            .ok_or("The current home directory is unavailable.")?;
        let sandbox = checked.plan["data"]["sandbox"]
            .as_str()
            .ok_or("The plan does not identify the hull instance.")?;
        crate::platform::hull::check_instance_socket_paths(std::path::Path::new(home), sandbox)?;
        let version = checked
            .project
            .agent_kind
            .command(crate::agents::AgentCommand::Version)?
            .in_profile();
        let command = command(&checked, run_arguments(&checked, &version));
        checked.project.begin_environment(projects::EnvironmentAttempt::pending(
            pending_token, terminal_identifier, checked.fingerprint.clone(),
        ))?;
        let terminal_target = terminal::AgentTarget::for_project(&checked.project);
        let mut registry = Registry::load(&file)?;
        registry.update(checked.project)?;
        storage::write(&file, &registry)?;
        Ok::<_, String>((command, terminal_target))
    })
    .await
    .map_err(|error| error.to_string())??;
    let completed_app = app.clone();
    let completed_project = project_id.clone();
    let completed_token = token.clone();
    let completed_environment = environment_id.clone();
    let on_exit = Box::new(move |code| {
        let result = if code == Some(0) {
            Ok(())
        } else {
            Err("Could not verify the guest agent version. Check the terminal output and prepare the environment again.".into())
        };
        complete_environment(
            &completed_app,
            &completed_project,
            &completed_environment,
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
            target: Some(terminal_target),
        },
    )
    .await;
    if let Err(error) = &result {
        complete_environment(
            &app,
            &project_id,
            &environment_id,
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
    environment_id: &EnvironmentId,
    token: &str,
    terminal_identifier: u64,
    process_result: Result<(), String>,
    verify: impl FnOnce() -> Result<(), String>,
) -> Result<Option<EnvironmentResult>, String> {
    let _file_guard = storage::lock(file)?;
    let mut registry = Registry::load(file)?;
    let mut project = registry.environment(identifier, environment_id)?;
    if project.removal_pending() {
        return Ok(None);
    }
    let Some(attempt) = &project.environment_attempt else {
        return Ok(None);
    };
    match attempt.completion(token, terminal_identifier) {
        projects::AttemptCompletion::Superseded => return Ok(None),
        projects::AttemptCompletion::Recorded(message) => {
            let message = message.map(str::to_owned);
            return Ok(Some(EnvironmentResult { terminal_identifier, message, project }));
        }
        projects::AttemptCompletion::Pending => {}
    }
    if project.stage != crate::projects::ProjectStage::EnvironmentStarting {
        return Err("The environment is no longer preparing. Refresh the project.".into());
    }
    let message = process_result.and_then(|_| verify()).err();
    project.environment_verified = Some(message.is_none());
    project.stage = crate::projects::ProjectStage::Ready;
    if message.is_none() {
        project.stage = crate::projects::ProjectStage::EnvironmentReady;
    }
    let attempt = project.environment_attempt.as_mut().unwrap();
    attempt.finish(message.clone().map_or(Ok(()), Err))?;
    registry.update(project.clone())?;
    storage::write(file, &registry)?;
    Ok(Some(EnvironmentResult {
        project,
        terminal_identifier,
        message,
    }))
}

fn verify_running_attempt(
    app: &AppHandle,
    identifier: &str,
    environment_id: &EnvironmentId,
) -> Result<(), String> {
    let checked = checked_environment(app, identifier, Some(environment_id), None)?;
    if checked
        .project
        .environment_attempt
        .as_ref()
        .and_then(|attempt| attempt.plan_fingerprint.as_deref())
        != Some(&checked.fingerprint)
    {
        return Err(
            "The execution plan changed during preparation. Prepare the environment again.".into(),
        );
    }
    if !is_running(&checked.brig, &checked.project, &checked.paths)? {
        return Err("The Brig environment did not remain running. Prepare it again.".into());
    }
    checked
        .brig
        .verify_agent_version(checked.project.agent_kind, checked.project.reference())
}

fn complete_environment(
    app: &AppHandle,
    identifier: &str,
    environment_id: &EnvironmentId,
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
            environment_id,
            token,
            terminal_identifier,
            process_result,
            || verify_running_attempt(app, identifier, environment_id),
        )
    })();
    match result {
        Ok(Some(result)) => {
            let _ = app.emit("environment-preparation-finished", result);
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!("Could not complete environment preparation: {error}");
            let _ = app.emit("environment-preparation-error", serde_json::json!({"projectId":identifier,"environmentId":environment_id,"terminalIdentifier":terminal_identifier,"message":error}));
        }
    }
}

#[tauri::command]
pub async fn finish_environment(
    app: AppHandle,
    projects: State<'_, Projects>,
    identifier: String,
    environment_id: Option<EnvironmentId>,
    terminal_identifier: u64,
) -> Result<Project, String> {
    let status = terminal::terminal_snapshot(terminal_identifier)
        .ok_or("The environment terminal record was not found.")?;
    if status.project_id != identifier || status.purpose != "environment" || status.running {
        return Err("Could not verify the guest agent version. Check the terminal output.".into());
    }
    let lock = projects.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = lock.lock().map_err(|error| error.to_string())?;
        let file = projects::file(&app)?;
        let project =
            Registry::load(&file)?.resolve_environment(&identifier, environment_id.as_ref())?;
        if !status.belongs_to(&project) {
            return Err("This terminal belongs to another agent environment.".into());
        }
        let environment_id = project.environment_id;
        let attempt = project
            .environment_attempt
            .ok_or("The preparation attempt was not found. Prepare the environment again.")?;
        let process_result = if status.exit_code == Some(0) {
            Ok(())
        } else {
            Err("Could not verify the guest agent version. Prepare the environment again.".into())
        };
        let result = complete_attempt(
            &file,
            &identifier,
            &environment_id,
            &attempt.token,
            terminal_identifier,
            process_result,
            || verify_running_attempt(&app, &identifier, &environment_id),
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
    fn preparation_results_and_running_plan_fingerprints_belong_to_one_environment() {
        let root =
            std::env::temp_dir().join(format!("tugboat-selected-attempt-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("projects.json");
        let mut registry = Registry::default();
        let mut codex = registry.register(&root, "codex").unwrap();
        let added = registry
            .ensure_environment(
                &codex.id,
                crate::agents::AgentKind::ClaudeCode,
                &Settings::default(),
            )
            .unwrap();
        let mut claude = registry
            .environment(&codex.id, &added.environment_id)
            .unwrap();
        let plan = serde_json::json!({"data":{"project":{"host":root},"network":"inspection"}});
        let definition = serde_json::json!({"binary":"fixture"});
        let codex_fingerprint = plan_fingerprint(&plan, &codex, &definition).unwrap();
        let claude_fingerprint = plan_fingerprint(&plan, &claude, &definition).unwrap();
        assert_ne!(codex_fingerprint, claude_fingerprint);
        codex.stage = crate::projects::ProjectStage::EnvironmentStarting;
        codex.environment_attempt = Some(projects::EnvironmentAttempt {
            plan_fingerprint: Some(codex_fingerprint.clone()),
            token: "codex-attempt".into(),
            terminal_identifier: 10,
            completed: false,
            error: None,
        });
        claude.stage = crate::projects::ProjectStage::EnvironmentStarting;
        claude.environment_attempt = Some(projects::EnvironmentAttempt {
            plan_fingerprint: Some(claude_fingerprint.clone()),
            token: "claude-attempt".into(),
            terminal_identifier: 20,
            completed: false,
            error: None,
        });
        registry.update(codex.clone()).unwrap();
        registry.update(claude.clone()).unwrap();
        storage::write(&file, &registry).unwrap();
        let original = std::fs::read(&file).unwrap();
        assert!(complete_attempt(
            &file,
            &claude.id,
            &claude.environment_id,
            "codex-attempt",
            10,
            Ok(()),
            || panic!("Another environment must not be verified")
        )
        .unwrap()
        .is_none());
        assert_eq!(std::fs::read(&file).unwrap(), original);
        let result = complete_attempt(
            &file,
            &claude.id,
            &claude.environment_id,
            "claude-attempt",
            20,
            Ok(()),
            || Ok(()),
        )
        .unwrap()
        .unwrap();
        assert_eq!(result.project.environment_id, claude.environment_id);
        assert_eq!(result.project.stage, "environment-ready");
        assert_eq!(
            result.project.environment_stage(true, &claude_fingerprint),
            "environment-ready"
        );
        assert_eq!(
            result.project.environment_stage(true, &codex_fingerprint),
            "ready"
        );
        assert_eq!(
            serde_json::to_value(Registry::load(&file).unwrap().project(&codex.id).unwrap())
                .unwrap(),
            serde_json::to_value(codex).unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

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
                project.stage = crate::projects::ProjectStage::EnvironmentStarting;
                project.environment_verified = Some(false);
                project.environment_attempt = Some(projects::EnvironmentAttempt {
                    plan_fingerprint: Some("fixture-plan".into()),
                    token: token.into(),
                    terminal_identifier,
                    completed: false,
                    error: None,
                });
                registry.update(project.clone()).unwrap();
                storage::write(&file, registry).unwrap();
            };
        set_attempt(&mut registry, &mut project, "first", 10);
        let success = complete_attempt(
            &file,
            &project.id,
            &project.environment_id,
            "first",
            10,
            Ok(()),
            || Ok(()),
        )
        .unwrap()
        .unwrap();
        assert_eq!(success.project.stage, "environment-ready");
        assert_eq!(success.project.environment_verified, Some(true));
        assert!(complete_attempt(
            &file,
            &project.id,
            &project.environment_id,
            "first",
            10,
            Ok(()),
            || panic!("Completed attempts must not run verification twice")
        )
        .unwrap()
        .unwrap()
        .message
        .is_none());
        set_attempt(&mut registry, &mut project, "newer", 20);
        assert!(complete_attempt(
            &file,
            &project.id,
            &project.environment_id,
            "first",
            10,
            Ok(()),
            || panic!("Old attempts must not verify the new environment")
        )
        .unwrap()
        .is_none());
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
            &project.environment_id,
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
            &project.environment_id,
            "spawn-failure",
            30,
            Err("Terminal could not open".into()),
            || Ok(()),
        )
        .unwrap()
        .unwrap();
        assert_eq!(spawn_failure.project.stage, "ready");
        set_attempt(&mut registry, &mut project, "retry", 40);
        let retry = complete_attempt(
            &file,
            &project.id,
            &project.environment_id,
            "retry",
            40,
            Ok(()),
            || Ok(()),
        )
        .unwrap()
        .unwrap();
        assert_eq!(retry.project.stage, "environment-ready");
        set_attempt(&mut registry, &mut project, "wrong-runtime", 50);
        let wrong = complete_attempt(
            &file,
            &project.id,
            &project.environment_id,
            "wrong-runtime",
            50,
            Ok(()),
            || Err("Guest home mismatch".into()),
        )
        .unwrap()
        .unwrap();
        assert_eq!(wrong.project.stage, "ready");
        assert_eq!(wrong.project.environment_verified, Some(false));
        set_attempt(&mut registry, &mut project, "late", 60);
        project.stage = crate::projects::ProjectStage::Removing;
        registry.update(project.clone()).unwrap();
        storage::write(&file, &registry).unwrap();
        assert!(complete_attempt(
            &file,
            &project.id,
            &project.environment_id,
            "late",
            60,
            Ok(()),
            || panic!("Removal must not be reset by a late terminal result")
        )
        .unwrap()
        .is_none());
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
        let plan = serde_json::json!({"data":{"home":{"host":"/fixture/home"},"project":{"guest":"/work/project"}}});
        let paths = brig::PlanPaths::from_plan(&plan).unwrap();
        let mut brig = Brig {
            executable,
            environment: crate::platform::command::environment(),
        };
        brig.environment
            .insert("TUGBOAT_LIST".into(), inventory("running"));
        brig.environment
            .insert("TUGBOAT_INFO".into(), info.to_string());
        assert!(is_running(&brig, &project, &paths).unwrap());
        brig.environment
            .insert("TUGBOAT_LIST".into(), inventory("stopped"));
        assert!(!is_running(&brig, &project, &paths).unwrap());
        brig.environment.insert("TUGBOAT_LIST".into(), serde_json::json!({"apiVersion":"brig.sh/v1alpha1","kind":"SandboxList","data":{"sandboxes":[]}}).to_string());
        assert!(!is_running(&brig, &project, &paths).unwrap());
        brig.environment
            .insert("TUGBOAT_INVENTORY_FAIL".into(), "1".into());
        assert!(is_running(&brig, &project, &paths)
            .unwrap_err()
            .contains("Could not verify"));
        brig.environment.remove("TUGBOAT_INVENTORY_FAIL");
        brig.environment
            .insert("TUGBOAT_LIST".into(), inventory("running"));
        let mut wrong = info;
        wrong["data"]["project"]["host"] = "/another/project".into();
        brig.environment
            .insert("TUGBOAT_INFO".into(), wrong.to_string());
        assert!(is_running(&brig, &project, &paths).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn plan_snapshot_ignores_inspection_prose_but_covers_execution_fields() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/brig/v0.5.2/ready.json"
        ))
        .unwrap();
        let mut plan: Value = serde_json::from_str(fixture["stdout"].as_str().unwrap()).unwrap();
        let root =
            std::env::temp_dir().join(format!("tugboat-plan-snapshot-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let project = Registry::default().register(&root, "codex").unwrap();
        let profile = serde_json::json!({"name":"codex","binary":"codex"});
        let first = plan_fingerprint(&plan, &project, &profile).unwrap();
        plan["data"]["network"] = "runtime inspection became available".into();
        assert_eq!(first, plan_fingerprint(&plan, &project, &profile).unwrap());
        for (field, value) in [
            ("project", serde_json::json!({"host":"/other"})),
            ("limits", serde_json::json!({"cpus":1,"mem":1024})),
            ("policies", serde_json::json!(["other"])),
        ] {
            let mut changed = plan.clone();
            changed["data"][field] = value;
            assert_ne!(
                first,
                plan_fingerprint(&changed, &project, &profile).unwrap()
            );
        }
        let changed_profile = serde_json::json!({"name":"codex","binary":"codex","env":[{"name":"CODEX_HOME","value":"/changed"}]});
        assert_ne!(
            first,
            plan_fingerprint(&plan, &project, &changed_profile).unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
