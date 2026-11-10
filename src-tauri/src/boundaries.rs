use crate::{
    command,
    environment::{self, CheckedProject},
    projects, storage,
};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Manager, State};

#[derive(Default)]
pub struct BoundaryTests(pub Arc<Mutex<()>>);

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    pub name: String,
    pub target: String,
    pub expected: String,
    pub observed: String,
    pub outcome: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub project_id: String,
    pub plan_fingerprint: String,
    pub checked_at: u64,
    pub passed: bool,
    pub cleanup: bool,
    pub results: Vec<Probe>,
    pub inner_codex: String,
}

fn probe(name: &str, target: String, expected: &str, observed: &str, outcome: &str) -> Probe {
    Probe {
        name: name.into(),
        target,
        expected: expected.into(),
        observed: observed.into(),
        outcome: outcome.into(),
    }
}
fn file(app: &AppHandle, identifier: &str) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_config_dir()
        .map_err(|error| error.to_string())?
        .join("boundaries")
        .join(format!("{identifier}.json")))
}

fn guest(checked: &CheckedProject, arguments: &[String]) -> Result<command::CommandOutput, String> {
    let mut command = vec!["sh".into(), checked.project.reference()];
    command.extend_from_slice(arguments);
    checked.brig.run(&command, Some(&checked.project.path))
}

