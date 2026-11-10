use crate::command::{self, CommandOutput};
use serde::Serialize;
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::State;

#[derive(Default)]
pub struct Inspection(pub Mutex<Option<Arc<AtomicBool>>>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tools {
    brig: Option<PathBuf>,
    hull: Option<PathBuf>,
    homebrew: Option<PathBuf>,
    git: Option<PathBuf>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatus {
    architecture: &'static str,
    os_version: String,
    tools: Tools,
    brig_status: String,
    version: Option<Value>,
    doctor: Option<Value>,
    message: Option<String>,
}

pub fn discover(name: &str, configured: Option<&str>) -> Option<PathBuf> {
    if let Some(configured) = configured {
        let path = PathBuf::from(configured);
        return path
            .is_absolute()
            .then_some(path)
            .filter(|path| executable(path));
    }
    if name == "hull" {
        if let Some(path) = crate::hull::executable().filter(|path| executable(path)) {
            return Some(path);
        }
    }
    ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"]
        .iter()
        .map(|directory| Path::new(directory).join(name))
        .find(|path| executable(path))
}

fn executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

pub fn envelope(output: &CommandOutput, kind: &str) -> Result<Value, String> {
    if output.cancelled {
        return Err("Operation cancelled.".into());
    }
    if output.timed_out {
        return Err("Brig did not respond. Try again.".into());
    }
    if output.output_exceeded {
        return Err("Brig output exceeded the size limit.".into());
    }
    if output.exit_code != Some(0) {
        return Err("Brig operation failed. Check the diagnostics.".into());
    }
    let value: Value =
        serde_json::from_str(&output.stdout).map_err(|_| "The Brig response is not valid JSON.")?;
    if value["apiVersion"] != "brig.sh/v1alpha1"
        || value["kind"] != kind
        || value.get("data").is_none()
    {
        return Err("The Brig response version or type is unsupported.".into());
    }
    if kind == "Doctor"
        && !value["data"].as_array().is_some_and(|rows| {
            rows.iter().all(|row| {
                row["name"].is_string() && row["state"].is_string() && row["finding"].is_string()
            })
        })
    {
        return Err("The Brig doctor response is missing required fields.".into());
    }
    Ok(value)
}

pub fn query(
    brig: &Path,
    arguments: &[&str],
    cancel: &AtomicBool,
) -> Result<CommandOutput, String> {
    command::run(
        brig,
        &arguments
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect::<Vec<_>>(),
        None,
        &command::environment(),
        Duration::from_secs(20),
        cancel,
    )
}

fn inspect(configured: Option<String>, cancel: &AtomicBool) -> HostStatus {
    let tools = Tools {
        brig: discover("brig", configured.as_deref()),
        hull: discover("hull", None),
        homebrew: discover("brew", None),
        git: discover("git", None),
    };
    let os_version = command::run(
        Path::new("/usr/bin/sw_vers"),
        &["-productVersion".into()],
        None,
        &command::environment(),
        Duration::from_secs(2),
        cancel,
    )
    .map(|output| output.stdout.trim().to_owned())
    .unwrap_or_else(|_| "Unavailable".into());
    let mut result = HostStatus {
        architecture: std::env::consts::ARCH,
        os_version,
        tools,
        brig_status: "missing".into(),
        version: None,
        doctor: None,
        message: None,
    };
    let Some(brig) = &result.tools.brig else {
        result.message =
            Some("Brig was not found. Edit the CLI path or install it with Homebrew.".into());
        return result;
    };
    let version =
        query(brig, &["--json", "version"], cancel).and_then(|output| envelope(&output, "Version"));
    let version = match version {
        Ok(version) => version,
        Err(error) => {
            result.brig_status = "incompatible".into();
            result.message = Some(error);
            return result;
        }
    };
    result.version = Some(version.clone());
    if version["data"]["version"] != "v0.5.2" {
        result.brig_status = "incompatible".into();
        result.message =
            Some("This app was verified with Brig v0.5.2. Select a compatible version.".into());
        return result;
    }
    match query(brig, &["--json", "doctor"], cancel).and_then(|output| envelope(&output, "Doctor"))
    {
        Ok(doctor) => {
            result.doctor = Some(doctor);
            result.brig_status = "installed".into();
        }
        Err(error) => {
            result.brig_status = "error".into();
            result.message = Some(error);
        }
    }
    result
}

#[tauri::command]
pub async fn inspect_host(
    state: State<'_, Inspection>,
    cli_path: Option<String>,
) -> Result<HostStatus, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut current = state.0.lock().map_err(|error| error.to_string())?;
        if current.is_some() {
            return Err("The Mac check is already running.".into());
        }
        *current = Some(cancel.clone());
    }
    let result = tauri::async_runtime::spawn_blocking(move || inspect(cli_path, &cancel))
        .await
        .map_err(|error| error.to_string());
    *state.0.lock().map_err(|error| error.to_string())? = None;
    result
}

#[tauri::command]
pub fn cancel_inspection(state: State<'_, Inspection>) -> Result<(), String> {
    if let Some(cancel) = state.0.lock().map_err(|error| error.to_string())?.as_ref() {
        cancel.store(true, Ordering::SeqCst);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_incompatible_installations_are_distinct() {
        let missing = inspect(Some("/not-present/brig".into()), &AtomicBool::new(false));
        assert_eq!(missing.brig_status, "missing");
        let incompatible = inspect(Some("/usr/bin/true".into()), &AtomicBool::new(false));
        assert_eq!(incompatible.brig_status, "incompatible");
        assert!(discover("brig", Some("relative/path")).is_none());
    }

    #[test]
    fn malformed_doctor_and_another_api_version_are_rejected() {
        let mut output = CommandOutput {
            stdout: r#"{"apiVersion":"brig.sh/v1alpha1","kind":"Doctor","data":{}}"#.into(),
            stderr: String::new(),
            exit_code: Some(0),
            cancelled: false,
            timed_out: false,
            output_exceeded: false,
        };
        assert!(envelope(&output, "Doctor").is_err());
        output.stdout = r#"{"apiVersion":"brig.sh/v2","kind":"Doctor","data":[]}"#.into();
        assert!(envelope(&output, "Doctor").is_err());
    }
}
