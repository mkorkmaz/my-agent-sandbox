use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use std::{io, thread};

const OUTPUT_LIMIT: usize = 1_048_576;
static ACTIVE: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

struct Running;
impl Running {
    fn enter() -> Result<Self, String> {
        let mut count = ACTIVE.0.lock().map_err(|error| error.to_string())?;
        if crate::lifecycle::is_closing() {
            return Err("The app is closing.".into());
        }
        *count += 1;
        Ok(Self)
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        let mut count = ACTIVE.0.lock().unwrap_or_else(|error| error.into_inner());
        *count -= 1;
        ACTIVE.1.notify_all();
    }
}

pub fn wait_for_idle(timeout: Duration) -> Result<(), String> {
    let count = ACTIVE.0.lock().map_err(|error| error.to_string())?;
    let (count, _) = ACTIVE
        .1
        .wait_timeout_while(count, timeout, |count| *count > 0)
        .map_err(|error| error.to_string())?;
    if *count > 0 {
        return Err("A background command has not exited. Retry closing the app.".into());
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub timed_out: bool,
    pub output_exceeded: bool,
}

pub fn environment() -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for key in ["HOME", "USER", "LOGNAME", "TMPDIR"] {
        if let Ok(value) = std::env::var(key) {
            values.insert(key.into(), value);
        }
    }
    values.insert(
        "PATH".into(),
        "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".into(),
    );
    if let Some(directory) =
        crate::hull::executable().and_then(|path| path.parent().map(Path::to_owned))
    {
        let path = values.get("PATH").expect("fixed command path");
        values.insert("PATH".into(), format!("{}:{path}", directory.display()));
    }
    values.insert("LANG".into(), "en_US.UTF-8".into());
    values.insert("TERM".into(), "dumb".into());
    values.insert("DO_NOT_TRACK".into(), "1".into());
    values
}

fn read_output(mut pipe: impl Read, exceeded: Arc<AtomicBool>) -> io::Result<Vec<u8>> {
    let mut result = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let length = match pipe.read(&mut buffer) {
            Ok(0) => return Ok(result),
            Ok(length) => length,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        let available = OUTPUT_LIMIT.saturating_sub(result.len());
        result.extend_from_slice(&buffer[..length.min(available)]);
        if length > available {
            exceeded.store(true, Ordering::SeqCst);
        }
    }
}

pub fn run(
    executable: &Path,
    arguments: &[String],
    directory: Option<&Path>,
    environment: &BTreeMap<String, String>,
    timeout: Duration,
    cancellation: &AtomicBool,
) -> Result<CommandOutput, String> {
    let _running = Running::enter()?;
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let exceeded = Arc::new(AtomicBool::new(false));
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let stdout_flag = exceeded.clone();
    let stderr_flag = exceeded.clone();
    let output_reader = thread::spawn(move || read_output(stdout, stdout_flag));
    let error_reader = thread::spawn(move || read_output(stderr, stderr_flag));
    let started = Instant::now();
    let mut timed_out = false;
    let mut cancelled = false;
    let status = loop {
        let result = child.try_wait();
        if let Ok(Some(status)) = result {
            // Query commands must not leave descendants holding their output pipes.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            break Ok(status);
        }
        if let Err(error) = result {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
            break Err(error.to_string());
        }
        timed_out = started.elapsed() >= timeout;
        cancelled = cancellation.load(Ordering::SeqCst) || crate::lifecycle::is_closing();
        if timed_out || cancelled || exceeded.load(Ordering::SeqCst) {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            break child.wait().map_err(|error| error.to_string());
        }
        thread::sleep(Duration::from_millis(20));
    };
    let output = output_reader
        .join()
        .map_err(|_| "Could not read process output.")?
        .map_err(|error| error.to_string())?;
    let errors = error_reader
        .join()
        .map_err(|_| "Could not read error output.")?
        .map_err(|error| error.to_string())?;
    Ok(CommandOutput {
        stdout: String::from_utf8_lossy(&output).into_owned(),
        stderr: String::from_utf8_lossy(&errors).into_owned(),
        exit_code: status?.code(),
        cancelled,
        timed_out,
        output_exceeded: exceeded.load(Ordering::SeqCst),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn confirmed_shutdown_cancels_and_reaps_background_commands() {
        if std::env::var_os("TUGBOAT_SHUTDOWN_TEST").is_some() {
            let query = thread::spawn(|| {
                run(
                    Path::new("/bin/sleep"),
                    &["60".into()],
                    None,
                    &environment(),
                    Duration::from_secs(70),
                    &AtomicBool::new(false),
                )
                .unwrap()
            });
            while *ACTIVE.0.lock().unwrap() == 0 {
                thread::yield_now();
            }
            // Before confirmation, the same running command remains active.
            assert!(wait_for_idle(Duration::from_millis(30)).is_err());
            crate::lifecycle::set_closing_for_test(true);
            wait_for_idle(Duration::from_secs(2)).unwrap();
            assert!(query.join().unwrap().cancelled);
            assert!(Running::enter().is_err());
            return;
        }
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "command::tests::confirmed_shutdown_cancels_and_reaps_background_commands",
                "--nocapture",
            ])
            .env("TUGBOAT_SHUTDOWN_TEST", "1")
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn executable_and_arguments_with_spaces_are_not_shell_text() {
        let directory = std::env::temp_dir().join(format!("brig runner {}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let executable = directory.join("fake brig");
        std::fs::write(&executable, "#!/bin/sh\nprintf '%s\\n' \"$1\" \"${OPENAI_API_KEY-unset}\" \"${BRIG_FORWARD_ENV-unset}\"\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let result = run(
            &executable,
            &["project ; $(false) with spaces".into()],
            None,
            &environment(),
            Duration::from_secs(2),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            result.stdout,
            "project ; $(false) with spaces\nunset\nunset\n"
        );
        assert_eq!(result.exit_code, Some(0));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn timeout_and_cancellation_have_separate_results() {
        let arguments = vec!["5".into()];
        let timed = run(
            Path::new("/bin/sleep"),
            &arguments,
            None,
            &environment(),
            Duration::from_millis(30),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(timed.timed_out && !timed.cancelled);
        let cancelled = run(
            Path::new("/bin/sleep"),
            &arguments,
            None,
            &environment(),
            Duration::from_secs(1),
            &AtomicBool::new(true),
        )
        .unwrap();
        assert!(cancelled.cancelled && !cancelled.timed_out);
    }

    #[test]
    fn background_descendants_do_not_hold_the_query_open() {
        let result = run(
            Path::new("/bin/sh"),
            &["-c".into(), "sleep 5 & printf done".into()],
            None,
            &environment(),
            Duration::from_secs(1),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(result.stdout, "done");
        assert_eq!(result.exit_code, Some(0));
    }

    #[test]
    fn output_is_bounded() {
        let result = run(
            Path::new("/usr/bin/yes"),
            &[],
            None,
            &environment(),
            Duration::from_secs(2),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(result.output_exceeded);
        assert!(result.stdout.len() <= OUTPUT_LIMIT);
    }
}
