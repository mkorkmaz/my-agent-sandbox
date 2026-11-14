use super::*;
use crate::{
    agents::AgentKind,
    conversations::{Ledger, SessionRecord, SessionState},
    projects::EnvironmentId,
    storage::data_location::RecordsDirectory,
    usage,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::{
    ffi::OsStringExt,
    fs::{symlink, PermissionsExt},
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const PROJECT: &str = "11111111111111111111111111111111";
const SESSION: &str = "00000000-0000-4000-8000-000000000001";
static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "tugboat-sqlite-codecs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn store(&self) -> (RecordsDirectory, Store) {
        let records = RecordsDirectory::fixture(&self.0);
        let store = Store::create_staging(&records, &self.0.join("usage.sqlite3")).unwrap();
        records.activate_sqlite().unwrap();
        (records, store)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn environment(kind: AgentKind) -> Environment {
    Environment::new(
        PROJECT.into(),
        EnvironmentId::for_agent(PROJECT, kind),
        kind,
    )
    .unwrap()
}

fn record() -> SessionRecord {
    let rates = json!({"date":"2024-01-02","source":"historical","input":0.1234567890123456,"cached":0.01,"write30m":0.0,"write12h":0.0,"output":2.1,"tier":"old-tier","tierAssumed":true,"longContext":true,"anthropic":{"model":"removed-model","write5m":0.4,"write1h":0.5,"speed":"fast","inferenceGeo":"us","multiplier":2.2,"fastMultiplier":2.0,"usMultiplier":1.1,"batchMultiplier":0.5,"assumptions":["second","first"]}});
    let tokens = json!({"input":101,"cached":11,"cacheWrite":12,"output":17,"reasoning":13,"write12h":null,"cacheKnown":false,"cachedKnown":true,"cacheWriteKnown":null});
    let correction =
        json!({"previousRates":rates,"previousCostUsd":0.0,"reason":"previous provider"});
    let metadata = json!({"cacheCreation":{"ephemeral5mInputTokens":5,"ephemeral1hInputTokens":7},"speed":"fast","serviceTier":null,"inferenceGeo":"us","invalidFields":["field-b","field-a"]});
    let request = json!({"key":"same-entry-key","at":"2026-11-13T01:02:03.000123456Z","model":"removed-model","tokens":tokens,"rates":rates,"costUsd":0.12345678901234566,"partial":true,"unpricedReason":"historical reason","serviceTier":"old-tier","tierSource":"recorded-source","rateCorrection":correction,"providerCorrection":correction,"amountCorrection":{"previousTokens":tokens,"previousCostUsd":null,"previousPartial":false,"previousUnpricedReason":"prior","reason":"amount"},"anthropic":metadata,"anthropicCorrections":[{"previousMetadata":metadata,"previousModel":"old-model","previousRates":rates,"previousCostUsd":null,"reason":"first"},{"previousMetadata":null,"previousModel":null,"previousRates":null,"previousCostUsd":0.0,"reason":"second"}],"source":{"guestNamespace":"guest:source","modelProvider":"anthropic","originThreadId":SESSION,"originTurnId":"different-origin","rootTurnId":"root-turn","responseId":"same-response"}});
    let mut record:SessionRecord=serde_json::from_value(json!({"agentKind":"claude-code","claudeSource":{"path":"/fixture/claude/root.jsonl","cwd":"/fixture/project","rootSessionId":SESSION,"agentId":null,"toolUseIds":["b","a"]},"guestNamespace":"guest:row","title":"Stored title","updatedAt":123,"state":"archived","available":true,"parserVersion":u32::MAX,"warning":"record warning","lineage":{"kind":"subagent","parentSessionId":"00000000-0000-4000-8000-000000000009","agentPath":"agent/a","nickname":"child","sourceVersion":"0.167.0","lineageVersion":1},"lineageProblem":"missingSource","sourceRootId":SESSION,"projectExclusion":{"rootSessionId":SESSION,"rootCwd":"/fixture/other-project"},"turns":{"bucket-differs":{"id":"native-turn","status":"some-legacy-status","originThreadId":SESSION,"rootTurnId":"root","parentTurnId":"parent","requests":[request.clone(),request]},"empty":{"id":"empty","status":"in-progress","requests":[]}},"cursors":{}})).unwrap();
    let cursor: usage::Cursor = serde_json::from_value({
        let mut value = serde_json::to_value(usage::Cursor::default()).unwrap();
        value["offset"] = json!(143);
        value["pendingBytes"] = json!(true);
        value["namespace"] = json!("guest:cursor");
        value["expectedOwner"] = json!(SESSION);
        value["ownershipProblem"] = json!("saved evidence");
        value["headerFingerprint"] = json!("header");
        value["matchedCounters"] = json!({"counter":["turn","request"]});
        value
    })
    .unwrap();
    record.cursors.insert(
        PathBuf::from(std::ffi::OsString::from_vec(
            b"/fixture/source.jsonl".to_vec(),
        )),
        cursor,
    );
    record
}

#[test]
fn exact_schema_and_all_fields_round_trip_after_reopen() {
    let fixture = Fixture::new();
    let (records, mut store) = fixture.store();
    let env = environment(AgentKind::Codex);
    let mut ledger = Ledger::default();
    let record = record();
    ledger.sessions.insert(SESSION.into(), record.clone());
    ledger.variants.insert(
        SESSION.into(),
        BTreeMap::from([("".into(), record.clone()), ("guest:slot".into(), record)]),
    );
    ledger.variants.insert(
        "00000000-0000-4000-8000-000000000002".into(),
        BTreeMap::new(),
    );
    ledger.warnings = vec!["second".into(), "first".into(), "second".into()];
    assert!(store.save_environment(&env, &ledger).unwrap());
    assert!(store.load_environment(&env).unwrap() == ledger);
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type='table'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        9
    );
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type='index' AND sql IS NOT NULL",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
    store.integrity_check().unwrap();
    store.close().unwrap();
    let reopened = Store::open(&records).unwrap();
    assert!(reopened.load_environment(&env).unwrap() == ledger);
    reopened.integrity_check().unwrap();
}

#[test]
fn delta_is_quiet_and_status_changes_only_one_record() {
    let fixture = Fixture::new();
    let (_, mut store) = fixture.store();
    let env = environment(AgentKind::Codex);
    let mut ledger = Ledger::default();
    let mut record = record();
    record.project_exclusion = None;
    ledger.sessions.insert(SESSION.into(), record);
    store.save_environment(&env, &ledger).unwrap();
    let before = store.connection.total_changes();
    assert!(!store.save_environment(&env, &ledger).unwrap());
    assert_eq!(store.connection.total_changes(), before);
    store.connection.execute_batch("CREATE TEMP TABLE mutation_trace(table_name TEXT); CREATE TEMP TRIGGER trace_records AFTER UPDATE ON session_records BEGIN INSERT INTO mutation_trace VALUES('records'); END; CREATE TEMP TRIGGER trace_requests AFTER UPDATE ON usage_requests BEGIN INSERT INTO mutation_trace VALUES('requests'); END; CREATE TEMP TRIGGER trace_cursors AFTER UPDATE ON source_cursors BEGIN INSERT INTO mutation_trace VALUES('cursors'); END;").unwrap();
    assert!(store
        .change_state(&env, SESSION, SessionState::Deleted)
        .unwrap());
    assert!(!store
        .change_state(&env, SESSION, SessionState::Deleted)
        .unwrap());
    let trace = store
        .connection
        .prepare("SELECT table_name FROM mutation_trace")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(trace, ["records"]);
    let request_before = store
        .connection
        .query_row("SELECT count(*) FROM usage_requests", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    let (scan, rollups) = store.load_scan_state(&env).unwrap();
    assert!(scan.sessions[SESSION].turns.is_empty());
    assert_eq!(scan.sessions[SESSION].cursors.len(), 1);
    assert_eq!(rollups.len(), 1);
    assert!(!store
        .save_partial_environment(&env, &scan, &Default::default())
        .unwrap());
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM usage_requests", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        request_before
    );
}

#[test]
fn checked_integers_constraints_and_unknown_values_fail_without_partial_facts() {
    let fixture = Fixture::new();
    let (_, mut store) = fixture.store();
    let env = environment(AgentKind::Codex);
    let mut ledger = Ledger::default();
    ledger.sessions.insert(SESSION.into(), record());
    store.save_environment(&env, &ledger).unwrap();
    let before = store.load_environment(&env).unwrap();
    ledger
        .sessions
        .get_mut(SESSION)
        .unwrap()
        .turns
        .get_mut("bucket-differs")
        .unwrap()
        .requests[0]
        .tokens
        .input = u64::MAX;
    assert!(store.save_environment(&env, &ledger).is_err());
    assert!(store.load_environment(&env).unwrap() == before);
    for sql in [
        "UPDATE usage_requests SET cached_tokens=input_tokens+1",
        "UPDATE usage_requests SET reasoning_tokens=output_tokens+1",
        "UPDATE usage_requests SET cached_known=2",
        "UPDATE usage_requests SET cost_usd=-1",
        "UPDATE usage_requests SET origin_thread_id=NULL",
        "UPDATE session_records SET lineage_problem='future'",
        "UPDATE session_records SET state='future'",
        "UPDATE session_records SET exclusion_root_id=NULL",
        "UPDATE source_cursors SET cursor_json='[]'",
    ] {
        assert!(store.connection.execute(sql, []).is_err(), "{sql}");
    }
    assert!(Environment::new(
        PROJECT.into(),
        EnvironmentId::for_agent(PROJECT, AgentKind::ClaudeCode),
        AgentKind::Codex
    )
    .is_err());
    store.integrity_check().unwrap();
}

#[test]
fn private_identity_open_rejects_links_replacements_and_foreign_headers() {
    let fixture = Fixture::new();
    let (records, store) = fixture.store();
    let path = fixture.0.join("usage.sqlite3");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    store.close().unwrap();
    let bytes = fs::read(&path).unwrap();
    let target = fixture.0.join("other-file");
    fs::write(&target, &bytes).unwrap();
    symlink(&target, fixture.0.join("usage.sqlite3-journal")).unwrap();
    assert!(Store::open(&records).is_err());
    assert_eq!(fs::read(&target).unwrap(), bytes);
    fs::remove_file(fixture.0.join("usage.sqlite3-journal")).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(&target, fixture.0.join("usage.sqlite3-journal")).unwrap();
    assert!(Store::open(&records).is_err());
    assert_eq!(fs::read(&target).unwrap(), bytes);
    fs::remove_file(fixture.0.join("usage.sqlite3-journal")).unwrap();
    let store = Store::open(&records).unwrap();
    fs::rename(&path, fixture.0.join("original.sqlite3")).unwrap();
    fs::write(&path, &bytes).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(store.verify().is_err());
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch("PRAGMA application_id=7; PRAGMA journal_mode=DELETE;")
        .unwrap();
    drop(connection);
    let before = fs::read(&path).unwrap();
    assert!(Store::open(&records).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn native_indexes_cover_environment_slots_turns_and_exact_source_probes() {
    let fixture = Fixture::new();
    let (_, store) = fixture.store();
    for (query,values) in [("SELECT * FROM usage_environments WHERE project_id=?1 AND environment_id=?2",vec!["a","b"]),("SELECT * FROM session_groups WHERE environment_key=?1 AND native_session_id=?2",vec!["1",SESSION]),("SELECT * FROM session_records WHERE session_key=?1 AND variant_key=?2",vec!["1",""]),("SELECT * FROM usage_turns WHERE record_id=?1",vec!["1"]),("SELECT * FROM usage_requests WHERE source_guest=?1 AND model_provider=?2 AND response_id=?3 AND origin_thread_id=?4",vec!["guest","anthropic","message",SESSION])] {
        let plan=store.connection.prepare(&format!("EXPLAIN QUERY PLAN {query}")).unwrap().query_map(rusqlite::params_from_iter(values),|row|row.get::<_,String>(3)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();assert!(plan.iter().any(|entry|entry.contains("SEARCH")),"{query}: {plan:?}");assert!(!plan.iter().any(|entry|entry.contains("SCAN usage_requests")),"{plan:?}");
        println!("{}",json!({"query":query,"queryPlan":plan}));
    }
    assert_eq!(
        store
            .connection
            .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "delete"
    );
    assert_eq!(
        store
            .connection
            .query_row("PRAGMA synchronous", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        3
    );
    let version: (String, String) = store
        .connection
        .query_row("SELECT sqlite_version(),sqlite_source_id()", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    let mut settings = serde_json::Map::new();
    for pragma in [
        "foreign_keys",
        "journal_mode",
        "synchronous",
        "fullfsync",
        "trusted_schema",
        "busy_timeout",
    ] {
        let value = store
            .connection
            .query_row(&format!("PRAGMA {pragma}"), [], |row| {
                let value = row.get_ref(0)?;
                Ok(match value {
                    rusqlite::types::ValueRef::Integer(value) => json!(value),
                    rusqlite::types::ValueRef::Text(value) => json!(String::from_utf8_lossy(value)),
                    _ => serde_json::Value::Null,
                })
            })
            .unwrap();
        settings.insert(pragma.into(), value);
    }
    println!(
        "{}",
        json!({"sqliteVersion":version.0,"sqliteSourceId":version.1,"pragmas":settings})
    );
}

#[test]
fn cursor_codec_retains_exact_non_unicode_path_bytes() {
    let fixture = Fixture::new();
    let (_, mut store) = fixture.store();
    let env = environment(AgentKind::Codex);
    let mut ledger = Ledger::default();
    ledger
        .sessions
        .insert(SESSION.into(), SessionRecord::default());
    store.save_environment(&env, &ledger).unwrap();
    let record_id = store
        .connection
        .query_row("SELECT record_id FROM session_records", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    let path = PathBuf::from(std::ffi::OsString::from_vec(
        b"/fixture/source-\xff.jsonl".to_vec(),
    ));
    codecs::insert_cursor(
        &store.connection,
        record_id,
        &path,
        &usage::Cursor::default(),
    )
    .unwrap();
    assert!(codecs::read_cursors(&store.connection, record_id)
        .unwrap()
        .contains_key(&path));
}

#[test]
fn typed_cursor_decoder_preserves_pending_states_and_bounds_invalid_input() {
    let fixture = Fixture::new();
    let (_, store) = fixture.store();
    let tokens = json!({"input":u64::MAX,"cached":0,"cacheWrite":0,"output":0,"reasoning":0,"write12h":null,"cacheKnown":false,"cachedKnown":null,"cacheWriteKnown":null});
    let source = json!({"guestNamespace":"guest:cursor","modelProvider":"openai","originThreadId":SESSION,"originTurnId":null,"rootTurnId":null,"responseId":"pending"});
    let mut value = serde_json::to_value(usage::Cursor::default()).unwrap();
    value["offset"] = json!(u64::MAX);
    value["inode"] = json!(u64::MAX);
    value["device"] = json!(u64::MAX);
    value["previous"] = tokens.clone();
    value["pendingExact"] = json!([{"source":source,"usage":tokens,"cumulative":tokens,"at":"fixture","model":null,"tier":null,"tier_source":"thread"}]);
    value["pendingCounters"] = json!([{"owner":SESSION,"turn":null,"usage":tokens,"cumulative":tokens,"key":null,"notification_key":"notification","tier":null,"tier_source":"thread"}]);
    value["matchedCounters"] = json!({"notification":["turn","request"]});
    let encoded = serde_json::to_string(&value).unwrap();
    let expected: usage::Cursor = exact_json::decode(&encoded).unwrap();
    let decoded = store
        .connection
        .query_row("SELECT ?1 AS cursor_json", [&encoded], |row| {
            codecs::decode_cursor(row, "cursor_json")
        })
        .unwrap();
    assert_eq!(decoded, expected);
    value["offset"] = json!("private fixture payload");
    let malformed = serde_json::to_string(&value).unwrap();
    let failure = store
        .connection
        .query_row("SELECT ?1 AS cursor_json", [&malformed], |row| {
            codecs::decode_cursor(row, "cursor_json")
        })
        .unwrap_err()
        .to_string();
    assert!(!failure.contains("private fixture payload"));
    for (depth, allowed) in [(127, true), (128, false)] {
        let nested = format!(
            "{},\"unknown\":{}0{}}}",
            encoded.strip_suffix('}').unwrap(),
            "[".repeat(depth),
            "]".repeat(depth)
        );
        assert_eq!(
            store
                .connection
                .query_row("SELECT ?1 AS cursor_json", [&nested], |row| {
                    codecs::decode_cursor(row, "cursor_json")
                })
                .is_ok(),
            allowed
        );
    }
}

#[test]
fn metadata_and_status_work_when_raw_request_and_cursor_access_is_denied() {
    unsafe extern "C" fn deny_facts(
        _context: *mut std::ffi::c_void,
        action: std::ffi::c_int,
        table: *const std::ffi::c_char,
        _column: *const std::ffi::c_char,
        _database: *const std::ffi::c_char,
        _trigger: *const std::ffi::c_char,
    ) -> std::ffi::c_int {
        if action != rusqlite::ffi::SQLITE_READ || table.is_null() {
            return rusqlite::ffi::SQLITE_OK;
        }
        let table = unsafe { std::ffi::CStr::from_ptr(table) }.to_bytes();
        if table == b"usage_requests" || table == b"source_cursors" {
            return rusqlite::ffi::SQLITE_DENY;
        }
        rusqlite::ffi::SQLITE_OK
    }
    let fixture = Fixture::new();
    let (_, mut store) = fixture.store();
    let env = environment(AgentKind::Codex);
    let mut ledger = Ledger::default();
    let mut record = record();
    record.project_exclusion = None;
    ledger.sessions.insert(SESSION.into(), record);
    store.save_environment(&env, &ledger).unwrap();
    unsafe {
        assert_eq!(
            rusqlite::ffi::sqlite3_set_authorizer(
                store.connection.handle(),
                Some(deny_facts),
                std::ptr::null_mut()
            ),
            rusqlite::ffi::SQLITE_OK
        );
    }
    let (metadata, rollups) = store.load_metadata(&env).unwrap();
    assert!(metadata.sessions[SESSION].turns.is_empty());
    assert!(metadata.sessions[SESSION].cursors.is_empty());
    assert_eq!(rollups.len(), 1);
    assert!(store
        .change_state(&env, SESSION, SessionState::Deleted)
        .unwrap());
    unsafe {
        assert_eq!(
            rusqlite::ffi::sqlite3_set_authorizer(
                store.connection.handle(),
                None,
                std::ptr::null_mut()
            ),
            rusqlite::ffi::SQLITE_OK
        );
    }
}

#[test]
fn exact_slot_load_reads_no_hidden_guest_other_family_or_environment() {
    let fixture = Fixture::new();
    let (_, mut store) = fixture.store();
    let env = environment(AgentKind::Codex);
    let mut ledger = Ledger::default();
    ledger.sessions.insert(SESSION.into(), record());
    ledger.variants.insert(
        SESSION.into(),
        BTreeMap::from([("hidden".into(), record())]),
    );
    ledger
        .sessions
        .insert("00000000-0000-4000-8000-000000000002".into(), record());
    store.save_environment(&env, &ledger).unwrap();
    store
        .save_environment(&environment(AgentKind::ClaudeCode), &ledger)
        .unwrap();
    let key = codecs::environment_key(&store.connection, &env)
        .unwrap()
        .unwrap();
    let expected=store.connection.query_row("SELECT r.record_id FROM session_groups g JOIN session_records r ON r.session_key=g.session_key WHERE g.environment_key=?1 AND g.native_session_id=?2 AND r.variant_key IS NULL",rusqlite::params![key,SESSION],|row|row.get::<_,i64>(0)).unwrap();
    record_reads::begin();
    let selected = store
        .load_slots(
            &env,
            &[RecordSlot {
                session_id: SESSION.into(),
                variant_key: None,
            }],
        )
        .unwrap();
    let reads = record_reads::finish();
    assert_eq!(reads, [expected]);
    assert_eq!(selected.sessions.len(), 1);
    assert!(selected.variants[SESSION].is_empty());
    assert_eq!(
        selected.sessions[SESSION].title,
        ledger.sessions[SESSION].title
    );
}

#[test]
fn guest_slot_switches_keep_row_ids_facts_and_quiet_replays_in_both_directions() {
    let fixture = Fixture::new();
    let (_, mut store) = fixture.store();
    let env = environment(AgentKind::Codex);
    let mut first = record();
    first.guest_namespace = Some("guest:first".into());
    first.title = "first".into();
    let mut second = record();
    second.guest_namespace = Some("guest:second-field".into());
    second.title = "second".into();
    let mut ledger = Ledger::default();
    ledger.sessions.insert(SESSION.into(), first.clone());
    ledger.variants.insert(
        SESSION.into(),
        BTreeMap::from([("guest:second-slot".into(), second.clone())]),
    );
    store.save_environment(&env, &ledger).unwrap();
    let row_ids = || {
        store
            .connection
            .prepare("SELECT record_id,title FROM session_records ORDER BY record_id")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    let before = row_ids();
    ledger.sessions.insert(SESSION.into(), second.clone());
    ledger.variants.insert(
        SESSION.into(),
        BTreeMap::from([("guest:first".into(), first.clone())]),
    );
    assert!(store.save_environment(&env, &ledger).unwrap());
    assert!(!store.save_environment(&env, &ledger).unwrap());
    assert!(store.load_environment(&env).unwrap() == ledger);
    ledger.sessions.clear();
    ledger.variants.insert(
        SESSION.into(),
        BTreeMap::from([
            ("guest:first".into(), first.clone()),
            ("guest:second-slot".into(), second.clone()),
        ]),
    );
    store.save_environment(&env, &ledger).unwrap();
    assert!(store.load_environment(&env).unwrap().sessions.is_empty());
    ledger.sessions.insert(SESSION.into(), first);
    ledger.variants.insert(
        SESSION.into(),
        BTreeMap::from([("guest:second-slot".into(), second)]),
    );
    store.save_environment(&env, &ledger).unwrap();
    assert!(!store.save_environment(&env, &ledger).unwrap());
    let after = store
        .connection
        .prepare("SELECT record_id,title FROM session_records ORDER BY record_id")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(before, after);
}

#[test]
fn records_directory_replacement_cannot_reuse_the_open_file_binding() {
    let fixture = Fixture::new();
    let (records, store) = fixture.store();
    let original = fixture.0.with_extension("original");
    fs::rename(&fixture.0, &original).unwrap();
    fs::create_dir(&fixture.0).unwrap();
    fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o700)).unwrap();
    fs::copy(
        original.join(".tugboat-data.json"),
        fixture.0.join(".tugboat-data.json"),
    )
    .unwrap();
    fs::hard_link(
        original.join("usage.sqlite3"),
        fixture.0.join("usage.sqlite3"),
    )
    .unwrap();
    records.verify().unwrap();
    assert!(store.verify().is_err());
    drop(store);
    fs::remove_dir_all(original).unwrap();
}

#[test]
fn nonprivate_records_folder_is_rejected_before_a_staging_file_is_created() {
    let fixture = Fixture::new();
    let records = RecordsDirectory::fixture(&fixture.0);
    fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Store::create_staging(&records, &fixture.0.join("usage.sqlite3")).is_err());
    assert!(!fixture.0.join("usage.sqlite3").exists());
}

#[test]
fn codex_provider_preparation_rolls_back_and_skips_all_facts_after_commit() {
    let fixture = Fixture::new();
    let (_, mut store) = fixture.store();
    let env = environment(AgentKind::Codex);
    let mut ledger = Ledger::default();
    ledger.sessions.insert(SESSION.into(), record());
    ledger.variants.insert(
        SESSION.into(),
        BTreeMap::from([("historical".into(), record())]),
    );
    store.save_environment(&env, &ledger).unwrap();
    let before = store.load_environment(&env).unwrap();
    store.connection.execute_batch("CREATE TEMP TRIGGER reject_correction BEFORE UPDATE ON usage_requests BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
    assert!(store.prepare_codex(&env).is_err());
    assert!(store.load_environment(&env).unwrap() == before);
    store
        .connection
        .execute_batch("DROP TRIGGER reject_correction")
        .unwrap();
    assert!(store.prepare_codex(&env).unwrap());
    let prepared = store.load_environment(&env).unwrap();
    for saved in prepared.sessions.values().chain(
        prepared
            .variants
            .values()
            .flat_map(|variants| variants.values()),
    ) {
        assert_eq!(saved.available, true);
        assert_eq!(saved.state, SessionState::Archived);
        for request in saved.turns.values().flat_map(|turn| &turn.requests) {
            assert_eq!(request.cost_usd, None);
            assert!(request.rates.is_none());
            assert_eq!(
                request.unpriced_reason.as_deref(),
                Some("Provider price is unavailable")
            );
            assert!(request.provider_correction.is_some());
        }
    }
    record_reads::begin();
    let changes = store.connection.total_changes();
    assert!(!store.prepare_codex(&env).unwrap());
    assert!(record_reads::finish().is_empty());
    assert_eq!(store.connection.total_changes(), changes);
    assert!(store.load_environment(&env).unwrap() == prepared);
}

#[test]
fn mixed_legacy_split_moves_the_anonymous_residual_without_replacing_its_row() {
    let fixture = Fixture::new();
    let (_, mut store) = fixture.store();
    let env = environment(AgentKind::Codex);
    let mut original = record();
    original.guest_namespace = None;
    original.cursors.clear();
    let mut residual = original.turns["bucket-differs"].requests[0].clone();
    residual.source = None;
    residual.key = "unsourced".into();
    original
        .turns
        .get_mut("bucket-differs")
        .unwrap()
        .requests
        .push(residual.clone());
    let mut ledger = Ledger::default();
    ledger.sessions.insert(SESSION.into(), original.clone());
    store.save_environment(&env, &ledger).unwrap();
    let original_id = store
        .connection
        .query_row(
            "SELECT record_id FROM session_records WHERE variant_key IS NULL",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    let mut anonymous = original.clone();
    anonymous.turns.get_mut("bucket-differs").unwrap().requests = vec![residual];
    let mut selected = SessionRecord::default();
    selected.guest_namespace = Some("guest:first".into());
    selected.turns.insert(
        "bucket-differs".into(),
        original.turns["bucket-differs"].clone(),
    );
    selected
        .turns
        .get_mut("bucket-differs")
        .unwrap()
        .requests
        .pop();
    ledger.sessions.insert(SESSION.into(), selected);
    ledger
        .variants
        .insert(SESSION.into(), BTreeMap::from([("".into(), anonymous)]));
    store.save_environment(&env, &ledger).unwrap();
    let residual_id = store
        .connection
        .query_row(
            "SELECT record_id FROM session_records WHERE variant_key=''",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(original_id, residual_id);
    assert!(store.load_environment(&env).unwrap() == ledger);
    assert!(!store.save_environment(&env, &ledger).unwrap());
}

#[test]
fn missing_rollups_rebuild_transactionally_from_streamed_facts() {
    let fixture = Fixture::new();
    let (_, mut store) = fixture.store();
    let env = environment(AgentKind::Codex);
    let mut ledger = Ledger::default();
    ledger.sessions.insert(SESSION.into(), record());
    store.save_environment(&env, &ledger).unwrap();
    let expected = store.load_metadata(&env).unwrap().1;
    store
        .connection
        .execute("DELETE FROM session_rollups", [])
        .unwrap();
    record_reads::begin();
    let rebuilt = store.load_metadata(&env).unwrap().1;
    assert!(record_reads::finish().is_empty());
    assert_eq!(rebuilt, expected);
    store.integrity_check().unwrap();
}

#[test]
fn failed_close_retains_the_bound_connection_until_the_records_folder_returns() {
    let fixture = Fixture::new();
    let (records, mut store) = fixture.store();
    let env = environment(AgentKind::Codex);
    let mut ledger = Ledger::default();
    let mut record = record();
    record.project_exclusion = None;
    ledger.sessions.insert(SESSION.into(), record);
    store.save_environment(&env, &ledger).unwrap();
    let disconnected = fixture.0.with_extension("disconnected");
    fs::rename(&fixture.0, &disconnected).unwrap();
    let (mut store, failure) = store
        .try_close()
        .err()
        .expect("the missing records folder must block close");
    assert!(failure.contains("unavailable"));
    assert!(!fixture.0.exists());
    fs::rename(&disconnected, &fixture.0).unwrap();
    assert!(store.load_environment(&env).unwrap() == ledger);
    store
        .change_state(&env, SESSION, SessionState::Deleted)
        .unwrap();
    assert!(store.try_close().is_ok());
    let reopened = Store::open(&records).unwrap();
    assert_eq!(
        reopened.load_environment(&env).unwrap().sessions[SESSION].state,
        SessionState::Deleted
    );
}

#[test]
fn failed_close_preserves_a_pending_transaction_for_an_explicit_retry() {
    let fixture = Fixture::new();
    let (records, mut store) = fixture.store();
    let env = environment(AgentKind::Codex);
    let mut ledger = Ledger::default();
    let mut record = record();
    record.project_exclusion = None;
    ledger.sessions.insert(SESSION.into(), record);
    store.save_environment(&env, &ledger).unwrap();
    store
        .connection
        .execute_batch("BEGIN IMMEDIATE; UPDATE session_records SET state='deleted';")
        .unwrap();
    let (mut store, failure) = store
        .try_close()
        .err()
        .expect("an unfinished transaction must block close");
    assert!(failure.contains("unfinished transaction"));
    assert!(!store.connection.is_autocommit());
    assert_eq!(
        store.load_environment(&env).unwrap().sessions[SESSION].state,
        SessionState::Deleted
    );
    store.connection.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        store.load_environment(&env).unwrap().sessions[SESSION].state,
        SessionState::Archived
    );
    store
        .change_state(&env, SESSION, SessionState::Deleted)
        .unwrap();
    assert!(store.try_close().is_ok());
    let reopened = Store::open(&records).unwrap();
    assert_eq!(
        reopened.load_environment(&env).unwrap().sessions[SESSION].state,
        SessionState::Deleted
    );
}

#[test]
fn sqlite_close_busy_retains_the_original_connection_and_can_retry() {
    let fixture = Fixture::new();
    let (records, store) = fixture.store();
    let mut statement = std::ptr::null_mut();
    unsafe {
        assert_eq!(
            rusqlite::ffi::sqlite3_prepare_v2(
                store.connection.handle(),
                c"SELECT 1".as_ptr(),
                -1,
                &mut statement,
                std::ptr::null_mut()
            ),
            rusqlite::ffi::SQLITE_OK
        );
    }
    let (store, failure) = store
        .try_close()
        .err()
        .expect("an unfinalized statement must block SQLite close");
    assert!(failure.contains("unfinalized statements"));
    store.verify().unwrap();
    unsafe {
        assert_eq!(
            rusqlite::ffi::sqlite3_finalize(statement),
            rusqlite::ffi::SQLITE_OK
        );
    }
    assert!(store.try_close().is_ok());
    Store::open(&records).unwrap().integrity_check().unwrap();
}

#[test]
fn verified_reconnect_closes_the_missing_path_handle_and_keeps_the_target_writable() {
    let fixture = Fixture::new();
    let original = fixture.0.join("original");
    fs::create_dir(&original).unwrap();
    let records = RecordsDirectory::fixture(&original);
    let mut source = Store::create_staging(&records, &original.join("usage.sqlite3")).unwrap();
    records.activate_sqlite().unwrap();
    let env = environment(AgentKind::Codex);
    let mut ledger = Ledger::default();
    let mut record = record();
    record.project_exclusion = None;
    ledger.sessions.insert(SESSION.into(), record);
    source.save_environment(&env, &ledger).unwrap();
    let marker = fs::read(original.join(".tugboat-data.json")).unwrap();
    let relocated = fixture.0.join("relocated");
    fs::rename(&original, &relocated).unwrap();
    let target_records = RecordsDirectory::fixture_existing(&relocated, records.id());
    let mut target = Store::open(&target_records).unwrap();
    assert!(source.verify().is_err());
    source.connection.execute_batch("BEGIN").unwrap();
    let (source, failure) = source
        .try_close_for_reconnect(&target)
        .err()
        .expect("reconnect must preserve an unfinished old transaction");
    assert!(failure.contains("unfinished transaction"));
    assert!(!source.connection.is_autocommit());
    source.connection.execute_batch("ROLLBACK").unwrap();
    assert!(source.try_close_for_reconnect(&target).is_ok());
    assert!(!original.exists());
    assert_eq!(
        fs::read(relocated.join(".tugboat-data.json")).unwrap(),
        marker
    );
    assert!(target.load_environment(&env).unwrap() == ledger);
    target
        .change_state(&env, SESSION, SessionState::Deleted)
        .unwrap();
    target.close().unwrap();
    let reopened = Store::open(&target_records).unwrap();
    assert_eq!(
        reopened.load_environment(&env).unwrap().sessions[SESSION].state,
        SessionState::Deleted
    );
}

#[test]
fn reconnect_to_a_valid_foreign_dataset_preserves_both_connections_and_files() {
    let fixture = Fixture::new();
    let original = fixture.0.join("original");
    let foreign = fixture.0.join("foreign");
    fs::create_dir(&original).unwrap();
    fs::create_dir(&foreign).unwrap();
    let records = RecordsDirectory::fixture(&original);
    let source = Store::create_staging(&records, &original.join("usage.sqlite3")).unwrap();
    records.activate_sqlite().unwrap();
    let foreign_records = RecordsDirectory::fixture_with_id(&foreign, &"b".repeat(32));
    let target = Store::create_staging(&foreign_records, &foreign.join("usage.sqlite3")).unwrap();
    foreign_records.activate_sqlite().unwrap();
    let source_bytes = fs::read(original.join("usage.sqlite3")).unwrap();
    let target_bytes = fs::read(foreign.join("usage.sqlite3")).unwrap();
    let (source, failure) = source
        .try_close_for_reconnect(&target)
        .err()
        .expect("a valid foreign dataset must not replace this dataset");
    assert!(failure.contains("different records folder"));
    source.verify().unwrap();
    target.verify().unwrap();
    assert_eq!(
        fs::read(original.join("usage.sqlite3")).unwrap(),
        source_bytes
    );
    assert_eq!(
        fs::read(foreign.join("usage.sqlite3")).unwrap(),
        target_bytes
    );
    source.close().unwrap();
    target.close().unwrap();
}

#[test]
fn claude_request_owner_lookup_keeps_projects_environments_and_hidden_slots_separate() {
    let fixture = Fixture::new();
    let (_, mut store) = fixture.store();
    let selected = environment(AgentKind::ClaudeCode);
    let other_agent = environment(AgentKind::Codex);
    let other_project_id = "22222222222222222222222222222222";
    let other_project = Environment::new(
        other_project_id.into(),
        EnvironmentId::for_agent(other_project_id, AgentKind::ClaudeCode),
        AgentKind::ClaudeCode,
    )
    .unwrap();
    let hidden_session = "00000000-0000-4000-8000-000000000002";
    let foreign_session = "00000000-0000-4000-8000-000000000003";
    let mut ledger = Ledger::default();
    ledger.sessions.insert(SESSION.into(), record());
    ledger.variants.insert(
        hidden_session.into(),
        BTreeMap::from([("hidden".into(), record())]),
    );
    store.save_environment(&selected, &ledger).unwrap();
    let mut foreign = Ledger::default();
    let mut same_session_without_requests = record();
    same_session_without_requests.turns.clear();
    foreign
        .sessions
        .insert(SESSION.into(), same_session_without_requests);
    foreign.sessions.insert(foreign_session.into(), record());
    store.save_environment(&other_agent, &foreign).unwrap();
    store.save_environment(&other_project, &foreign).unwrap();
    let identities = std::collections::BTreeSet::from([(
        "guest:source".into(),
        "anthropic".into(),
        "same-response".into(),
    )]);
    record_reads::begin();
    assert_eq!(
        store
            .claude_request_owner_ids(&selected, identities.clone())
            .unwrap(),
        [SESSION]
    );
    assert_eq!(
        store
            .claude_request_owner_ids(&other_agent, identities.clone())
            .unwrap(),
        [foreign_session]
    );
    assert_eq!(
        store
            .claude_request_owner_ids(&other_project, identities)
            .unwrap(),
        [foreign_session]
    );
    assert!(record_reads::finish().is_empty());
}
