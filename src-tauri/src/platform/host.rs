use crate::platform::command::{self, CommandOutput};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::State;

pub const SUPPORTED_BRIG_VERSION: &str = "v0.5.2";

#[derive(Default)]
pub struct Inspection(Mutex<InspectionState>);

#[derive(Default)]
enum InspectionState {
    #[default]
    Idle,
    Checking(Arc<AtomicBool>),
    Cancelling(Arc<AtomicBool>),
}

impl InspectionState {
    fn begin(&mut self, token: Arc<AtomicBool>) -> Result<(), String> {
        if !matches!(self, Self::Idle) {
            return Err("The Mac check is already running.".into());
        }
        *self = Self::Checking(token);
        Ok(())
    }

    fn cancel(&mut self) {
        let token = match self {
            Self::Checking(token) | Self::Cancelling(token) => token.clone(),
            Self::Idle => return,
        };
        token.store(true, Ordering::SeqCst);
        *self = Self::Cancelling(token);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tools {
    brig: Option<PathBuf>,
    hull: Option<PathBuf>,
    hvi: Option<PathBuf>,
    cosign: Option<PathBuf>,
    mke2fs: Option<PathBuf>,
    debugfs: Option<PathBuf>,
    e2fsck: Option<PathBuf>,
    git: Option<PathBuf>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatus {
    architecture: &'static str,
    os_version: String,
    tools: Tools,
    brig_source: &'static str,
    brig_status: String,
    version: Option<Value>,
    doctor: Option<Value>,
    message: Option<String>,
}

impl HostStatus {
    pub fn ready(&self) -> bool { self.brig_status == "installed" }
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
        return crate::platform::hull::executable();
    }
    if name == "brig" {
        return crate::platform::runtime::tool("brig");
    }
    ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"]
        .iter()
        .map(|directory| Path::new(directory).join(name))
        .find(|path| executable(path))
}

fn executable(path: &Path) -> bool {
    crate::platform::runtime::executable(path)
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
        &crate::desktop::settings::Settings::default().environment(),
        Duration::from_secs(20),
        cancel,
    )
}

enum InspectionFailure {
    Cancelled,
    Timeout,
    Invalid(String),
    Failed(String),
}
impl InspectionFailure {
    fn status(&self) -> &str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Timeout => "timed-out",
            Self::Invalid(_) => "invalid-response",
            Self::Failed(_) => "error",
        }
    }
    fn message(self) -> String {
        match self {
            Self::Cancelled => "The Mac check was cancelled. Run it again when ready.".into(),
            Self::Timeout => "The Mac check timed out. Try checking again.".into(),
            Self::Invalid(message) | Self::Failed(message) => message,
        }
    }
}

fn inspection_output(
    output: Result<CommandOutput, String>,
    kind: &str,
) -> Result<Value, InspectionFailure> {
    let output = output.map_err(InspectionFailure::Failed)?;
    if output.cancelled {
        return Err(InspectionFailure::Cancelled);
    }
    if output.timed_out {
        return Err(InspectionFailure::Timeout);
    }
    if output.output_exceeded {
        return Err(InspectionFailure::Invalid(
            "The Brig response exceeded the size limit.".into(),
        ));
    }
    if output.exit_code != Some(0) {
        return Err(InspectionFailure::Failed(
            "The Brig check command failed. Review the CLI path and retry.".into(),
        ));
    }
    let value = envelope(&output, kind).map_err(InspectionFailure::Invalid)?;
    if kind == "Version"
        && !value["data"]["version"]
            .as_str()
            .is_some_and(|version| !version.trim().is_empty())
    {
        return Err(InspectionFailure::Invalid(
            "The Brig version response is missing its version.".into(),
        ));
    }
    Ok(value)
}

fn inspect(configured: Option<String>, cancel: &AtomicBool) -> HostStatus {
    inspect_with_runtime(configured, cancel, crate::platform::runtime::validate)
}

pub fn runtime_check() -> HostStatus {
    inspect(None, &AtomicBool::new(false))
}

