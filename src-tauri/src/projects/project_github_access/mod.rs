use crate::{
    desktop::settings,
    environment,
    platform::brig::Brig,
    projects::{self, EnvironmentId, Project, Projects, Registry},
    storage,
};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tauri::{AppHandle, Manager};

mod broker;
mod keychain;
mod policy;
mod state;
#[cfg(test)]
mod tests;
use keychain::Keychain;
pub(crate) use policy::verify_plan;
pub(crate) use state::Grant;
use state::Phase;

pub(crate) fn approved_policy(project: &Project) -> Option<&str> {
    project
        .github_access
        .as_ref()
        .filter(|grant| grant.phase == Phase::Saved)
        .map(|grant| grant.name.as_str())
}

pub(super) fn revoke_for_removal(
    brig: &Brig,
    file: &Path,
    target: &Project,
    settings: &settings::Settings,
) -> Result<(), String> {
    revoke_for_removal_with(brig, file, target, settings, Keychain::host)
}

fn revoke_for_removal_with(
    brig: &Brig,
    file: &Path,
    target: &Project,
    settings: &settings::Settings,
    keychain: impl FnOnce() -> Result<Keychain, String>,
) -> Result<(), String> {
    let _lock = storage::lock(file)?;
    let mut registry = Registry::load(file)?;
    let project = registry.environment(&target.id, &target.environment_id)?;
    if project.github_access.is_none() {
        return Ok(());
    }
    project.check_path()?;
    let keychain = keychain()?;
    apply(
        &mut registry,
        file,
        &project,
        settings,
        brig,
        &keychain,
        Action::Revoke,
        None,
    )?;
    Ok(())
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Read,
    Save,
    Revoke,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Status {
    Unavailable,
    None,
    Saving,
    Saved,
    Revoking,
    RevokedPendingCleanup,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    project_id: String,
    repository: Option<String>,
    status: Status,
    running: bool,
}

#[tauri::command]
pub async fn project_github_access(
    app: AppHandle,
    identifier: String,
    environment_id: EnvironmentId,
    action: Action,
    repository: Option<String>,
    token: Option<String>,
) -> Result<View, String> {
    let input = match action {
        Action::Save => Some((
            state::repository(
                repository
                    .as_deref()
                    .ok_or("Enter the GitHub repository.")?,
            )?,
            keychain::token(token.ok_or("Enter a GitHub access token.")?)?,
        )),
        Action::Read | Action::Revoke if token.is_some() || repository.is_some() => {
            return Err("Unexpected GitHub access fields.".into())
        }
        _ => None,
    };
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Projects>();
        let _projects = state.0.lock().map_err(|error| error.to_string())?;
        let file = projects::file(&app)?;
        let _records = storage::lock(&file)?;
        let mut registry = Registry::load(&file)?;
        storage::data_location::validate_project_records(&app, &registry)?;
        let project = registry.environment(&identifier, &environment_id)?;
        let _operation =
            environment::EnvironmentOperation::begin(&identifier, &project.environment_id)?;
        project.require_active()?;
        project.check_path()?;
        let settings = project.settings(settings::load(&settings::file(&app)?)?)?;
        let mut brig = environment::brig_for_project(&app, &project, &settings)?;
        if matches!(action, Action::Save) {
            brig.enable_host_keychain(
                file.parent()
                    .ok_or("The project data directory is missing.")?,
            )?;
        }
        let keychain = Keychain::host()?;
        apply(
            &mut registry,
            &file,
            &project,
            &settings,
            &brig,
            &keychain,
            action,
            input,
        )
    })
    .await
    .map_err(|_| "Could not complete the GitHub access change.")?
}

