use crate::{platform::command, platform::host, storage};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tauri::AppHandle;

mod migration;
#[cfg(test)]
mod migration_tests;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentDefaults {
    pub profile: String,
    pub allowed_hosts: Vec<String>,
}

impl AgentDefaults {
    fn validate(&self) -> Result<(), String> {
        if !valid_identifier(&self.profile) {
            return Err(
                "Use lowercase letters, numbers, dots, hyphens or underscores in the profile name."
                    .into(),
            );
        }
        if self.allowed_hosts.len() > 64 || self.allowed_hosts.iter().any(|host| !valid_host(host))
        {
            return Err("Enter one valid host per line. Wildcards are not allowed.".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    Isolated,
    Offline,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Verification {
    Require,
    Warn,
    Off,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct Settings {
    pub schema_version: u32,
    pub cli_path: Option<String>,
    pub codex: AgentDefaults,
    pub claude_code: AgentDefaults,
    pub cpus: u16,
    pub memory_mb: u32,
    pub verification: Verification,
    pub network: Network,
    pub telemetry: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: 2,
            cli_path: None,
            codex: AgentDefaults {
                profile: "codex".into(),
                allowed_hosts: vec![
                    "auth.openai.com".into(),
                    "chatgpt.com".into(),
                    "api.openai.com".into(),
                ],
            },
            claude_code: AgentDefaults {
                profile: "tugboat-claude".into(),
                allowed_hosts: vec![
                    "api.anthropic.com".into(),
                    "platform.claude.com".into(),
                    "claude.ai".into(),
                ],
            },
            cpus: 4,
            memory_mb: 4096,
            verification: Verification::Require,
            network: Network::Isolated,
            telemetry: false,
        }
    }
}

pub fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
        && value.as_bytes()[0].is_ascii_alphanumeric()
}

pub fn valid_host(value: &str) -> bool {
    value.len() <= 253
        && value.contains('.')
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.as_bytes()[0].is_ascii_alphanumeric()
                && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

impl Settings {
    pub fn agent_defaults(&self, kind: crate::agents::AgentKind) -> &AgentDefaults {
        match kind {
            crate::agents::AgentKind::Codex => &self.codex,
            crate::agents::AgentKind::ClaudeCode => &self.claude_code,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 2 {
            return Err("This settings file version is unsupported.".into());
        }
        self.codex.validate()?;
        self.claude_code.validate()?;
        if self.cpus == 0 || self.cpus > 32 {
            return Err("CPU count must be between 1 and 32.".into());
        }
        if !(1024..=65536).contains(&self.memory_mb) || !self.memory_mb.is_multiple_of(256) {
            return Err("Memory must be between 1024 and 65536 MB, in multiples of 256 MB.".into());
        }
        if let Some(path) = &self.cli_path {
            if !Path::new(path).is_absolute() || path.len() > 4096 || path.contains('\0') {
                return Err("Enter the absolute path to the Brig executable.".into());
            }
        }
        Ok(())
    }

    pub fn brig(&self) -> Result<PathBuf, String> {
        crate::platform::runtime::validate()?;
        let brig = host::discover("brig", self.cli_path.as_deref()).ok_or_else(|| {
            match self.cli_path {
                Some(_) => "The selected Brig executable is unavailable. Clear or update the CLI path in Settings.",
                None => "The bundled Brig executable is unavailable. Reinstall Tugboat.",
            }.to_owned()
        })?;
        let output = host::query(&brig, &["--json", "version"], &AtomicBool::new(false))?;
        let version = host::envelope(&output, "Version")?;
        if version["data"]["version"] != host::SUPPORTED_BRIG_VERSION {
            return Err(format!("This app requires Brig {}. Clear or update the CLI path in Settings.", host::SUPPORTED_BRIG_VERSION));
        }
        Ok(brig)
    }

    pub fn environment(&self) -> BTreeMap<String, String> {
        let mut values = command::brig_environment();
        values.insert("BRIG_HYPERVISOR".into(), "hvi".into());
        values.insert("BRIG_RUNTIME".into(), "hull".into());
        values.insert(
            "BRIG_VERIFY".into(),
            match self.verification {
                Verification::Require => "require",
                Verification::Warn => "warn",
                Verification::Off => "off",
            }
            .into(),
        );
        values.insert("BRIG_NETWORK".into(), self.network_name().into());
        if self.telemetry {
            values.remove("DO_NOT_TRACK");
        }
        values
    }

    pub fn network_name(&self) -> &'static str {
        match self.network {
            Network::Isolated => "isolated",
            Network::Offline => "offline",
        }
    }

    pub fn arguments(&self) -> Vec<String> {
        vec![
            "--network".into(),
            self.network_name().into(),
            "--cpus".into(),
            self.cpus.to_string(),
            "--mem".into(),
            self.memory_mb.to_string(),
        ]
    }
}

pub fn load(file: &Path) -> Result<Settings, String> {
    let Some(bytes) = storage::read_regular_bytes(file)? else {
        return Ok(Settings::default());
    };
    Ok(migration::decode(&bytes)?.into_record())
}

pub fn file(app: &AppHandle) -> Result<PathBuf, String> {
    let records = crate::storage::data_location::records_directory(app)?;
    let file = crate::storage::data_location::directory(app)?.join("settings.json");
    migrate(&file, &records)?;
    Ok(file)
}

pub fn migrate(
    file: &Path,
    records: &crate::storage::data_location::RecordsDirectory,
) -> Result<bool, String> {
    storage::upgrade_record(file, records, |bytes| {
        migration::decode(bytes)
    })
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InterfacePreferences {
    sidebar_width: Option<u32>,
}

#[tauri::command]
pub fn load_interface_preferences(app: AppHandle) -> Result<InterfacePreferences, String> {
    let file = crate::storage::data_location::directory(&app)?.join("interface.json");
    Ok(storage::read(&file)?.unwrap_or_default())
}

#[tauri::command]
pub fn save_interface_preferences(app: AppHandle, sidebar_width: u32) -> Result<(), String> {
    if !(280..=16384).contains(&sidebar_width) {
        return Err("The sidebar width must be between 280 and 16384 pixels.".into());
    }
    let file = crate::storage::data_location::directory(&app)?.join("interface.json");
    let _guard = storage::lock(&file)?;
    storage::write(
        &file,
        &InterfacePreferences {
            sidebar_width: Some(sidebar_width),
        },
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryStatus {
    pub recorded: Option<String>,
    pub effective: String,
    pub message: Option<String>,
}

fn recorded_telemetry(stdout: &str) -> Result<String, String> {
    match stdout.lines().next() {
        Some("telemetry: on") => Ok("on".into()),
        Some("telemetry: off") => Ok("off".into()),
        Some("telemetry: not answered yet") => Ok("unanswered".into()),
        _ => Err("The Brig telemetry response was not recognized.".into()),
    }
}

fn telemetry(settings: &Settings, apply: bool) -> TelemetryStatus {
    let effective = if settings.telemetry { "on" } else { "off" }.to_owned();
    let operation = || -> Result<String, String> {
        let brig = settings.brig()?;
        let mut environment = settings.environment();
        environment.remove("DO_NOT_TRACK");
        if apply {
            let output = command::run(
                &brig,
                &["telemetry".into(), effective.clone()],
                None,
                &environment,
                Duration::from_secs(10),
                &AtomicBool::new(false),
            )?;
            if output.exit_code != Some(0) {
                return Err("Could not apply the Brig telemetry preference.".into());
            }
        }
        let output = command::run(
            &brig,
            &["telemetry".into(), "status".into()],
            None,
            &environment,
            Duration::from_secs(10),
            &AtomicBool::new(false),
        )?;
        if output.exit_code != Some(0) {
            return Err("Could not read the Brig telemetry preference.".into());
        }
        recorded_telemetry(&output.stdout)
    };
    match operation() {
        Ok(recorded) => TelemetryStatus {
            recorded: Some(recorded),
            effective,
            message: None,
        },
        Err(error) => TelemetryStatus {
            recorded: None,
            effective,
            message: Some(error),
        },
    }
}

#[tauri::command]
pub fn load_settings(app: AppHandle) -> Result<Settings, String> {
    load(&file(&app)?)
}

#[tauri::command]
pub async fn telemetry_status(app: AppHandle) -> Result<TelemetryStatus, String> {
    let settings = load(&file(&app)?)?;
    tauri::async_runtime::spawn_blocking(move || telemetry(&settings, false))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn save_settings(app: AppHandle, settings: Settings) -> Result<TelemetryStatus, String> {
    settings.validate()?;
    let file = file(&app)?;
    let _lock = storage::lock(&file)?;
    crate::storage::data_location::records_directory(&app)?
        .verify_at(file.parent().ok_or("Invalid settings location.")?)?;
    storage::write(&file, &settings)?;
    drop(_lock);
    tauri::async_runtime::spawn_blocking(move || telemetry(&settings, true))
        .await
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn telemetry_recognizes_fresh_profiles_and_explicit_choices() {
        for (message, expected) in [
            ("telemetry: not answered yet", "unanswered"),
            ("telemetry: on", "on"),
            ("telemetry: off", "off"),
        ] {
            assert_eq!(recorded_telemetry(message).unwrap(), expected);
        }
        assert!(recorded_telemetry("unexpected response").is_err());
    }

    #[test]
    fn resources_and_hosts_are_validated() {
        let mut settings = Settings::default();
        assert!(settings.validate().is_ok());
        for cpus in [0, 33] {
            settings.cpus = cpus;
            assert!(settings.validate().is_err());
        }
        settings.cpus = 4;
        settings.memory_mb = 4097;
        assert!(settings.validate().is_err());
        settings.memory_mb = 4096;
        settings.codex.allowed_hosts = vec!["*.openai.com".into()];
        assert!(settings.validate().is_err());
        for invalid in ["a.com\nallow: all", "-a.com", "a..com", "a.com/path"] {
            assert!(!valid_host(invalid));
        }
    }

    #[test]
    fn atomic_preferences_reopen_and_migrate_without_losing_values() {
        let directory = std::env::temp_dir().join(format!("brig-settings-{}", std::process::id()));
        let file = directory.join("settings.json");
        let mut settings = Settings {
            cpus: 2,
            memory_mb: 8192,
            ..Settings::default()
        };
        storage::write(&file, &settings).unwrap();
        assert_eq!(load(&file).unwrap(), settings);
        let mut legacy = serde_json::to_value(&settings).unwrap();
        legacy["schemaVersion"] = 0.into();
        legacy["profile"] = legacy["codex"]["profile"].clone();
        legacy["allowedHosts"] = legacy["codex"]["allowedHosts"].clone();
        legacy.as_object_mut().unwrap().remove("codex");
        legacy.as_object_mut().unwrap().remove("claudeCode");
        storage::write(&file, &legacy).unwrap();
        let directory = std::fs::canonicalize(&directory).unwrap();
        let file = directory.join("settings.json");
        let records = crate::storage::data_location::RecordsDirectory::fixture(&directory);
        assert!(migrate(&file, &records).unwrap());
        assert_eq!(load(&file).unwrap(), settings);
        assert_eq!(storage::read::<Settings>(&file).unwrap().unwrap(), settings);
        settings.schema_version = 99;
        storage::write(&file, &settings).unwrap();
        assert!(load(&file).is_err());
        assert_eq!(
            storage::read::<Settings>(&file)
                .unwrap()
                .unwrap()
                .schema_version,
            99
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn environment_is_an_allow_list_and_telemetry_has_an_effective_value() {
        let mut settings = Settings::default();
        let environment = settings.environment();
        let runtime = crate::platform::runtime::directory().unwrap();
        assert_eq!(
            environment.get("PATH").map(String::as_str),
            Some(format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", runtime.display()).as_str())
        );
        assert_eq!(
            environment.get("MKE2FS_CONFIG").map(String::as_str),
            crate::platform::runtime::configuration()
                .as_ref()
                .and_then(|path| path.to_str())
        );
        assert!(command::environment()["PATH"].contains("/opt/homebrew/bin"));
        for denied in [
            "BRIG_FORWARD_ENV",
            "BRIG_ALLOW_DENIED",
            "BRIG_GIT_CONFIG",
            "BRIG_CODEX_FORWARD_ENV",
            "BRIG_CODEX_ALLOW_DENIED",
            "BRIG_CODEX_GIT_CONFIG",
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "OPENAI_API_KEY",
        ] {
            assert!(!environment.contains_key(denied));
        }
        assert_eq!(
            environment.get("DO_NOT_TRACK").map(String::as_str),
            Some("1")
        );
        settings.telemetry = true;
        assert!(!settings.environment().contains_key("DO_NOT_TRACK"));
    }
}
