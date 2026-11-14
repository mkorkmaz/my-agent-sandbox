use super::*;
use crate::{
    agents::AgentKind, conversations::Ledger, projects::EnvironmentId, usage_store::Environment,
};
use serde_json::{json, Value};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;
use std::process::Command;

const PROJECT: &str = "11111111111111111111111111111111";
const SESSION: &str = "00000000-0000-4000-8000-000000000001";
static NEXT_TEST: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "tugboat-sqlite-import-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn records(&self) -> RecordsDirectory {
        RecordsDirectory::fixture(&self.0)
    }
    fn ledger(&self, kind: AgentKind, version: u32) -> PathBuf {
        let path = if kind == AgentKind::Codex {
            self.0.join(format!("conversation-ledgers/{PROJECT}.json"))
        } else {
            self.0.join(format!(
                "conversation-ledgers/{PROJECT}/{}.json",
                EnvironmentId::for_agent(PROJECT, kind)
            ))
        };
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_vec(&legacy(version)).unwrap()).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn legacy(version: u32) -> Value {
    let request = json!({"key":"source:copied","at":"2026-11-13T01:02:03.000123Z","model":"removed-model","tokens":{"input":10,"cached":2,"cacheWrite":3,"output":4,"reasoning":1,"write12h":null,"cacheKnown":false,"cachedKnown":true,"cacheWriteKnown":null},"rates":null,"costUsd":0.012345678901234567,"partial":true,"unpricedReason":null,"source":{"guestNamespace":"guest:original","modelProvider":"anthropic","originThreadId":SESSION,"originTurnId":"public-turn","rootTurnId":null,"responseId":"same-response"}});
    let record = json!({"title":"Recorded title","updatedAt":123,"state":"archived","available":true,"guestNamespace":"guest:row-field","turns":{"map-key-differs":{"id":"public-turn","status":"complete","requests":[request.clone(),request],"originThreadId":SESSION},"empty":{"id":"empty","status":"in-progress","requests":[]}},"cursors":{}});
    json!({"schemaVersion":version,"variants":{SESSION:{"":record.clone(),"guest:slot-key":record.clone()},"00000000-0000-4000-8000-000000000002":{}},"sessions":{SESSION:record},"warnings":["second","first","second"]})
}

fn environment(kind: AgentKind) -> Environment {
    Environment::new(
        PROJECT.into(),
        EnvironmentId::for_agent(PROJECT, kind),
        kind,
    )
    .unwrap()
}