fn curl_arguments(target: &str) -> Vec<String> {
    [
        "curl",
        "-q",
        "--noproxy",
        "*",
        "--head",
        "--connect-timeout",
        "5",
        "--max-time",
        "10",
        "--silent",
        "--show-error",
        "--output",
        "/dev/null",
        "--write-out",
        "http=%{http_code}",
        target,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
fn connected(output: &command::CommandOutput) -> bool {
    output.exit_code == Some(0)
        && !output.timed_out
        && !output.cancelled
        && !output.output_exceeded
        && output.stdout.contains("http=")
        && !output.stdout.contains("http=000")
}

fn denial(
    output: &command::CommandOutput,
    allowed: bool,
    host_reachable: bool,
) -> (&'static str, &'static str) {
    if connected(output) {
        return ("fail", "The guest connected to the denied destination.");
    }
    if !allowed {
        return (
            "inconclusive",
            "The allowed destination also failed. The network boundary is unverified.",
        );
    }
    if !host_reachable {
        return (
            "inconclusive",
            "The host could not reach the denied destination either.",
        );
    }
    match output.exit_code {
        Some(6) => ("pass", "The host reached the destination. Guest DNS was denied, consistent with the active deny policy."),
        Some(7) | Some(28) => ("pass", "The host reached the destination. The guest connection was blocked, consistent with the active deny policy."),
        Some(35) | Some(51) | Some(58) | Some(60) => ("inconclusive", "A TLS verification error does not prove a policy denial."),
        Some(125) | Some(127) => ("inconclusive", "curl was not found in the guest."),
        _ => ("inconclusive", "Could not verify the cause of the connection failure."),
    }
}

struct Canary {
    directory: PathBuf,
    outside: PathBuf,
    inside: PathBuf,
    token: String,
    device: u64,
    inode: u64,
}
impl Canary {
    fn create(project: &projects::Project) -> Result<Self, String> {
        let unique = format!(
            "{}-{}-{}",
            &project.id[..12],
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_nanos()
        );
        let directory = std::env::temp_dir().join(format!("brig-boundary-{unique}"));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|error| error.to_string())?;
        let outside = directory.join("outside-canary.txt");
        let inside = project.path.join(format!(".brig-boundary-{unique}.txt"));
        let token = format!("brig-desktop-canary-{unique}");
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&outside)
            .map_err(|error| error.to_string())?;
        output
            .write_all(token.as_bytes())
            .map_err(|error| error.to_string())?;
        let metadata = output.metadata().map_err(|error| error.to_string())?;
        Ok(Self {
            directory,
            outside,
            inside,
            token,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    fn clean(&self) -> bool {
        let outside = fs::symlink_metadata(&self.outside).is_ok_and(|metadata| {
            metadata.is_file() && metadata.dev() == self.device && metadata.ino() == self.inode
        });
        if !outside || fs::read(&self.outside).ok().as_deref() != Some(self.token.as_bytes()) {
            return false;
        }
        if self.inside.exists() {
            if !fs::symlink_metadata(&self.inside).is_ok_and(|metadata| metadata.is_file())
                || fs::read(&self.inside).ok().as_deref() != Some(self.token.as_bytes())
            {
                return false;
            }
            if fs::remove_file(&self.inside).is_err() {
                return false;
            }
        }
        fs::remove_file(&self.outside).is_ok() && fs::remove_dir(&self.directory).is_ok()
    }
}
impl Drop for Canary {
    fn drop(&mut self) {
        if self.outside.exists() {
            let _ = self.clean();
        }
    }
}

fn run(app: &AppHandle, identifier: &str) -> Result<Report, String> {
    let checked = environment::checked_project(app, identifier, None)?;
    if checked.project.stage != "environment-ready" {
        return Err("Prepare the environment before checking boundaries.".into());
    }
    storage::write(
        &file(app, identifier)?,
        &Report {
            project_id: identifier.into(),
            plan_fingerprint: checked.fingerprint.clone(),
            checked_at: projects::now(),
            passed: false,
            cleanup: false,
            results: Vec::new(),
            inner_codex: "pending-interactive-verification".into(),
        },
    )?;
    let canary = Canary::create(&checked.project)?;
    let mut results = Vec::new();
    let kernel = guest(&checked, &["uname".into(), "-s".into()])?;
    let linux = kernel.exit_code == Some(0) && kernel.stdout.trim() == "Linux";
    results.push(probe(
        "Guest kernel",
        "uname -s".into(),
        "Linux",
        kernel.stdout.trim(),
        if linux { "pass" } else { "fail" },
    ));
    let guest_project = checked.plan["data"]["project"]["guest"]
        .as_str()
        .ok_or("The guest project path is missing.")?;
    let guest_inside =
        Path::new(guest_project).join(canary.inside.file_name().ok_or("Invalid probe file.")?);
    let write = guest(
        &checked,
        &[
            "sh".into(),
            "-c".into(),
            "set -eC; umask 077; printf '%s' \"$1\" > \"$2\"; printf probe-write:ok".into(),
            "brig-boundary".into(),
            canary.token.clone(),
            guest_inside.to_string_lossy().into_owned(),
        ],
    )?;
    let write_ok = write.exit_code == Some(0)
        && write.stdout.contains("probe-write:ok")
        && fs::read(&canary.inside).ok().as_deref() == Some(canary.token.as_bytes());
    results.push(probe(
        "Project write access",
        guest_project.into(),
        "Guest writes a file and the host reads the same content",
        if write_ok {
            "File content verified on the host."
        } else {
            "Could not verify write access to the shared folder."
        },
        if write_ok { "pass" } else { "fail" },
    ));
    let outside = guest(
        &checked,
        &[
            "sh".into(),
            "-c".into(),
            "if test -e \"$1\"; then printf outside:visible; else printf outside:absent; fi".into(),
            "brig-boundary".into(),
            canary.outside.to_string_lossy().into_owned(),
        ],
    )?;
    let absent = linux && outside.exit_code == Some(0) && outside.stdout.contains("outside:absent");
    results.push(probe(
        "Host file outside the share",
        canary.outside.to_string_lossy().into_owned(),
        "The host probe file must not be visible to the guest",
        if absent {
            "The file exists on the host and is not visible to the guest."
        } else {
            "Could not verify the file boundary."
        },
        if absent { "pass" } else { "fail" },
    ));
    let allowed = checked.plan["data"]["egress"]["allow"]
        .as_array()
        .and_then(|rules| {
            rules
                .iter()
                .filter_map(|rule| rule["host"].as_str())
                .find(|host| settings_host(host))
        });
    let allowed_url = allowed.map(|host| format!("https://{host}"));
    let mut allowed_ok = false;
    if let Some(target) = &allowed_url {
        let output = guest(&checked, &curl_arguments(target))?;
        allowed_ok = connected(&output);
        let observation = match output.exit_code {
            Some(125) | Some(127) => "curl was not found in the guest.",
            Some(6) => "DNS lookup failed for the allowed destination.",
            Some(35) | Some(51) | Some(58) | Some(60) => "TLS failed for the allowed destination.",
            _ if allowed_ok => "HTTPS response received.",
            _ => "The allowed destination could not be reached.",
        };
        results.push(probe(
            "Allowed network destination",
            target.clone(),
            "An HTTPS connection must succeed",
            observation,
            if allowed_ok { "pass" } else { "inconclusive" },
        ));
    }
    if allowed_url.is_none() {
        results.push(probe(
            "Allowed network destination",
            "None".into(),
            "The policy must contain an explicit host to test",
            "No suitable host was found.",
            "inconclusive",
        ));
    }
    let denied_url = "https://example.com";
    let arguments = curl_arguments(denied_url);
    let host_output = command::run(
        Path::new("/usr/bin/curl"),
        &arguments[1..],
        None,
        &command::environment(),
        Duration::from_secs(15),
        &AtomicBool::new(false),
    )?;
    let guest_output = guest(&checked, &arguments)?;
    let (outcome, observed) = denial(&guest_output, allowed_ok, connected(&host_output));
    results.push(probe(
        "Denied network destination",
        denied_url.into(),
        "Host connects and guest connection is denied",
        observed,
        outcome,
    ));
    let unchanged =
        environment::checked_project(app, identifier, Some(&checked.fingerprint)).is_ok();
    let cleanup = canary.clean();
    if !unchanged {
        results.push(probe(
            "Policy consistency",
            checked.project.reference(),
            "The same plan must remain active",
            "The execution plan changed during the check.",
            "inconclusive",
        ));
    }
    let passed = cleanup && unchanged && results.iter().all(|result| result.outcome == "pass");
    let report = Report {
        project_id: identifier.into(),
        plan_fingerprint: checked.fingerprint,
        checked_at: projects::now(),
        passed,
        cleanup,
        results,
        inner_codex: "pending-interactive-verification".into(),
    };
    storage::write(&file(app, identifier)?, &report)?;
    Ok(report)
}

fn settings_host(host: &str) -> bool {
    crate::settings::valid_host(host)
}

pub fn require_pass(app: &AppHandle, identifier: &str, fingerprint: &str) -> Result<(), String> {
    let report = storage::read::<Report>(&file(app, identifier)?)?
        .ok_or("Complete the Brig file and network boundary checks first.")?;
    let now = projects::now();
    if report.project_id != identifier
        || !report.passed
        || !report.cleanup
        || report.plan_fingerprint != fingerprint
        || report.checked_at > now
        || now.saturating_sub(report.checked_at) > 3600
    {
        return Err("Run the Brig boundary checks again with this execution plan.".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn run_boundary_tests(
    app: AppHandle,
    state: State<'_, BoundaryTests>,
    identifier: String,
) -> Result<Report, String> {
    let lock = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = lock
            .try_lock()
            .map_err(|_| "The boundary check is already running.")?;
        run(&app, &identifier)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn copy_inner_boundary_prompt(app: AppHandle) -> Result<(), String> {
    let prompt = "Use your own command tool to test three boundaries in this conversation. Write a unique temporary file inside this project, verify that you created it, then remove it. Try creating a unique probe file under /etc and report whether the write was blocked. Use the command tool to make an HTTPS request to https://example.com with a maximum duration of 10 seconds. Report the target, expected result and observed result for each check. Wait for my response to any approval request. Do not change sandbox or permission settings to bypass a boundary.".as_bytes().to_vec();
    crate::terminal::on_main(app, move || {
        unsafe {
            copy_text(prompt.as_ptr(), prompt.len());
        }
        Ok(())
    })
    .await
}
unsafe extern "C" {
    #[link_name = "brig_terminal_copy_text"]
    fn copy_text(bytes: *const u8, length: usize);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn result(code: i32) -> command::CommandOutput {
        command::CommandOutput {
            stdout: "http=000".into(),
            stderr: String::new(),
            exit_code: Some(code),
            cancelled: false,
            timed_out: false,
            output_exceeded: false,
        }
    }
    #[test]
    fn network_failures_are_not_automatically_policy_passes() {
        assert_eq!(denial(&result(6), false, true).0, "inconclusive");
        assert_eq!(denial(&result(6), true, false).0, "inconclusive");
        assert_eq!(denial(&result(60), true, true).0, "inconclusive");
        assert_eq!(denial(&result(127), true, true).0, "inconclusive");
        assert_eq!(denial(&result(6), true, true).0, "pass");
        let connected = command::CommandOutput {
            stdout: "http=200".into(),
            ..result(0)
        };
        assert_eq!(denial(&connected, true, true).0, "fail");
    }
}
