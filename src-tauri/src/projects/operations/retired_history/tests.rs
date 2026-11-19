use super::*;
use crate::usage_store::{Environment, Store};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::{
    cell::Cell,
    os::unix::fs::{symlink, PermissionsExt},
    sync::atomic::{AtomicU64, Ordering},
};

mod live;

const SANDBOX: &str = "brig-retired-codex";
const V1: &str = include_str!("../../../../../tests/fixtures/tugboat/usage-schema-v1.sql");

struct Fixture {
    root: PathBuf,
    records: RecordsDirectory,
    first: Project,
    second: Project,
    unrelated: Project,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let root = std::env::temp_dir().join(format!(
            "tugboat-reset-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let records = root.join("records");
        for name in [
            "records",
            "first",
            "zero",
            "unrelated",
            "external",
            "state/homes/brig-retired-codex",
            "state/homes/new",
        ] {
            let path = root.join(name);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("canary"), b"owned or unrelated fixture").unwrap();
        }
        fs::create_dir(records.join("chatgpt")).unwrap();
        fs::write(
            records.join("chatgpt/account.json"),
            b"provider-identity-fixture",
        )
        .unwrap();
        let records = RecordsDirectory::fixture(&records);
        let mut active = Registry::default();
        let first = active.register(&root.join("first")).unwrap();
        let second = active.register(&root.join("zero")).unwrap();
        let unrelated = Registry::default()
            .register(&root.join("unrelated"))
            .unwrap();
        let first_record = former(&active, &first, &[AgentKind::Codex, AgentKind::ClaudeCode]);
        let second_record = former(&active, &second, &[]);
        let mut other_record = former(
            &Registry {
                projects: vec![unrelated.clone().into()],
                ..Registry::default()
            },
            &unrelated,
            &[AgentKind::Codex],
        );
        other_record["environments"][0]["label"] = "retired-unrelated".into();
        storage::write(
            &records.path().join("projects.json"),
            &json!({"schemaVersion":2,"projects":[first_record.clone(),second_record]}),
        )
        .unwrap();
        storage::write(
            &records.path().join("projects.json.v1.backup"),
            &json!({"schemaVersion":2,"projects":[first_record,other_record]}),
        )
        .unwrap();
        storage::write(&root.join("state/sessions.json"), &json!({
            "codex@retired-codex": { "home":root.join("state/homes").join(SANDBOX), "sandbox": SANDBOX, "project": first.path, "ephemeral": true },
            "tugboat-claude@retired-claude": { "home": root.join("external"), "sandbox":"brig-external", "project": first.path },
            first.reference(): { "home":root.join("state/homes/new"), "sandbox":"brig-new", "project": first.path, "ephemeral": true }
        })).unwrap();
        fs::write(root.join("brig"), "#!/bin/sh\n[ \"$*\" = '--json ls' ] || exit 5\nexec /bin/cat \"$BRIG_STATE_DIR/runtime.json\"\n").unwrap();
        fs::set_permissions(root.join("brig"), fs::Permissions::from_mode(0o700)).unwrap();
        let fixture = Self {
            root,
            records,
            first,
            second,
            unrelated,
        };
        fixture.runtime(json!([]));
        for path in [
            fixture.ledger(AgentKind::Codex),
            fixture.ledger(AgentKind::ClaudeCode),
        ] {
            storage::write(&path, &json!({"oldConversation":"fixture"})).unwrap();
            storage::write(
                &storage::v1_backup_path(&path),
                &json!({"backupConversation":"fixture"}),
            )
            .unwrap();
        }
        fixture
    }

    fn brig(&self) -> Brig {
        Brig {
            executable: self.root.join("brig"),
            environment: [(
                "BRIG_STATE_DIR".into(),
                self.root.join("state").to_str().unwrap().into(),
            )]
            .into(),
        }
    }

    fn runtime(&self, sandboxes: Value) {
        storage::write(&self.root.join("state/runtime.json"), &json!({"apiVersion":"brig.sh/v1alpha1","kind":"SandboxList","data":{"runtime":{"kind":"hull","available":true},"sandboxes":sandboxes}})).unwrap();
    }