#[test]
fn empty_installation_activates_private_bound_database_without_receipts() {
    let fixture = Fixture::new();
    let records = fixture.records();
    activate(&records).unwrap();
    assert_eq!(records.schema_version().unwrap(), 2);
    assert_eq!(
        fs::metadata(fixture.0.join(DATABASE))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let store = Store::open(&records).unwrap();
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM legacy_imports", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    store.close().unwrap();
    activate(&records).unwrap();
}

#[test]
fn raw_v1_v2_import_preserves_slots_keys_prices_backups_and_reopen_identity() {
    let fixture = Fixture::new();
    let records = fixture.records();
    let codex = fixture.ledger(AgentKind::Codex, 1);
    let claude = fixture.ledger(AgentKind::ClaudeCode, 2);
    let original_codex = fs::read(&codex).unwrap();
    let original_claude = fs::read(&claude).unwrap();
    let prior_backup = crate::storage::v1_backup_path(&claude);
    fs::write(&prior_backup, b"an older v1 snapshot").unwrap();
    activate(&records).unwrap();
    let store = Store::open(&records).unwrap();
    for (kind, path) in [(AgentKind::Codex, &codex), (AgentKind::ClaudeCode, &claude)] {
        let mut typed: Ledger = serde_json::from_value(
            serde_json::from_slice::<Value>(&fs::read(path).unwrap()).unwrap(),
        )
        .unwrap();
        typed.schema_version = 2;
        assert_eq!(
            serde_json::to_value(store.load_environment(&environment(kind)).unwrap()).unwrap(),
            serde_json::to_value(typed).unwrap()
        );
    }
    assert_eq!(store.connection.query_row("SELECT claude_pricing_version FROM usage_environments WHERE agent_kind='claude-code'",[],|row|row.get::<_,i64>(0)).unwrap(),0);
    assert_eq!(fs::read(&codex).unwrap(), original_codex);
    assert_eq!(fs::read(&claude).unwrap(), original_claude);
    assert_eq!(
        fs::read(crate::storage::v1_backup_path(&codex)).unwrap(),
        original_codex
    );
    assert_eq!(fs::read(&prior_backup).unwrap(), b"an older v1 snapshot");
    store.close().unwrap();
    activate(&records).unwrap();
    assert_eq!(
        Store::open(&records)
            .unwrap()
            .connection
            .query_row("SELECT count(*) FROM usage_requests", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        12
    );
}

#[test]
fn malformed_unknown_duplicate_oversized_integer_and_backup_conflicts_never_activate() {
    for case in [
        "version",
        "unknown",
        "duplicate",
        "integer",
        "backup",
        "nested-path",
        "bad-id",
    ] {
        let fixture = Fixture::new();
        let records = fixture.records();
        let ledger = fixture.ledger(AgentKind::Codex, 1);
        let mut value = legacy(1);
        match case {
            "version" => value["schemaVersion"] = json!(3),
            "unknown" => value["sessions"][SESSION]["newFact"] = json!({"mustNotDiscard":true}),
            "integer" => value["sessions"][SESSION]["updatedAt"] = json!(u64::MAX),
            "bad-id" => {
                let record = value["sessions"]
                    .as_object_mut()
                    .unwrap()
                    .remove(SESSION)
                    .unwrap();
                value["sessions"]["--bad"] = record;
            }
            "backup" => {
                fs::write(
                    crate::storage::v1_backup_path(&ledger),
                    "different original",
                )
                .unwrap();
            }
            "nested-path" => {
                fs::create_dir(fixture.0.join("conversation-ledgers/unexpected")).unwrap();
            }
            _ => {}
        }
        let bytes = if case == "duplicate" {
            format!(
                "{{\"schemaVersion\":1,\"sessions\":{{\"{SESSION}\":{},\"{SESSION}\":{}}}}}",
                value["sessions"][SESSION], value["sessions"][SESSION]
            )
            .into_bytes()
        } else {
            serde_json::to_vec(&value).unwrap()
        };
        fs::write(&ledger, &bytes).unwrap();
        assert!(activate(&records).is_err(), "{case}");
        assert_eq!(records.schema_version().unwrap(), 1, "{case}");
        assert_eq!(fs::read(&ledger).unwrap(), bytes, "{case}");
        assert!(!fixture.0.join(DATABASE).exists(), "{case}");
    }
}

#[test]
fn foreign_staging_symlink_and_missing_active_database_are_preserved() {
    let fixture = Fixture::new();
    let records = fixture.records();
    fixture.ledger(AgentKind::Codex, 2);
    let unrelated = fixture.0.join("unrelated");
    fs::write(&unrelated, b"unrelated data").unwrap();
    symlink(&unrelated, fixture.0.join(STAGING)).unwrap();
    assert!(activate(&records).is_err());
    assert_eq!(fs::read(&unrelated).unwrap(), b"unrelated data");
    assert!(fixture
        .0
        .join(STAGING)
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());
    fs::remove_file(fixture.0.join(STAGING)).unwrap();
    let wrong = Fixture::new();
    let wrong_records = wrong.records();
    Store::create_staging(&wrong_records, &wrong.0.join(STAGING))
        .unwrap()
        .close()
        .unwrap();
    let foreign = rusqlite::Connection::open(wrong.0.join(STAGING)).unwrap();
    foreign
        .execute(
            "UPDATE store_identity SET records_directory_id=?1",
            ["b".repeat(32)],
        )
        .unwrap();
    drop(foreign);
    fs::copy(wrong.0.join(STAGING), fixture.0.join(STAGING)).unwrap();
    fs::set_permissions(fixture.0.join(STAGING), fs::Permissions::from_mode(0o600)).unwrap();
    let bytes = fs::read(fixture.0.join(STAGING)).unwrap();
    assert!(activate(&records).is_err());
    assert_eq!(fs::read(fixture.0.join(STAGING)).unwrap(), bytes);
    fs::remove_file(fixture.0.join(STAGING)).unwrap();
    activate(&records).unwrap();
    fs::remove_file(fixture.0.join(DATABASE)).unwrap();
    assert!(activate(&records).is_err());
    assert!(!fixture.0.join(DATABASE).exists());
}

#[test]
fn interrupted_import_boundaries_recover_without_duplicates_or_json_writes() {
    for phase in [
        "bootstrap-committed",
        "stage-published",
        "facts-written-before-commit",
        "facts-committed",
        "parity-complete",
        "canonical-published",
        "before-marker",
        "marker-activated",
    ] {
        let fixture = Fixture::new();
        let records = fixture.records();
        let ledger = fixture.ledger(AgentKind::ClaudeCode, 2);
        let original = fs::read(&ledger).unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "usage_store::importer::tests::import_subprocess",
            ])
            .env("TUGBOAT_IMPORT_TEST_DIRECTORY", &fixture.0)
            .env("TUGBOAT_IMPORT_FAILPOINT", phase)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(91), "{phase}");
        assert_eq!(fs::read(&ledger).unwrap(), original, "{phase}");
        activate(&records).unwrap();
        assert_eq!(records.schema_version().unwrap(), 2, "{phase}");
        let store = Store::open(&records).unwrap();
        assert_eq!(
            store
                .connection
                .query_row("SELECT count(*) FROM usage_requests", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            6,
            "{phase}"
        );
        store.integrity_check().unwrap();
    }
}

