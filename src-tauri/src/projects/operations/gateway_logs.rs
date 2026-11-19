use super::verify_registered_reference;
use crate::{
    desktop::settings,
    environment,
    platform::{brig::Brig, command::CommandOutput},
    projects::{self, EnvironmentId, Project, Registry},
    storage::data_location,
    terminal,
};
use serde::Serialize;
use tauri::AppHandle;

mod summary;
#[cfg(test)]
mod tests;

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub(crate) enum GatewayLogs {
    Available { text: String },
    Empty,
    Offline,
    Unavailable,
    Timeout,
    OutputLimit,
    Failed,
}

fn log_result(output: CommandOutput) -> GatewayLogs {
    if output.timed_out {
        return GatewayLogs::Timeout;
    }
    if output.output_exceeded {
        return GatewayLogs::OutputLimit;
    }
    if output.cancelled {
        return GatewayLogs::Failed;
    }
    match output.exit_code {
        Some(0) => summary::redact(&output.stdout)
            .map_or(GatewayLogs::Empty, |text| GatewayLogs::Available { text }),
        Some(3) => GatewayLogs::Unavailable,
        _ => GatewayLogs::Failed,
    }
}

fn read_registered(mut brig: Brig, project: &Project) -> GatewayLogs {
    // Brig recovers the recorded posture when there is no override for the next boot.
    brig.environment.remove("BRIG_NETWORK");
    let Ok(info) = verify_registered_reference(&brig, project) else {
        return GatewayLogs::Unavailable;
    };
    let network = info["data"]["network"].as_str().unwrap_or_default();
    if network.split(';').next() == Some("offline (no egress)") {
        return GatewayLogs::Offline;
    }
    let arguments = [
        "logs".into(),
        "--gateway".into(),
        project.reference(),
        "--tail".into(),
        summary::LINE_LIMIT.to_string(),
    ];
    brig.run(&arguments, None)
        .map_or(GatewayLogs::Failed, log_result)
}

fn read_selected(
    app: &AppHandle,
    identifier: &str,
    environment_id: &EnvironmentId,
    reference: &str,
) -> Result<GatewayLogs, String> {
    let _operation = environment::EnvironmentOperation::begin(identifier, environment_id)?;
    let registry = Registry::load(&projects::file(app)?)?;
    data_location::validate_project_records(app, &registry)?;
    let project = registry.environment(identifier, environment_id)?;
    project.require_active()?;
    project.check_path()?;
    if project.reference() != reference {
        return Ok(GatewayLogs::Unavailable);
    }
    let settings = project.settings(settings::load(&settings::file(app)?)?)?;
    let brig = environment::brig_for_project(app, &project, &settings)?;
    Ok(read_registered(brig, &project))
}

#[tauri::command]
pub(crate) async fn read_project_gateway_logs(
    app: AppHandle,
    identifier: String,
    environment_id: EnvironmentId,
    reference: String,
) -> GatewayLogs {
    tauri::async_runtime::spawn_blocking(move || {
        read_selected(&app, &identifier, &environment_id, &reference)
            .unwrap_or(GatewayLogs::Unavailable)
    })
    .await
    .unwrap_or(GatewayLogs::Failed)
}

#[tauri::command]
pub(crate) async fn copy_project_gateway_logs(app: AppHandle, text: String) -> Result<(), String> {
    if !summary::can_copy(&text) {
        return Err("The gateway log summary is invalid. Read it again.".into());
    }
    terminal::copy_text(app, text).await
}
