use crate::{
    brig::{self, Brig},
    environment,
    projects::{self, Project, Projects, Registry},
    settings, storage,
    terminal::{self, Layout},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::process::Command;
use tauri::{AppHandle, Manager, State, WebviewWindow};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    GitStatus,
    GitDiff,
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
            Self::Tests => "tests",
            Self::Doctor => "doctor",
            Self::Info => "info",
            Self::PolicyCheck => "policy-check",
            Self::Logs => "logs",
        }
    }
    fn needs_guest(self) -> bool {
        matches!(self, Self::GitStatus | Self::GitDiff | Self::Tests)
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
            Some("exec git --no-pager -C \"$BRIG_PROJECT\" diff --no-ext-diff --no-textconv")
        }
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
    fingerprint: Option<String>,
    operation: Operation,
    layout: Layout,
) -> Result<u64, String> {
    let app = window.app_handle().clone();
    let project_id = identifier.clone();
    let command = tauri::async_runtime::spawn_blocking(move || {
        let project = Registry::load(&projects::file(&app)?)?.project(&identifier)?;
        let settings = project.settings(settings::load(&settings::file(&app)?)?)?;
        let brig = Brig::new(&settings)?;
        if operation.needs_guest() {
            let fingerprint = fingerprint
                .as_deref()
                .ok_or("Review the current execution plan first.")?;
            let checked = environment::checked_project(&app, &identifier, Some(fingerprint))?;
            if checked.project.stage != "environment-ready" {
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
        Ok::<_, String>(command)
    })
    .await
    .map_err(|error| error.to_string())??;
    terminal::start(
        window,
        project_id,
        operation.purpose(),
        command,
        layout,
        None,
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
) -> Result<Project, String> {
    let preflight_app = app.clone();
    let preflight_id = identifier.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let project = Registry::load(&projects::file(&preflight_app)?)?.project(&preflight_id)?;
        let settings = project.settings(settings::load(&settings::file(&preflight_app)?)?)?;
        verify_target(&Brig::new(&settings)?, &project)
    })
    .await
    .map_err(|error| error.to_string())??;
    terminal::close_project(app.clone(), &identifier).await?;
    let lock = projects.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = lock.lock().map_err(|error| error.to_string())?;
        let file = projects::file(&app)?;
        let mut registry = Registry::load(&file)?;
        let mut project = registry.project(&identifier)?;
        let settings = project.settings(settings::load(&settings::file(&app)?)?)?;
        let brig = Brig::new(&settings)?;
        verify_target(&brig, &project)?;
        brig.checked(&["stop".into(), project.reference()], None)?;
        project.stage = "ready".into();
        registry.update(project.clone())?;
        storage::write(&file, &registry)?;
        Ok(project)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovalPreview {
    pub reference: String,
    pub workspace: String,
    pub description: String,
    pub fingerprint: String,
    pub external: bool,
    pub has_environment: bool,
}

pub(crate) fn preview(brig: &Brig, project: &Project) -> Result<RemovalPreview, String> {
    let list = brig.json(&["--json".into(), "ls".into()], "SandboxList", None)?;
    let has_environment = list["data"]["sandboxes"]
        .as_array()
        .ok_or("Could not read the Brig session list.")?
        .iter()
        .any(|sandbox| sandbox["ref"] == project.reference());
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
    let description = if has_environment {
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
    let fingerprint = brig::fingerprint(
        &serde_json::json!({"reference":project.reference(),"workspace":workspace,"project":project.path,"dryRun":description,"policy":project.policy,"hasEnvironment":has_environment}),
    )?;
    Ok(RemovalPreview {
        reference: project.reference(),
        workspace,
        description,
        fingerprint,
        external: !project.owns_session,
        has_environment,
    })
}

#[tauri::command]
pub async fn preview_removal(app: AppHandle, identifier: String) -> Result<RemovalPreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let project = Registry::load(&projects::file(&app)?)?.project(&identifier)?;
        let settings = project.settings(settings::load(&settings::file(&app)?)?)?;
        preview(&Brig::new(&settings)?, &project)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn remove_project(
    app: AppHandle,
    projects: State<'_, Projects>,
    identifier: String,
    fingerprint: String,
    confirmation: String,
    logout: bool,
) -> Result<String, String> {
    let preflight_app = app.clone();
    let preflight_id = identifier.clone();
    let expected = fingerprint.clone();
    let entered = confirmation.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let project = Registry::load(&projects::file(&preflight_app)?)?.project(&preflight_id)?;
        let settings = project.settings(settings::load(&settings::file(&preflight_app)?)?)?;
        let current = preview(&Brig::new(&settings)?, &project)?;
        if logout && !current.has_environment {
            return Err("No guest environment exists to sign out of.".to_owned());
        }
        if current.fingerprint != expected || current.reference != entered {
            return Err(
                "The preview or session name does not match. The operation was not started."
                    .to_owned(),
            );
        }
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())??;
    terminal::close_project(app.clone(), &identifier).await?;
    let lock = projects.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = lock.lock().map_err(|error| error.to_string())?;
        let file = projects::file(&app)?;
        let mut registry = Registry::load(&file)?;
        let mut project = registry.project(&identifier)?;
        let settings = project.settings(settings::load(&settings::file(&app)?)?)?;
        let brig = Brig::new(&settings)?;
        let current = preview(&brig, &project)?;
        if current.fingerprint != fingerprint || current.reference != confirmation {
            return Err(
                "The removal target changed or the session name does not match. Refresh the preview."
                    .into(),
            );
        }
        project.stage = "removing".into();
        registry.update(project.clone())?;
        storage::write(&file, &registry)?;
        if logout {
            brig.checked(
                &[
                    "sh".into(),
                    project.reference(),
                    "codex".into(),
                    "logout".into(),
                ],
                None,
            )?;
        }
        if current.has_environment {
            brig.checked(&["stop".into(), project.reference()], None)?;
            brig.checked(&["rm".into(), project.reference()], None)?;
        }
        let mut cleanup = if current.has_environment {
            "Brig session removed. Project files were preserved."
        } else {
            "Project setup record removed. Existing guest home and project files were preserved."
        }.to_owned();
        if let Some(policy) = &project.policy {
            if project.owns_binding {
                let detached = brig.run(
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
                if detached.exit_code != Some(0) {
                    cleanup.push_str(
                        " Could not remove the policy binding. Review the Brig policy list.",
                    );
                }
            }
            if policy.owned {
                let document = brig.policy(&policy.name)?;
                let unchanged = document
                    .as_ref()
                    .map(brig::fingerprint)
                    .transpose()?
                    .as_deref()
                    == policy.fingerprint.as_deref();
                if unchanged {
                    let removed =
                        brig.run(&["policy".into(), "rm".into(), policy.name.clone()], None)?;
                    if removed.exit_code != Some(0) {
                        cleanup
                            .push_str(" The policy was preserved because it has other bindings or could not be removed.");
                    }
                }
                if !unchanged {
                    cleanup.push_str(" The externally modified policy was preserved.");
                }
            }
        }
        registry.projects.retain(|project| project.id != identifier);
        storage::write(&file, &registry)?;
        Ok(cleanup)
    })
    .await
    .map_err(|error| error.to_string())?
}
