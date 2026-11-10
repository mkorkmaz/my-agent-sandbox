use crate::{
    brig::{self, Brig},
    policy_editor::PolicyEditor,
    projects::{self, Policy, Project, Projects, Registry},
    settings::{self, Settings},
    storage,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};

#[derive(Default)]
pub struct PreparationLocks(pub Mutex<HashMap<String, Arc<Mutex<()>>>>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preparation {
    pub project: Project,
    pub plan: Option<Value>,
    pub candidates: Vec<String>,
    pub message: Option<String>,
    pub fingerprint: Option<String>,
}

fn save(registry_file: &Path, project: &Project) -> Result<(), String> {
    let mut registry = Registry::load(registry_file)?;
    registry.update(project.clone())?;
    storage::write(registry_file, &registry)
}

fn matches_template(policy: &Value, settings: &Settings) -> bool {
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
        return settings.allowed_hosts.is_empty() && spec["egress"]["allow"].is_null();
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
    let mut expected = settings.allowed_hosts.clone();
    expected.sort();
    expected.dedup();
    hosts == expected
}

fn candidates(brig: &Brig, project: &Project) -> Result<Vec<String>, String> {
    let list = brig.json(&["--json".into(), "ls".into()], "SandboxList", None)?;
    let sandboxes = list["data"]["sandboxes"]
        .as_array()
        .ok_or("Could not read the Brig session list.")?;
    if sandboxes.len() > 256 {
        return Err("The Brig session list is too large. Review unused sessions.".into());
    }
    let mut matches = Vec::new();
    for sandbox in sandboxes {
        if sandbox["agent"] != project.profile {
            continue;
        }
        let Some(reference) = sandbox["ref"].as_str() else {
            return Err("The Brig session identity is missing.".into());
        };
        let Some((profile, label)) = reference.split_once('@') else {
            continue;
        };
        if profile != project.profile || !settings::valid_identifier(label) {
            continue;
        }
        let info = brig.json(
            &["--json".into(), "info".into(), reference.into()],
            "Info",
            None,
        )?;
        let Some(path) = info["data"]["project"]["host"].as_str() else {
            continue;
        };
        if std::fs::canonicalize(path).ok().as_ref() == Some(&project.path) {
            matches.push(reference.to_owned());
        }
    }
    Ok(matches)
}

fn prepare(
    brig: &Brig,
    registry_file: &Path,
    settings: &Settings,
    identifier: &str,
    chosen: Option<&str>,
) -> Result<Preparation, String> {
    let mut project = Registry::load(registry_file)?.project(identifier)?;
    let environment_was_ready = project.stage == "environment-ready";
    project.check_path()?;
    project.check_guest_path()?;
    brig.validate_codex_profile(&project.profile)?;
    if project.policy.is_none() && project.stage == "selected" {
        let matches = candidates(brig, &project)?;
        if matches.len() > 1 && chosen.is_none() {
            return Ok(Preparation {
                project,
                plan: None,
                candidates: matches,
                message: Some(
                    "Multiple Brig sessions use this folder. Select the session to use.".into(),
                ),
                fingerprint: None,
            });
        }
        let selection = chosen.or_else(|| matches.first().map(String::as_str));
        if let Some(reference) = selection {
            if !matches.iter().any(|candidate| candidate == reference) {
                return Err(
                    "The selected Brig session does not belong to this project folder.".into(),
                );
            }
            project.label = reference
                .split_once('@')
                .ok_or("Invalid Brig session.")?
                .1
                .into();
            project.owns_session = false;
            let plan = brig.plan(&project, settings)?;
            if let Some(name) = plan["data"]["policies"]
                .as_array()
                .and_then(|policies| policies.first())
                .and_then(Value::as_str)
            {
                if !settings::valid_identifier(name) {
                    return Err("Invalid Brig policy name.".into());
                }
                let policy = brig
                    .policy(name)?
                    .ok_or("The existing session policy was not found.")?;
                project.policy = Some(Policy {
                    name: name.into(),
                    owned: false,
                    fingerprint: Some(brig::fingerprint(&policy)?),
                });
            }
            save(registry_file, &project)?;
        }
    }
    if project.policy.is_none() {
        let name = format!("bd-{}", project.id);
        if brig.policy(&name)?.is_some() {
            return Err("This project policy name is already in use. The existing policy was preserved. Check the Brig policy list.".into());
        }
        project.policy = Some(Policy {
            name,
            owned: true,
            fingerprint: None,
        });
        project.stage = "creating-policy".into();
        save(registry_file, &project)?;
    }
    let policy_link = project.policy.clone().ok_or("Policy record is missing.")?;
    let mut policy = brig.policy(&policy_link.name)?;
    if policy.is_none() {
        if !policy_link.owned || project.stage != "creating-policy" {
            return Err(
                "The project policy was removed outside the app. Check the existing bindings."
                    .into(),
            );
        }
        let editor = PolicyEditor::create(&policy_link.name, settings)?;
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
        && (!policy_link.owned || !matches_template(&policy, settings))
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
    project.stage = "policy-created".into();
    save(registry_file, &project)?;
    let before = brig.plan(&project, settings)?;
    if !before["data"]["policies"]
        .as_array()
        .is_some_and(|policies| policies.iter().any(|name| name == &policy_link.name))
    {
        project.stage = "attaching-policy".into();
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
    let plan = brig.plan(&project, settings)?;
    brig::validate_plan(&plan, &project, settings)?;
    project.stage = if environment_was_ready {
        "environment-ready"
    } else {
        "ready"
    }
    .into();
    save(registry_file, &project)?;
    let fingerprint = Some(crate::environment::plan_fingerprint(&plan)?);
    Ok(Preparation {
        project,
        plan: Some(plan),
        candidates: Vec::new(),
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
    chosen_reference: Option<String>,
) -> Result<Preparation, String> {
    let lock = {
        locks
            .0
            .lock()
            .map_err(|error| error.to_string())?
            .entry(identifier.clone())
            .or_default()
            .clone()
    };
    let registry_file = projects::file(&app)?;
    // Registry writes serialize with folder selection as well as project preparation.
    let registry_lock = projects.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _project_guard = lock
            .try_lock()
            .map_err(|_| "This project setup is already running.")?;
        let _registry_guard = registry_lock.lock().map_err(|error| error.to_string())?;
        let project = Registry::load(&registry_file)?.project(&identifier)?;
        let settings = project.settings(settings::load(&settings::file(&app)?)?)?;
        let brig = Brig::new(&settings)?;
        prepare(
            &brig,
            &registry_file,
            &settings,
            &identifier,
            chosen_reference.as_deref(),
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
    fn real_brig_policy_preparation_resumes_and_preserves_external_edits() {
        let root = std::env::temp_dir().join(format!("brig-preparation-{}", std::process::id()));
        fs::create_dir_all(root.join("sample-project")).unwrap();
        let registry_file = root.join("projects.json");
        let settings = Settings::default();
        let mut registry = Registry::load(&registry_file).unwrap();
        let project = registry
            .register(&root.join("sample-project"), "codex")
            .unwrap();
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
        fs::write(&inventory, "#!/bin/sh\ncase \"$1\" in\nps) printf 'NAME\\tSTATUS\\n'; exit 0;;\ninspect) exit 1;;\nesac\nexec /opt/homebrew/bin/hull \"$@\"\n").unwrap();
        fs::set_permissions(&inventory, fs::Permissions::from_mode(0o700)).unwrap();
        brig.environment.insert(
            "BRIG_RUNTIME_BIN".into(),
            inventory.to_string_lossy().into_owned(),
        );
        let first = prepare(&brig, &registry_file, &settings, &project.id, None).unwrap();
        assert_eq!(first.project.stage, "ready");
        let second = prepare(&brig, &registry_file, &settings, &project.id, None).unwrap();
        assert_eq!(first.project.reference(), second.project.reference());
        let absent_preview = crate::operations::preview(&brig, &second.project).unwrap();
        assert!(!absent_preview.has_environment);
        assert!(absent_preview.description.contains("will be preserved"));
        let policy_name = first.project.policy.as_ref().unwrap().name.clone();
        let file = root.join("policies").join(format!("{policy_name}.yaml"));
        let original = fs::read(&file).unwrap();
        let changed = crate::project_policy::change(
            &brig,
            &registry_file,
            &settings,
            &project.id,
            first
                .project
                .policy
                .as_ref()
                .unwrap()
                .fingerprint
                .as_ref()
                .unwrap(),
            crate::project_policy::Action::Save,
            vec!["example.org".into()],
        )
        .unwrap();
        assert!(fs::read_to_string(&file).unwrap().contains("example.org"));
        brig.checked(
            &[
                "policy".into(),
                "attach".into(),
                policy_name.clone(),
                "codex".into(),
                "-n".into(),
                "other-project".into(),
            ],
            None,
        )
        .unwrap();
        assert!(crate::project_policy::change(
            &brig,
            &registry_file,
            &settings,
            &project.id,
            changed
                .policy
                .as_ref()
                .unwrap()
                .fingerprint
                .as_ref()
                .unwrap(),
            crate::project_policy::Action::Save,
            settings.allowed_hosts.clone()
        )
        .is_err());
        assert!(fs::read_to_string(&file).unwrap().contains("example.org"));
        brig.checked(
            &[
                "policy".into(),
                "detach".into(),
                policy_name.clone(),
                "codex".into(),
                "-n".into(),
                "other-project".into(),
            ],
            None,
        )
        .unwrap();
        let detached = crate::project_policy::change(
            &brig,
            &registry_file,
            &settings,
            &project.id,
            changed
                .policy
                .as_ref()
                .unwrap()
                .fingerprint
                .as_ref()
                .unwrap(),
            crate::project_policy::Action::Detach,
            Vec::new(),
        )
        .unwrap();
        assert_eq!(detached.stage, "policy-detached");
        let attached = crate::project_policy::change(
            &brig,
            &registry_file,
            &settings,
            &project.id,
            detached
                .policy
                .as_ref()
                .unwrap()
                .fingerprint
                .as_ref()
                .unwrap(),
            crate::project_policy::Action::Attach,
            Vec::new(),
        )
        .unwrap();
        assert!(attached.owns_binding);
        crate::project_policy::change(
            &brig,
            &registry_file,
            &settings,
            &project.id,
            attached
                .policy
                .as_ref()
                .unwrap()
                .fingerprint
                .as_ref()
                .unwrap(),
            crate::project_policy::Action::Save,
            settings.allowed_hosts.clone(),
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
        interrupted.stage = "creating-policy".into();
        interrupted.policy.as_mut().unwrap().fingerprint = None;
        save(&registry_file, &interrupted).unwrap();
        assert_eq!(
            prepare(&brig, &registry_file, &settings, &project.id, None)
                .unwrap()
                .project
                .stage,
            "ready"
        );
        let edited = String::from_utf8(original)
            .unwrap()
            .replace("Prepared by Tugboat", "User maintained policy");
        fs::write(&file, &edited).unwrap();
        assert!(prepare(&brig, &registry_file, &settings, &project.id, None).is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), edited);
        fs::remove_dir_all(root).unwrap();
    }
}
