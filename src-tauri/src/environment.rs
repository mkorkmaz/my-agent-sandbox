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
use tauri::{AppHandle, Manager, State, WebviewWindow};

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
    let project = Registry::load(&projects::file(app)?)?.project(identifier)?;
    project.check_path()?;
    project.check_guest_path()?;
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
    let registry_lock = projects.0.clone();
    let project_id = identifier.clone();
    let command = tauri::async_runtime::spawn_blocking(move || {
        let _guard = registry_lock.lock().map_err(|error| error.to_string())?;
        let mut checked = checked_project(&app, &identifier, Some(&fingerprint))?;
        let command = command(&checked, run_arguments(&checked, &["--version"]));
        checked.project.stage = "environment-starting".into();
        let file = projects::file(&app)?;
        let mut registry = Registry::load(&file)?;
        registry.update(checked.project)?;
        storage::write(&file, &registry)?;
        Ok::<_, String>(command)
    })
    .await
    .map_err(|error| error.to_string())??;
    terminal::start(window, project_id, "environment", command, layout, None).await
}

#[tauri::command]
pub async fn finish_environment(
    app: AppHandle,
    projects: State<'_, Projects>,
    identifier: String,
    terminal_identifier: u64,
) -> Result<Project, String> {
    let status =
        terminal::status(&identifier).ok_or("The environment terminal record was not found.")?;
    if status.identifier != terminal_identifier
        || status.purpose != "environment"
        || status.running
        || status.exit_code != Some(0)
    {
        return Err("Could not verify the guest Codex version. Check the terminal output.".into());
    }
    let lock = projects.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = lock.lock().map_err(|error| error.to_string())?;
        let mut checked = checked_project(&app, &identifier, None)?;
        let reference = checked.project.reference();
        let info = checked.brig.json(
            &["--json".into(), "info".into(), reference.clone()],
            "Info",
            None,
        )?;
        let list = checked
            .brig
            .json(&["--json".into(), "ls".into()], "SandboxList", None)?;
        if info["data"]["project"]["host"] != checked.plan["data"]["project"]["host"]
            || info["data"]["workspace"] != checked.plan["data"]["home"]["host"]
            || !list["data"]["sandboxes"]
                .as_array()
                .is_some_and(|sandboxes| {
                    sandboxes
                        .iter()
                        .any(|sandbox| sandbox["ref"] == reference && sandbox["state"] == "running")
                })
        {
            return Err("Could not verify the Brig environment project or running state.".into());
        }
        checked.project.stage = "environment-ready".into();
        let file = projects::file(&app)?;
        let mut registry = Registry::load(&file)?;
        registry.update(checked.project.clone())?;
        storage::write(&file, &registry)?;
        Ok(checked.project)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
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