#[test]
#[ignore = "only runs as a generated-data interruption subprocess"]
fn import_subprocess() {
    let path = PathBuf::from(
        std::env::var_os("TUGBOAT_IMPORT_TEST_DIRECTORY").expect("isolated test directory"),
    );
    let marker: Value =
        serde_json::from_slice(&fs::read(path.join(".tugboat-data.json")).unwrap()).unwrap();
    assert_eq!(marker["id"], "a".repeat(32));
    assert_eq!(marker["schemaVersion"], 1);
    let records = RecordsDirectory::fixture(&path);
    activate(&records).unwrap();
}

#[test]
fn streamed_rollups_match_domain_with_aliases_owners_unknowns_and_cursors() {
    for kind in [AgentKind::Codex, AgentKind::ClaudeCode] {
        let fixture = Fixture::new();
        let records = fixture.records();
        let path = fixture.ledger(kind, 2);
        let mut source = legacy(2);
        let record = &mut source["sessions"][SESSION];
        record["agentKind"] = json!(kind);
        record.as_object_mut().unwrap().remove("guestNamespace");
        let mut exact = record["turns"]["map-key-differs"]["requests"][0].clone();
        exact["key"] = json!("canonical exact");
        exact["source"]["guestNamespace"] = json!("guest:cursor-owner");
        exact["source"]["rootTurnId"] = json!("root-turn");
        exact["costUsd"] = json!(0.0);
        let mut foreign = exact.clone();
        foreign["source"]["originThreadId"] = json!("00000000-0000-4000-8000-000000000099");
        let mut legacy_request = exact.clone();
        legacy_request["source"] = Value::Null;
        legacy_request["costUsd"] = Value::Null;
        record["turns"]["a-alias"] = json!({"id":"public-turn","status":"complete","requests":[legacy_request.clone(),exact.clone()],"originThreadId":SESSION});
        record["turns"]["b-foreign"] = json!({"id":"copied","status":"complete","requests":[exact.clone(),foreign],"originThreadId":"foreign-session"});
        record["turns"]["c-override"] = json!({"id":"public-turn","status":"in-progress","requests":[legacy_request],"originThreadId":SESSION});
        record["turns"]["orphan-attribution"] = json!({"id":"different-public-id","status":"complete","requests":[exact],"originThreadId":SESSION});
        let mut cursor = serde_json::to_value(crate::usage::Cursor::default()).unwrap();
        cursor["namespace"] = json!("guest:cursor-owner");
        cursor["pendingBytes"] = json!(true);
        cursor["ownershipProblem"] = json!("fixture ownership evidence");
        record["cursors"] = json!({"/fixture/saved/transcript.jsonl":cursor});
        fs::write(&path, serde_json::to_vec(&source).unwrap()).unwrap();
        let typed: Ledger = serde_json::from_value(
            serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap(),
        )
        .unwrap();
        activate(&records).unwrap();
        let store = Store::open(&records).unwrap();
        let (_, rollups) = store.load_metadata(&environment(kind)).unwrap();
        for (slot, actual) in rollups {
            let record = match slot.variant_key {
                Some(key) => &typed.variants[&slot.session_id][&key],
                None => &typed.sessions[&slot.session_id],
            };
            let expected =
                crate::conversations::sql_projection::rollup(&slot.session_id, record).unwrap();
            assert_eq!(actual, expected, "{kind:?} {}", slot.session_id);
        }
    }
}

