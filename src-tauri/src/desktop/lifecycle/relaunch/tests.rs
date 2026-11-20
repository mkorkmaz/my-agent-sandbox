use super::*;

#[test]
fn a_process_that_exits_before_the_handoff_reports_a_restart_failure() {
    let result = launch(Path::new("/usr/bin/false"), "0.4.1", "");
    assert!(result
        .unwrap_err()
        .contains("did not pass the restart check"));
}

#[test]
fn restart_handoff_checks_the_actual_binary_version_and_records_id() {
    let version = "0.4.1";
    assert!(!run_relaunch_supervisor(&[], version).unwrap());
    assert!(run_relaunch_supervisor(&[SUPERVISE.into()], version).is_err());
    assert!(run_relaunch_supervisor(
        &[SUPERVISE.into(), "0".into(), version.into(), "".into()],
        version
    )
    .is_err());
    assert!(RelaunchConfirmation::from_arguments(
        &[START_APP.into(), "0.4.2".into(), "".into()],
        version,
    )
    .is_err());
    assert!(RelaunchConfirmation::from_arguments(
        &[START_APP.into(), version.into(), "invalid".into()],
        version,
    )
    .is_err());
    let id = "a".repeat(32);
    assert!(RelaunchConfirmation::from_arguments(
        &[START_APP.into(), version.into(), id.clone().into()],
        version,
    )
    .is_ok());
    assert!(verify_records_id(&id, &id).is_ok());
    assert!(verify_records_id(&id, &"b".repeat(32)).is_err());
}

const HANDOFF_FIXTURE: &str = "TUGBOAT_TEST_RESTART_FIXTURE";
const HANDOFF_TEST: &str = "desktop::lifecycle::relaunch::tests::handoff_fixture_process";

#[test]
fn a_restart_confirms_success_or_failure_after_the_previous_app_releases_its_lease() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("tugboat-restart-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    for (name, script) in [
        ("started", "printf '\\001'"),
        ("crashed", "exit 9"),
        ("records-unavailable", "printf '\\002'"),
    ] {
        let directory = root.join(name);
        std::fs::create_dir(&directory).unwrap();
        let status = fixture(&directory)
            .env("TUGBOAT_TEST_RESTART_SCRIPT", script)
            .status()
            .unwrap();
        assert!(status.success());
        assert_fixture_marker(&directory.join("result"), name);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_fixture_process() {
    let Some(directory) = std::env::var_os(HANDOFF_FIXTURE) else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    let records = directory.join("records.json");
    let preflight = directory.join("preflight");
    let Ok(parent) = std::env::var("TUGBOAT_TEST_RESTART_PARENT") else {
        let _lease = crate::storage::lock(&records).unwrap();
        fixture(&directory)
            .env(
                "TUGBOAT_TEST_RESTART_PARENT",
                std::process::id().to_string(),
            )
            .spawn()
            .unwrap();
        assert_fixture_marker(&preflight, "waiting");
        return;
    };
    let parent = parent.parse().unwrap();
    assert_eq!(unsafe { libc::getppid() }, parent);
    let descriptor = std::fs::File::open(directory.join("records.json.lock")).unwrap();
    assert_eq!(
        unsafe { libc::flock(descriptor.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        -1
    );
    std::fs::write(&preflight, "waiting").unwrap();
    assert!(wait_for_exit(parent, PREFLIGHT_LIMIT));
    assert_eq!(
        unsafe { libc::flock(descriptor.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );

    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(std::env::var("TUGBOAT_TEST_RESTART_SCRIPT").unwrap());
    let result = match watch_startup(command, PREFLIGHT_LIMIT) {
        Ok(()) => "started",
        Err(error) if error.message.contains("verified records") => "records-unavailable",
        Err(error) => {
            assert!(error.message.contains("did not finish starting"));
            "crashed"
        }
    };
    std::fs::write(directory.join("result"), result).unwrap();
}

fn fixture(directory: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", HANDOFF_TEST])
        .env(HANDOFF_FIXTURE, directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    command
}

fn assert_fixture_marker(path: &Path, expected: &str) {
    let deadline = Instant::now() + PREFLIGHT_LIMIT;
    while Instant::now() < deadline {
        if std::fs::read_to_string(path).is_ok_and(|contents| contents == expected) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "Missing restart fixture result {expected} at {}",
        path.display()
    );
}
