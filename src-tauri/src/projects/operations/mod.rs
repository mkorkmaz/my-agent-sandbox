use crate::{
    desktop::settings,
    environment,
    platform::brig::{self, Brig},
    projects::{self, EnvironmentId, Project, Projects, Registry},
    storage,
    terminal::{self, Layout},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::process::Command;
use tauri::{AppHandle, Manager, State, WebviewWindow};

#[cfg(test)]
mod environment_tests;
mod removal_target;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    GitStatus,
    GitDiff,
    GitStagedDiff,
    Tests,
    Doctor,
    Info,
    PolicyCheck,
    Logs,
}
impl Operation {
    fn purpose(self) -> &'static str {
        match self {
            Self::GitStatus => "git-status",
            Self::GitDiff => "git-diff",
            Self::GitStagedDiff => "git-staged-diff",
            Self::Tests => "tests",
            Self::Doctor => "doctor",
            Self::Info => "info",
            Self::PolicyCheck => "policy-check",
            Self::Logs => "logs",
        }
    }
    fn needs_guest(self) -> bool {
        matches!(
            self,
            Self::GitStatus | Self::GitDiff | Self::GitStagedDiff | Self::Tests
        )
    }
}

fn test_command(path: &Path) -> Option<&'static str> {
    for (file, command) in [
        ("package.json", "npm test"),
        ("composer.json", "composer test"),
    ] {
        let bytes = match std::fs::read(path.join(file)) {
            Ok(bytes) if bytes.len() < 1_048_576 => bytes,
            _ => continue,
        };
        if serde_json::from_slice::<Value>(&bytes)
            .ok()
            .is_some_and(|value| {
                value["scripts"]["test"].is_string()
                    || (file == "composer.json" && value["scripts"]["test"].is_array())
            })
        {
            return Some(command);
        }
    }
    if path.join("Cargo.toml").is_file() {
        return Some("cargo test");
    }
    if path.join("pytest.ini").is_file()
        || std::fs::read_to_string(path.join("pyproject.toml"))
            .ok()
            .is_some_and(|text| text.contains("[tool.pytest"))
    {
        return Some("python3 -m pytest");
    }
    None
}

fn arguments(project: &Project, operation: Operation) -> Result<Vec<String>, String> {
    let reference = project.reference();
    let script = match operation {
        Operation::GitStatus => Some("exec git --no-pager -C \"$BRIG_PROJECT\" status --short"),
        Operation::GitDiff => {
            Some("git --no-pager -C \"$BRIG_PROJECT\" diff --no-ext-diff --no-textconv --exit-code; result=$?; if test \"$result\" -eq 0; then printf 'No working tree changes.\\n'; elif test \"$result\" -eq 1; then exit 0; else exit \"$result\"; fi")
        }
        Operation::GitStagedDiff => Some("git --no-pager -C \"$BRIG_PROJECT\" diff --cached --no-ext-diff --no-textconv --exit-code; result=$?; if test \"$result\" -eq 0; then printf 'No staged changes.\\n'; elif test \"$result\" -eq 1; then exit 0; else exit \"$result\"; fi"),
        Operation::Tests => Some(
            match test_command(&project.path).ok_or(
                "No test command is configured. Run the project tests in the guest terminal.",
            )? {
                "npm test" => "cd \"$BRIG_PROJECT\" && exec npm test",
                "composer test" => "cd \"$BRIG_PROJECT\" && exec composer test",
                "cargo test" => "cd \"$BRIG_PROJECT\" && exec cargo test",
                _ => "cd \"$BRIG_PROJECT\" && exec python3 -m pytest",
            },
        ),
        _ => None,
    };
    if let Some(script) = script {
        return Ok(vec![
            "sh".into(),
            reference,
            "sh".into(),
            "-c".into(),
            script.into(),
        ]);
    }
    Ok(match operation {
        Operation::Doctor => vec!["doctor".into(), project.profile.clone()],
        Operation::Info => vec!["info".into(), reference],
        Operation::PolicyCheck => vec![
            "policy".into(),
            "check".into(),
            project.profile.clone(),
            "-n".into(),
            project.label.clone(),
        ],
        Operation::Logs => vec!["logs".into(), reference, "--tail".into(), "100".into()],
        _ => unreachable!("guest operations returned above"),
    })
}