fn inspect_with_runtime(
    configured: Option<String>,
    cancel: &AtomicBool,
    validate_runtime: impl FnOnce() -> Result<(), String>,
) -> HostStatus {
    let tools = Tools {
        brig: discover("brig", configured.as_deref()),
        hull: discover("hull", None),
        hvi: crate::platform::runtime::tool("hvi"),
        cosign: crate::platform::runtime::tool("cosign"),
        mke2fs: crate::platform::runtime::tool("mke2fs"),
        debugfs: crate::platform::runtime::tool("debugfs"),
        e2fsck: crate::platform::runtime::tool("e2fsck"),
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
        brig_source: match configured {
            Some(_) => "custom",
            None => match crate::platform::runtime::directory() {
                Some(directory) if directory.file_name().is_some_and(|name| name == "MacOS") => "bundled",
                _ => "development",
            },
        },
        brig_status: "missing".into(),
        version: None,
        doctor: None,
        message: None,
    };
    let Some(brig) = &result.tools.brig else {
        result.message =
            Some(match configured {
                Some(_) => "The selected Brig executable is unavailable. Clear or update the CLI path in Settings.",
                None => "The bundled Brig executable is unavailable. Reinstall Tugboat or prepare the development runtime.",
            }.into());
        return result;
    };
    if let Err(error) = validate_runtime() {
        result.brig_status = "error".into();
        result.message = Some(error);
        return result;
    }
    let version = inspection_output(query(brig, &["--json", "version"], cancel), "Version");
    let version = match version {
        Ok(version) => version,
        Err(error) => {
            result.brig_status = error.status().into();
            result.message = Some(error.message());
            return result;
        }
    };
    result.version = Some(version.clone());
    if version["data"]["version"] != SUPPORTED_BRIG_VERSION {
        result.brig_status = "incompatible".into();
        result.message =
            Some("This app was verified with Brig v0.5.2. Select a compatible version.".into());
        return result;
    }
    match inspection_output(query(brig, &["--json", "doctor"], cancel), "Doctor") {
        Ok(doctor) => {
            result.doctor = Some(doctor);
            result.brig_status = "installed".into();
        }
        Err(error) => {
            result.brig_status = error.status().into();
            result.message = Some(error.message());
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
        current.begin(cancel.clone())?;
    }
    let result = tauri::async_runtime::spawn_blocking(move || inspect(cli_path, &cancel))
        .await
        .map_err(|error| error.to_string());
    *state.0.lock().map_err(|error| error.to_string())? = InspectionState::Idle;
    result
}

#[tauri::command]
pub fn cancel_inspection(state: State<'_, Inspection>) -> Result<(), String> {
    state.0.lock().map_err(|error| error.to_string())?.cancel();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn inspection_preserves_cancellation_timeout_and_invalid_response_causes() {
        for (cancelled, timed_out, body, expected) in [
            (true, false, "", "cancelled"),
            (false, true, "", "timed-out"),
            (false, false, "not json", "invalid-response"),
            (
                false,
                false,
                r#"{"apiVersion":"brig.sh/v1alpha1","kind":"Version","data":{}}"#,
                "invalid-response",
            ),
        ] {
            let result = inspection_output(
                Ok(CommandOutput {
                    stdout: body.into(),
                    stderr: String::new(),
                    exit_code: Some(0),
                    cancelled,
                    timed_out,
                    output_exceeded: false,
                }),
                "Version",
            );
            assert_eq!(result.err().unwrap().status(), expected);
        }
    }

    #[test]
    fn a_cancelled_slow_version_check_can_be_retried_and_only_a_known_mismatch_is_incompatible() {
        use std::fs;
        let root =
            std::env::temp_dir().join(format!("tugboat-inspect-cancel-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let executable = root.join("brig");
        fs::write(
            &executable,
            r#"#!/bin/sh
root=${0%/*}
if test "$2" = version; then
  if test -e "$root/slow"; then : > "$root/started"; /bin/sleep 60; fi
  cat "$root/version.json"
else
  printf '{"apiVersion":"brig.sh/v1alpha1","kind":"Doctor","data":[]}'
fi
"#,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let version = |value: &str| {
            serde_json::json!({"apiVersion":"brig.sh/v1alpha1","kind":"Version","data":{"version":value}}).to_string()
        };
        fs::write(root.join("version.json"), version(SUPPORTED_BRIG_VERSION)).unwrap();
        fs::write(root.join("slow"), "1").unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let stopping = cancel.clone();
        let path = executable.to_string_lossy().into_owned();
        let worker = std::thread::spawn(move || inspect_with_runtime(Some(path), &stopping, || Ok(())));
        let started = std::time::Instant::now();
        while !root.join("started").is_file() && started.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(5));
        }
        cancel.store(true, Ordering::SeqCst);
        let result = worker.join().unwrap();
        assert_eq!(result.brig_status, "cancelled");
        assert!(root.join("started").is_file());
        fs::remove_file(root.join("slow")).unwrap();
        let check = || {
            inspect_with_runtime(
                Some(executable.to_string_lossy().into_owned()),
                &AtomicBool::new(false),
                || Ok(()),
            )
        };
        assert_eq!(check().brig_status, "installed");
        fs::write(root.join("version.json"), version("v9.9.9")).unwrap();
        assert_eq!(check().brig_status, "incompatible");
        fs::write(root.join("version.json"), "not json").unwrap();
        assert_eq!(check().brig_status, "invalid-response");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_and_incompatible_installations_are_distinct() {
        let missing = inspect(Some("/not-present/brig".into()), &AtomicBool::new(false));
        assert_eq!(missing.brig_status, "missing");
        let incompatible = inspect_with_runtime(Some("/usr/bin/true".into()), &AtomicBool::new(false), || Ok(()));
        assert_eq!(incompatible.brig_status, "invalid-response");
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

    #[test]
    fn doctor_findings_from_an_empty_guest_cache_do_not_hide_local_runtime_readiness() {
        let mut output = CommandOutput {
            stdout: r#"{"apiVersion":"brig.sh/v1alpha1","kind":"Doctor","data":[{"name":"boot","state":"!!","finding":"Boot assets need a download"}]}"#.into(),
            stderr: String::new(), exit_code: Some(0), cancelled: false,
            timed_out: false, output_exceeded: false,
        };
        assert!(inspection_output(Ok(output), "Doctor").is_ok());
        for exit_code in [4, 6] {
            output = CommandOutput {
                stdout: r#"{"apiVersion":"brig.sh/v1alpha1","kind":"Doctor","data":[{"name":"runtime","state":"!!","finding":"The runtime or secret store is unavailable"}]}"#.into(),
                stderr: String::new(), exit_code: Some(exit_code), cancelled: false,
                timed_out: false, output_exceeded: false,
            };
            assert_eq!(inspection_output(Ok(output), "Doctor").err().unwrap().status(), "error");
        }
    }

    #[test]
    fn default_brig_selection_uses_the_bundle_and_invalid_overrides_do_not_fall_back() {
        assert_eq!(
            discover("brig", None),
            crate::platform::runtime::tool("brig")
        );
        assert!(discover("brig", Some("/missing/custom/brig")).is_none());
    }

    #[test]
    fn damaged_runtime_is_rejected_before_the_selected_cli_runs() {
        let root = std::env::temp_dir().join(format!("tugboat-preflight-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let executable = root.join("brig");
        std::fs::write(&executable, "#!/bin/sh\ntouch \"$0.ran\"\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let result = inspect_with_runtime(Some(executable.to_string_lossy().into_owned()),
            &AtomicBool::new(false), || Err("Damaged fixture runtime".into()));
        assert_eq!(result.brig_status, "error");
        assert_eq!(result.message.as_deref(), Some("Damaged fixture runtime"));
        assert!(!root.join("brig.ran").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
