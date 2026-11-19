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

mod runtime;
mod state;
#[cfg(test)]
pub(super) mod tests;
pub(super) use runtime::verify_sandbox;
pub use state::Mapping;
pub(crate) use state::{validate_plan_ports, Approval};
use state::{Observed, Phase};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Read,
    Publish,
    Close,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Status {
    Closed,
    Publishing,
    GuestStopped,
    MappingUnavailable,
    WaitingForServer,
    Ready,
    Closing,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    project_id: String,
    status: Status,
    mapping: Option<Mapping>,
}

#[tauri::command]
pub async fn project_preview(
    app: AppHandle,
    identifier: String,
    environment_id: EnvironmentId,
    action: Action,
    mapping: Option<Mapping>,
) -> Result<View, String> {
    tauri::async_runtime::spawn_blocking(move || {
        with_project(
            &app,
            &identifier,
            &environment_id,
            |registry, file, project, settings, brig| {
                change(registry, file, project, settings, brig, action, mapping)
            },
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

fn with_project<T>(
    app: &AppHandle,
    identifier: &str,
    environment_id: &EnvironmentId,
    operation: impl FnOnce(
        &mut Registry,
        &Path,
        &Project,
        &settings::Settings,
        &Brig,
    ) -> Result<T, String>,
) -> Result<T, String> {
    let state = app.state::<Projects>();
    let _projects = state.0.lock().map_err(|error| error.to_string())?;
    let file = projects::file(app)?;
    let _registry = storage::lock(&file)?;
    let mut registry = Registry::load(&file)?;
    storage::data_location::validate_project_records(app, &registry)?;
    let selected = registry.environment(identifier, environment_id)?;
    selected.require_active()?;
    let target = match &selected.preview {
        Some(approval) => registry.environment(identifier, &approval.environment_id)?,
        None => selected,
    };
    let _operation = environment::EnvironmentOperation::begin(identifier, &target.environment_id)?;
    target.require_active()?;
    target.check_path()?;
    let settings = target.settings(settings::load(&settings::file(app)?)?)?;
    let brig = environment::brig_for_project(app, &target, &settings)?;
    operation(&mut registry, &file, &target, &settings, &brig)
}

fn change(
    registry: &mut Registry,
    file: &Path,
    project: &Project,
    settings: &settings::Settings,
    brig: &Brig,
    action: Action,
    mapping: Option<Mapping>,
) -> Result<View, String> {
    let mut approval = project.preview.clone();
    let inspect = |mapping| runtime::inspect(brig, project, settings, mapping);
    let mut snapshot = inspect(approval.as_ref().map(|value| value.mapping))?;
    match action {
        Action::Read => {}
        Action::Publish => {
            let mapping = mapping.ok_or("Choose host and guest ports first.")?;
            mapping.validate()?;
            snapshot.require_publishable()?;
            if approval
                .as_ref()
                .is_some_and(|value| value.mapping != mapping || value.phase == Phase::Closing)
            {
                return Err(
                    "Close the existing local preview before publishing another port.".into(),
                );
            }
            if snapshot.ports == Observed::Absent {
                reserve(registry, file, project, mapping, &mut approval)?;
                runtime::publish(brig, project, mapping)?;
                snapshot = inspect(Some(mapping))?;
                if snapshot.ports == Observed::Absent {
                    return Err(
                        "Brig did not confirm the preview port. Retry the publication.".into(),
                    );
                }
            }
        }
        Action::Close => {
            if let Some(value) = approval.as_mut() {
                value.phase = Phase::Closing;
                let mapping = value.mapping;
                save(registry, file, &project.id, approval.clone())?;
                if snapshot.ports != Observed::Absent {
                    runtime::unpublish(brig, project, mapping)?;
                    snapshot = inspect(Some(mapping))?;
                    if snapshot.ports != Observed::Absent {
                        return Err("Brig still reports the preview port. Retry closing it.".into());
                    }
                }
            }
        }
    }
    let reconciled = reconcile(approval, snapshot.ports);
    if registry.record(&project.id)?.preview.as_ref() != reconciled.as_ref() {
        save(registry, file, &project.id, reconciled.clone())?;
    }
    Ok(view(project, reconciled.as_ref(), snapshot))
}

fn reserve(
    registry: &mut Registry,
    file: &Path,
    project: &Project,
    mapping: Mapping,
    approval: &mut Option<Approval>,
) -> Result<(), String> {
    if registry.projects.iter().any(|sibling| {
        sibling.id != project.id
            && sibling
                .preview
                .as_ref()
                .is_some_and(|value| value.mapping.host_port == mapping.host_port)
    }) {
        return Err("The Mac port is approved for another project.".into());
    }
    runtime::require_free_port(mapping)?;
    *approval = Some(Approval::new(project, mapping));
    save(registry, file, &project.id, approval.clone())
}

fn reconcile(mut approval: Option<Approval>, observed: Observed) -> Option<Approval> {
    match approval.as_mut() {
        Some(value) if value.phase == Phase::Closing && observed == Observed::Absent => None,
        Some(value) if value.phase == Phase::Publishing && observed != Observed::Absent => {
            value.phase = Phase::Published;
            approval
        }
        _ => approval,
    }
}

fn save(
    registry: &mut Registry,
    file: &Path,
    project: &str,
    approval: Option<Approval>,
) -> Result<(), String> {
    registry
        .projects
        .iter_mut()
        .find(|record| record.id == project)
        .ok_or("Project not found.")?
        .preview = approval;
    registry.validate()?;
    storage::write(file, registry)
}

fn view(project: &Project, approval: Option<&Approval>, snapshot: runtime::Snapshot) -> View {
    let status = match approval {
        None => Status::Closed,
        Some(value) if value.phase == Phase::Closing => Status::Closing,
        Some(value) if value.phase == Phase::Publishing => Status::Publishing,
        Some(_) if snapshot.ports == Observed::Recorded && !snapshot.running => {
            Status::GuestStopped
        }
        Some(_) if snapshot.ports != Observed::Live => Status::MappingUnavailable,
        Some(value) if runtime::server_responds(value.mapping) => Status::Ready,
        Some(_) => Status::WaitingForServer,
    };
    View {
        project_id: project.id.clone(),
        status,
        mapping: approval.map(|value| value.mapping),
    }
}

#[tauri::command]
pub async fn open_project_preview(
    app: AppHandle,
    identifier: String,
    environment_id: EnvironmentId,
    mapping: Mapping,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        with_project(
            &app,
            &identifier,
            &environment_id,
            |_, _, project, settings, brig| {
                open_verified(
                    brig,
                    project,
                    settings,
                    mapping,
                    runtime::server_responds,
                    runtime::open,
                )
            },
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

fn open_verified(
    brig: &Brig,
    project: &Project,
    settings: &settings::Settings,
    mapping: Mapping,
    server_responds: impl FnOnce(Mapping) -> bool,
    launch: impl FnOnce(Mapping) -> Result<(), String>,
) -> Result<(), String> {
    mapping.validate()?;
    if !project
        .preview
        .as_ref()
        .is_some_and(|approval| approval.phase == Phase::Published && approval.mapping == mapping)
    {
        return Err("The approved preview has changed. Check it again.".into());
    }
    let snapshot = runtime::inspect(brig, project, settings, Some(mapping))?;
    snapshot.require_publishable()?;
    if snapshot.ports != Observed::Live {
        return Err(
            "The approved port is unavailable. Check the project environment and retry.".into(),
        );
    }
    if !server_responds(mapping) {
        return Err("The port is published. Start the guest web server and try again.".into());
    }
    launch(mapping)
}
