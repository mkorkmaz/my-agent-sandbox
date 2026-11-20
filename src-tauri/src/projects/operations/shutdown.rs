use super::{inventory::SandboxInventory, verify_target};
use crate::{
    desktop::settings::Settings,
    platform::brig::Brig,
    projects::{Project, ProjectStage, Projects, Registry, RegistryRead},
    storage::{self, data_location},
    usage_store,
};
use std::path::Path;
use tauri::{AppHandle, Manager};

pub(crate) fn stop_environments(app: &AppHandle) -> Result<(), String> {
    // Startup may still be resetting former registrations when the window closes.
    let had_store = app
        .state::<usage_store::Runtime>()
        .existing_store()?
        .is_some();
    let records = match data_location::records_directory(app) {
        Ok(records) => records,
        Err(_) if !had_store => return Ok(()), // A recovery-only launch has no verified targets.
        Err(error) => return Err(error),
    };
    let projects = app.state::<Projects>();
    let _guard = projects.0.lock().map_err(|error| error.to_string())?;
    let file = records.path().join("projects.json");
    let _writer = storage::lock(&file)?;
    let RegistryRead::Current(mut registry) = Registry::read_state(&file)? else {
        // The former registry requires a restart to create shared project environments.
        return Ok(());
    };
    if registry.projects.is_empty() {
        return Ok(());
    }
    data_location::validate_project_records(app, &registry)?;
    let brig = Brig::new(&Settings::default())?;
    stop_registered(&brig, &file, &mut registry)
}

pub(crate) fn stop_registered(
    brig: &Brig,
    file: &Path,
    registry: &mut Registry,
) -> Result<(), String> {
    // Brig needs the managed profile to resolve a sandbox name to its project reference.
    let inventory =
        SandboxInventory::read(&brig.with_shared_profile(&file.with_file_name("profiles"))?)?;
    let mut failures = Vec::new();
    for mut project in registry.project_views()? {
        if let Err(error) = stop_project(brig, &inventory, file, registry, &mut project) {
            failures.push(format!(
                "Could not confirm shutdown for {}. {error}",
                project.name
            ));
        }
    }
    if failures.is_empty() {
        return Ok(());
    }
    Err(failures.join("\n"))
}

fn stop_project(
    brig: &Brig,
    inventory: &SandboxInventory,
    file: &Path,
    registry: &mut Registry,
    project: &mut Project,
) -> Result<(), String> {
    let home = brig.shared_project_home(project)?;
    let sandbox = format!("brig-{}-{}", project.profile, project.label);
    let running = inventory.running(project, &sandbox, &home)?;
    if running.is_none() || (running == Some(false) && project.stage != ProjectStage::Stopping) {
        return record_stopped(file, registry, project);
    }
    if project
        .removal
        .as_ref()
        .is_some_and(|removal| removal.environment_removed)
    {
        return Err(
            "The recorded guest was removed. Review the replacement in Project settings.".into(),
        );
    }
    let records = file
        .parent()
        .ok_or("The project records folder is unavailable.")?;
    let mut brig = brig.for_registered_project(project, records)?;
    let settings = project.settings(Settings::default())?;
    brig.environment
        .insert("BRIG_NETWORK".into(), settings.network_name().into());
    verify_target(&brig, project)?;
    // Preserve removal, image update, and policy checkpoints for their own recovery paths.
    if matches!(
        project.stage,
        ProjectStage::Ready
            | ProjectStage::EnvironmentReady
            | ProjectStage::EnvironmentStarting
            | ProjectStage::EnvironmentUnverified
    ) {
        project.stop_environment()?;
        save(file, registry, project)?;
    }
    brig.checked(&["stop".into(), project.reference()], None)?;
    if SandboxInventory::read(&brig)?.running(project, &sandbox, &home)? == Some(true) {
        return Err("Brig still reports the guest as running. Retry closing the app.".into());
    }
    record_stopped(file, registry, project)
}

fn record_stopped(
    file: &Path,
    registry: &mut Registry,
    project: &mut Project,
) -> Result<(), String> {
    if matches!(
        project.stage,
        ProjectStage::EnvironmentReady | ProjectStage::EnvironmentStarting
    ) {
        project.stop_environment()?;
    }
    if project.stage != ProjectStage::Stopping {
        return Ok(());
    }
    project.finish_stop()?;
    save(file, registry, project)
}

fn save(file: &Path, registry: &mut Registry, project: &Project) -> Result<(), String> {
    registry.update(project.clone())?;
    storage::write(file, registry)
}

#[cfg(test)]
mod tests;
