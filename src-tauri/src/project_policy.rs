use crate::{
    brig::{self, Brig},
    policy_editor::PolicyEditor,
    projects::{self, Project, Projects, Registry},
    settings::{self, Settings},
    storage, terminal,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use tauri::{AppHandle, State};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyView {
    document: Value,
    fingerprint: String,
    editable: bool,
    attached: bool,
    allowed_hosts: Vec<String>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Save,
    Attach,
    Detach,
    Accept,
}

fn simple_hosts(document: &Value) -> Option<Vec<String>> {
    if document["spec"]["egress"]["default"] != "deny"
        || document["spec"]
            .as_object()?
            .keys()
            .any(|key| key != "egress")
        || document["spec"]["egress"]
            .as_object()?
            .keys()
            .any(|key| !["default", "allow"].contains(&key.as_str()))
    {
        return None;
    }
    if document["spec"]["egress"]["allow"].is_null() {
        return Some(Vec::new());
    }
    document["spec"]["egress"]["allow"]
        .as_array()?
        .iter()
        .map(|rule| {
            if rule.as_object()?.len() != 1 {
                return None;
            }
            let host = rule["host"].as_str()?;
            settings::valid_host(host).then(|| host.to_owned())
        })
        .collect()
}

fn sole_binding(list: &str, policy: &str, project: &Project) -> bool {
    let mut selected = false;
    let mut found = false;
    for line in list.lines() {
        if !line.starts_with(char::is_whitespace) {
            selected = line.split_whitespace().next() == Some(policy);
            found |= selected;
            continue;
        }
        if selected && line.trim() != format!("bound to: {} -n {}", project.profile, project.label)
        {
            return false;
        }
    }
    found
}

fn view(brig: &Brig, project: &Project, settings: &Settings) -> Result<PolicyView, String> {
    let link = project
        .policy
        .as_ref()
        .ok_or("Prepare the project policy first.")?;
    let document = brig.policy(&link.name)?.ok_or("Policy not found.")?;
    let plan = brig.plan(project, settings)?;
    let attached = plan["data"]["policies"]
        .as_array()
        .is_some_and(|policies| policies.iter().any(|name| name == &link.name));
    let hosts = simple_hosts(&document);
    let list = brig.checked(&["policy".into(), "ls".into()], None)?;
    Ok(PolicyView {
        fingerprint: brig::fingerprint(&document)?,
        editable: link.owned && hosts.is_some() && sole_binding(&list.stdout, &link.name, project),
        allowed_hosts: hosts.unwrap_or_default(),
        document,
        attached,
    })
}

pub(crate) fn change(
    brig: &Brig,
    file: &Path,
    settings: &Settings,
    identifier: &str,
    expected: &str,
    action: Action,
    hosts: Vec<String>,
) -> Result<Project, String> {
    let mut registry = Registry::load(file)?;
    let mut project = registry.project(identifier)?;
    project.check_path()?;
    let current = view(brig, &project, settings)?;
    if current.fingerprint != expected {
        return Err("The policy changed. Read the current document again.".into());
    }
    let list = brig.json(&["--json".into(), "ls".into()], "SandboxList", None)?;
    let sandboxes = list["data"]["sandboxes"]
        .as_array()
        .ok_or("The Brig state is missing.")?;
    if sandboxes
        .iter()
        .any(|sandbox| sandbox["ref"] == project.reference() && sandbox["state"] != "stopped")
    {
        return Err("Stop the Brig environment before changing its policy.".into());
    }
    let link = project.policy.clone().ok_or("Policy record is missing.")?;
    if matches!(action, Action::Save) && !current.editable {
        return Err(
            "This policy is externally managed or shared with other sessions. The document was preserved."
                .into(),
        );
    }
    if matches!(action, Action::Detach) && !project.owns_binding {
        return Err("The policy binding created outside the app was preserved.".into());
    }
    let mut template = settings.clone();
    template.allowed_hosts = hosts;
    template.validate()?;
    project.stage = "editing-policy".into();
    registry.update(project.clone())?;
    storage::write(file, &registry)?;
    if matches!(action, Action::Save) {
        let editor = PolicyEditor::create(&link.name, &template)?;
        let mut editing = Brig {
            executable: brig.executable.clone(),
            environment: brig.environment.clone(),
        };
        for key in ["EDITOR", "VISUAL"] {
            editing
                .environment
                .insert(key.into(), editor.executable.to_string_lossy().into_owned());
        }
        editing.environment.insert(
            "BRIG_DESKTOP_POLICY_SOURCE".into(),
            editor.source.to_string_lossy().into_owned(),
        );
        editing.checked(&["policy".into(), "edit".into(), link.name.clone()], None)?;
        let document = brig
            .policy(&link.name)?
            .ok_or("Could not read the saved policy.")?;
        if simple_hosts(&document) != Some(template.allowed_hosts.clone()) {
            return Err("The saved policy does not match the requested hosts.".into());
        }
        project
            .policy
            .as_mut()
            .ok_or("Policy record is missing.")?
            .fingerprint = Some(brig::fingerprint(&document)?);
    }
    if matches!(action, Action::Attach | Action::Detach) {
        // Save ownership before attach so an interrupted operation can be recovered.
        if matches!(action, Action::Attach) && !current.attached {
            project.owns_binding = true;
            registry.update(project.clone())?;
            storage::write(file, &registry)?;
        }
        brig.checked(
            &[
                "policy".into(),
                if matches!(action, Action::Attach) {
                    "attach"
                } else {
                    "detach"
                }
                .into(),
                link.name,
                project.profile.clone(),
                "-n".into(),
                project.label.clone(),
            ],
            None,
        )?;
        if matches!(action, Action::Detach) {
            project.owns_binding = false;
        }
    }
    if matches!(action, Action::Accept) {
        let plan = brig.plan(&project, settings)?;
        brig::validate_plan(&plan, &project, settings)?;
        project
            .policy
            .as_mut()
            .ok_or("Policy record is missing.")?
            .fingerprint = Some(current.fingerprint);
    }
    project.stage = if matches!(action, Action::Detach) {
        "policy-detached"
    } else {
        "ready"
    }
    .into();
    registry.update(project.clone())?;
    storage::write(file, &registry)?;
    Ok(project)
}

#[tauri::command]
pub async fn read_project_policy(app: AppHandle, identifier: String) -> Result<PolicyView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let project = Registry::load(&projects::file(&app)?)?.project(&identifier)?;
        let settings = project.settings(settings::load(&settings::file(&app)?)?)?;
        view(&Brig::new(&settings)?, &project, &settings)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn change_project_policy(
    app: AppHandle,
    projects: State<'_, Projects>,
    identifier: String,
    fingerprint: String,
    action: Action,
    hosts: Vec<String>,
) -> Result<Project, String> {
    if terminal::status(&identifier).is_some_and(|status| status.running) {
        return Err("End the open terminal process and stop the Brig environment first.".into());
    }
    let lock = projects.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = lock.lock().map_err(|error| error.to_string())?;
        let project = Registry::load(&projects::file(&app)?)?.project(&identifier)?;
        let settings = project.settings(settings::load(&settings::file(&app)?)?)?;
        change(
            &Brig::new(&settings)?,
            &projects::file(&app)?,
            &settings,
            &identifier,
            &fingerprint,
            action,
            hosts,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complex_policies_are_read_only() {
        assert_eq!(
            simple_hosts(
                &serde_json::json!({"spec":{"egress":{"default":"deny","allow":[{"host":"chatgpt.com"}]}}})
            ),
            Some(vec!["chatgpt.com".into()])
        );
        assert!(simple_hosts(&serde_json::json!({"spec":{"egress":{"default":"deny","allow":[{"cidr":"0.0.0.0/0"}]}}})).is_none());
        assert!(simple_hosts(
            &serde_json::json!({"spec":{"egress":{"default":"deny"},"routes":[{}]}})
        )
        .is_none());
    }
}
