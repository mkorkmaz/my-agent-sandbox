use super::*;
use crate::usage_store::Store;
use std::os::unix::fs::symlink;
use std::process::{Command, Stdio};

struct Fixture {
    root: PathBuf,
    default: PathBuf,
    legacy: PathBuf,
}
impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "tugboat-sqlite-location-{name}-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        create_directory(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        Self {
            default: root.join("default"),
            legacy: root.join("legacy"),
            root,
        }
    }
    fn open(&self) -> DataLocation {
        DataLocation::open(self.default.clone(), self.legacy.clone())
    }
    fn seed(&self, location: &DataLocation) -> RecordsDirectory {
        let saved = location.saved.lock().unwrap();
        let current = saved.location.as_ref().unwrap();
        let records = RecordsDirectory {
            directory: current.directory.clone(),
            id: current.directory_id.clone(),
        };
        let store = Store::create_staging(&records, &records.path().join("usage.sqlite3")).unwrap();
        store.connection.execute("INSERT INTO usage_environments(project_id,environment_id,agent_kind) VALUES(?1,?2,'codex')", ["1".repeat(32), "2".repeat(32)]).unwrap();
        store.close().unwrap();
        records.activate_sqlite().unwrap();
        create_directory(&records.path().join("conversation-ledgers")).unwrap();
        fs::write(
            records.path().join("conversation-ledgers/original.json"),
            b"original usage fixture\n",
        )
        .unwrap();
        fs::write(
            records
                .path()
                .join("conversation-ledgers/original.json.v1.backup"),
            b"immutable fixture backup\0\n",
        )
        .unwrap();
        fs::write(
            records.path().join("interface.json"),
            b"unrelated fixture settings\n",
        )
        .unwrap();
        records
    }
}

#[test]
fn reopening_a_registered_custom_folder_repairs_permissions_before_usage_activation() {
    let fixture = Fixture::new("reopen-custom-permissions");
    let custom = fixture.root.join("user-records");
    create_directory(&custom).unwrap();
    let initial = fixture.open();
    initial.schedule(&custom, false).unwrap();
    drop(initial);
    let moved = fixture.open();
    assert_eq!(moved.status().unwrap().directory.as_ref(), Some(&custom));
    let id = marker(&custom).unwrap().id;
    drop(moved);
    let settings = custom.join("interface.json");
    fs::write(&settings, b"{\"sidebarWidth\":371}").unwrap();
    for _ in 0..2 {
        fs::set_permissions(&custom, fs::Permissions::from_mode(0o755)).unwrap();
        let reopened = fixture.open();
        let records = RecordsDirectory::fixture_existing(&custom, &id);
        crate::usage_store::importer::activate(&records).unwrap();
        Store::open(&records).unwrap().close().unwrap();
        assert_eq!(fs::metadata(&custom).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::read(&settings).unwrap(), b"{\"sidebarWidth\":371}");
        assert!(reopened.status().unwrap().message.is_none());
    }
}