#[test]
fn ledger_larger_than_reader_buffer_and_large_single_turn_stream_without_total_limit() {
    use std::io::Write;
    let fixture = Fixture::new();
    let records = fixture.records();
    let path = fixture.ledger(AgentKind::ClaudeCode, 2);
    let mut output = File::create(&path).unwrap();
    output
        .write_all(b"{\"schemaVersion\":1,\"sessions\":{")
        .unwrap();
    let template = legacy(2)["sessions"][SESSION].clone();
    for number in 0..300 {
        if number != 0 {
            output.write_all(b",").unwrap();
        }
        write!(
            output,
            "\"{number:08x}-0000-4000-8000-000000000001\":{template}"
        )
        .unwrap();
    }
    output.write_all(b"},\"variants\":{}}").unwrap();
    output.sync_all().unwrap();
    drop(output);
    assert!(fs::metadata(&path).unwrap().len() > 65_536);
    let codex = fixture.ledger(AgentKind::Codex, 2);
    let mut output = File::create(&codex).unwrap();
    write!(output,"{{\"schemaVersion\":2,\"sessions\":{{\"{SESSION}\":{{\"title\":\"large record\",\"updatedAt\":1,\"state\":\"active\",\"turns\":{{\"turn\":{{\"requests\":[").unwrap();
    let request = &template["turns"]["map-key-differs"]["requests"][0];
    for number in 0..5000 {
        if number != 0 {
            output.write_all(b",").unwrap();
        }
        write!(output, "{request}").unwrap();
    }
    output
        .write_all(b"],\"status\":\"complete\",\"id\":\"turn\"}},\"cursors\":{}}}}")
        .unwrap();
    output.sync_all().unwrap();
    drop(output);
    assert!(fs::metadata(&codex).unwrap().len() > 65_536);
    activate(&records).unwrap();
    let store = Store::open(&records).unwrap();
    let request_count: i64 = store
        .connection
        .query_row("SELECT count(*) FROM usage_requests", [], |row| row.get(0))
        .unwrap();
    assert_eq!(request_count, 5600);
    store.integrity_check().unwrap();
}

