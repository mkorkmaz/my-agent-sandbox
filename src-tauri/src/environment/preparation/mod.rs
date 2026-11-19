use crate::{
    desktop::settings::{self, Settings},
    environment::managed_profile,
    platform::brig::{self, Brig},
    projects::project_policy::PolicyEditor,
    projects::{self, EnvironmentId, Policy, Project, Projects, Registry},
    storage,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};

mod session_index;
#[cfg(all(test, target_os = "macos"))]
mod shared_guest_tests;
pub(crate) use session_index::GuestHomes;

#[derive(Default)]
pub struct PreparationLocks(pub Mutex<HashMap<(String, EnvironmentId), Arc<Mutex<()>>>>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preparation {
    pub project: Project,
    pub plan: Option<Value>,
    pub message: Option<String>,
    pub fingerprint: Option<String>,
}

fn save(registry_file: &Path, project: &Project) -> Result<(), String> {
    let mut registry = Registry::load(registry_file)?;
    registry.update(project.clone())?;
    storage::write(registry_file, &registry)
}

fn allowed_hosts(settings: &Settings) -> Vec<String> {
    let mut hosts = settings.codex.allowed_hosts.clone();
    hosts.extend(settings.claude_code.allowed_hosts.iter().cloned());
    hosts.sort();
    hosts.dedup();
    hosts
}

fn matches_template(policy: &Value, expected: &[String]) -> bool {
    let spec = &policy["spec"];
    if spec["egress"]["default"] != "deny"
        || spec
            .get("routes")
            .is_some_and(|routes| !routes.as_array().is_some_and(Vec::is_empty))
    {
        return false;
    }
    if spec["egress"]
        .get("deny")
        .is_some_and(|deny| !deny.as_array().is_some_and(Vec::is_empty))
    {
        return false;
    }
    let Some(rules) = spec["egress"]["allow"].as_array() else {
        return expected.is_empty() && spec["egress"]["allow"].is_null();
    };
    let mut hosts = Vec::new();
    for rule in rules {
        if rule.as_object().is_none_or(|rule| rule.len() != 1) {
            return false;
        }
        let Some(host) = rule["host"].as_str() else {
            return false;
        };
        hosts.push(host.to_owned());
    }
    hosts.sort();
    hosts.dedup();
    hosts == expected
}

fn checked_plan(
    brig: &Brig,
    project: &Project,
    settings: &Settings,
    private_records: &[PathBuf],
) -> Result<Value, String> {
    let plan = brig.plan(project, settings)?;
    brig::validate_plan_context(&plan, project, settings)?;
    crate::storage::data_location::require_private_plan_records(private_records, &plan)?;
    Ok(plan)
}

#[cfg(test)]
pub(crate) fn prepare(
    brig: &Brig,
    registry_file: &Path,
    settings: &Settings,
    identifier: &str,
) -> Result<Preparation, String> {
    let project = Registry::load(registry_file)?.project(identifier)?;
    prepare_selected(
        brig,
        registry_file,
        settings,
        identifier,
        &project.environment_id,
        &[],
    )
}

fn prepare_selected(
    brig: &Brig,
    registry_file: &Path,
    settings: &Settings,
    identifier: &str,
    environment_id: &EnvironmentId,
    private_records: &[PathBuf],
) -> Result<Preparation, String> {
    let _file_guard = storage::lock(registry_file)?;
    let mut project = Registry::load(registry_file)?.environment(identifier, environment_id)?;
    let configured = project.settings(settings.clone())?;
    let settings = &configured;
    if project.removal_pending() {
        return Ok(Preparation { project, plan: None, message: Some("Removal is incomplete. Review the removal preview to continue. Project setup is paused.".into()), fingerprint: None });
    }
    if matches!(
        project.stage,
        crate::projects::ProjectStage::EnvironmentStarting
            | crate::projects::ProjectStage::Stopping
    ) {
        return Ok(Preparation {
            project,
            plan: None,
            message: Some("An environment operation is in progress. Wait for it to finish.".into()),
            fingerprint: None,
        });
    }
    if project.image_update.is_some() {
        return Ok(Preparation {
            project, plan: None, fingerprint: None,
            message: Some("Finish guest image recovery in Project settings before preparing the environment.".into()),
        });
    }
    project.check_path()?;
    project.check_guest_path()?;
    if project.policy_detached {
        return Ok(Preparation {
            project,
            plan: None,
            message: Some("Policy detached. Read the policy and choose Attach to this session before preparing the environment.".into()),
            fingerprint: None,
        });
    }
    let definition = brig.agent_profile(&project.profile)?;
    managed_profile::validate(&project, &definition)?;
    checked_plan(brig, &project, settings, private_records)?;
    let hosts = allowed_hosts(settings);
    if project.policy.is_none() {
        if hosts.len() > 64 {
            return Err(
                "Use up to 64 distinct project allowed hosts across both providers.".into(),
            );
        }
        let inventory = brig.json(&["--json".into(), "ls".into()], "SandboxList", None)?;
        let inventory = inventory["data"]["sandboxes"]
            .as_array()
            .ok_or("Could not read the Brig session list.")?;
        if inventory
            .iter()
            .any(|entry| entry["ref"] == project.reference())
        {
            return Err("A Brig session already uses this project's reserved name. Resolve the existing session before preparing this project.".into());
        }
        let base = format!("bd-{}", project.id);
        let mut available = None;
        for index in 0..32 {
            let candidate = if index == 0 {
                base.clone()
            } else {
                format!("{base}-{}", index + 1)
            };
            if brig.policy(&candidate)?.is_none() {
                available = Some(candidate);
                break;
            }
        }
        let name = available.ok_or("Could not find a free project policy name. Existing policies were preserved; review unused policies in Brig.")?;
        project.policy = Some(Policy {
            name,
            owned: true,
            fingerprint: None,
        });
        project.stage = crate::projects::ProjectStage::CreatingPolicy;
        save(registry_file, &project)?;
    }
    let policy_link = project.policy.clone().ok_or("Policy record is missing.")?;
    let mut policy = brig.policy(&policy_link.name)?;
    if policy.is_none() {
        if !policy_link.owned || project.stage != crate::projects::ProjectStage::CreatingPolicy {
            return Err(
                "The project policy was removed outside the app. Check the existing bindings."
                    .into(),
            );
        }
        let editor = PolicyEditor::create(&policy_link.name, &hosts)?;
        let mut editor_brig = Brig {
            executable: brig.executable.clone(),
            environment: brig.environment.clone(),
        };
        editor_brig.environment.insert(
            "EDITOR".into(),
            editor.executable.to_string_lossy().into_owned(),
        );
        editor_brig.environment.insert(
            "VISUAL".into(),
            editor.executable.to_string_lossy().into_owned(),
        );
        editor_brig.environment.insert(
            "BRIG_DESKTOP_POLICY_SOURCE".into(),
            editor.source.to_string_lossy().into_owned(),
        );
        editor_brig.checked(
            &["policy".into(), "create".into(), policy_link.name.clone()],
            None,
        )?;
        policy = brig.policy(&policy_link.name)?;
    }
    let policy = policy.ok_or("Could not read the policy after creation.")?;
    let fingerprint = brig::fingerprint(&policy)?;
    if let Some(previous) = &policy_link.fingerprint {
        if *previous != fingerprint {
            return Err("The policy changed outside the app. Changes were preserved. Review the current policy before accepting it.".into());
        }
    }
    if policy_link.fingerprint.is_none()
        && (!policy_link.owned || !matches_template(&policy, &hosts))
    {
        return Err(
            "The interrupted policy does not match the expected template. The document was preserved.".into(),
        );
    }
    project
        .policy
        .as_mut()
        .ok_or("Policy record is missing.")?
        .fingerprint = Some(fingerprint);
    project.stage = crate::projects::ProjectStage::PolicyCreated;
    save(registry_file, &project)?;
    let before = brig.plan(&project, settings)?;
    if !before["data"]["policies"]
        .as_array()
        .is_some_and(|policies| policies.iter().any(|name| name == &policy_link.name))
    {
        project.stage = crate::projects::ProjectStage::AttachingPolicy;
        project.owns_binding = true;
        save(registry_file, &project)?;
        brig.checked(
            &[
                "policy".into(),
                "attach".into(),
                policy_link.name.clone(),
                project.profile.clone(),
                "-n".into(),
                project.label.clone(),
            ],
            None,
        )?;
    }
    let check = brig.checked(
        &[
            "policy".into(),
            "check".into(),
            project.profile.clone(),
            "-n".into(),
            project.label.clone(),
        ],
        None,
    )?;
    if !check
        .stdout
        .lines()
        .any(|line| line.trim() == policy_link.name)
    {
        return Err("Brig could not verify the policy binding.".into());
    }
    let plan = checked_plan(brig, &project, settings, private_records)?;
    let paths = brig::validate_plan(&plan, &project, settings)?;
    let running = match crate::environment::is_running(brig, &project, &paths) {
        Ok(running) => running,
        Err(message) => {
            project.stage = crate::projects::ProjectStage::EnvironmentUnverified;
            save(registry_file, &project)?;
            return Ok(Preparation {
                project,
                plan: None,
                message: Some(message),
                fingerprint: None,
            });
        }
    };
    let fingerprint = crate::environment::plan_fingerprint(&plan, &project, &definition)?;
    project.stage = project.environment_stage(running, &fingerprint).into();
    save(registry_file, &project)?;
    let fingerprint = Some(fingerprint);
    Ok(Preparation {
        project,
        plan: Some(plan),
        message: None,
        fingerprint,
    })
}

#[tauri::command]
pub async fn prepare_project(
    app: AppHandle,
    projects: State<'_, Projects>,
    locks: State<'_, PreparationLocks>,
    identifier: String,
    environment_id: Option<EnvironmentId>,
) -> Result<Preparation, String> {
    let registry_file = projects::file(&app)?;
    let registry = Registry::load(&registry_file)?;
    crate::storage::data_location::validate_project_records(&app, &registry)?;
    let project = registry.resolve_environment(&identifier, environment_id.as_ref())?;
    let environment_id = project.environment_id;
    let lock = {
        locks
            .0
            .lock()
            .map_err(|error| error.to_string())?
            .entry((identifier.clone(), environment_id.clone()))
            .or_default()
            .clone()
    };
    // Registry writes serialize with folder selection as well as project preparation.
    let registry_lock = projects.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _project_guard = lock.lock().map_err(|error| error.to_string())?;
        let _registry_guard = registry_lock.lock().map_err(|error| error.to_string())?;
        let _file_guard = storage::lock(&registry_file)?;
        let registry = Registry::load(&registry_file)?;
        crate::storage::data_location::validate_project_records(&app, &registry)?;
        let mut project = registry.environment(&identifier, &environment_id)?;
        let active =
            crate::environment::EnvironmentOperation::pending(&identifier, &environment_id)
                || crate::terminal::terminal_sessions(identifier.clone())
                    .iter()
                    .any(|session| {
                        session.belongs_to_environment(&identifier, &environment_id)
                            && session.purpose == "environment"
                            && session.running
                    });
        if project.recover_preparation(active)? {
            save(&registry_file, &project)?;
        }
        let settings = project.settings(settings::load(&settings::file(&app)?)?)?;
        let brig = crate::environment::brig_for_project(&app, &project, &settings)?;
        drop(_file_guard);
        prepare_selected(
            &brig,
            &registry_file,
            &settings,
            &identifier,
            &environment_id,
            &crate::storage::data_location::protected_directories(&app)?,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn reopening_incomplete_removal_never_runs_setup_or_changes_progress() {
        let root =
            std::env::temp_dir().join(format!("tugboat-removing-open-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let file = root.join("projects.json");
        let mut registry = Registry::load(&file).unwrap();
        let mut project = registry.register(&root).unwrap();
        project.stage = crate::projects::ProjectStage::Removing;
        registry.update(project.clone()).unwrap();
        storage::write(&file, &registry).unwrap();
        let before = fs::read(&file).unwrap();
        let brig = Brig {
            executable: "/missing/brig-must-not-run".into(),
            environment: Default::default(),
        };
        for _ in 0..2 {
            let result = prepare(&brig, &file, &Settings::default(), &project.id).unwrap();
            assert_eq!(result.project.stage, "removing");
            assert!(result.plan.is_none());
            assert!(result.fingerprint.is_none());
            assert!(result.project.require_active().is_err());
            assert_eq!(fs::read(&file).unwrap(), before);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn real_brig_policy_preparation_resumes_and_preserves_external_edits() {
        let root = std::env::temp_dir().join(format!("brig-preparation-{}", std::process::id()));
        fs::create_dir_all(root.join("sample-project")).unwrap();
        let registry_file = root.join("projects.json");
        let settings = Settings::default();
        let mut registry = Registry::load(&registry_file).unwrap();
        let project = registry.register(&root.join("sample-project")).unwrap();
        storage::write(&registry_file, &registry).unwrap();
        let mut brig = Brig::new(&settings).unwrap();
        for (name, directory) in [
            ("HOME", "home"),
            ("XDG_CONFIG_HOME", "config"),
            ("BRIG_STATE_DIR", "state"),
            ("BRIG_POLICY_DIR", "policies"),
        ] {
            let directory = root.join(directory);
            fs::create_dir_all(&directory).unwrap();
            brig.environment
                .insert(name.into(), directory.to_string_lossy().into_owned());
        }
        // No VM is booted here. Isolate the runtime inventory while real Brig owns policy I/O.
        let inventory = root.join("hull-metadata-probe");
        fs::write(&inventory, "#!/bin/sh\ncase \"$1\" in\nps) printf 'NAME\\tSTATUS\\n'; exit 0;;\ninspect) exit 1;;\nesac\nexec \"$TUGBOAT_TEST_HULL\" \"$@\"\n").unwrap();
        fs::set_permissions(&inventory, fs::Permissions::from_mode(0o700)).unwrap();
        brig.environment.insert(
            "TUGBOAT_TEST_HULL".into(),
            crate::platform::hull::executable()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        );
        brig.environment.insert(
            "BRIG_RUNTIME_BIN".into(),
            inventory.to_string_lossy().into_owned(),
        );
        let brig = brig.for_project(&project, &root.join("profiles")).unwrap();
        let first = prepare(&brig, &registry_file, &settings, &project.id).unwrap();
        assert_eq!(first.project.stage, "ready");
        let second = prepare(&brig, &registry_file, &settings, &project.id).unwrap();
        assert_eq!(first.project.reference(), second.project.reference());
        let mut stale_ready = second.project.clone();
        stale_ready.stage = crate::projects::ProjectStage::EnvironmentReady;
        save(&registry_file, &stale_ready).unwrap();
        assert_eq!(
            prepare(&brig, &registry_file, &settings, &project.id)
                .unwrap()
                .project
                .stage,
            "ready"
        );
        let absent_preview = crate::projects::operations::preview(&brig, &second.project).unwrap();
        assert!(!absent_preview.has_environment);
        assert!(absent_preview.description.contains("project files stay"));
        let policy_name = first.project.policy.as_ref().unwrap().name.clone();
        let file = root.join("policies").join(format!("{policy_name}.yaml"));
        let original = fs::read(&file).unwrap();
        let changed = crate::projects::project_policy::change(
            &brig,
            &registry_file,
            &settings,
            &project.id,
            &project.environment_id,
            first
                .project
                .policy
                .as_ref()
                .unwrap()
                .fingerprint
                .as_ref()
                .unwrap(),
            crate::projects::project_policy::Action::Save,
            vec!["example.org".into()],
            &[],
        )
        .unwrap();
        assert!(fs::read_to_string(&file).unwrap().contains("example.org"));
        brig.checked(
            &[
                "policy".into(),
                "attach".into(),
                policy_name.clone(),
                project.profile.clone(),
                "-n".into(),
                "other-project".into(),
            ],
            None,
        )
        .unwrap();
        assert!(crate::projects::project_policy::change(
            &brig,
            &registry_file,
            &settings,
            &project.id,
            &project.environment_id,
            changed
                .policy
                .as_ref()
                .unwrap()
                .fingerprint
                .as_ref()
                .unwrap(),
            crate::projects::project_policy::Action::Save,
            allowed_hosts(&settings),
            &[],
        )
        .is_err());
        assert!(fs::read_to_string(&file).unwrap().contains("example.org"));
        brig.checked(
            &[
                "policy".into(),
                "detach".into(),
                policy_name.clone(),
                project.profile.clone(),
                "-n".into(),
                "other-project".into(),
            ],
            None,
        )
        .unwrap();
        let detached = crate::projects::project_policy::change(
            &brig,
            &registry_file,
            &settings,
            &project.id,
            &project.environment_id,
            changed
                .policy
                .as_ref()
                .unwrap()
                .fingerprint
                .as_ref()
                .unwrap(),
            crate::projects::project_policy::Action::Detach,
            Vec::new(),
            &[],
        )
        .unwrap();
        assert_eq!(detached.stage, "policy-detached");
        for _ in 0..2 {
            let reopened = prepare(&brig, &registry_file, &settings, &project.id).unwrap();
            assert_eq!(reopened.project.stage, "policy-detached");
            assert!(reopened.project.policy_detached);
            assert!(reopened.plan.is_none());
            assert!(reopened.fingerprint.is_none());
            assert!(brig.plan(&detached, &settings).unwrap()["data"]["policies"]
                .as_array()
                .unwrap()
                .is_empty());
        }
        let saved_detached = crate::projects::project_policy::change(
            &brig,
            &registry_file,
            &settings,
            &project.id,
            &project.environment_id,
            detached
                .policy
                .as_ref()
                .unwrap()
                .fingerprint
                .as_ref()
                .unwrap(),
            crate::projects::project_policy::Action::Save,
            allowed_hosts(&settings),
            &[],
        )
        .unwrap();
        assert!(saved_detached.policy_detached);
        assert_eq!(saved_detached.stage, "policy-detached");
        let attached = crate::projects::project_policy::change(
            &brig,
            &registry_file,
            &settings,
            &project.id,
            &project.environment_id,
            saved_detached
                .policy
                .as_ref()
                .unwrap()
                .fingerprint
                .as_ref()
                .unwrap(),
            crate::projects::project_policy::Action::Attach,
            Vec::new(),
            &[],
        )
        .unwrap();
        assert!(attached.owns_binding);
        assert!(!attached.policy_detached);
        crate::projects::project_policy::change(
            &brig,
            &registry_file,
            &settings,
            &project.id,
            &project.environment_id,
            attached
                .policy
                .as_ref()
                .unwrap()
                .fingerprint
                .as_ref()
                .unwrap(),
            crate::projects::project_policy::Action::Save,
            allowed_hosts(&settings),
            &[],
        )
        .unwrap();
        assert_eq!(
            fs::read_dir(root.join("policies"))
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("bd-"))
                .count(),
            1
        );
        let mut interrupted = first.project.clone();
        interrupted.stage = crate::projects::ProjectStage::CreatingPolicy;
        interrupted.policy.as_mut().unwrap().fingerprint = None;
        save(&registry_file, &interrupted).unwrap();
        assert_eq!(
            prepare(&brig, &registry_file, &settings, &project.id)
                .unwrap()
                .project
                .stage,
            "ready"
        );
        let edited = String::from_utf8(original)
            .unwrap()
            .replace("Prepared by Tugboat", "User maintained policy");
        fs::write(&file, &edited).unwrap();
        assert!(prepare(&brig, &registry_file, &settings, &project.id).is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), edited);
        let preview = crate::projects::operations::preview(&brig, &first.project).unwrap();
        let removed = crate::projects::operations::continue_removal(
            &brig,
            &registry_file,
            &project.id,
            &preview.fingerprint,
            &project.reference(),
        )
        .unwrap();
        assert!(removed.contains("preserved"));
        assert_eq!(fs::read_to_string(&file).unwrap(), edited);
        let mut registry = Registry::load(&registry_file).unwrap();
        assert!(registry.projects.is_empty());
        let readded = registry.register(&project.path).unwrap();
        assert_eq!(readded.id, project.id);
        storage::write(&registry_file, &registry).unwrap();
        let replacement = prepare(&brig, &registry_file, &settings, &project.id).unwrap();
        let new_name = replacement.project.policy.as_ref().unwrap().name.clone();
        assert_ne!(new_name, policy_name);
        assert!(replacement.project.policy.as_ref().unwrap().owned);
        assert!(replacement.plan.is_some());
        assert_eq!(fs::read_to_string(&file).unwrap(), edited);
        let mut interrupted = replacement.project.clone();
        interrupted.stage = crate::projects::ProjectStage::CreatingPolicy;
        interrupted.policy.as_mut().unwrap().fingerprint = None;
        save(&registry_file, &interrupted).unwrap();
        for _ in 0..2 {
            let retried = prepare(&brig, &registry_file, &settings, &project.id).unwrap();
            assert_eq!(retried.project.policy.as_ref().unwrap().name, new_name);
        }
        assert_eq!(
            fs::read_dir(root.join("policies"))
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("bd-"))
                .count(),
            2
        );
        let preview = crate::projects::operations::preview(&brig, &replacement.project).unwrap();
        crate::projects::operations::continue_removal(
            &brig,
            &registry_file,
            &project.id,
            &preview.fingerprint,
            &project.reference(),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), edited);
        assert!(!root
            .join("policies")
            .join(format!("{new_name}.yaml"))
            .exists());
        fs::remove_dir_all(root).unwrap();
    }
}
