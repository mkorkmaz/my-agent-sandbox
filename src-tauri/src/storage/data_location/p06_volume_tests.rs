//! Runs only against a disposable volume created by the P06 driver.
use super::*;
use crate::{
    agents::AgentKind,
    conversations::Ledger,
    projects::EnvironmentId,
    usage_store::{Environment, Store},
};
use serde_json::{json, Value};
use std::os::unix::fs::MetadataExt;

const PROJECT: &str = "11111111111111111111111111111111";
const SESSION: &str = "aaaaaaaa-0000-4000-8000-000000000001";
const FILES: &[&str] = &[
    "usage.sqlite3",
    "conversation-ledgers/11111111111111111111111111111111.json",
    "conversation-ledgers/11111111111111111111111111111111.json.v1.backup",
    "interface.json",
    "settings.json",
];

fn environment() -> Environment {
    Environment::new(
        PROJECT.into(),
        EnvironmentId::for_agent(PROJECT, AgentKind::Codex),
        AgentKind::Codex,
    )
    .unwrap()
}

fn records(location: &DataLocation) -> RecordsDirectory {
    let saved = location.saved.lock().unwrap();
    let current = saved.location.as_ref().unwrap();
    RecordsDirectory {
        directory: current.directory.clone(),
        id: current.directory_id.clone(),
    }
}

fn ledger() -> Ledger {
    serde_json::from_value(json!({"schemaVersion":2,"warnings":["volume fixture"],"sessions":{SESSION:{
        "title":"Generated volume fixture", "updatedAt":1, "state":"archived", "agentKind":"codex",
        "guestNamespace":"guest:p06-volume", "available":false, "parserVersion":crate::usage::PARSER_VERSION,
        "cursors":{}, "turns":{"stored-key":{"id":"turn-1","status":"complete","requests":[{
            "key":"volume-request","at":"2026-11-13T00:00:00.123456Z","model":"gpt-6.1-sol",
            "tokens":{"input":100,"cached":0,"cacheWrite":0,"output":10,"reasoning":0,"write12h":null,
                "cacheKnown":true,"cachedKnown":true,"cacheWriteKnown":true},
            "partial":false,"costUsd":0.125,
            "source":{"guestNamespace":"guest:p06-volume","modelProvider":"openai","originThreadId":SESSION,
                "responseId":"volume-response","originTurnId":"turn-1","rootTurnId":"turn-1"}
        }]}}
    }}})).unwrap()
}

fn hashes(directory: &Path) -> Value {
    let mut result = serde_json::Map::new();
    for relative in FILES {
        let bytes = fs::read(directory.join(relative)).unwrap();
        result.insert(
            (*relative).into(),
            json!(crate::hash::hex(Sha256::digest(bytes))),
        );
    }
    Value::Object(result)
}

