use super::*;
use crate::storage::data_location::RecordsDirectory;
use std::ffi::{c_void, CStr};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

mod sync_fault;

static NEXT_FIXTURE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

const WORKER: &str =
    "conversations::environment_history_tests::sqlite_usage_tests::acceptance::atomic_worker";

fn emit(name: &str, value: &impl Serialize) {
    let Ok(directory) = std::env::var("TUGBOAT_SQLITE_ACCEPTANCE_EXPORT") else {
        return;
    };
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        Path::new(&directory).join(format!("{name}.json")),
        serde_json::to_vec_pretty(value).unwrap(),
    )
    .unwrap();
}

fn generated() -> (Fixture, Watch, PathBuf, PathBuf, Ledger) {
    let number = NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let fixture = Fixture::new(&format!("sqlite-acceptance-generated-{number}"));
    fs::write(
        fixture.0.join(".acceptance-fixture"),
        b"synthetic-sqlite-acceptance-v1",
    )
    .unwrap();
    let legacy = fixture.watch(AgentKind::Codex);
    let native = write_codex(&legacy);
    synchronize(&legacy).unwrap();
    let ledger = legacy.selected_ledger().unwrap();
    let (sql, original) = imported(&fixture, &legacy, &ledger);
    (fixture, sql, native, original, ledger)
}

