use crate::{storage::data_location, usage_store};
use std::ffi::OsString;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

mod recovery;
#[cfg(test)]
mod tests;

const SUPERVISE: &str = "--wait-for-update-relaunch";
const START_APP: &str = "--confirm-update-relaunch";
const READY: u8 = 1;
const RECORDS_UNAVAILABLE: u8 = 2;
const PREFLIGHT_LIMIT: Duration = Duration::from_secs(5);
const STARTUP_LIMIT: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(crate) struct RelaunchConfirmation(Mutex<Option<String>>);

pub(super) fn launch(executable: &Path, version: &str, records_id: &str) -> Result<Child, String> {
    let mut child = Command::new(executable)
        .args([
            SUPERVISE,
            &std::process::id().to_string(),
            version,
            records_id,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not start the Tugboat restart process. {error}"))?;
    if wait_for_signal(&mut child, PREFLIGHT_LIMIT) != Some(READY) {
        // Do not leave an unacknowledged process waiting for this app to quit.
        cancel(&mut child);
        return Err(format!("Tugboat {version} did not pass the restart check."));
    }
    Ok(child)
}

pub(super) fn cancel(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn wait_for_signal(child: &mut Child, timeout: Duration) -> Option<u8> {
    let mut output = child.stdout.take()?;
    let mut descriptor = libc::pollfd {
        fd: output.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.checked_duration_since(Instant::now())?;
        let wait = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
        let available = unsafe { libc::poll(&mut descriptor, 1, wait) };
        if available == -1 && std::io::Error::last_os_error().kind() == ErrorKind::Interrupted {
            continue;
        }
        if available != 1 || descriptor.revents & libc::POLLIN == 0 {
            return None;
        }
        let mut signal = [0];
        output.read_exact(&mut signal).ok()?;
        return Some(signal[0]);
    }
}

pub(crate) fn run_relaunch_supervisor(
    arguments: &[OsString],
    version: &str,
) -> Result<bool, String> {
    if arguments
        .first()
        .is_none_or(|argument| argument != SUPERVISE)
    {
        return Ok(false);
    }
    let [_, parent, expected_version, records_id] = arguments else {
        return Err("The app restart arguments are invalid.".into());
    };
    verify_version(expected_version, version)?;
    let records_id = parse_records_id(records_id)?;
    let parent = parent
        .to_str()
        .and_then(|process| process.parse::<libc::pid_t>().ok())
        .filter(|process| *process > 1 && *process == unsafe { libc::getppid() })
        .ok_or("The previous Tugboat process is unavailable.")?;
    let executable = tauri::utils::platform::current_exe().map_err(|error| error.to_string())?;
    let bundle = executable
        .parent()
        .filter(|macos| macos.ends_with("MacOS"))
        .and_then(Path::parent)
        .filter(|contents| contents.ends_with("Contents"))
        .and_then(Path::parent)
        .ok_or("The installed Tugboat app bundle is unavailable.")?;
    send_signal(READY)?;

    // Tauri setup needs the exclusive records lease. The old process holds it until exit.
    if wait_for_exit(parent, Duration::from_secs(15)) {
        let mut command = Command::new(&executable);
        command.args([START_APP, version, &records_id]);
        if let Err(error) = watch_startup(command, STARTUP_LIMIT) {
            recovery::show(
                bundle,
                &executable,
                version,
                &error.message,
                error.can_reopen,
            );
        }
        return Ok(true);
    }
    recovery::show(
        bundle,
        &executable,
        version,
        "The previous Tugboat process is still open. Quit it, then open Tugboat again.",
        false,
    );
    Ok(true)
}

fn wait_for_exit(parent: libc::pid_t, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while unsafe { libc::getppid() } == parent {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}

#[derive(Debug)]
struct StartupFailure {
    message: String,
    can_reopen: bool,
}

fn watch_startup(mut command: Command, timeout: Duration) -> Result<(), StartupFailure> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| StartupFailure {
            message: format!("Could not open the installed app. {error}"),
            can_reopen: true,
        })?;
    // An open copy may still be starting or may show Settings for records recovery.
    // Leave that copy running so users can quit it normally or recover their records.
    let message = match wait_for_signal(&mut child, timeout) {
        Some(READY) => return Ok(()),
        Some(RECORDS_UNAVAILABLE) =>
            "Tugboat opened but could not open the verified records and usage database. Check the Records folder in Settings.",
        _ => "The installed app did not finish starting. If it is still open, quit it before you reopen Tugboat.",
    };
    // Reopen only when the unsuccessful child exited. A running app can still hold the lease.
    Err(StartupFailure {
        message: message.into(),
        can_reopen: child.try_wait().is_ok_and(|status| status.is_some()),
    })
}

impl RelaunchConfirmation {
    pub(crate) fn from_arguments(arguments: &[OsString], version: &str) -> Result<Self, String> {
        if arguments
            .first()
            .is_none_or(|argument| argument != START_APP)
        {
            return Ok(Self::default());
        }
        let [_, expected_version, records_id] = arguments else {
            return Err("The app restart confirmation arguments are invalid.".into());
        };
        verify_version(expected_version, version)?;
        Ok(Self(Mutex::new(Some(parse_records_id(records_id)?))))
    }

    pub(crate) fn confirm(&self, app: &AppHandle) {
        let Some(expected_id) = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        else {
            return;
        };
        let app = app.clone();
        // Wait for usage storage off the window thread. Ordinary startup does not use this path.
        std::thread::spawn(move || {
            let result = app.state::<usage_store::Runtime>().wait().and_then(|_| {
                data_location::records_directory(&app)
                    .and_then(|records| verify_records_id(&expected_id, records.id()))
            });
            let signal = if result.is_ok() {
                READY
            } else {
                RECORDS_UNAVAILABLE
            };
            if let Err(error) = send_signal(signal) {
                eprintln!("Could not confirm Tugboat restart. {error}");
            }
        });
    }
}

fn verify_version(expected_version: &OsString, version: &str) -> Result<(), String> {
    if expected_version != version {
        return Err(format!("The app executable reports Tugboat {version}."));
    }
    Ok(())
}

fn parse_records_id(input: &OsString) -> Result<String, String> {
    input
        .to_str()
        .filter(|id| {
            id.is_empty() || (id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
        })
        .map(str::to_owned)
        .ok_or_else(|| "The app restart records identifier is invalid.".into())
}

fn verify_records_id(expected: &str, actual: &str) -> Result<(), String> {
    if !expected.is_empty() && expected != actual {
        return Err("Tugboat opened a different records folder.".into());
    }
    Ok(())
}

fn send_signal(signal: u8) -> Result<(), String> {
    let mut output = std::io::stdout().lock();
    output
        .write_all(&[signal])
        .and_then(|()| output.flush())
        .map_err(|error| format!("Could not send restart confirmation. {error}"))
}
