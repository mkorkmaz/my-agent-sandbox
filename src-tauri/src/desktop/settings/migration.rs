use crate::storage::RecordSchema;
use super::{AgentDefaults, Network, Settings, Verification};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
struct LegacySettings {
    schema_version: u32,
    cli_path: Option<String>,
    profile: String,
    cpus: u16,
    memory_mb: u32,
    verification: Verification,
    network: Network,
    allowed_hosts: Vec<String>,
    telemetry: bool,
}

impl Default for LegacySettings {
    fn default() -> Self {
        let settings = Settings::default();
        Self {
            schema_version: 1,
            cli_path: settings.cli_path,
            profile: settings.codex.profile,
            cpus: settings.cpus,
            memory_mb: settings.memory_mb,
            verification: settings.verification,
            network: settings.network,
            allowed_hosts: settings.codex.allowed_hosts,
            telemetry: settings.telemetry,
        }
    }
}

pub(super) fn decode(bytes: &[u8]) -> Result<RecordSchema<Settings>, String> {
    let document: Value =
        serde_json::from_slice(bytes).map_err(|_| "Could not read the saved settings.")?;
    let version = document
        .get("schemaVersion")
        .map(Value::as_u64)
        .unwrap_or(Some(1));
    if version == Some(2) {
        let settings: Settings =
            serde_json::from_value(document).map_err(|_| "Could not read the saved settings.")?;
        settings.validate()?;
        return Ok(RecordSchema::Current(settings));
    }
    if !matches!(version, Some(0 | 1)) {
        return Err("This settings file version is unsupported.".into());
    }
    let old: LegacySettings =
        serde_json::from_value(document).map_err(|_| "Could not read the saved settings.")?;
    let settings = Settings {
        schema_version: 2,
        cli_path: old.cli_path,
        cpus: old.cpus,
        memory_mb: old.memory_mb,
        verification: old.verification,
        network: old.network,
        telemetry: old.telemetry,
        codex: AgentDefaults {
            profile: old.profile,
            allowed_hosts: old.allowed_hosts,
        },
        claude_code: Settings::default().claude_code,
    };
    settings.validate()?;
    Ok(RecordSchema::Upgrade(settings))
}