fn append(native: &Path) {
    let mut output = OpenOptions::new().append(true).open(native).unwrap();
    let count = |factor: u64| json!({"input_tokens":100*factor,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":10*factor,"total_tokens":110*factor});
    writeln!(output, "{}", json!({"type":"token_usage_record","timestamp":"2026-11-13T00:00:02.000123Z","payload":{"thread_id":CHAT,"turn_id":"aaaaaaaa-0000-4000-8000-000000000001","root_turn_id":"aaaaaaaa-0000-4000-8000-000000000001","response_id":"acceptance-response-2","usage":count(1),"thread_token_usage":count(2)}})).unwrap();
}

fn environment(watch: &Watch) -> crate::usage_store::Environment {
    watch.target.as_ref().unwrap().usage_environment().unwrap()
}

fn worker_watch(root: &Path) -> Watch {
    assert!(fs::canonicalize(root)
        .unwrap()
        .starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
    assert_eq!(
        fs::read(root.join(".acceptance-fixture")).unwrap(),
        b"synthetic-sqlite-acceptance-v1"
    );
    let records = RecordsDirectory::fixture_existing(&root.join("records"), &"a".repeat(32));
    Watch {
        target: Some(environment_history::Target {
            project_id: PROJECT.into(),
            environment_id: projects::EnvironmentId::for_agent(PROJECT, AgentKind::Codex),
            agent_kind: AgentKind::Codex,
        }),
        records_directory: Some(records.clone()),
        store: Some(SharedStore::new(Store::open(&records).unwrap())),
        home: root.join("codex"),
        project: "/work/p".into(),
        ledger: root.join("codex-ledger.json"),
    }
}

struct CrashPoint {
    phase: String,
    request: bool,
    cursor: bool,
}

unsafe extern "C" fn trace(
    mask: u32,
    context: *mut c_void,
    statement: *mut c_void,
    _value: *mut c_void,
) -> i32 {
    let point = unsafe { &mut *context.cast::<CrashPoint>() };
    let sql = unsafe { rusqlite::ffi::sqlite3_sql(statement.cast()) };
    if sql.is_null() {
        return 0;
    }
    let sql = unsafe { CStr::from_ptr(sql) }.to_bytes();
    let request = sql.starts_with(b"INSERT INTO usage_requests");
    let cursor = sql.starts_with(b"INSERT INTO source_cursors");
    if mask == rusqlite::ffi::SQLITE_TRACE_STMT && request {
        if point.phase == "before-request" {
            std::process::exit(92);
        }
        point.request = true;
    }
    if mask == rusqlite::ffi::SQLITE_TRACE_STMT && cursor && point.request {
        if point.phase == "between-request-cursor" {
            std::process::exit(92);
        }
        point.cursor = true;
    }
    if mask == rusqlite::ffi::SQLITE_TRACE_PROFILE
        && cursor
        && point.request
        && point.phase == "after-cursor-before-commit"
    {
        std::process::exit(92);
    }
    0
}

unsafe extern "C" fn commit(context: *mut c_void) -> i32 {
    let point = unsafe { &*context.cast::<CrashPoint>() };
    if point.request && point.cursor && point.phase == "before-commit" {
        std::process::exit(92);
    }
    0
}

#[test]
#[ignore = "subprocess entry; parent tests supply a generated fixture"]
fn atomic_worker() {
    let root = PathBuf::from(std::env::var_os("TUGBOAT_SQLITE_ATOMIC_FIXTURE").unwrap());
    let phase = std::env::var("TUGBOAT_SQLITE_ATOMIC_PHASE").unwrap();
    let watch = worker_watch(&root);
    if phase == "lock" {
        watch
            .store
            .as_ref()
            .unwrap()
            .with(|store| {
                store
                    .connection
                    .execute_batch("BEGIN IMMEDIATE")
                    .map_err(crate::usage_store::error)
            })
            .unwrap();
        fs::write(root.join("writer-locked"), b"ready").unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::process::exit(93);
    }
    let mut point = CrashPoint {
        phase: phase.clone(),
        request: false,
        cursor: false,
    };
    watch
        .store
        .as_ref()
        .unwrap()
        .with(|store| {
            store.prepare_codex(&environment(&watch))?;
            let context = (&mut point as *mut CrashPoint).cast::<c_void>();
            unsafe {
                assert_eq!(
                    rusqlite::ffi::sqlite3_trace_v2(
                        store.connection.handle(),
                        rusqlite::ffi::SQLITE_TRACE_STMT | rusqlite::ffi::SQLITE_TRACE_PROFILE,
                        Some(trace),
                        context
                    ),
                    rusqlite::ffi::SQLITE_OK
                );
                rusqlite::ffi::sqlite3_commit_hook(
                    store.connection.handle(),
                    Some(commit),
                    context,
                );
            }
            Ok(())
        })
        .unwrap();
    synchronize(&watch).unwrap();
    if phase == "after-commit" {
        std::process::exit(92);
    }
    panic!("The requested SQLite write boundary was not reached");
}

fn command(root: &Path, phase: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", WORKER])
        .env("TUGBOAT_SQLITE_ATOMIC_FIXTURE", root)
        .env("TUGBOAT_SQLITE_ATOMIC_PHASE", phase)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[test]
fn killed_sql_writers_keep_request_cursor_and_rollup_in_one_commit() {
    let mut evidence = Vec::new();
    for phase in [
        "before-request",
        "between-request-cursor",
        "after-cursor-before-commit",
        "before-commit",
        "after-commit",
    ] {
        let (fixture, sql, native, original, baseline) = generated();
        let original_bytes = fs::read(&original).unwrap();
        sql.store.as_ref().unwrap().close().unwrap();
        append(&native);
        let status = command(&fixture.0, phase).status().unwrap();
        assert_eq!(status.code(), Some(92), "{phase}");
        let reopened = worker_watch(&fixture.0);
        let facts = reopened.load().unwrap();
        let count = usage::summarize(&facts.sessions[CHAT].turns).requests;
        assert_eq!(
            count,
            if phase == "after-commit" { 2 } else { 1 },
            "{phase}"
        );
        if phase != "after-commit" {
            assert_eq!(value(&facts), value(&baseline), "{phase}");
        }
        let (dashboard, _) = synchronize(&reopened).unwrap();
        assert_eq!(dashboard.usage.requests, 2, "{phase}");
        assert!(!synchronize(&reopened).unwrap().1, "{phase}");
        reopened
            .store
            .as_ref()
            .unwrap()
            .with(|store| store.integrity_check())
            .unwrap();
        assert_eq!(fs::read(&original).unwrap(), original_bytes);
        evidence.push(json!({"phase":phase,"exitCode":92,"committedRequestsBeforeReplay":count,"requestsAfterReplay":2,"integrity":"ok","originalSha256":crate::hash::hex(Sha256::digest(original_bytes))}));
    }
    emit("sql-write-crashes", &evidence);
}

#[test]
fn a_busy_external_writer_times_out_without_advancing_saved_usage() {
    let (fixture, sql, native, original, baseline) = generated();
    let original_bytes = fs::read(&original).unwrap();
    append(&native);
    let mut locker = command(&fixture.0, "lock").spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !fixture.0.join("writer-locked").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(fixture.0.join("writer-locked").exists());
    let start = Instant::now();
    let result = synchronize(&sql);
    let elapsed = start.elapsed();
    locker.kill().unwrap();
    locker.wait().unwrap();
    assert!(result.is_err());
    assert!(elapsed >= Duration::from_millis(4500) && elapsed < Duration::from_secs(9));
    assert_eq!(value(&sql.load().unwrap()), value(&baseline));
    assert_eq!(synchronize(&sql).unwrap().0.usage.requests, 2);
    assert_eq!(fs::read(&original).unwrap(), original_bytes);
    emit(
        "busy-writer",
        &json!({"timeoutMilliseconds":elapsed.as_millis(),"beforeRequests":1,"afterRecoveryRequests":2,"result":"pass"}),
    );
}

#[test]
fn sqlite_full_rolls_back_a_real_failed_write_and_keeps_the_store_reusable() {
    let (_fixture, sql, _native, original, baseline) = generated();
    let original_bytes = fs::read(&original).unwrap();
    let mut oversized = baseline.clone();
    oversized.sessions.get_mut(CHAT).unwrap().title = "f".repeat(2 * 1024 * 1024);
    let store = sql.store.as_ref().unwrap();
    let result = store.with(|store| {
        let pages: i64 = store
            .connection
            .query_row("PRAGMA page_count", [], |row| row.get(0))
            .map_err(crate::usage_store::error)?;
        store
            .connection
            .pragma_update(None, "max_page_count", pages)
            .map_err(crate::usage_store::error)?;
        store.save_environment(&environment(&sql), &oversized)
    });
    assert!(result.unwrap_err().to_lowercase().contains("full"));
    assert_eq!(value(&sql.load().unwrap()), value(&baseline));
    store
        .with(|store| {
            store
                .connection
                .pragma_update(None, "max_page_count", 1_073_741_823i64)
                .map_err(crate::usage_store::error)
        })
        .unwrap();
    let mut recovered = baseline.clone();
    recovered
        .warnings
        .push("Recovered generated page quota".into());
    store
        .with(|store| store.save_environment(&environment(&sql), &recovered))
        .unwrap();
    assert_eq!(value(&sql.load().unwrap()), value(&recovered));
    assert_eq!(fs::read(&original).unwrap(), original_bytes);
    emit(
        "sqlite-full",
        &json!({"injection":"SQLite max_page_count","error":"SQLITE_FULL","rolledBack":true,"subsequentWrite":true}),
    );
}

#[test]
fn failed_database_sync_restores_the_last_commit_before_reopen() {
    let (_fixture, sql, native, original, baseline) = generated();
    let original_bytes = fs::read(&original).unwrap();
    append(&native);
    let hook = sql
        .store
        .as_ref()
        .unwrap()
        .with(|store| Ok(unsafe { sync_fault::SyncFault::install(&store.connection) }))
        .unwrap();
    let result = synchronize(&sql);
    assert!(hook.failed());
    drop(hook);
    assert!(result.is_err());
    let records = sql.records_directory.clone().unwrap();
    drop(sql);
    let reopened = Store::open(&records).unwrap();
    reopened.integrity_check().unwrap();
    assert_eq!(
        value(&reopened.load_environment(&environment_for_codex()).unwrap()),
        value(&baseline)
    );
    reopened.close().unwrap();
    let restored = worker_watch(records.path().parent().unwrap());
    assert_eq!(synchronize(&restored).unwrap().0.usage.requests, 2);
    assert_eq!(fs::read(&original).unwrap(), original_bytes);
    emit(
        "sync-failure",
        &json!({"injection":"database sqlite3_io_methods.xSync","error":"SQLITE_IOERR_FSYNC","rolledBack":true,"replayRequests":2}),
    );
}

fn environment_for_codex() -> crate::usage_store::Environment {
    crate::usage_store::Environment::new(
        PROJECT.into(),
        projects::EnvironmentId::for_agent(PROJECT, AgentKind::Codex),
        AgentKind::Codex,
    )
    .unwrap()
}

#[test]
fn interior_page_corruption_and_a_newer_schema_block_activation_without_json_fallback() {
    use std::io::{Seek, SeekFrom};
    let mut results = Vec::new();
    for case in ["corrupt-page", "newer-schema"] {
        let (_fixture, sql, _native, original, _baseline) = generated();
        let original_bytes = fs::read(&original).unwrap();
        let records = sql.records_directory.clone().unwrap();
        let (page, size) = sql
            .store
            .as_ref()
            .unwrap()
            .with(|store| {
                Ok((
                    store
                        .connection
                        .query_row(
                            "SELECT rootpage FROM sqlite_schema WHERE name='usage_requests'",
                            [],
                            |row| row.get::<_, i64>(0),
                        )
                        .map_err(crate::usage_store::error)?,
                    store
                        .connection
                        .query_row("PRAGMA page_size", [], |row| row.get::<_, i64>(0))
                        .map_err(crate::usage_store::error)?,
                ))
            })
            .unwrap();
        sql.store.as_ref().unwrap().close().unwrap();
        let database = records.path().join("usage.sqlite3");
        if case == "corrupt-page" {
            let mut file = OpenOptions::new().write(true).open(&database).unwrap();
            file.seek(SeekFrom::Start(u64::try_from((page - 1) * size).unwrap()))
                .unwrap();
            file.write_all(&[0xff]).unwrap();
            file.sync_all().unwrap();
        }
        if case == "newer-schema" {
            let connection = rusqlite::Connection::open(&database).unwrap();
            connection
                .pragma_update(None, "user_version", 2i64)
                .unwrap();
            connection.close().unwrap();
        }
        let damaged = fs::read(&database).unwrap();
        assert!(importer::activate(&records).is_err(), "{case}");
        assert_eq!(records.schema_version().unwrap(), 2);
        assert_eq!(fs::read(&database).unwrap(), damaged);
        assert_eq!(fs::read(&original).unwrap(), original_bytes);
        results.push(json!({"case":case,"originalLedgerSha256":crate::hash::hex(Sha256::digest(original_bytes)),"databaseSha256":crate::hash::hex(Sha256::digest(damaged)),"markerVersion":2,"activationRejected":true}));
    }
    emit("corruption-compatibility", &results);
}

#[test]
#[ignore = "requires the separately compiled frozen P01 v1 binary"]
fn the_actual_frozen_v1_binary_rejects_the_activated_dataset() {
    use std::os::unix::fs::PermissionsExt;
    let (fixture, sql, _native, original, _baseline) = generated();
    let binary = PathBuf::from(std::env::var_os("TUGBOAT_SQLITE_OLD_BINARY").unwrap());
    let records = sql.records_directory.as_ref().unwrap();
    let bootstrap = fixture.0.join("old-bootstrap");
    fs::create_dir(&bootstrap).unwrap();
    fs::set_permissions(&bootstrap, fs::Permissions::from_mode(0o700)).unwrap();
    storage::write(&bootstrap.join("storage-location.json"), &json!({"schemaVersion":1,"directory":records.path(),"directoryId":records.id(),"pending":null,"previousDirectory":null})).unwrap();
    sql.store.as_ref().unwrap().close().unwrap();
    let ledger_bytes = fs::read(&original).unwrap();
    let database_bytes = fs::read(records.path().join("usage.sqlite3")).unwrap();
    let marker_bytes = fs::read(records.path().join(".tugboat-data.json")).unwrap();
    let output = Command::new(&binary)
        .args(["--ignored", "sqlite_downgrade_guard", "--test-threads=1"])
        .env("TUGBOAT_SQLITE_OLD_GUARD_ROOT", &fixture.0)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("2 passed"));
    assert_eq!(fs::read(&original).unwrap(), ledger_bytes);
    assert_eq!(
        fs::read(records.path().join("usage.sqlite3")).unwrap(),
        database_bytes
    );
    assert_eq!(
        fs::read(records.path().join(".tugboat-data.json")).unwrap(),
        marker_bytes
    );
    emit(
        "actual-old-binary",
        &json!({"binary":binary,"binarySha256":crate::hash::hex(Sha256::digest(fs::read(&binary).unwrap())),"oldGuardTestsPassed":2,"ledgerSha256":crate::hash::hex(Sha256::digest(ledger_bytes)),"databaseSha256":crate::hash::hex(Sha256::digest(database_bytes)),"markerUnchanged":true}),
    );
}

mod billing;

mod contracts;

mod append;

#[test]
fn pending_counter_exact_pairs_survive_import_restart_and_duplicate_occurrences() {
    let fixture = Fixture::new("sqlite-acceptance-pending-counter-exact");
    let legacy = fixture.watch(AgentKind::Codex);
    let source = write_codex(&legacy);
    let initial = fs::read_to_string(&source)
        .unwrap()
        .lines()
        .take(2)
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    let counts = json!({"input_tokens":100,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":10,"total_tokens":110});
    let counter = json!({"timestamp":"2026-11-13T00:00:01.123456Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":counts,"total_token_usage":counts}}});
    fs::write(&source, format!("{initial}{counter}\n")).unwrap();
    synchronize(&legacy).unwrap();
    let raw = legacy.selected_ledger().unwrap();
    let cursor = raw.sessions[CHAT].cursors.values().next().unwrap();
    assert!(!value(cursor)["pendingCounters"]
        .as_array()
        .unwrap()
        .is_empty());
    let first_key = raw.sessions[CHAT].turns.values().next().unwrap().requests[0]
        .key
        .clone();
    let (sql, original) = imported(&fixture, &legacy, &raw);
    let original_bytes = fs::read(&original).unwrap();
    let exact = json!({"type":"token_usage_record","timestamp":"2026-11-13T00:00:01.123456Z","payload":{"thread_id":CHAT,"turn_id":"aaaaaaaa-0000-4000-8000-000000000001","root_turn_id":"aaaaaaaa-0000-4000-8000-000000000001","response_id":"late-exact-response-1","usage":counts,"thread_token_usage":counts}});
    let mut file = OpenOptions::new().append(true).open(&source).unwrap();
    writeln!(file, "{exact}").unwrap();
    drop(file);
    let expected = synchronize(&legacy).unwrap().0;
    let actual = synchronize(&sql).unwrap().0;
    assert_eq!(value(&actual), value(&expected));
    assert_eq!(actual.usage.requests, 1);
    let facts = sql.load().unwrap();
    let request = &facts.sessions[CHAT].turns.values().next().unwrap().requests[0];
    assert_eq!(request.key, first_key);
    assert!(request.source.is_some());
    assert!(
        !value(facts.sessions[CHAT].cursors.values().next().unwrap())["matchedCounters"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    let records = sql.records_directory.clone().unwrap();
    sql.store.as_ref().unwrap().close().unwrap();
    let mut reopened = sql.clone();
    reopened.store = Some(SharedStore::new(Store::open(&records).unwrap()));
    assert!(!synchronize(&reopened).unwrap().1);
    let mut file = OpenOptions::new().append(true).open(&source).unwrap();
    writeln!(file, "{exact}").unwrap();
    drop(file);
    assert_eq!(synchronize(&reopened).unwrap().0.usage.requests, 1);
    assert!(!synchronize(&reopened).unwrap().1);
    assert_eq!(fs::read(&original).unwrap(), original_bytes);
    emit(
        "pending-counter-exact",
        &json!({"before":raw,"after":reopened.load().unwrap(),"requests":1,"stableRequestKey":first_key,"originalSha256":crate::hash::hex(Sha256::digest(original_bytes))}),
    );
}

#[test]
fn an_io_failure_requires_a_fresh_verified_connection_before_the_next_poll() {
    let (_fixture, sql, native, _original, _baseline) = generated();
    let store = sql.store.as_ref().unwrap();
    store
        .with(|store| {
            store
                .connection
                .execute_batch("CREATE TEMP TABLE prior_connection_marker(value INTEGER);")
                .map_err(crate::usage_store::error)
        })
        .unwrap();
    append(&native);
    let hook = store
        .with(|store| Ok(unsafe { sync_fault::SyncFault::install(&store.connection) }))
        .unwrap();
    assert!(synchronize(&sql).is_err());
    assert!(hook.failed());
    drop(hook);
    let recovered = synchronize(&sql).unwrap().0;
    assert_eq!(recovered.usage.requests, 2);
    let stale = store
        .with(|store| {
            store
                .connection
                .query_row(
                    "SELECT count(*) FROM sqlite_temp_schema WHERE name='prior_connection_marker'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(crate::usage_store::error)
        })
        .unwrap();
    assert_eq!(
        stale, 0,
        "The connection from the uncertain I/O failure was reused"
    );
    assert!(!synchronize(&sql).unwrap().1);
    emit(
        "public-io-reopen",
        &json!({"syncFault":true,"freshConnection":true,"committedCursorVerified":true,"requests":2,"quietReplay":true}),
    );
}

#[test]
fn detected_corruption_blocks_later_writes_through_the_same_shared_handle() {
    use std::io::{Seek, SeekFrom};
    let (_fixture, sql, _native, _original, _baseline) = generated();
    let store = sql.store.as_ref().unwrap();
    let (page, size) = store
        .with(|store| {
            Ok((
                store
                    .connection
                    .query_row(
                        "SELECT rootpage FROM sqlite_schema WHERE name='usage_requests'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(crate::usage_store::error)?,
                store
                    .connection
                    .query_row("PRAGMA page_size", [], |row| row.get::<_, i64>(0))
                    .map_err(crate::usage_store::error)?,
            ))
        })
        .unwrap();
    store
        .with(|store| {
            store
                .connection
                .execute_batch("PRAGMA shrink_memory")
                .map_err(crate::usage_store::error)
        })
        .unwrap();
    let database = sql
        .records_directory
        .as_ref()
        .unwrap()
        .path()
        .join("usage.sqlite3");
    let mut file = OpenOptions::new().write(true).open(&database).unwrap();
    file.seek(SeekFrom::Start(u64::try_from((page - 1) * size).unwrap()))
        .unwrap();
    file.write_all(&[0xff]).unwrap();
    file.sync_all().unwrap();
    drop(file);
    assert!(store.with(|store| store.integrity_check()).is_err());
    let before = fs::read(&database).unwrap();
    assert!(
        store
            .with(|store| store.change_state(&environment(&sql), CHAT, SessionState::Archived))
            .is_err(),
        "A shared handle remained writable after corruption was detected"
    );
    assert_eq!(fs::read(&database).unwrap(), before);
    emit(
        "corruption-stops-writes",
        &json!({"detected":true,"sameSharedHandleWriteRejected":true,"damagedDatabasePreserved":true}),
    );
}
