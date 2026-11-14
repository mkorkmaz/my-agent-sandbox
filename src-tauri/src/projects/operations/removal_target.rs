use super::{preview as environment_preview, *};
use std::sync::{Arc, Mutex};

pub(super) fn select(
    registry: &Registry,
    identifier: &str,
    environment: Option<&EnvironmentId>,
) -> Result<Vec<Project>, String> {
    if let Some(environment) = environment {
        return Ok(vec![registry.environment(identifier, environment)?]);
    }
    registry
        .environments(identifier)?
        .iter()
        .map(|environment| registry.environment(identifier, &environment.environment_id))
        .collect()
}

pub(super) fn snapshots(
    app: &AppHandle,
    targets: &[Project],
) -> Result<Vec<(Project, RemovalPreview)>, String> {
    targets
        .iter()
        .map(|project| {
            let settings = project.settings(settings::load(&settings::file(app)?)?)?;
            let brig = environment::brig_for_project(app, project, &settings)?;
            Ok((project.clone(), environment_preview(&brig, project)?))
        })
        .collect()
}

pub(super) fn preview(app: &AppHandle, targets: &[Project]) -> Result<RemovalPreview, String> {
    combine(&snapshots(app, targets)?)
}

pub(super) fn combine(snapshots: &[(Project, RemovalPreview)]) -> Result<RemovalPreview, String> {
    let (first, _) = snapshots
        .first()
        .ok_or("No agent environments remain in this project.")?;
    if snapshots.len() == 1 {
        return Ok(snapshots[0].1.clone());
    }
    let descriptions: Vec<_> = snapshots
        .iter()
        .map(|(_, preview)| format!("{}\n{}", preview.reference, preview.description))
        .collect();
    let previews: Vec<_> = snapshots
        .iter()
        .map(|(_, preview)| preview.clone())
        .collect();
    let fingerprint = brig::fingerprint(
        &serde_json::json!({"projectId": first.id, "name":first.name, "environments":previews}),
    )?;
    Ok(RemovalPreview {
        reference: first.name.clone(),
        workspace: snapshots.iter().map(|(_, preview)| preview.workspace.as_str()).collect::<Vec<_>>().join("\n"),
        description: format!("Remove all {} agent environments from this project. Project files will be preserved.\n\n{}", snapshots.len(), descriptions.join("\n\n")),
        fingerprint,
        external: snapshots.iter().any(|(_, preview)| preview.external),
        has_environment: snapshots.iter().any(|(_, preview)| preview.has_environment),
        running: snapshots.iter().any(|(_, preview)| preview.running),
        logout: None,
        environment_id: None,
        profile: None,
        label: None,
        policy: None,
        owns_binding: false,
        environments: previews,
    })
}

pub(super) fn begin(
    app: &AppHandle,
    lock: Arc<Mutex<()>>,
    target: &Project,
    expected: &str,
    logout: bool,
) -> Result<(), String> {
    let operation = (target.agent_kind == crate::agents::AgentKind::ClaudeCode)
        .then(|| {
            app.state::<crate::agent_sessions::Accounts>()
                .operation(target)
        })
        .transpose()?;
    let _claude_guard = operation
        .as_ref()
        .map(|operation| operation.blocking_lock());
    let shared = app
        .state::<crate::providers::shared_chatgpt::SharedChatgpt>()
        .0
        .clone();
    let _codex_guard =
        (target.agent_kind == crate::agents::AgentKind::Codex).then(|| shared.blocking_lock());
    let _guard = lock.lock().map_err(|error| error.to_string())?;
    let file = projects::file(app)?;
    let _file_guard = storage::lock(&file)?;
    let mut project = Registry::load(&file)?.environment(&target.id, &target.environment_id)?;
    let settings = project.settings(settings::load(&settings::file(app)?)?)?;
    let brig = environment::brig_for_project(app, &project, &settings)?;
    let current = environment_preview(&brig, &project)?;
    if current.fingerprint != expected {
        return Err(
            "The removal target or progress changed. Refresh its preview before continuing.".into(),
        );
    }
    if logout && !current.has_environment && project.removal.is_none() {
        return Err("No guest environment exists to sign out of.".into());
    }
    project.stage = crate::projects::ProjectStage::Removing;
    project.environment_verified = Some(false);
    project.environment_attempt = None;
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
    persist_removal(&file, &project)
}

pub(super) fn finish(
    app: &AppHandle,
    lock: Arc<Mutex<()>>,
    target: &Project,
) -> Result<String, String> {
    let shared = app
        .state::<crate::providers::shared_chatgpt::SharedChatgpt>()
        .0
        .clone();
    let mut account =
        (target.agent_kind == crate::agents::AgentKind::Codex).then(|| shared.blocking_lock());
    let _guard = lock.lock().map_err(|error| error.to_string())?;
    let file = projects::file(app)?;
    let project = Registry::load(&file)?.environment(&target.id, &target.environment_id)?;
    let settings = project.settings(settings::load(&settings::file(app)?)?)?;
    let brig = environment::brig_for_project(app, &project, &settings)?;
    let current = environment_preview(&brig, &project)?;
    let logout = project
        .removal
        .as_ref()
        .ok_or("Removal progress is missing. Refresh its preview.")?
        .logout;
    if !logout {
        if let Some(account) = account.as_mut() {
            crate::providers::shared_chatgpt::detach_project(app, account, &project.id, false)?;
        }
        if project.agent_kind == crate::agents::AgentKind::ClaudeCode {
            crate::providers::shared_claude::detach_environment(app, &brig, &project, false)?;
        }
    }
    crate::conversations::unobserve_environment(app, &project.id, &project.environment_id)?;
    let result = continue_removal_with(
        &brig,
        &file,
        &project.id,
        &project.environment_id,
        &current.fingerprint,
        &project.reference(),
        logout,
        |brig, project| {
            if project.agent_kind == crate::agents::AgentKind::ClaudeCode
                && crate::providers::shared_claude::detach_environment(
                    app,
                    brig,
                    project,
                    current.running,
                )?
            {
                return Ok(());
            }
            if let Some(account) = account.as_mut() {
                if crate::providers::shared_chatgpt::detach_project(
                    app,
                    account,
                    &project.id,
                    true,
                )? {
                    return Ok(());
                }
            }
            guest_logout(brig, project)
        },
    )?;
    Ok(result)
}