#[tauri::command]
pub async fn start_project_operation(
    window: WebviewWindow,
    identifier: String,
    environment_id: Option<EnvironmentId>,
    fingerprint: Option<String>,
    operation: Operation,
    layout: Layout,
) -> Result<u64, String> {
    let app = window.app_handle().clone();
    let selected = Registry::load(&projects::file(&app)?)?
        .resolve_environment(&identifier, environment_id.as_ref())?;
    let account_operation = (selected.agent_kind == crate::agents::AgentKind::ClaudeCode)
        .then(|| {
            app.state::<crate::agent_sessions::Accounts>()
                .operation(&selected)
        })
        .transpose()?;
    let _claude_guard = match account_operation {
        Some(operation) => Some(operation.lock_owned().await),
        None => None,
    };
    let shared = app
        .state::<crate::providers::shared_chatgpt::SharedChatgpt>()
        .0
        .clone();
    let _codex_guard = match selected.agent_kind {
        crate::agents::AgentKind::Codex => Some(shared.lock_owned().await),
        crate::agents::AgentKind::ClaudeCode => None,
    };
    let (command, project) = tauri::async_runtime::spawn_blocking(move || {
        let registry = Registry::load(&projects::file(&app)?)?;
        crate::storage::data_location::validate_project_records(&app, &registry)?;
        let project = registry.resolve_environment(&identifier, environment_id.as_ref())?;
        project.require_active()?;
        if project.stage == crate::projects::ProjectStage::Stopping {
            return Err(
                "Wait for this environment to stop before starting another operation.".into(),
            );
        }
        let settings = project.settings(settings::load(&settings::file(&app)?)?)?;
        let brig = environment::brig_for_project(&app, &project, &settings)?;
        if operation.needs_guest() {
            let fingerprint = fingerprint
                .as_deref()
                .ok_or("Review the current execution plan first.")?;
            let checked = environment::checked_environment(
                &app,
                &identifier,
                Some(&project.environment_id),
                Some(fingerprint),
            )?;
            if checked.project.stage != crate::projects::ProjectStage::EnvironmentReady {
                return Err("Prepare the Brig environment first.".into());
            }
        }
        let mut command = Command::new(&brig.executable);
        command
            .args(arguments(&project, operation)?)
            .current_dir(&project.path)
            .env_clear()
            .envs(brig.environment)
            .env("TERM", "xterm-256color");
        Ok::<_, String>((command, project))
    })
    .await
    .map_err(|error| error.to_string())??;
    let target = terminal::AgentTarget::for_project(&project);
    terminal::start_reserved(
        window,
        project.id,
        operation.purpose(),
        command,
        layout,
        None,
        terminal::Reservation {
            identifier: terminal::reserve_identifier(),
            on_exit: None,
            target: Some(target),
        },
    )
    .await
}

fn verify_target(brig: &Brig, project: &Project) -> Result<Value, String> {
    let info = brig.json(
        &["--json".into(), "info".into(), project.reference()],
        "Info",
        None,
    )?;
    if info["data"]["profile"] != project.profile
        || info["data"]["session"] != project.label
        || info["data"]["project"]["host"].as_str() != project.path.to_str()
    {
        return Err(
            "The Brig session does not match the registered project. The operation was stopped."
                .into(),
        );
    }
    Ok(info)
}

