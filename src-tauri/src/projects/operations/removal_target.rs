use super::{preview as environment_preview, *};
use std::sync::{Arc, Mutex};

pub(super) fn preview(app: &AppHandle, project: &Project) -> Result<RemovalPreview, String> {
    let settings = project.settings(settings::load(&settings::file(app)?)?)?;
    let brig = environment::brig_for_project(app, project, &settings)?;
    environment_preview(&brig, project)
}

pub(super) fn begin(
    app: &AppHandle,
    lock: Arc<Mutex<()>>,
    target: &Project,
    expected: &str,
) -> Result<(), RemovalError> {
    let operation = app.state::<crate::agent_sessions::Accounts>().operation(target)?;
    let _claude_guard = operation.blocking_lock();
    let shared = app
        .state::<crate::providers::shared_chatgpt::SharedChatgpt>()
        .0
        .clone();
    let _codex_guard = shared.blocking_lock();
    let _guard = lock.lock().map_err(|error| error.to_string())?;
    let file = projects::file(app)?;
    let _file_guard = storage::lock(&file)?;
    let mut project = Registry::load(&file)?.environment(&target.id, &target.environment_id)?;
    let settings = project.settings(settings::load(&settings::file(app)?)?)?;
    let brig = environment::brig_for_project(app, &project, &settings)?;
    let current = environment_preview(&brig, &project)?;
    current.check_fingerprint(expected)?;
    project.stage = crate::projects::ProjectStage::Removing;
    project.environment_verified = Some(false);
    project.environment_attempt = None;
    project.removal.get_or_insert(projects::Removal {
        reference: project.reference(),
        workspace: current.workspace,
        had_environment: current.has_environment,
        stopped: false,
        environment_removed: false,
        binding_detached: false,
        policy_handled: false,
        preserved_policy: false,
    });
    persist_removal(&file, &project)?;
    Ok(())
}

pub(super) fn finish(
    app: &AppHandle,
    lock: Arc<Mutex<()>>,
    target: &Project,
) -> Result<String, String> {
    let operation = app.state::<crate::agent_sessions::Accounts>().operation(target)?;
    let _claude_guard = operation.blocking_lock();
    let shared = app
        .state::<crate::providers::shared_chatgpt::SharedChatgpt>()
        .0
        .clone();
    let mut account = shared.blocking_lock();
    let _guard = lock.lock().map_err(|error| error.to_string())?;
    let file = projects::file(app)?;
    let project = Registry::load(&file)?.environment(&target.id, &target.environment_id)?;
    let settings = project.settings(settings::load(&settings::file(app)?)?)?;
    let brig = environment::brig_for_project(app, &project, &settings)?;
    crate::projects::project_github_access::revoke_for_removal(&brig, &file, &project, &settings)?;
    let project = Registry::load(&file)?.environment(&target.id, &target.environment_id)?;
    let current = environment_preview(&brig, &project)?;
    crate::providers::shared_chatgpt::detach_project(app, &mut account, &project.id, true)?;
    crate::providers::shared_claude::detach_environment(app, &brig, &project, false)?;
    crate::conversations::unobserve_environment(app, &project.id, &project.environment_id)?;
    let result = continue_removal_with(
        &brig,
        &file,
        &project.id,
        &project.environment_id,
        &current.fingerprint,
        &project.reference(),
    )?;
    Ok(result)
}
