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
use tauri::{AppHandle, State};

const CHECK_VERSION: u32 = 1;

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
    #[serde(default)]
    pub check_version: u32,
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
    Ok(crate::data_location::directory(app)?
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
    if output.cancelled || output.timed_out || output.output_exceeded {
        return ("inconclusive", "The network probe was cancelled, exceeded its time limit or returned incomplete output.");
    }
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

struct NetworkScenario {
    allowed_url: Option<String>,
    denied_url: Option<String>,
    requires_allowed: bool,
    offline: bool,
}

fn network_scenario(plan: &serde_json::Value) -> Result<NetworkScenario, &'static str> {
    let data = &plan["data"];
    if data["posture"] == "offline" {
        return Ok(NetworkScenario {
            allowed_url: None,
            denied_url: Some("https://example.com".into()),
            requires_allowed: false,
            offline: true,
        });
    }
    if data["posture"] != "isolated" || data["egress"]["default"] != "deny" {
        return Err("The effective network mode or default rule is unsupported by this check.");
    }
    let egress = &data["egress"];
    if egress["deny"]
        .as_array()
        .is_some_and(|rules| !rules.is_empty())
        || (!egress["deny"].is_null() && !egress["deny"].is_array())
        || egress.as_object().is_none_or(|fields| {
            fields
                .keys()
                .any(|key| !["default", "allow", "deny"].contains(&key.as_str()))
        })
    {
        return Err("Advanced egress rules need a manual check. A suitable comparison could not be verified.");
    }
    let rules = match egress["allow"].as_array() {
        Some(rules) => rules.as_slice(),
        None if egress["allow"].is_null() => &[],
        None => return Err("The resolved allow rules could not be read."),
    };
    let mut hosts = Vec::new();
    for rule in rules {
        let host = rule["host"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .trim_end_matches('.')
            .to_ascii_lowercase();
        if !settings_host(&host) || rule.as_object().is_none_or(|fields| fields.len() != 1) {
            return Err("CIDR, wildcard or advanced rules need a manual check. A suitable comparison could not be verified.");
        }
        hosts.push(host);
    }
    let denied = ["example.com", "example.org", "www.iana.org", "www.ietf.org"]
        .into_iter()
        .find(|candidate| !hosts.iter().any(|host| host == candidate));
    Ok(NetworkScenario {
        allowed_url: hosts.first().map(|host| format!("https://{host}")),
        denied_url: denied.map(|host| format!("https://{host}")),
        requires_allowed: !hosts.is_empty(),
        offline: false,
    })
}

fn network_probes(
    plan: &serde_json::Value,
    mut run_guest: impl FnMut(&[String]) -> Result<command::CommandOutput, String>,
    mut run_host: impl FnMut(&[String]) -> Result<command::CommandOutput, String>,
) -> Result<Vec<Probe>, String> {
    let scenario = match network_scenario(plan) {
        Ok(scenario) => scenario,
        Err(message) => {
            return Ok(vec![probe(
                "Network policy",
                "Current execution plan".into(),
                "A verified comparison for the effective policy",
                message,
                "inconclusive",
            )])
        }
    };
    let mut results = Vec::new();
    let mut allowed_ok = false;
    if let Some(target) = &scenario.allowed_url {
        let output = run_guest(&curl_arguments(target))?;
        allowed_ok = connected(&output);
        results.push(probe("Allowed guest destination", target.clone(), "An HTTPS connection must succeed", if allowed_ok { "HTTPS response received." } else { "The allowed destination could not be reached. DNS, TLS, missing curl or network failure may be responsible." }, if allowed_ok { "pass" } else { "inconclusive" }));
    }
    let Some(target) = &scenario.denied_url else {
        results.push(probe(
            "Denied guest destination",
            "None".into(),
            "A comparison host outside the allow rules",
            "All built-in comparison hosts are allowed. No denied target could be verified.",
            "inconclusive",
        ));
        return Ok(results);
    };
    let arguments = curl_arguments(target);
    let host = run_host(&arguments)?;
    let guest = run_guest(&arguments)?;
    let (outcome, observed) = if scenario.requires_allowed && connected(&guest) {
        ("inconclusive", "The guest reached the comparison host. Host rules can permit shared IP addresses, so this check cannot prove that the connection was outside the policy.")
    } else {
        denial(
            &guest,
            !scenario.requires_allowed || allowed_ok,
            connected(&host),
        )
    };
    let name = if scenario.offline {
        "Offline guest network"
    } else if !scenario.requires_allowed {
        "Deny-all guest network"
    } else {
        "Denied guest destination"
    };
    results.push(probe(
        name,
        target.clone(),
        "Host connects and guest connection is denied",
        observed,
        outcome,
    ));
    Ok(results)
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
            check_version: CHECK_VERSION,
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
    results.extend(network_probes(
        &checked.plan,
        |arguments| guest(&checked, arguments),
        |arguments| {
            command::run(
                Path::new("/usr/bin/curl"),
                &arguments[1..],
                None,
                &command::environment(),
                Duration::from_secs(15),
                &AtomicBool::new(false),
            )
        },
    )?);
    let unchanged = environment::checked_project(app, identifier, Some(&checked.fingerprint))
        .is_ok_and(|current| current.project.stage == "environment-ready");
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
        check_version: CHECK_VERSION,
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
        || report.check_version != CHECK_VERSION
        || !report.passed
        || !report.cleanup
        || report.results.is_empty()
        || report.results.iter().any(|probe| probe.outcome != "pass")
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
        for output in [
            command::CommandOutput {
                timed_out: true,
                ..result(28)
            },
            command::CommandOutput {
                cancelled: true,
                ..result(6)
            },
            command::CommandOutput {
                output_exceeded: true,
                ..result(7)
            },
        ] {
            assert_eq!(denial(&output, true, true).0, "inconclusive");
        }
    }

    #[test]
    fn network_checks_select_targets_from_the_effective_mode_and_policy() {
        let plan = |posture, allow| serde_json::json!({"data":{"posture":posture,"egress":{"default":"deny","allow":allow,"deny":[]}}});
        let success = || command::CommandOutput {
            stdout: "http=200".into(),
            ..result(0)
        };
        let allow_example = plan("isolated", serde_json::json!([{"host":"example.com"}]));
        let scenario = network_scenario(&allow_example).unwrap();
        assert_eq!(scenario.allowed_url.as_deref(), Some("https://example.com"));
        assert_eq!(scenario.denied_url.as_deref(), Some("https://example.org"));
        let probes = network_probes(
            &allow_example,
            |arguments| {
                Ok(if arguments.last().unwrap() == "https://example.com" {
                    success()
                } else {
                    result(6)
                })
            },
            |_| Ok(success()),
        )
        .unwrap();
        assert!(probes.iter().all(|probe| probe.outcome == "pass"));
        let regular = plan("isolated", serde_json::json!([{"host":"api.openai.com"}]));
        let probes = network_probes(
            &regular,
            |arguments| {
                Ok(if arguments.last().unwrap() == "https://api.openai.com" {
                    success()
                } else {
                    result(7)
                })
            },
            |_| Ok(success()),
        )
        .unwrap();
        assert_eq!(probes[1].target, "https://example.com");
        assert!(probes.iter().all(|probe| probe.outcome == "pass"));
        for (posture, rules, name) in [
            ("isolated", serde_json::json!([]), "Deny-all guest network"),
            (
                "offline",
                serde_json::json!([{"host":"example.com"}]),
                "Offline guest network",
            ),
        ] {
            let current = plan(posture, rules);
            let probes = network_probes(&current, |_| Ok(result(6)), |_| Ok(success())).unwrap();
            assert_eq!(probes.len(), 1);
            assert_eq!(probes[0].name, name);
            assert_eq!(probes[0].outcome, "pass");
            assert_eq!(
                network_probes(&current, |_| Ok(result(6)), |_| Ok(result(6))).unwrap()[0].outcome,
                "inconclusive"
            );
            assert_eq!(
                network_probes(&current, |_| Ok(success()), |_| Ok(success())).unwrap()[0].outcome,
                "fail"
            );
        }
        let probes = network_probes(&regular, |_| Ok(result(60)), |_| Ok(success())).unwrap();
        assert!(probes.iter().all(|probe| probe.outcome == "inconclusive"));
        let all_hosts = plan(
            "isolated",
            serde_json::json!([{"host":"example.com"},{"host":"example.org"},{"host":"www.iana.org"},{"host":"www.ietf.org"}]),
        );
        assert_eq!(
            network_probes(
                &all_hosts,
                |_| Ok(success()),
                |_| panic!("No denied target exists")
            )
            .unwrap()
            .last()
            .unwrap()
            .outcome,
            "inconclusive"
        );
        let advanced = plan("isolated", serde_json::json!([{"cidr":"0.0.0.0/0"}]));
        assert_eq!(
            network_probes(
                &advanced,
                |_| panic!("Unproven target"),
                |_| panic!("Unproven target")
            )
            .unwrap()[0]
                .outcome,
            "inconclusive"
        );
        let shared = network_probes(&regular, |_| Ok(success()), |_| Ok(success())).unwrap();
        assert_eq!(shared[1].outcome, "inconclusive");
    }
}