fn check_dataset(directory: &Path, expected: &Value) {
    assert_eq!(marker(directory).unwrap().schema_version, 2);
    assert_eq!(
        marker(directory).unwrap().id,
        expected["directoryId"].as_str().unwrap()
    );
    assert_eq!(hashes(directory), expected["hashes"]);
    let records = RecordsDirectory {
        directory: directory.into(),
        id: marker(directory).unwrap().id,
    };
    let store = Store::open(&records).unwrap();
    store.integrity_check().unwrap();
    assert_eq!(store.load_environment(&environment()).unwrap(), ledger());
    store.close().unwrap();
    assert_eq!(
        fs::metadata(directory.join("usage.sqlite3"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

fn seed_and_move(root: &Path, mount: &Path) -> Value {
    let default = root.join("data/default");
    let legacy = root.join("data/legacy");
    let location = DataLocation::open(default.clone(), legacy.clone());
    assert!(location.status().unwrap().available);
    let records = records(&location);
    Store::create_staging(&records, &records.path().join("usage.sqlite3"))
        .unwrap()
        .close()
        .unwrap();
    records.activate_sqlite().unwrap();
    let mut store = Store::open(&records).unwrap();
    store.save_environment(&environment(), &ledger()).unwrap();
    store.close().unwrap();
    storage::write(&records.path().join(FILES[1]), &ledger()).unwrap();
    storage::write_bytes(
        &records.path().join(FILES[2]),
        b"immutable generated historical backup\n",
    )
    .unwrap();
    storage::write_bytes(
        &records.path().join("interface.json"),
        b"{\"fixture\":\"unchanged\"}\n",
    )
    .unwrap();
    storage::write_bytes(
        &records.path().join("settings.json"),
        b"{\"fixture\":\"private settings\"}\n",
    )
    .unwrap();
    storage::write_bytes(
        &root.join("data/guest/transcript.jsonl"),
        b"generated native transcript remains outside records\n",
    )
    .unwrap();
    let source_device = fs::metadata(records.path()).unwrap().dev();
    let target_device = fs::metadata(mount).unwrap().dev();
    assert_ne!(
        source_device, target_device,
        "The fixture must use a different mounted filesystem."
    );
    let target = mount.join("records");
    create_directory(&target).unwrap();
    let mut expected = json!({"directoryId":records.id(),"hashes":hashes(records.path()),
        "sourceDevice":source_device,"mountedDevice":target_device,"phase":"move-to-volume"});
    location.schedule(&target, false).unwrap();
    drop(location);
    let moved = DataLocation::open(default, legacy);
    assert_eq!(moved.status().unwrap().directory, Some(target.clone()));
    assert!(moved.status().unwrap().message.is_none());
    check_dataset(&target, &expected);
    expected["pointerSha256"] = json!(crate::hash::hex(Sha256::digest(fs::read(&moved.pointer).unwrap())));
    storage::write_bytes(
        &root.join("expected.json"),
        &serde_json::to_vec_pretty(&expected).unwrap(),
    )
    .unwrap();
    expected
}

fn probe_detached(root: &Path, mount: &Path) -> Value {
    let expected: Value =
        serde_json::from_slice(&fs::read(root.join("expected.json")).unwrap()).unwrap();
    assert!(!mount.join("records").exists());
    let location = DataLocation::open(root.join("data/default"), root.join("data/legacy"));
    assert!(!location.status().unwrap().available);
    assert!(location.status().unwrap().message.is_some());
    let records = records(&location);
    assert!(Store::open(&records).is_err());
    assert!(location
        .schedule(&root.join("data/default"), false)
        .is_err());
    assert_eq!(
        crate::hash::hex(Sha256::digest(fs::read(&location.pointer).unwrap())),
        expected["pointerSha256"].as_str().unwrap()
    );
    assert!(!mount.join("records").exists());
    assert!(!root.join("data/default/usage.sqlite3").exists());
    json!({"phase":"detached-probe","available":false,"pointerUnchanged":true,"replacementCreated":false})
}

fn move_back(root: &Path, mount: &Path) -> Value {
    let expected: Value =
        serde_json::from_slice(&fs::read(root.join("expected.json")).unwrap()).unwrap();
    let default = root.join("data/default");
    let legacy = root.join("data/legacy");
    let location = DataLocation::open(default.clone(), legacy.clone());
    assert!(location.status().unwrap().available);
    assert!(location.status().unwrap().message.is_none());
    check_dataset(&mount.join("records"), &expected);
    location.schedule(&default, false).unwrap();
    drop(location);
    let restored = DataLocation::open(default.clone(), legacy);
    assert_eq!(restored.status().unwrap().directory, Some(default.clone()));
    assert!(restored.status().unwrap().message.is_none());
    check_dataset(&default, &expected);
    assert_eq!(
        fs::read(root.join("data/guest/transcript.jsonl")).unwrap(),
        b"generated native transcript remains outside records\n"
    );
    json!({"phase":"move-back","hashes":hashes(&default),"directoryId":marker(&default).unwrap().id,
        "available":true,"nativeTranscriptUnchanged":true,"databaseMode":"0600","folderMode":"0700"})
}

#[test]
#[ignore = "requires the disposable mounted-volume P06 driver"]
fn p06_disposable_volume_worker() {
    let root =
        PathBuf::from(std::env::var_os("TUGBOAT_P06_VOLUME_ROOT").expect("temporary fixture root"));
    let owner: Value =
        serde_json::from_slice(&fs::read(root.join("fixture-owner.json")).unwrap()).unwrap();
    assert_eq!(owner["kind"], "tugboat-p06-volume-fixture-v1");
    assert!(root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("tugboat-p06-volume-"));
    let mount = PathBuf::from(owner["mount"].as_str().unwrap());
    assert_eq!(mount.parent(), Some(root.as_path()));
    let phase = std::env::var("TUGBOAT_P06_VOLUME_PHASE").unwrap();
    let result = match phase.as_str() {
        "seed-move" => seed_and_move(&root, &mount),
        "detached-probe" => probe_detached(&root, &mount),
        "move-back" => move_back(&root, &mount),
        _ => panic!("Unknown fixture phase"),
    };
    storage::write_bytes(
        &root.join(format!("{phase}.json")),
        &serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
}
