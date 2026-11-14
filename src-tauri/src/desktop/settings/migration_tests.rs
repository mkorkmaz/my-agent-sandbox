use super::*;
use serde_json::json;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "tugboat-settings-migration-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn legacy_settings_keep_custom_codex_defaults_and_create_separate_claude_defaults() {
    for version in [0, 1] {
        let fixture = Fixture::new();
        let file = fixture.0.join("settings.json");
        let records = crate::storage::data_location::RecordsDirectory::fixture(&fixture.0);
        let legacy = json!({"schemaVersion":version,"profile":"custom-codex","allowedHosts":["private.example"],
            "cliPath":"/custom/brig","cpus":2,"memoryMb":8192,"verification":"warn","network":"offline","telemetry":true});
        storage::write(&file, &legacy).unwrap();
        let original = fs::read(&file).unwrap();
        let preview = load(&file).unwrap();
        assert_eq!(
            fs::read(&file).unwrap(),
            original,
            "Reading a legacy record must not perform an unguarded write"
        );
        assert_eq!(preview.codex.profile, "custom-codex");
        assert_eq!(preview.codex.allowed_hosts, vec!["private.example"]);
        assert_eq!(preview.claude_code.profile, "tugboat-claude");
        assert_eq!(preview.network, Network::Offline);
        assert_eq!(preview.memory_mb, 8192);
        assert_eq!(preview.cli_path.as_deref(), Some("/custom/brig"));
        assert!(preview.telemetry);
        assert!(migrate(&file, &records).unwrap());
        assert_eq!(fs::read(storage::v1_backup_path(&file)).unwrap(), original);
        assert_eq!(load(&file).unwrap(), preview);
        assert!(!migrate(&file, &records).unwrap());
        let value: serde_json::Value = storage::read(&file).unwrap().unwrap();
        assert_eq!(value["schemaVersion"], 2);
        assert!(value.get("profile").is_none());
        assert_eq!(value["codex"]["profile"], "custom-codex");
    }
}

#[test]
fn malformed_or_unsupported_settings_and_missing_records_marker_are_preserved() {
    let fixture = Fixture::new();
    let file = fixture.0.join("settings.json");
    let records = crate::storage::data_location::RecordsDirectory::fixture(&fixture.0);
    for invalid in [
        json!({"schemaVersion":99}),
        json!({"schemaVersion":1,"profile":"custom","allowedHosts":["*.example.com"]}),
        json!({"schemaVersion":1,"cpus":"four"}),
    ] {
        storage::write(&file, &invalid).unwrap();
        let original = fs::read(&file).unwrap();
        assert!(migrate(&file, &records).is_err());
        assert_eq!(fs::read(&file).unwrap(), original);
        assert!(!storage::v1_backup_path(&file).exists());
    }
    storage::write(&file, &json!({"schemaVersion":1,"profile":"custom"})).unwrap();
    fs::remove_file(fixture.0.join(".tugboat-data.json")).unwrap();
    assert!(migrate(&file, &records).is_err());
    assert!(!storage::v1_backup_path(&file).exists());
}