#[tauri::command]
pub async fn stop_project(
    app: AppHandle,
    projects: State<'_, Projects>,
    identifier: String,
    environment_id: Option<EnvironmentId>,
) -> Result<Project, String> {
    let lock = projects.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let selected = Registry::load(&projects::file(&app)?)?
            .resolve_environment(&identifier, environment_id.as_ref())?;
        let _operation = environment::EnvironmentOperation::begin(&selected.environment_id)?;
        let operation = (selected.agent_kind == crate::agents::AgentKind::ClaudeCode)
            .then(|| {
                app.state::<crate::agent_sessions::Accounts>()
                    .operation(&selected)
            })
            .transpose()?;
        let claude_guard = operation
            .as_ref()
            .map(|operation| operation.blocking_lock());
        let shared = app
            .state::<crate::providers::shared_chatgpt::SharedChatgpt>()
            .0
            .clone();
        let codex_guard = (selected.agent_kind == crate::agents::AgentKind::Codex)
            .then(|| shared.blocking_lock());
        let _guard = lock.lock().map_err(|error| error.to_string())?;
        let file = projects::file(&app)?;
        let _file_guard = storage::lock(&file)?;
        let mut registry = Registry::load(&file)?;
        crate::storage::data_location::validate_project_records(&app, &registry)?;
        let mut project = registry.resolve_environment(&identifier, environment_id.as_ref())?;
        project.require_active()?;
        let settings = project.settings(settings::load(&settings::file(&app)?)?)?;
        let brig = environment::brig_for_project(&app, &project, &settings)?;
        verify_target(&brig, &project)?;
        project.stop_environment()?;
        registry.update(project.clone())?;
        storage::write(&file, &registry)?;
        drop(_file_guard);
        drop(codex_guard);
        drop(claude_guard);
        drop(_guard);
        terminal::stop_environment_processes(&project)?;
        crate::agent_sessions::invalidate(&app, &project)?;
        let _guard = lock.lock().map_err(|error| error.to_string())?;
        let _file_guard = storage::lock(&file)?;
        let mut registry = Registry::load(&file)?;
        let mut project = registry.environment(&identifier, &project.environment_id)?;
        if project.stage != crate::projects::ProjectStage::Stopping {
            return Err("The environment changed while stopping. Refresh and retry.".into());
        }
        verify_target(&brig, &project)?;
        brig.checked(&["stop".into(), project.reference()], None)?;
        project.finish_stop()?;
        registry.update(project.clone())?;
        storage::write(&file, &registry)?;
        Ok(project)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovalPreview {
    pub reference: String,
    pub workspace: String,
    pub description: String,
    pub fingerprint: String,
    pub external: bool,
    pub has_environment: bool,
    pub environment_id: Option<EnvironmentId>,
    pub profile: Option<String>,
    pub label: Option<String>,
    pub policy: Option<projects::Policy>,
    pub owns_binding: bool,
    pub environments: Vec<RemovalPreview>,
    running: bool,
    logout: Option<bool>,
}

pub(crate) fn preview(brig: &Brig, project: &Project) -> Result<RemovalPreview, String> {
    let list = brig.json(&["--json".into(), "ls".into()], "SandboxList", None)?;
    let inventory = list["data"]["sandboxes"]
        .as_array()
        .ok_or("Could not read the Brig session list.")?;
    let has_environment = inventory
        .iter()
        .any(|sandbox| sandbox["ref"] == project.reference());
    let running = inventory
        .iter()
        .any(|sandbox| sandbox["ref"] == project.reference() && sandbox["state"] != "stopped");
    let info = brig.json(
        &["--json".into(), "info".into(), project.reference()],
        "Info",
        None,
    )?;
    if info["data"]["profile"] != project.profile || info["data"]["session"] != project.label {
        return Err("The Brig session identity changed. Refresh the preview.".into());
    }
    if has_environment && info["data"]["project"]["host"].as_str() != project.path.to_str() {
        return Err("The Brig session belongs to another project.".into());
    }
    let mut description = if has_environment {
        brig.checked(
            &["rm".into(), project.reference(), "--dry-run".into()],
            None,
        )?
        .stdout
    } else {
        format!("No Brig environment exists for {}. Remove only the app project record and its owned policy binding. Existing guest home and project files will be preserved.", project.reference())
    };
    let workspace = info["data"]["workspace"]
        .as_str()
        .ok_or("Could not read the Brig guest home.")?
        .to_owned();
    if let Some(removal) = &project.removal {
        if removal.workspace != workspace || (removal.environment_removed && has_environment) {
            return Err("The removal target was replaced or its guest home changed. The new environment was preserved.".into());
        }
        let completed: Vec<_> = [
            (removal.logged_out, "guest signed out"),
            (removal.stopped, "environment stopped"),
            (removal.environment_removed, "environment removed"),
            (removal.binding_detached, "app binding cleaned"),
            (removal.policy_handled, "policy reviewed"),
        ]
        .into_iter()
        .filter_map(|(done, name)| done.then_some(name))
        .collect();
        let completed = if completed.is_empty() {
            "no completed steps recorded".into()
        } else {
            completed.join(", ")
        };
        description =
            format!("Continue incomplete removal. Completed: {completed}.\n\n{description}");
    } else if project.removal_pending() {
        description = format!("Continue an earlier incomplete removal. Completed steps will be reconciled with the current Brig state.\n\n{description}");
    }
    description.push_str(&format!(
        "\n\nProfile: {}\nLabel: {}\nGuest home: {}\nSession owned by app: {}\nPolicy binding owned by app: {}\nPolicy: {}",
        project.profile, project.label, workspace, project.owns_session, project.owns_binding,
        project.policy.as_ref().map(|policy| format!("{} (owned by app: {})", policy.name, policy.owned)).unwrap_or_else(|| "none".into()),
    ));
    let fingerprint = brig::fingerprint(
        &serde_json::json!({"reference":project.reference(),"workspace":workspace,"project":project.path,"dryRun":description,"policy":project.policy,"hasEnvironment":has_environment,"removal":project.removal}),
    )?;
    Ok(RemovalPreview {
        reference: project.reference(),
        workspace,
        description,
        fingerprint,
        external: !project.owns_session,
        has_environment,
        environment_id: Some(project.environment_id.clone()),
        profile: Some(project.profile.clone()),
        label: Some(project.label.clone()),
        policy: project.policy.clone(),
        owns_binding: project.owns_binding,
        environments: Vec::new(),
        running,
        logout: project.removal.as_ref().map(|removal| removal.logout),
    })
}

#[tauri::command]
pub async fn preview_removal(
    app: AppHandle,
    identifier: String,
    environment_id: Option<EnvironmentId>,
) -> Result<RemovalPreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let registry = Registry::load(&projects::file(&app)?)?;
        crate::storage::data_location::validate_project_records(&app, &registry)?;
        let targets = removal_target::select(&registry, &identifier, environment_id.as_ref())?;
        removal_target::preview(&app, &targets)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn remove_project(
    app: AppHandle,
    projects: State<'_, Projects>,
    identifier: String,
    environment_id: Option<EnvironmentId>,
    fingerprint: String,
    confirmation: String,
    logout: bool,
) -> Result<String, String> {
    let preflight_app = app.clone();
    let preflight_id = identifier.clone();
    let selected_environment = environment_id.clone();
    let expected = fingerprint.clone();
    let entered = confirmation.clone();
    let targets = tauri::async_runtime::spawn_blocking(move || {
        let registry = Registry::load(&projects::file(&preflight_app)?)?;
        crate::storage::data_location::validate_project_records(&preflight_app, &registry)?;
        let targets =
            removal_target::select(&registry, &preflight_id, selected_environment.as_ref())?;
        let snapshots = removal_target::snapshots(&preflight_app, &targets)?;
        let current = removal_target::combine(&snapshots)?;
        if current.fingerprint != expected || current.reference != entered {
            return Err(
                "The preview or session name does not match. The operation was not started."
                    .to_owned(),
            );
        }
        Ok(snapshots)
    })
    .await
    .map_err(|error| error.to_string())??;
    let mut messages = Vec::new();
    for (target, previous) in targets {
        let _operation = environment::EnvironmentOperation::begin(&target.environment_id)?;
        let begin_app = app.clone();
        let begin_target = target.clone();
        let lock = projects.0.clone();
        tauri::async_runtime::spawn_blocking(move || {
            removal_target::begin(
                &begin_app,
                lock,
                &begin_target,
                &previous.fingerprint,
                previous.logout.unwrap_or(logout),
            )
        })
        .await
        .map_err(|error| error.to_string())??;
        terminal::close_environment(app.clone(), &target).await?;
        crate::agent_sessions::invalidate(&app, &target)?;
        let remove_app = app.clone();
        let lock = projects.0.clone();
        let cleanup = tauri::async_runtime::spawn_blocking(move || {
            removal_target::finish(&remove_app, lock, &target)
        })
        .await
        .map_err(|error| error.to_string())??;
        messages.push(cleanup);
    }
    Ok(messages.join("\n"))
}

fn persist_removal(file: &Path, project: &Project) -> Result<(), String> {
    let mut registry = Registry::load(file)?;
    registry.update(project.clone())?;
    storage::write(file, &registry)
}

#[cfg(test)]
fn removal_project(file: &Path, identifier: &str) -> Result<Project, String> {
    let registry = Registry::load(file)?;
    if registry.environments(identifier)?.iter().count() != 1 {
        return Err("Removal is not yet available for projects with multiple agent environments. All environments were preserved.".into());
    }
    registry.project(identifier)
}

fn guest_logout(brig: &Brig, project: &Project) -> Result<(), String> {
    brig.checked(
        &project
            .agent_kind
            .command(crate::agents::AgentCommand::Logout)?
            .in_guest(project.reference()),
        None,
    )
    .map(|_| ())
}

#[cfg(test)]
pub(crate) fn continue_removal(
    brig: &Brig,
    file: &Path,
    identifier: &str,
    fingerprint: &str,
    confirmation: &str,
    logout: bool,
) -> Result<String, String> {
    continue_removal_with(
        brig,
        file,
        identifier,
        &removal_project(file, identifier)?.environment_id,
        fingerprint,
        confirmation,
        logout,
        guest_logout,
    )
}

fn continue_removal_with(
    brig: &Brig,
    file: &Path,
    identifier: &str,
    environment_id: &EnvironmentId,
    fingerprint: &str,
    confirmation: &str,
    logout: bool,
    mut sign_out: impl FnMut(&Brig, &Project) -> Result<(), String>,
) -> Result<String, String> {
    let _file_guard = storage::lock(file)?;
    let mut project = Registry::load(file)?.environment(identifier, environment_id)?;
    let current = preview(brig, &project)?;
    if current.fingerprint != fingerprint || current.reference != confirmation {
        return Err("The removal target or progress changed. Close and refresh the removal preview before continuing.".into());
    }
    if current.logout.is_some_and(|previous| previous != logout) {
        return Err("Keep the sign-out choice recorded for this removal attempt.".into());
    }
    if logout && !current.has_environment && project.removal.is_none() {
        return Err("No guest environment exists to sign out of.".into());
    }
    project.stage = crate::projects::ProjectStage::Removing;
    project.removal.get_or_insert(projects::Removal {
        reference: project.reference(),
        workspace: current.workspace,
        logout,
        had_environment: current.has_environment,
        logged_out: false,
        stopped: false,
        environment_removed: false,
        binding_detached: false,
        policy_handled: false,
        preserved_policy: false,
    });
    persist_removal(file, &project)?;
    let progress = project.removal.as_ref().unwrap();
    if progress.next_step() == projects::RemovalStep::SignOut {
        if !current.has_environment {
            return Err("The guest disappeared before sign-out could be verified. Guest credentials may remain; review the guest home before continuing.".into());
        }
        sign_out(brig, &project)?;
        project.removal.as_mut().unwrap().complete(projects::RemovalStep::SignOut)?;
        persist_removal(file, &project)?;
    }
    if project.removal.as_ref().unwrap().next_step() == projects::RemovalStep::Environment {
        if current.has_environment {
            if project.removal.as_ref().unwrap().needs_stop(current.running) {
                verify_target(brig, &project)?;
                brig.checked(&["stop".into(), project.reference()], None)?;
                project.removal.as_mut().unwrap().stopped = true;
                persist_removal(file, &project)?;
            }
            verify_target(brig, &project)?;
            brig.checked(&["rm".into(), project.reference()], None)?;
        }
        project.removal.as_mut().unwrap().complete(projects::RemovalStep::Environment)?;
        persist_removal(file, &project)?;
    }
    if project.removal.as_ref().unwrap().next_step() == projects::RemovalStep::Binding {
        if let Some(policy) = &project.policy {
            if project.owns_binding {
                let bindings = brig.checked(
                    &[
                        "policy".into(),
                        "check".into(),
                        project.profile.clone(),
                        "-n".into(),
                        project.label.clone(),
                    ],
                    None,
                )?;
                if bindings
                    .stdout
                    .lines()
                    .any(|line| line.trim() == policy.name)
                {
                    brig.checked(
                        &[
                            "policy".into(),
                            "detach".into(),
                            policy.name.clone(),
                            project.profile.clone(),
                            "-n".into(),
                            project.label.clone(),
                        ],
                        None,
                    )?;
                }
            }
        }
        project.removal.as_mut().unwrap().complete(projects::RemovalStep::Binding)?;
        persist_removal(file, &project)?;
    }
    if project.removal.as_ref().unwrap().next_step() == projects::RemovalStep::Policy {
        if let Some(policy) = &project.policy {
            if policy.owned {
                if let Some(document) = brig.policy(&policy.name)? {
                    let unchanged = Some(brig::fingerprint(&document)?) == policy.fingerprint;
                    let list = brig.checked(&["policy".into(), "ls".into()], None)?;
                    if unchanged
                        && crate::projects::project_policy::sole_binding(
                            &list.stdout,
                            &policy.name,
                            &project,
                        )
                    {
                        brig.checked(&["policy".into(), "rm".into(), policy.name.clone()], None)?;
                    } else {
                        project.removal.as_mut().unwrap().preserved_policy = true;
                    }
                }
            }
        }
        project.removal.as_mut().unwrap().complete(projects::RemovalStep::Policy)?;
        persist_removal(file, &project)?;
    }
    let progress = project.removal.as_ref().unwrap();
    let mut message = if progress.had_environment {
        "Brig session removed. Project files were preserved."
    } else {
        "Project setup record removed. Existing guest home and project files were preserved."
    }
    .to_owned();
    if progress.preserved_policy {
        message.push_str(" The externally modified or shared policy was preserved.");
    }
    let mut registry = Registry::load(file)?;
    registry.remove_environment(identifier, environment_id)?;
    storage::write(file, &registry)?;
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn legacy_removal_preserves_multiple_environments_before_any_brig_operation() {
        let root =
            std::env::temp_dir().join(format!("tugboat-multiple-removal-{}", std::process::id()));
        fs::create_dir_all(root.join("project")).unwrap();
        let file = root.join("projects.json");
        let mut registry = Registry::default();
        let codex = registry.register(&root.join("project"), "codex").unwrap();
        registry
            .ensure_environment(
                &codex.id,
                crate::agents::AgentKind::ClaudeCode,
                &settings::Settings::default(),
            )
            .unwrap();
        storage::write(&file, &registry).unwrap();
        let original = fs::read(&file).unwrap();
        let brig = Brig {
            executable: root.join("must-not-run"),
            environment: crate::platform::command::environment(),
        };
        let error = continue_removal(&brig, &file, &codex.id, "unused", &codex.reference(), false)
            .unwrap_err();
        assert!(error.contains("multiple agent environments"), "{error}");
        assert_eq!(fs::read(&file).unwrap(), original);
        assert_eq!(
            Registry::load(&file)
                .unwrap()
                .environments(&codex.id)
                .unwrap()
                .iter()
                .count(),
            2
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn working_tree_and_staged_diffs_cover_each_git_state_without_external_drivers() {
        let root = std::env::temp_dir().join(format!("tugboat-diffs-{}", std::process::id()));
        fs::create_dir_all(root.join("project folder")).unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        let project = Registry::load(&root.join("registry.json"))
            .unwrap()
            .register(&root.join("project folder"), "codex")
            .unwrap();
        let mut environment = crate::platform::command::environment();
        environment.insert(
            "HOME".into(),
            root.join("home").to_string_lossy().into_owned(),
        );
        environment.insert(
            "XDG_CONFIG_HOME".into(),
            root.join("home/config").to_string_lossy().into_owned(),
        );
        environment.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
        environment.insert(
            "BRIG_PROJECT".into(),
            project.path.to_string_lossy().into_owned(),
        );
        let git = crate::platform::host::discover("git", None).unwrap();
        let run_git = |args: &[&str]| {
            let output = crate::platform::command::run(
                &git,
                &args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>(),
                Some(&project.path),
                &environment,
                std::time::Duration::from_secs(3),
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(output.exit_code, Some(0), "{}", output.stderr);
        };
        run_git(&["init", "--template=", "--initial-branch=main"]);
        let driver = root.join("diff-driver");
        fs::write(
            &driver,
            "#!/bin/sh\nroot=${0%/*}\n: > \"$root/driver-used\"\nprintf 'External driver ran'\n",
        )
        .unwrap();
        fs::set_permissions(&driver, fs::Permissions::from_mode(0o700)).unwrap();
        run_git(&["config", "diff.fixture.command", driver.to_str().unwrap()]);
        run_git(&["config", "diff.fixture.textconv", driver.to_str().unwrap()]);
        fs::write(project.path.join(".gitattributes"), "*.txt diff=fixture\n").unwrap();
        fs::write(project.path.join("sample.txt"), "base\n").unwrap();
        run_git(&["add", "--", ".gitattributes", "sample.txt"]);
        run_git(&[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit",
            "-m",
            "Initial",
        ]);
        let diff = |operation| {
            let arguments = arguments(&project, operation).unwrap();
            let output = crate::platform::command::run(
                Path::new("/bin/sh"),
                &["-c".into(), arguments.last().unwrap().clone()],
                Some(&project.path),
                &environment,
                std::time::Duration::from_secs(3),
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(output.exit_code, Some(0), "{}", output.stderr);
            output.stdout
        };
        fs::write(project.path.join("sample.txt"), "staged-one\n").unwrap();
        run_git(&["add", "--", "sample.txt"]);
        assert_eq!(diff(Operation::GitDiff), "No working tree changes.\n");
        assert!(diff(Operation::GitStagedDiff).contains("+staged-one"));
        fs::write(project.path.join("sample.txt"), "work-only\n").unwrap();
        assert!(diff(Operation::GitDiff).contains("+work-only"));
        assert!(diff(Operation::GitStagedDiff).contains("+staged-one"));
        run_git(&["restore", "--staged", "--", "sample.txt"]);
        assert!(diff(Operation::GitDiff).contains("+work-only"));
        assert_eq!(diff(Operation::GitStagedDiff), "No staged changes.\n");
        fs::write(project.path.join("sample.txt"), "base\n").unwrap();
        assert_eq!(diff(Operation::GitDiff), "No working tree changes.\n");
        assert_eq!(diff(Operation::GitStagedDiff), "No staged changes.\n");
        assert!(!root.join("driver-used").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interrupted_removal_reconciles_effects_and_resumes_without_repeating_completed_steps() {
        for failure in ["rm-before", "rm-after", "detach", "policy-rm"] {
            let root = std::env::temp_dir()
                .join(format!("tugboat-remove-{}-{failure}", std::process::id()));
            fs::create_dir_all(root.join("project")).unwrap();
            fs::write(root.join("project/kept"), "source").unwrap();
            let file = root.join("registry.json");
            let mut registry = Registry::load(&file).unwrap();
            let mut project = registry.register(&root.join("project"), "codex").unwrap();
            let document = serde_json::json!({"apiVersion":"brig.sh/v1alpha1","kind":"Policy","metadata":{"name":"owned"},"spec":{"egress":{"default":"deny"}}});
            project.policy = Some(projects::Policy {
                name: "owned".into(),
                owned: true,
                fingerprint: Some(brig::fingerprint(&document).unwrap()),
            });
            project.owns_binding = true;
            project.stage = crate::projects::ProjectStage::Ready;
            registry.update(project.clone()).unwrap();
            storage::write(&file, &registry).unwrap();
            fs::write(root.join("policy.json"), document.to_string()).unwrap();
            fs::write(root.join("environment"), "owned").unwrap();
            fs::write(root.join("binding"), "owned").unwrap();
            fs::write(root.join("failure"), failure).unwrap();
            for (name, value) in [
                (
                    "present.json",
                    serde_json::json!({"apiVersion":"brig.sh/v1alpha1","kind":"SandboxList","data":{"sandboxes":[{"ref":project.reference(),"state":"running"}]}}),
                ),
                (
                    "stopped.json",
                    serde_json::json!({"apiVersion":"brig.sh/v1alpha1","kind":"SandboxList","data":{"sandboxes":[{"ref":project.reference(),"state":"stopped"}]}}),
                ),
                (
                    "absent.json",
                    serde_json::json!({"apiVersion":"brig.sh/v1alpha1","kind":"SandboxList","data":{"sandboxes":[]}}),
                ),
                (
                    "info.json",
                    serde_json::json!({"apiVersion":"brig.sh/v1alpha1","kind":"Info","data":{"profile":project.profile,"session":project.label,"project":{"host":project.path},"workspace":root.join("guest-home")}}),
                ),
            ] {
                fs::write(root.join(name), value.to_string()).unwrap();
            }
            let executable = root.join("brig");
            fs::write(&executable, r#"#!/bin/sh
root=${0%/*}
printf '%s\n' "$*" >> "$root/calls"
fail() { test -f "$root/failure" && test "$(cat "$root/failure")" = "$1"; }
case "$1:$2" in
--json:ls) if test -e "$root/environment"; then if test -e "$root/stopped"; then cat "$root/stopped.json"; else cat "$root/present.json"; fi; else cat "$root/absent.json"; fi;;
--json:info) cat "$root/info.json";;
sh:*) : > "$root/logged-out";;
stop:*) : > "$root/stopped";;
rm:*) if test "$3" = --dry-run; then printf 'Remove selected fixture environment\n'; elif fail rm-before; then exit 9; else rm -f "$root/environment"; if fail rm-after; then exit 9; fi; fi;;
policy:check) if test -e "$root/binding"; then printf 'owned\n'; fi;;
policy:detach) if fail detach; then exit 9; else rm -f "$root/binding"; fi;;
policy:show) if test -e "$root/policy.json"; then cat "$root/policy.json"; else printf 'unknown policy' >&2; exit 1; fi;;
policy:ls) if test -e "$root/policy.json"; then printf 'owned\n'; fi;;
policy:rm) if fail policy-rm; then exit 9; else rm -f "$root/policy.json"; fi;;
*) exit 8;;
esac
"#).unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
            let brig = Brig {
                executable,
                environment: crate::platform::command::environment(),
            };
            let initial = preview(&brig, &project).unwrap();
            assert!(continue_removal(
                &brig,
                &file,
                &project.id,
                &initial.fingerprint,
                &project.reference(),
                true
            )
            .is_err());
            let pending = Registry::load(&file).unwrap().project(&project.id).unwrap();
            assert_eq!(pending.stage, "removing");
            assert!(pending.removal.as_ref().unwrap().logged_out);
            assert!(pending.removal.as_ref().unwrap().stopped);
            assert!(pending.require_active().is_err());
            if failure == "policy-rm" {
                fs::write(root.join("environment"), "replacement").unwrap();
                assert!(preview(&brig, &pending).err().unwrap().contains("replaced"));
                assert_eq!(
                    fs::read_to_string(root.join("environment")).unwrap(),
                    "replacement"
                );
                fs::remove_file(root.join("environment")).unwrap();
            }
            fs::remove_file(root.join("failure")).unwrap();
            let recovery = preview(&brig, &pending).unwrap();
            assert!(recovery.description.contains("Continue incomplete removal"));
            assert_eq!(recovery.logout, Some(true));
            assert!(continue_removal(
                &brig,
                &file,
                &project.id,
                &initial.fingerprint,
                &project.reference(),
                true
            )
            .is_err());
            continue_removal(
                &brig,
                &file,
                &project.id,
                &recovery.fingerprint,
                &project.reference(),
                true,
            )
            .unwrap();
            assert!(Registry::load(&file).unwrap().projects.is_empty());
            let calls = fs::read_to_string(root.join("calls")).unwrap();
            assert_eq!(
                calls.lines().filter(|line| line.starts_with("sh ")).count(),
                1,
                "{calls}"
            );
            assert_eq!(
                calls
                    .lines()
                    .filter(|line| line.starts_with("stop "))
                    .count(),
                1,
                "{calls}"
            );
            assert_eq!(
                calls
                    .lines()
                    .filter(|line| line.starts_with("rm ") && !line.ends_with("--dry-run"))
                    .count(),
                if failure == "rm-before" { 2 } else { 1 },
                "{calls}"
            );
            assert!(!root.join("environment").exists());
            assert!(!root.join("binding").exists());
            assert!(!root.join("policy.json").exists());
            assert_eq!(
                fs::read_to_string(root.join("project/kept")).unwrap(),
                "source"
            );
            fs::remove_dir_all(root).unwrap();
        }
    }
}