    fn ledger(&self, kind: AgentKind) -> PathBuf {
        let mut old = self.first.clone();
        old.environment_id = EnvironmentId::for_agent(&old.id, kind);
        old.agent_kind = kind;
        ledger_path(&old, self.records.path())
    }

    fn home(&self) -> PathBuf {
        self.root.join("state/homes").join(SANDBOX)
    }
    fn registry(&self) -> PathBuf {
        self.records.path().join("projects.json")
    }
    fn checkpoint(&self) -> PathBuf {
        self.records.path().join(CHECKPOINT)
    }
    fn bootstrap(&self, idle: impl Fn() -> Result<(), String>) -> Result<(), String> {
        activate(
            &self.records,
            &settings::Settings::default(),
            || Ok(self.brig()),
            idle,
        )
    }

    fn database(&self, name: &str) -> Connection {
        let path = self.records.path().join(name);
        let connection = Connection::open(&path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        connection.execute_batch(V1).unwrap();
        connection
            .execute(
                "INSERT INTO store_identity VALUES(1,?1)",
                [self.records.id()],
            )
            .unwrap();
        for (project, kind, current) in [
            (&self.first, AgentKind::Codex, false),
            (&self.first, AgentKind::ClaudeCode, false),
            (&self.unrelated, AgentKind::Codex, false),
            (&self.first, AgentKind::Codex, true),
        ] {
            let id = if current {
                EnvironmentId::for_project(&project.id)
            } else {
                EnvironmentId::for_agent(&project.id, kind)
            };
            let agent = if kind == AgentKind::Codex {
                "codex"
            } else {
                "claude-code"
            };
            connection.execute("INSERT INTO usage_environments(project_id,environment_id,agent_kind) VALUES(?1,?2,?3)", params![project.id,id.to_string(),agent]).unwrap();
            if !current {
                let key = connection.last_insert_rowid();
                connection.execute("INSERT INTO session_groups(environment_key,native_session_id,has_variants_bucket) VALUES(?1,'00000000-0000-0000-0000-000000000000',0)", [key]).unwrap();
                let group = connection.last_insert_rowid();
                connection.execute("INSERT INTO session_records(session_key, title, updated_at, state, available, parser_version, lineage_json) VALUES(?1,'Former chat',1,'active',1,1,'{}')", [group]).unwrap();
                connection
                    .execute(
                        "INSERT INTO legacy_imports VALUES(?1,?2,1,zeroblob(32),50,1,0,0)",
                        params![format!("conversation-ledgers/{id}.json"), key],
                    )
                    .unwrap();
            }
        }
        connection
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn former(registry: &Registry, project: &Project, agents: &[AgentKind]) -> Value {
    let mut row = serde_json::to_value(
        registry
            .projects
            .iter()
            .find(|record| record.id == project.id)
            .unwrap(),
    )
    .unwrap();
    row.as_object_mut().unwrap().remove("environment").unwrap();
    row["environments"] = Value::Array(agents.iter().map(|kind| {
        let (profile, label) = if *kind == AgentKind::Codex { ("codex", "retired-codex") } else { ("tugboat-claude", "retired-claude") };
        json!({"environmentId":EnvironmentId::for_agent(&project.id, *kind),"agentKind":kind,"profile":profile,"label":label,"network":"isolated","policy":null,"stage":"selected","ownsSession":*kind == AgentKind::Codex,"ownsBinding":false,"policyDetached":false,"environmentVerified":false,"environmentAttempt":null,"removal":null})
    }).collect());
    row
}

#[test]
fn startup_retires_old_work_transactionally_and_preserves_fresh_usage_on_retry() {
    let fixture = Fixture::new();
    let old_registry = fs::read(fixture.registry()).unwrap();
    drop(fixture.database("usage.sqlite3"));
    let staging = fixture.database("usage.sqlite3.importing");
    staging
        .execute(
            "DELETE FROM usage_environments WHERE project_id=?1 AND environment_id=?2",
            params![fixture.first.id, fixture.first.environment_id.to_string()],
        )
        .unwrap();
    drop(staging);
    fixture.records.activate_sqlite().unwrap();
    assert!(Registry::load(&fixture.registry()).is_err());
    symlink(
        fixture.root.join("first"),
        fixture.home().join("worktree-link"),
    )
    .unwrap();
    let step = Cell::new(0);
    assert_eq!(
        fixture
            .bootstrap(|| {
                step.set(step.get() + 1);
                if step.get() == 4 {
                    return Err("fixture interruption".into());
                }
                Ok(())
            })
            .unwrap_err(),
        "fixture interruption"
    );
    assert!(fixture.checkpoint().is_file());
    assert_eq!(fs::read(fixture.registry()).unwrap(), old_registry);
    assert!(!fixture.home().exists());
    assert!(fixture.ledger(AgentKind::Codex).is_file());
    fixture.bootstrap(|| Ok(())).unwrap();
    fixture
        .bootstrap(|| panic!("A committed reset must not restart retired work."))
        .unwrap();
    assert!(!fixture.checkpoint().exists());
    for kind in [AgentKind::Codex, AgentKind::ClaudeCode] {
        let ledger = fixture.ledger(kind);
        assert!(!ledger.exists());
        assert!(!storage::v1_backup_path(&ledger).exists());
    }
    let fresh = Registry::load(&fixture.registry()).unwrap();
    assert_eq!(fresh.projects.len(), 2);
    for project in [&fixture.first, &fixture.second] {
        let [codex, claude] = fresh.agent_views(&project.id).unwrap();
        assert_eq!(codex.environment_id, claude.environment_id);
        assert_eq!(codex.path, project.path);
        assert_eq!(
            codex.environment_id,
            EnvironmentId::for_project(&project.id)
        );
    }
    let mut store = Store::open(&fixture.records).unwrap();
    for kind in [AgentKind::Codex, AgentKind::ClaudeCode] {
        let environment = Environment::new(
            fixture.first.id.clone(),
            fixture.first.environment_id.clone(),
            kind,
        )
        .unwrap();
        assert!(store
            .load_environment(&environment)
            .unwrap()
            .warnings
            .is_empty());
        let mut fresh_usage = crate::conversations::Ledger::default();
        fresh_usage.warnings.push(format!("fresh {kind:?}"));
        store.save_environment(&environment, &fresh_usage).unwrap();
    }
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM usage_environments", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        3
    );
    store.close().unwrap();
    let stage = Connection::open(fixture.records.path().join("usage.sqlite3.importing")).unwrap();
    let owner: String = stage
        .query_row("SELECT project_id FROM usage_environments", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(owner, fixture.unrelated.id);
    assert_eq!(
        stage
            .query_row("SELECT COUNT(*) FROM session_records", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let backup: Value = storage::read(&fixture.records.path().join("projects.json.v1.backup"))
        .unwrap()
        .unwrap();
    assert_eq!(backup["projects"].as_array().unwrap().len(), 1);
    assert_eq!(backup["projects"][0]["id"], fixture.unrelated.id);
    assert_eq!(
        fs::read(fixture.records.path().join("chatgpt/account.json")).unwrap(),
        b"provider-identity-fixture"
    );
    for name in ["first", "zero", "external", "unrelated", "state/homes/new"] {
        assert!(fixture.root.join(name).join("canary").is_file());
    }
}

#[test]
fn a_busy_or_replaced_retired_guest_stops_reset_without_erasing_work() {
    let fixture = Fixture::new();
    let registry = fs::read(fixture.registry()).unwrap();
    assert!(fixture.bootstrap(|| Err("active terminal".into())).is_err());
    assert!(!fixture.checkpoint().exists());
    fixture.runtime(json!([{"sandbox":SANDBOX,"ref":"codex@retired-codex","state":"running"}]));
    assert!(fixture
        .bootstrap(|| Ok(()))
        .unwrap_err()
        .contains("Active guest work"));
    assert!(fixture.checkpoint().exists());
    fixture.runtime(json!([]));
    let original = fixture.root.join("held-retired-home");
    fs::rename(fixture.home(), &original).unwrap();
    symlink(fixture.root.join("first"), fixture.home()).unwrap();
    assert!(fixture.bootstrap(|| Ok(())).is_err());
    assert!(fixture.root.join("first/canary").is_file());
    assert!(original.join("canary").is_file());
    assert!(fixture.ledger(AgentKind::Codex).is_file());
    assert_eq!(fs::read(fixture.registry()).unwrap(), registry);
}

#[test]
fn a_missing_reviewed_ledger_or_unavailable_records_fail_closed_on_retry() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger(AgentKind::Codex);
    fs::remove_file(&ledger).unwrap();
    fixture.runtime(json!([{"sandbox":SANDBOX,"ref":"codex@retired-codex","state":"running"}]));
    assert!(fixture.bootstrap(|| Ok(())).is_err());
    assert!(fixture.checkpoint().exists());
    fixture.runtime(json!([]));
    fs::write(&ledger, "new data appeared").unwrap();
    assert!(fixture
        .bootstrap(|| Ok(()))
        .unwrap_err()
        .contains("appeared after review"));
    let marker = fixture.records.path().join(".tugboat-data.json");
    fs::rename(&marker, fixture.root.join("disconnected-marker")).unwrap();
    assert!(fixture.bootstrap(|| Ok(())).is_err());
    assert_eq!(fs::read(&ledger).unwrap(), b"new data appeared");
    assert!(fixture.home().exists());
}

#[test]
fn a_sqlite_failure_rolls_back_the_retired_rows_and_blocks_registry_publication() {
    let fixture = Fixture::new();
    let database = fixture.database("usage.sqlite3");
    database.execute_batch("CREATE TRIGGER hold_claude BEFORE DELETE ON usage_environments WHEN OLD.agent_kind='claude-code' BEGIN SELECT RAISE(ABORT,'disposable sqlite failure'); END;").unwrap();
    drop(database);
    let original = fs::read(fixture.registry()).unwrap();
    assert!(fixture
        .bootstrap(|| Ok(()))
        .unwrap_err()
        .contains("disposable sqlite failure"));
    assert_eq!(fs::read(fixture.registry()).unwrap(), original);
    assert!(fixture.checkpoint().exists());
    let database = Connection::open(fixture.records.path().join("usage.sqlite3")).unwrap();
    assert_eq!(
        database
            .query_row("SELECT COUNT(*) FROM usage_environments", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        4
    );
    assert_eq!(
        database
            .query_row("SELECT COUNT(*) FROM session_records", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        3
    );
    database.execute_batch("DROP TRIGGER hold_claude;").unwrap();
    drop(database);
    fixture.bootstrap(|| Ok(())).unwrap();
    assert!(!fixture.checkpoint().exists());
    assert!(Registry::load(&fixture.registry()).is_ok());
}

#[test]
fn provider_cleanup_resumes_after_one_store_has_been_written() {
    let fixture = Fixture::new();
    let path = fixture.records.path().join("chatgpt/session.json");
    let old = EnvironmentId::for_agent(&fixture.first.id, AgentKind::Codex);
    let current = fixture.first.environment_id.clone();
    let old_home = fixture.home();
    let home = fixture.root.join("state/homes/new");
    let receipt = |environment: &EnvironmentId, directory: PathBuf| {
        json!({
            "projectId": fixture.first.id, "environmentId": environment, "directory": directory.join(".codex"),
            "authHash": null, "accessHash": null, "pending": true, "identity": [0,0]
        })
    };
    storage::write(&path, &json!({"schemaVersion":1,"generation":3,"credentials":null,"disconnected":false,"reconnectRequired":false,
        "bindings":[receipt(&old,old_home),receipt(&current,home)],"pendingLogin":null})).unwrap();
    let other = fixture.records.path().join("claude/session.json");
    fs::create_dir(other.parent().unwrap()).unwrap();
    fs::write(&other, "incomplete disposable provider record").unwrap();
    assert!(fixture
        .bootstrap(|| Ok(()))
        .unwrap_err()
        .contains("Claude record"));
    assert!(fixture.checkpoint().exists());
    assert!(Registry::load(&fixture.registry()).is_err());
    let interrupted = fs::read(&path).unwrap();
    let gpt: Value = serde_json::from_slice(&interrupted).unwrap();
    assert_eq!(gpt["bindings"].as_array().unwrap().len(), 1);
    assert_eq!(gpt["bindings"][0]["environmentId"], current.to_string());
    assert_eq!(gpt["generation"], 3);
    assert_eq!(
        fs::read_to_string(&other).unwrap(),
        "incomplete disposable provider record"
    );
    let old_claude = EnvironmentId::for_agent(&fixture.first.id, AgentKind::ClaudeCode);
    storage::write(&other,&json!({"schemaVersion":1,"generation":5,"account":{"state":"disconnected"},"bindings":[
        {"projectId":fixture.first.id,"environmentId":old_claude},{"projectId":fixture.second.id,"environmentId":fixture.second.environment_id}
    ]})).unwrap();
    fixture.bootstrap(|| Ok(())).unwrap();
    assert_eq!(fs::read(path).unwrap(), interrupted);
    let claude: Value = storage::read(&other).unwrap().unwrap();
    assert_eq!(claude["bindings"].as_array().unwrap().len(), 1);
    assert_eq!(claude["bindings"][0]["projectId"], fixture.second.id);
    assert_eq!(claude["generation"], 5);
    assert!(Registry::load(&fixture.registry()).is_ok());
}

#[test]
fn a_replaced_usage_database_is_preserved_even_when_its_dataset_id_matches() {
    let fixture = Fixture::new();
    drop(fixture.database("usage.sqlite3"));
    fixture.runtime(json!([{"sandbox":SANDBOX,"ref":"codex@retired-codex","state":"running"}]));
    assert!(fixture
        .bootstrap(|| Ok(()))
        .unwrap_err()
        .contains("Active guest work"));
    let file = fixture.records.path().join("usage.sqlite3");
    let original = fixture.root.join("held-database");
    fs::rename(&file, &original).unwrap();
    drop(fixture.database("usage.sqlite3"));
    let replacement = fs::read(&file).unwrap();
    fixture.runtime(json!([]));
    assert!(fixture
        .bootstrap(|| Ok(()))
        .unwrap_err()
        .contains("replacement was preserved"));
    assert_eq!(fs::read(&file).unwrap(), replacement);
    assert!(fixture.home().is_dir());
}

#[test]
fn stopped_former_guest_requires_the_reviewed_info_home_before_exact_removal() {
    let fixture = Fixture::new();
    let state = fixture.root.join("state");
    let reference = "codex@retired-codex";
    fixture.runtime(json!([]));
    fs::copy(
        state.join("runtime.json"),
        state.join("empty-inventory.json"),
    )
    .unwrap();
    fixture.runtime(
        json!([{"sandbox":SANDBOX,"ref":reference,"state":"stopped","workspace":fixture.home()}]),
    );
    let info = |home: &Path| {
        storage::write(&state.join("old-info.json"),&json!({"apiVersion":"brig.sh/v1alpha1","kind":"Info","data":{
        "profile":"codex","session":"retired-codex","project":{"host":fixture.first.path},"workspace":home}})).unwrap()
    };
    info(&fixture.root.join("external"));
    fs::write(
        state.join("dry-run"),
        format!(
            "would remove {reference} (sandbox {SANDBOX}) and its guest home {}\n",
            fixture.home().display()
        ),
    )
    .unwrap();
    fs::write(
        fixture.root.join("brig"),
        r#"#!/bin/sh
set -eu
case "$*" in
    '--json ls') exec /bin/cat "$BRIG_STATE_DIR/runtime.json";;
    '--json info codex@retired-codex') exec /bin/cat "$BRIG_STATE_DIR/old-info.json";;
    'rm codex@retired-codex --dry-run') exec /bin/cat "$BRIG_STATE_DIR/dry-run";;
    'rm codex@retired-codex')
        printf '%s' "$2" > "$BRIG_STATE_DIR/removed"
        exec /bin/cp "$BRIG_STATE_DIR/empty-inventory.json" "$BRIG_STATE_DIR/runtime.json";;
esac
exit 7
"#,
    )
    .unwrap();
    assert!(fixture
        .bootstrap(|| Ok(()))
        .unwrap_err()
        .contains("different retired guest home"));
    assert!(fixture.home().is_dir());
    assert!(!state.join("removed").exists());
    info(&fixture.home());
    fixture.bootstrap(|| Ok(())).unwrap();
    assert_eq!(
        fs::read_to_string(state.join("removed")).unwrap(),
        reference
    );
    assert!(!fixture.home().exists());
    assert!(fixture.root.join("external/canary").is_file());
}
