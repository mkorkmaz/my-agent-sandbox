use crate::storage::RecordSchema;
use super::{EnvironmentId, Project, Registry};
use crate::{agents::AgentKind, desktop::settings};
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyRegistry {
    schema_version: u32,
    projects: Vec<Value>,
}

pub(super) fn decode(file: &Path, bytes: &[u8]) -> Result<RecordSchema<Registry>, String> {
    let document: Value =
        serde_json::from_slice(bytes).map_err(|_| "Could not read the saved projects.")?;
    if document["schemaVersion"] == 2 {
        let registry: Registry =
            serde_json::from_value(document).map_err(|_| "Could not read the saved projects.")?;
        registry.validate()?;
        return Ok(RecordSchema::Current(registry));
    }
    let legacy: LegacyRegistry =
        serde_json::from_value(document).map_err(|_| "Could not read the saved projects.")?;
    if legacy.schema_version != 1 {
        return Err("This project registry version is unsupported.".into());
    }
    let mut registry = Registry::default();
    if legacy.projects.is_empty() {
        return Ok(RecordSchema::Upgrade(registry));
    }
    let defaults = settings::load(&file.with_file_name("settings.json"))?;
    for document in legacy.projects {
        registry
            .projects
            .push(legacy_project(document, &defaults)?.into());
    }
    registry.validate()?;
    Ok(RecordSchema::Upgrade(registry))
}

fn legacy_project(mut document: Value, defaults: &settings::Settings) -> Result<Project, String> {
    let project_id = document["id"]
        .as_str()
        .ok_or("The legacy project identity is missing.")?;
    let environment_id = EnvironmentId::for_agent(project_id, AgentKind::Codex);
    let fields = document
        .as_object_mut()
        .ok_or("Invalid legacy project record.")?;
    if ["environmentId", "agentKind", "network", "environments"]
        .iter()
        .any(|field| fields.contains_key(*field))
    {
        return Err("Invalid legacy project environment fields.".into());
    }
    let network = fields
        .get("options")
        .and_then(|options| options.get("network"))
        .cloned()
        .unwrap_or(serde_json::to_value(&defaults.network).map_err(|error| error.to_string())?);
    fields.insert(
        "environmentId".into(),
        serde_json::to_value(environment_id).map_err(|error| error.to_string())?,
    );
    fields.insert(
        "agentKind".into(),
        serde_json::to_value(AgentKind::Codex).map_err(|error| error.to_string())?,
    );
    fields.insert("network".into(), network);
    let mut project: Project =
        serde_json::from_value(document).map_err(|_| "Could not read the legacy project.")?;
    project.policy_detached |= project.stage == crate::projects::ProjectStage::PolicyDetached;
    if project.removal.is_some() {
        project.stage = crate::projects::ProjectStage::Removing;
    }
    project
        .environment_verified
        .get_or_insert(project.stage == crate::projects::ProjectStage::EnvironmentReady);
    project
        .last_opened_at
        .get_or_insert(project.updated_at.saturating_mul(1000));
    project.validate()?;
    Ok(project)
}