fn apply(
    registry: &mut Registry,
    file: &Path,
    project: &Project,
    settings: &settings::Settings,
    brig: &Brig,
    keychain: &Keychain,
    action: Action,
    input: Option<(String, Vec<u8>)>,
) -> Result<View, String> {
    let scope = policy::Scope::read(brig, project, settings)?;
    let existing = project.github_access.as_ref();
    let (present, attached) = match existing {
        Some(grant) => scope.observe(brig, project, grant)?,
        None => {
            scope.verify_no_route()?;
            (false, false)
        }
    };
    match action {
        Action::Read => {
            let status = match existing.map(|grant| grant.phase) {
                None => Status::None,
                Some(Phase::Saving) => Status::Saving,
                Some(Phase::Revoking) if !attached && !broker::serves(brig, &scope.sandbox)? => {
                    Status::RevokedPendingCleanup
                }
                Some(Phase::Revoking) => Status::Revoking,
                Some(Phase::Saved) => {
                    let grant = existing.ok_or("The GitHub grant is missing.")?;
                    if present
                        && attached
                        && keychain.contains(&grant.name)?
                        && scope.keychain_available()
                    {
                        Status::Saved
                    } else {
                        Status::Unavailable
                    }
                }
            };
            Ok(view(project, existing, status, scope.running))
        }
        Action::Save => {
            let (repository, token) = input.ok_or("The GitHub token is missing.")?;
            scope.require_stopped()?;
            if settings.network_name() != "isolated"
                || project.policy.is_none()
                || project.policy_detached
            {
                return Err(
                    "Attach the project policy and use Isolated mode before saving GitHub access."
                        .into(),
                );
            }
            let mut grant = match existing {
                Some(grant) if grant.phase == Phase::Revoking => {
                    return Err("Finish revoking GitHub access first.".into())
                }
                Some(grant) if grant.repository != repository => {
                    return Err(
                        "Revoke the existing access before selecting a different repository."
                            .into(),
                    )
                }
                Some(grant) => grant.clone(),
                None => Grant::new(&project.id, repository)?,
            };
            let key_exists = keychain.contains(&grant.name)?;
            if existing.is_none()
                && (key_exists
                    || brig
                        .policy(&grant.name)
                        .map_err(|_| "Could not check the GitHub policy identity.")?
                        .is_some())
            {
                return Err(
                    "The new GitHub grant identity is already in use. Retry saving access.".into(),
                );
            }
            grant.phase = Phase::Saving;
            persist(registry, file, project, Some(&grant))?;
            keychain.write(&grant.name, token, key_exists)?;
            if !present {
                policy::create(brig, &grant)?;
            }
            if !attached {
                policy::bind(brig, project, &grant, "attach")?;
            }
            grant.phase = Phase::Saved;
            let mut saved = project.clone();
            saved.github_access = Some(grant.clone());
            let plan = brig
                .plan(&saved, settings)
                .map_err(|_| "Could not verify the saved GitHub access.")?;
            policy::verify_plan(brig, &saved, &plan)?;
            persist(registry, file, project, Some(&grant))?;
            Ok(view(project, Some(&grant), Status::Saved, false))
        }
        Action::Revoke => {
            let Some(mut grant) = existing.cloned() else {
                return Ok(view(project, None, Status::None, scope.running));
            };
            grant.phase = Phase::Revoking;
            persist(registry, file, project, Some(&grant))?;
            if attached {
                policy::bind(brig, project, &grant, "detach")?;
            }
            policy::Scope::read(brig, project, settings)?.verify_no_route()?;
            broker::revoke(brig, &scope.sandbox)?;
            if present {
                policy::remove(brig, &grant)?;
            }
            keychain.delete(&grant.name)?;
            persist(registry, file, project, None)?;
            Ok(view(project, None, Status::None, scope.running))
        }
    }
}

fn persist(
    registry: &mut Registry,
    file: &Path,
    project: &Project,
    grant: Option<&Grant>,
) -> Result<(), String> {
    registry
        .projects
        .iter_mut()
        .find(|record| record.id == project.id)
        .ok_or("Project not found.")?
        .github_access = grant.cloned();
    registry.validate()?;
    storage::write(file, registry)
}

fn view(project: &Project, grant: Option<&Grant>, status: Status, running: bool) -> View {
    View {
        project_id: project.id.clone(),
        repository: grant.map(|grant| grant.repository.clone()),
        status,
        running,
    }
}