#[test]
fn published_unactivated_database_rejects_changed_source_without_replacing_either_copy() {
    let fixture = Fixture::new();
    let records = fixture.records();
    let ledger = fixture.ledger(AgentKind::Codex, 2);
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "usage_store::importer::tests::import_subprocess",
        ])
        .env("TUGBOAT_IMPORT_TEST_DIRECTORY", &fixture.0)
        .env("TUGBOAT_IMPORT_FAILPOINT", "canonical-published")
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(91));
    let database = fs::read(fixture.0.join(DATABASE)).unwrap();
    let mut changed = legacy(2);
    changed["sessions"][SESSION]["title"] = json!("Changed after publication");
    let source = serde_json::to_vec(&changed).unwrap();
    fs::write(&ledger, &source).unwrap();
    assert!(activate(&records).is_err());
    assert_eq!(records.schema_version().unwrap(), 1);
    assert_eq!(fs::read(fixture.0.join(DATABASE)).unwrap(), database);
    assert_eq!(fs::read(&ledger).unwrap(), source);
}

#[test]
fn existing_stage_requires_raw_unprepared_receipts_before_reuse() {
    for mutation in [
        "prepared",
        "missing-receipt",
        "wrong-count",
        "newer-state",
        "changed-source",
        "changed-warnings",
        "receipt-path",
        "extra-json-path",
        "projection",
    ] {
        let fixture = Fixture::new();
        let records = fixture.records();
        let ledger = fixture.ledger(AgentKind::ClaudeCode, 2);
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "usage_store::importer::tests::import_subprocess",
            ])
            .env("TUGBOAT_IMPORT_TEST_DIRECTORY", &fixture.0)
            .env("TUGBOAT_IMPORT_FAILPOINT", "facts-committed")
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(91));
        let staging = fixture.0.join(STAGING);
        let store = Store::open_at(&records, &staging).unwrap();
        match mutation {
            "prepared" => {
                store
                    .connection
                    .execute("UPDATE usage_environments SET claude_pricing_version=1", [])
                    .unwrap();
                store
                    .connection
                    .execute(
                        "UPDATE session_records SET state='deleted' WHERE variant_key IS NULL",
                        [],
                    )
                    .unwrap();
            }
            "missing-receipt" => {
                store
                    .connection
                    .execute("DELETE FROM legacy_imports", [])
                    .unwrap();
            }
            "wrong-count" => {
                store
                    .connection
                    .execute(
                        "UPDATE legacy_imports SET request_count=request_count+1",
                        [],
                    )
                    .unwrap();
            }
            "newer-state" => {
                store
                    .connection
                    .execute(
                        "UPDATE session_records SET state='deleted' WHERE variant_key IS NULL",
                        [],
                    )
                    .unwrap();
            }
            "changed-source" => {
                let mut changed = legacy(2);
                changed["sessions"][SESSION]["title"] = json!("Changed during interrupted import");
                fs::write(&ledger, serde_json::to_vec(&changed).unwrap()).unwrap();
            }
            "changed-warnings" => {
                store
                    .connection
                    .execute(
                        "UPDATE usage_environments SET warnings_json='[\"different warning\"]'",
                        [],
                    )
                    .unwrap();
            }
            "receipt-path" => {
                store.connection.execute("UPDATE legacy_imports SET relative_path='conversation-ledgers/unexpected.json'",[]).unwrap();
            }
            "extra-json-path" => {
                fs::write(
                    fixture.0.join("conversation-ledgers/unexpected.json"),
                    b"{\"schemaVersion\":2,\"sessions\":{}}",
                )
                .unwrap();
            }
            "projection" => {
                store.connection.execute("UPDATE session_rollups SET own_summary_json=json_set(own_summary_json,'$.costUsd',99.0)",[]).unwrap();
            }
            _ => unreachable!(),
        }
        store.close().unwrap();
        let staged = fs::read(&staging).unwrap();
        let original = fs::read(&ledger).unwrap();
        assert!(activate(&records).is_err(), "{mutation}");
        assert_eq!(records.schema_version().unwrap(), 1, "{mutation}");
        assert_eq!(fs::read(&staging).unwrap(), staged, "{mutation}");
        assert_eq!(fs::read(&ledger).unwrap(), original, "{mutation}");
        assert!(!fixture.0.join(DATABASE).exists(), "{mutation}");
    }
}