#[test]
fn an_unrelated_folder_keeps_its_permissions_and_a_missing_folder_is_not_created() {
    let fixture = Fixture::new("registered-permissions");
    let folder = fixture.root.join("other-dataset");
    create_directory(&folder).unwrap();
    let records = RecordsDirectory::fixture(&folder);
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(private_records_directory(&folder, &"b".repeat(32)).is_err());
    assert_eq!(fs::metadata(&folder).unwrap().mode() & 0o777, 0o755);
    let alias = fixture.root.join("alias");
    symlink(&folder, &alias).unwrap();
    assert!(private_records_directory(&alias, records.id()).is_err());
    assert_eq!(fs::metadata(&folder).unwrap().mode() & 0o777, 0o755);
    let missing = fixture.root.join("offline");
    assert!(private_records_directory(&missing, records.id()).is_err());
    assert!(!missing.exists());
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn a_closed_sqlite_dataset_moves_out_and_back_with_its_marker_and_originals() {
    let fixture = Fixture::new("round-trip");
    let location = fixture.open();
    let records = fixture.seed(&location);
    let before = fs::read(records.path().join("usage.sqlite3")).unwrap();
    let drive = fixture.root.join("drive");
    create_directory(&drive).unwrap();
    fs::set_permissions(&drive, fs::Permissions::from_mode(0o755)).unwrap();
    location.schedule(&drive, false).unwrap();
    drop(location);
    let moved = fixture.open();
    assert_eq!(moved.status().unwrap().directory, Some(drive.clone()));
    assert!(moved.status().unwrap().message.is_none());
    assert_eq!(
        fs::metadata(&drive).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(marker(&drive).unwrap().schema_version, 2);
    assert_eq!(marker(&drive).unwrap().id, records.id());
    assert_eq!(fs::read(drive.join("usage.sqlite3")).unwrap(), before);
    assert_eq!(
        fs::read(drive.join("conversation-ledgers/original.json.v1.backup")).unwrap(),
        b"immutable fixture backup\0\n"
    );
    assert_eq!(
        fs::read(drive.join("interface.json")).unwrap(),
        b"unrelated fixture settings\n"
    );
    assert_eq!(
        fs::metadata(drive.join("usage.sqlite3"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    moved.schedule(&fixture.default, false).unwrap();
    drop(moved);
    let restored = fixture.open();
    assert_eq!(
        restored.status().unwrap().directory,
        Some(fixture.default.clone())
    );
    assert!(restored.status().unwrap().message.is_none());
    assert_eq!(marker(&fixture.default).unwrap().schema_version, 2);
    assert_eq!(
        fs::read(fixture.default.join("usage.sqlite3")).unwrap(),
        before
    );
}

#[test]
fn a_missing_or_linked_active_database_never_creates_a_replacement() {
    for case in ["missing", "link"] {
        let fixture = Fixture::new(case);
        let location = fixture.open();
        let records = fixture.seed(&location);
        let database = records.path().join("usage.sqlite3");
        let preserved = fixture.root.join("preserved.sqlite3");
        fs::rename(&database, &preserved).unwrap();
        if case == "link" {
            symlink(&preserved, &database).unwrap();
        }
        let pointer = fs::read(&location.pointer).unwrap();
        let drive = fixture.root.join("drive");
        create_directory(&drive).unwrap();
        assert!(location.schedule(&drive, false).is_err());
        assert_eq!(fs::read(&location.pointer).unwrap(), pointer);
        assert!(fs::read_dir(&drive).unwrap().next().is_none());
        assert!(preserved.is_file());
        if case == "missing" {
            assert!(!database.exists());
        }
    }
}

#[test]
fn corrupt_or_foreign_databases_block_the_pending_move_and_preserve_the_pointer() {
    for case in ["corrupt", "foreign"] {
        let fixture = Fixture::new(case);
        let location = fixture.open();
        let records = fixture.seed(&location);
        let database = records.path().join("usage.sqlite3");
        if case == "foreign" {
            let store = Store::open(&records).unwrap();
            store
                .connection
                .execute(
                    "UPDATE store_identity SET records_directory_id=?1",
                    ["f".repeat(32)],
                )
                .unwrap();
            drop(store);
        }
        if case == "corrupt" {
            let mut bytes = fs::read(&database).unwrap();
            bytes[..16].fill(0xff);
            fs::write(&database, &bytes).unwrap();
        }
        let preserved = fs::read(&database).unwrap();
        let drive = fixture.root.join("drive");
        create_directory(&drive).unwrap();
        location.schedule(&drive, false).unwrap();
        let pointer = fs::read(&location.pointer).unwrap();
        drop(location);
        let blocked = fixture.open();
        assert_eq!(
            blocked.status().unwrap().directory,
            Some(fixture.default.clone())
        );
        assert!(blocked.status().unwrap().message.is_some());
        assert_eq!(fs::read(&blocked.pointer).unwrap(), pointer);
        assert_eq!(fs::read(&database).unwrap(), preserved);
        assert!(fs::read_dir(&drive).unwrap().next().is_none());
    }
}

#[test]
fn sqlite_move_process() {
    let Some(root) = std::env::var_os("TUGBOAT_SQLITE_LOCATION_FIXTURE") else {
        return;
    };
    let root = PathBuf::from(root);
    let location = DataLocation::open(root.join("default"), root.join("legacy"));
    assert!(location.status().unwrap().available);
    assert!(location.status().unwrap().message.is_none());
}

#[test]
fn interrupted_sqlite_moves_recover_before_copy_after_target_and_after_pointer_commit() {
    for phase in ["before-copy", "target-complete", "pointer-committed"] {
        let fixture = Fixture::new(phase);
        let location = fixture.open();
        let records = fixture.seed(&location);
        let before = fs::read(records.path().join("usage.sqlite3")).unwrap();
        let drive = fixture.root.join("drive");
        create_directory(&drive).unwrap();
        location.schedule(&drive, false).unwrap();
        let pointer = location.pointer.clone();
        drop(location);
        let stopped = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "storage::data_location::sqlite_tests::sqlite_move_process",
                "--nocapture",
            ])
            .env("TUGBOAT_SQLITE_LOCATION_FIXTURE", &fixture.root)
            .env("TUGBOAT_SQLITE_MOVE_POINTER", &pointer)
            .env("TUGBOAT_SQLITE_MOVE_PHASE", phase)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert_eq!(stopped.code(), Some(87), "checkpoint {phase}");
        let recovered = fixture.open();
        assert!(
            recovered.status().unwrap().message.is_none(),
            "{phase}: {:?}",
            recovered.status().unwrap().message
        );
        assert_eq!(recovered.status().unwrap().directory, Some(drive.clone()));
        assert_eq!(fs::read(drive.join("usage.sqlite3")).unwrap(), before);
        assert_eq!(marker(&drive).unwrap().schema_version, 2);
        assert_eq!(
            fs::read(drive.join("conversation-ledgers/original.json")).unwrap(),
            b"original usage fixture\n"
        );
    }
}

#[test]
fn a_sqlite_dataset_reconnects_at_a_new_path_without_reimport() {
    let fixture = Fixture::new("reconnect");
    let location = fixture.open();
    let records = fixture.seed(&location);
    let before = fs::read(records.path().join("usage.sqlite3")).unwrap();
    let drive = fixture.root.join("drive");
    create_directory(&drive).unwrap();
    location.schedule(&drive, false).unwrap();
    drop(location);
    let moved = fixture.open();
    let reconnected = fixture.root.join("reconnected");
    fs::rename(&drive, &reconnected).unwrap();
    drop(moved);
    let offline = fixture.open();
    assert!(!offline.status().unwrap().available);
    assert!(!drive.exists());
    offline.schedule(&reconnected, true).unwrap();
    drop(offline);
    let located = fixture.open();
    assert!(located.status().unwrap().available);
    assert!(located.status().unwrap().message.is_none());
    assert_eq!(
        located.status().unwrap().directory,
        Some(reconnected.clone())
    );
    assert_eq!(fs::read(reconnected.join("usage.sqlite3")).unwrap(), before);
    assert_eq!(marker(&reconnected).unwrap().schema_version, 2);
}

#[test]
fn sqlite_hot_journal_process() {
    let Some(root) = std::env::var_os("TUGBOAT_SQLITE_HOT_JOURNAL_FIXTURE") else {
        return;
    };
    let root = PathBuf::from(root);
    let directory = root.join("default");
    let records = RecordsDirectory {
        id: marker(&directory).unwrap().id,
        directory,
    };
    let store = Store::open(&records).unwrap();
    store.connection.execute_batch("PRAGMA cache_size=1; BEGIN IMMEDIATE; UPDATE usage_environments SET warnings_json='[\"uncommitted fixture\"]'").unwrap();
    for index in 3..2000 {
        store.connection.execute("INSERT INTO usage_environments(project_id,environment_id,agent_kind,warnings_json) VALUES(?1,?2,'codex','[]')", [format!("{index:032x}"), format!("{index:032x}")]).unwrap();
    }
    std::process::exit(87);
}

#[test]
fn hot_rollback_journals_recover_before_the_database_is_copied() {
    let fixture = Fixture::new("hot-journal");
    let location = fixture.open();
    let records = fixture.seed(&location);
    let drive = fixture.root.join("drive");
    create_directory(&drive).unwrap();
    location.schedule(&drive, false).unwrap();
    drop(location);
    let stopped = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "storage::data_location::sqlite_tests::sqlite_hot_journal_process",
        ])
        .env("TUGBOAT_SQLITE_HOT_JOURNAL_FIXTURE", &fixture.root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(stopped.code(), Some(87));
    assert!(
        records
            .path()
            .join("usage.sqlite3-journal")
            .metadata()
            .unwrap()
            .len()
            > 512
    );
    let recovered = fixture.open();
    assert!(recovered.status().unwrap().message.is_none());
    assert_eq!(recovered.status().unwrap().directory, Some(drive.clone()));
    assert!(!drive.join("usage.sqlite3-journal").exists());
    let copied = RecordsDirectory {
        directory: drive,
        id: records.id().to_owned(),
    };
    let store = Store::open(&copied).unwrap();
    let (count, warnings): (i64, String) = store
        .connection
        .query_row(
            "SELECT count(*), min(warnings_json) FROM usage_environments",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(warnings, "[]");
    store.integrity_check().unwrap();
    store.close().unwrap();
}

#[test]
fn a_foreign_database_in_a_partial_target_is_never_overwritten() {
    let fixture = Fixture::new("foreign-partial-target");
    let location = fixture.open();
    let records = fixture.seed(&location);
    let drive = fixture.root.join("drive");
    create_directory(&drive).unwrap();
    location.schedule(&drive, false).unwrap();
    storage::write(
        &drive.join(MARKER),
        &DirectoryMarker {
            schema_version: 2,
            id: records.id().to_owned(),
            state: MarkerState::Incomplete,
        },
    )
    .unwrap();
    copy_record(
        &records.path().join("usage.sqlite3"),
        &drive.join("usage.sqlite3"),
    )
    .unwrap();
    let foreign = rusqlite::Connection::open(drive.join("usage.sqlite3")).unwrap();
    foreign
        .execute(
            "UPDATE store_identity SET records_directory_id=?1",
            ["f".repeat(32)],
        )
        .unwrap();
    foreign.close().unwrap();
    let before = fs::read(drive.join("usage.sqlite3")).unwrap();
    let pointer = fs::read(&location.pointer).unwrap();
    drop(location);
    let blocked = fixture.open();
    assert!(blocked.status().unwrap().message.is_some());
    assert_eq!(fs::read(drive.join("usage.sqlite3")).unwrap(), before);
    assert_eq!(fs::read(&blocked.pointer).unwrap(), pointer);
    assert!(records.path().join("usage.sqlite3").exists());
}

#[test]
fn sqlite_lease_process() {
    let Some(root) = std::env::var_os("TUGBOAT_SQLITE_LEASE_FIXTURE") else {
        return;
    };
    let root = PathBuf::from(root);
    let location = DataLocation::open(root.join("default"), root.join("legacy"));
    assert!(location._lease.is_some());
    fs::write(root.join("lease-held"), b"ready").unwrap();
    std::thread::sleep(std::time::Duration::from_secs(20));
}

#[test]
fn a_second_instance_cannot_open_or_migrate_the_owned_records() {
    let fixture = Fixture::new("second-instance");
    let location = fixture.open();
    let records = fixture.seed(&location);
    let pointer = fs::read(&location.pointer).unwrap();
    let database = fs::read(records.path().join("usage.sqlite3")).unwrap();
    drop(location);
    let mut owner = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "storage::data_location::sqlite_tests::sqlite_lease_process",
        ])
        .env("TUGBOAT_SQLITE_LEASE_FIXTURE", &fixture.root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !fixture.root.join("lease-held").exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(fixture.root.join("lease-held").exists());
    let second = fixture.open();
    owner.kill().unwrap();
    owner.wait().unwrap();
    assert!(second._lease.is_none());
    assert!(!second.status().unwrap().available);
    assert!(!second.status().unwrap().can_change);
    assert_eq!(fs::read(fixture.default.join(POINTER)).unwrap(), pointer);
    assert_eq!(
        fs::read(records.path().join("usage.sqlite3")).unwrap(),
        database
    );
}

#[test]
fn linked_rollback_journals_block_a_move_without_changing_their_targets() {
    let fixture = Fixture::new("linked-journal");
    let location = fixture.open();
    let records = fixture.seed(&location);
    let drive = fixture.root.join("drive");
    create_directory(&drive).unwrap();
    location.schedule(&drive, false).unwrap();
    let external = fixture.root.join("keep-fixture-bytes");
    fs::write(&external, b"unrelated generated fixture\n").unwrap();
    symlink(&external, records.path().join("usage.sqlite3-journal")).unwrap();
    let pointer = fs::read(&location.pointer).unwrap();
    drop(location);
    let blocked = fixture.open();
    assert!(blocked.status().unwrap().message.is_some());
    assert_eq!(fs::read(&blocked.pointer).unwrap(), pointer);
    assert_eq!(
        fs::read(&external).unwrap(),
        b"unrelated generated fixture\n"
    );
    assert!(fs::read_dir(&drive).unwrap().next().is_none());
}

#[test]
fn bootstrap_tightens_only_the_app_owned_directories_and_preserves_their_files() {
    let fixture = Fixture::new("private-bootstrap");
    for directory in [&fixture.default, &fixture.legacy] {
        create_directory(directory).unwrap();
        fs::set_permissions(directory, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            directory.join("preserved-fixture"),
            b"unchanged app fixture\n",
        )
        .unwrap();
    }
    prepare_bootstrap_directories(&fixture.default, &fixture.legacy).unwrap();
    for directory in [&fixture.default, &fixture.legacy] {
        assert_eq!(
            fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::read(directory.join("preserved-fixture")).unwrap(),
            b"unchanged app fixture\n"
        );
    }
}

#[test]
fn bootstrap_does_not_follow_a_link_or_change_the_linked_folder() {
    let fixture = Fixture::new("linked-bootstrap");
    let outside = fixture.root.join("outside");
    create_directory(&outside).unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(outside.join("preserved-fixture"), b"unrelated fixture\n").unwrap();
    symlink(&outside, &fixture.default).unwrap();
    let blocked = fixture.open();
    assert!(!blocked.status().unwrap().available);
    assert!(!blocked.status().unwrap().can_change);
    assert_eq!(
        fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(
        fs::read(outside.join("preserved-fixture")).unwrap(),
        b"unrelated fixture\n"
    );
    assert!(!outside.join(POINTER).exists());
    assert!(!outside.join(format!("{POINTER}.lock")).exists());
}

fn write_native_usage(home: &Path, append: bool) {
    use std::io::Write;
    let directory = home.join(".codex/sessions");
    fs::create_dir_all(&directory).unwrap();
    let source = directory.join("rollout-reconnect.jsonl");
    let count = if append { 2 } else { 1 };
    let tokens = |amount: u64| serde_json::json!({"input_tokens":100*amount,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":10*amount,"total_tokens":110*amount});
    let request = serde_json::json!({"type":"token_usage_record","timestamp":format!("2026-11-13T00:00:{count:02}Z"),"payload":{"thread_id":"1a5bc5b5-af21-432d-bf07-e2218dd9f9bf","turn_id":"aaaaaaaa-0000-4000-8000-000000000001","root_turn_id":"aaaaaaaa-0000-4000-8000-000000000001","response_id":format!("reconnect-response-{count}"),"usage":tokens(1),"thread_token_usage":tokens(count)}});
    if !append {
        let documents = [
            serde_json::json!({"type":"session_meta","payload":{"id":"1a5bc5b5-af21-432d-bf07-e2218dd9f9bf","cwd":"/work/p","source":"cli","cli_version":"0.167.0","model_provider":"openai"}}),
            serde_json::json!({"type":"turn_context","payload":{"turn_id":"aaaaaaaa-0000-4000-8000-000000000001","root_turn_id":"aaaaaaaa-0000-4000-8000-000000000001","model":"gpt-6.1-sol"}}),
        ];
        fs::write(
            &source,
            documents
                .iter()
                .map(|document| format!("{document}\n"))
                .collect::<String>(),
        )
        .unwrap();
    }
    let mut output = fs::OpenOptions::new().append(true).open(source).unwrap();
    writeln!(output, "{request}").unwrap();
}

#[test]
fn located_runtime_records_flush_the_final_native_request_before_pointer_commit() {
    use crate::conversations::sql_reconnect::fixture_history;
    use crate::usage_store::{Environment, SharedStore};
    let fixture = Fixture::new("runtime-locate-final-request");
    let initial = fixture.open();
    let original = fixture.seed(&initial);
    let drive = fixture.root.join("drive");
    create_directory(&drive).unwrap();
    initial.schedule(&drive, false).unwrap();
    drop(initial);
    let location = fixture.open();
    let records = RecordsDirectory::fixture_existing(&drive, original.id());
    let shared = SharedStore::new(Store::open(&records).unwrap());
    let home = fixture.root.join("generated-guest");
    write_native_usage(&home, false);
    let history = fixture_history(&records, &shared, &home);
    history.flush().unwrap();
    let located = fixture.root.join("located");
    shared
        .with(|store| {
            store
                .connection
                .execute_batch("BEGIN DEFERRED")
                .map_err(crate::usage_store::error)
        })
        .unwrap();
    fs::rename(records.path(), &located).unwrap();
    location.schedule(&located, true).unwrap();
    let pointer = fs::read(&location.pointer).unwrap();
    write_native_usage(&home, true);
    assert!(history.flush().is_err());
    let target = location.pending_reconnect().unwrap().unwrap();
    assert!(history.reconnect_records(&shared, &target).is_err());
    assert!(location.saved.lock().unwrap().reconnected.is_none());
    assert_eq!(fs::read(&location.pointer).unwrap(), pointer);
    fs::rename(&located, records.path()).unwrap();
    shared
        .with(|store| {
            store
                .connection
                .execute_batch("ROLLBACK")
                .map_err(crate::usage_store::error)
        })
        .unwrap();
    fs::rename(records.path(), &located).unwrap();
    let foreign_path = fixture.root.join("foreign-records");
    create_directory(&foreign_path).unwrap();
    let foreign = RecordsDirectory::fixture_with_id(&foreign_path, &"b".repeat(32));
    Store::create_staging(&foreign, &foreign_path.join("usage.sqlite3"))
        .unwrap()
        .close()
        .unwrap();
    foreign.activate_sqlite().unwrap();
    let foreign_bytes = fs::read(foreign_path.join("usage.sqlite3")).unwrap();
    assert!(history.reconnect_records(&shared, &foreign).is_err());
    assert_eq!(
        fs::read(foreign_path.join("usage.sqlite3")).unwrap(),
        foreign_bytes
    );
    assert_eq!(fs::read(&location.pointer).unwrap(), pointer);
    history.reconnect_records(&shared, &target).unwrap();
    location.use_reconnected(&target).unwrap();
    shared.with(|store| store.connection.execute_batch("CREATE TEMP TRIGGER fail_final_cursor BEFORE UPDATE ON source_cursors BEGIN SELECT RAISE(ABORT,'injected final flush failure'); END;").map_err(crate::usage_store::error)).unwrap();
    assert!(history.flush().is_err());
    assert_eq!(fs::read(&location.pointer).unwrap(), pointer);
    assert!(!records.path().exists());
    history.reconnect_records(&shared, &target).unwrap();
    location.use_reconnected(&target).unwrap();
    shared
        .with(|store| {
            store
                .connection
                .execute_batch("DROP TRIGGER fail_final_cursor")
                .map_err(crate::usage_store::error)
        })
        .unwrap();
    history.flush().unwrap();
    let project = "1".repeat(32);
    let environment = Environment::new(
        project.clone(),
        crate::projects::EnvironmentId::for_agent(&project, crate::agents::AgentKind::Codex),
        crate::agents::AgentKind::Codex,
    )
    .unwrap();
    let ledger = shared
        .with(|store| store.load_environment(&environment))
        .unwrap();
    assert_eq!(
        crate::usage::summarize(&ledger.sessions["1a5bc5b5-af21-432d-bf07-e2218dd9f9bf"].turns)
            .requests,
        2
    );
    assert_eq!(fs::read(&location.pointer).unwrap(), pointer);
    assert!(location.schedule(&fixture.root, false).is_err());
    shared.close().unwrap();
    drop(history);
    drop(location);
    let restarted = fixture.open();
    assert!(restarted.status().unwrap().message.is_none());
    assert_eq!(restarted.status().unwrap().directory, Some(located.clone()));
    let final_records = RecordsDirectory::fixture_existing(&located, records.id());
    let store = Store::open(&final_records).unwrap();
    let ledger = store.load_environment(&environment).unwrap();
    assert_eq!(
        crate::usage::summarize(&ledger.sessions["1a5bc5b5-af21-432d-bf07-e2218dd9f9bf"].turns)
            .requests,
        2
    );
    store.close().unwrap();
}
