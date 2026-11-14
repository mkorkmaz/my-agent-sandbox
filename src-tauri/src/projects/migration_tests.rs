use super::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

pub(crate) fn legacy_registry(registry: &Registry) -> Value {
    let projects: Vec<_> = registry
        .codex_projects()
        .unwrap()
        .into_iter()
        .map(|project| {
            let mut value = serde_json::to_value(project).unwrap();
            let object = value.as_object_mut().unwrap();
            for field in ["environmentId", "agentKind", "network"] {
                object.remove(field);
            }
            value
        })
        .collect();
    json!({"schemaVersion":1,"projects":projects})
}

struct Fixture {
    root: PathBuf,
    records: crate::storage::data_location::RecordsDirectory,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "tugboat-environment-migration-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("records")).unwrap();
        fs::create_dir(root.join("project")).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let records =
            crate::storage::data_location::RecordsDirectory::fixture(&root.join("records"));
        Self { root, records }
    }
    fn file(&self) -> PathBuf {
        self.root.join("records/projects.json")
    }
    fn registry(&self) -> (Registry, Project) {
        let mut registry = Registry::default();
        let project = registry
            .register(&self.root.join("project"), "custom-codex")
            .unwrap();
        (registry, project)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn unknown_environment_stages_are_rejected_without_rewriting_saved_records() {
    let fixture = Fixture::new();
    let (registry, _) = fixture.registry();
    let mut document = serde_json::to_value(registry).unwrap();
    document["projects"][0]["environments"][0]["stage"] = json!("unexpected-stage");
    storage::write(&fixture.file(), &document).unwrap();
    let original = fs::read(fixture.file()).unwrap();
    assert!(Registry::load(&fixture.file()).is_err());
    assert_eq!(fs::read(fixture.file()).unwrap(), original);
}

#[test]
fn legacy_migration_preserves_external_references_and_partial_removal_with_an_immutable_backup() {
    let fixture = Fixture::new();
    let (mut registry, mut project) = fixture.registry();
    project.profile = "external-codex".into();
    project.label = "saved-external".into();
    project.owns_session = false;
    project.owns_binding = false;
    project.policy = Some(Policy {
        name: "existing-policy".into(),
        owned: false,
        fingerprint: Some("saved-fingerprint".into()),
    });
    project.removal = Some(Removal {
        reference: project.reference(),
        workspace: fixture.root.join("external-home").to_string_lossy().into(),
        logout: false,
        had_environment: true,
        logged_out: false,
        stopped: true,
        environment_removed: false,
        binding_detached: false,
        policy_handled: false,
        preserved_policy: true,
    });
    project.stage = crate::projects::ProjectStage::Removing;
    registry.update(project.clone()).unwrap();
    storage::write(&fixture.file(), &legacy_registry(&registry)).unwrap();
    let original = fs::read(fixture.file()).unwrap();
    assert!(migrate(&fixture.file(), &fixture.records).unwrap());
    assert_eq!(
        fs::read(storage::v1_backup_path(&fixture.file())).unwrap(),
        original
    );
    let reopened = Registry::load(&fixture.file()).unwrap();
    let restored = reopened.project(&project.id).unwrap();
    assert_eq!(
        serde_json::to_value(&restored).unwrap(),
        serde_json::to_value(&project).unwrap()
    );
    let document: Value = storage::read(&fixture.file()).unwrap().unwrap();
    assert_eq!(document["schemaVersion"], 2);
    assert_eq!(
        document["projects"][0]["environments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(document["projects"][0].get("profile").is_none());
    let once = fs::read(fixture.file()).unwrap();
    assert!(!migrate(&fixture.file(), &fixture.records).unwrap());
    assert_eq!(fs::read(fixture.file()).unwrap(), once);
    assert_eq!(
        fs::read(storage::v1_backup_path(&fixture.file())).unwrap(),
        original
    );
}

#[test]
fn a_second_agent_is_created_once_with_its_own_reference_network_and_ledger() {
    let fixture = Fixture::new();
    let (mut registry, mut codex) = fixture.registry();
    codex.network = settings::Network::Offline;
    registry.update(codex.clone()).unwrap();
    let defaults = settings::Settings::default();
    let first = registry
        .ensure_environment(&codex.id, AgentKind::ClaudeCode, &defaults)
        .unwrap();
    let repeated = registry
        .ensure_environment(&codex.id, AgentKind::ClaudeCode, &defaults)
        .unwrap();
    assert_eq!(first.environment_id, repeated.environment_id);
    assert_eq!(registry.environments(&codex.id).unwrap().iter().count(), 2);
    let claude = registry
        .environment(&codex.id, &first.environment_id)
        .unwrap();
    assert_eq!(claude.network, settings::Network::Isolated);
    assert_eq!(
        registry.project(&codex.id).unwrap().network,
        settings::Network::Offline
    );
    assert_ne!(codex.reference(), claude.reference());
    assert_ne!(codex.environment_id, claude.environment_id);
    assert_eq!(
        codex.ledger_file(&fixture.root).unwrap(),
        fixture
            .root
            .join(format!("conversation-ledgers/{}.json", codex.id))
    );
    assert_eq!(
        claude.ledger_file(&fixture.root).unwrap(),
        fixture
            .root
            .join("conversation-ledgers")
            .join(&codex.id)
            .join(first.environment_id.filename())
    );
    assert!(registry
        .environment(&"b".repeat(32), &first.environment_id)
        .is_err());
    assert!(registry
        .environment(
            &codex.id,
            &EnvironmentId::for_agent(&"b".repeat(32), AgentKind::ClaudeCode)
        )
        .is_err());
    storage::write(&fixture.file(), &registry).unwrap();
    assert_eq!(
        Registry::load(&fixture.file())
            .unwrap()
            .environments(&codex.id)
            .unwrap()
            .iter()
            .count(),
        2
    );
}

#[test]
fn updating_one_environment_preserves_the_other_and_shared_resource_changes_invalidate_both() {
    let fixture = Fixture::new();
    let (mut registry, codex) = fixture.registry();
    let other = registry
        .ensure_environment(
            &codex.id,
            AgentKind::ClaudeCode,
            &settings::Settings::default(),
        )
        .unwrap();
    let mut claude = registry
        .environment(&codex.id, &other.environment_id)
        .unwrap();
    claude.stage = crate::projects::ProjectStage::Ready;
    claude.policy = Some(Policy {
        name: "claude-policy".into(),
        owned: true,
        fingerprint: None,
    });
    registry.update(claude).unwrap();
    assert_eq!(
        serde_json::to_value(registry.project(&codex.id).unwrap()).unwrap(),
        serde_json::to_value(&codex).unwrap()
    );
    let mut codex = registry.project(&codex.id).unwrap();
    let previous = codex.settings(settings::Settings::default()).unwrap();
    codex.set_preferences(
        "Shared project name",
        ProjectOptions {
            cpus: 2,
            memory_mb: 8192,
            verification: settings::Verification::Require,
            network: settings::Network::Offline,
        },
        &previous,
    );
    registry.update(codex.clone()).unwrap();
    let claude = registry
        .environment(&codex.id, &other.environment_id)
        .unwrap();
    assert_eq!(claude.name, "Shared project name");
    assert_eq!(
        claude.settings(settings::Settings::default()).unwrap().cpus,
        2
    );
    assert_eq!(claude.network, settings::Network::Isolated);
    assert_eq!(codex.network, settings::Network::Offline);
    assert_eq!(claude.policy.unwrap().name, "claude-policy");
    assert_eq!(claude.environment_verified, Some(false));
}

#[test]
fn legacy_ledgers_states_prices_and_existing_ledger_backups_are_not_rewritten() {
    let fixture = Fixture::new();
    let (registry, project) = fixture.registry();
    storage::write(&fixture.file(), &legacy_registry(&registry)).unwrap();
    let ledger = project.ledger_file(&fixture.root.join("records")).unwrap();
    let original = json!({"schemaVersion":2,"sessions":{
        "archive":{"title":"Saved chat","updatedAt":1,"state":"archived","turns":{},"cursors":{}},
        "trash":{"title":"Deleted chat","updatedAt":2,"state":"deleted","turns":{},"cursors":{}}
    },"savedPriceEvidence":{"costUsd":0.1234,"source":"historical-rate"}});
    storage::write(&ledger, &original).unwrap();
    fs::write(
        storage::v1_backup_path(&ledger),
        b"immutable original ledger bytes\0\n",
    )
    .unwrap();
    let before = fs::read(&ledger).unwrap();
    migrate(&fixture.file(), &fixture.records).unwrap();
    let mut reopened = Registry::load(&fixture.file()).unwrap();
    reopened
        .ensure_environment(
            &project.id,
            AgentKind::ClaudeCode,
            &settings::Settings::default(),
        )
        .unwrap();
    storage::write(&fixture.file(), &reopened).unwrap();
    assert_eq!(fs::read(&ledger).unwrap(), before);
    assert_eq!(
        fs::read(storage::v1_backup_path(&ledger)).unwrap(),
        b"immutable original ledger bytes\0\n"
    );
}

#[test]
fn invalid_or_duplicate_environments_and_conflicting_backup_leave_the_source_unchanged() {
    let fixture = Fixture::new();
    let (registry, _) = fixture.registry();
    storage::write(&fixture.file(), &legacy_registry(&registry)).unwrap();
    let original = fs::read(fixture.file()).unwrap();
    fs::write(
        storage::v1_backup_path(&fixture.file()),
        b"different saved data",
    )
    .unwrap();
    assert!(migrate(&fixture.file(), &fixture.records)
        .unwrap_err()
        .contains("differs"));
    assert_eq!(fs::read(fixture.file()).unwrap(), original);
    let mut value = serde_json::to_value(&registry).unwrap();
    let duplicate = value["projects"][0]["environments"][0].clone();
    value["projects"][0]["environments"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    storage::write(&fixture.file(), &value).unwrap();
    assert!(Registry::load(&fixture.file()).is_err());
    value["projects"][0]["environments"] = json!([]);
    storage::write(&fixture.file(), &value).unwrap();
    assert!(Registry::load(&fixture.file()).is_err());
    let mut value = serde_json::to_value(&registry).unwrap();
    value["projects"][0]["environments"][0]["environmentId"] = json!("../foreign");
    storage::write(&fixture.file(), &value).unwrap();
    assert!(Registry::load(&fixture.file()).is_err());
    value["projects"][0]["environments"][0]["environmentId"] = json!("f".repeat(32));
    storage::write(&fixture.file(), &value).unwrap();
    assert!(Registry::load(&fixture.file()).is_err());
}

#[test]
fn interrupted_migration_reuses_the_backup_and_preserves_partial_preparation() {
    let fixture = Fixture::new();
    let (mut registry, mut project) = fixture.registry();
    project.stage = crate::projects::ProjectStage::EnvironmentStarting;
    project.environment_attempt = Some(EnvironmentAttempt {
        plan_fingerprint: Some("saved-plan".into()),
        token: "saved-attempt".into(),
        terminal_identifier: 42,
        completed: false,
        error: Some("Preparation interrupted.".into()),
    });
    registry.update(project.clone()).unwrap();
    storage::write(&fixture.file(), &legacy_registry(&registry)).unwrap();
    let original = fs::read(fixture.file()).unwrap();
    {
        let _lock = storage::lock(&fixture.file()).unwrap();
        storage::immutable_v1_backup(&fixture.file()).unwrap();
    }
    assert!(migrate(&fixture.file(), &fixture.records).unwrap());
    let restored = Registry::load(&fixture.file())
        .unwrap()
        .project(&project.id)
        .unwrap();
    assert_eq!(
        serde_json::to_value(restored).unwrap(),
        serde_json::to_value(project).unwrap()
    );
    assert_eq!(
        fs::read(storage::v1_backup_path(&fixture.file())).unwrap(),
        original
    );
    assert!(!migrate(&fixture.file(), &fixture.records).unwrap());
}

#[test]
fn records_disconnected_during_migration_are_not_recreated() {
    let fixture = Fixture::new();
    let (registry, _) = fixture.registry();
    storage::write(&fixture.file(), &legacy_registry(&registry)).unwrap();
    let original = fs::read(fixture.file()).unwrap();
    let result = storage::upgrade_record(&fixture.file(), &fixture.records, |_| {
        fs::rename(
            fixture.root.join("records"),
            fixture.root.join("disconnected-records"),
        )
        .map_err(|error| error.to_string())?;
        Ok(storage::RecordSchema::Upgrade(&registry))
    });
    assert!(result.is_err());
    assert!(!fixture.root.join("records").exists());
    assert_eq!(
        fs::read(fixture.root.join("disconnected-records/projects.json")).unwrap(),
        original
    );
    assert!(!fixture
        .root
        .join("disconnected-records/projects.json.v1.backup")
        .exists());
}

#[test]
fn project_shared_records_are_not_migrated() {
    let fixture = Fixture::new();
    let mut registry = Registry::default();
    registry.register(&fixture.root, "custom-codex").unwrap();
    storage::write(&fixture.file(), &legacy_registry(&registry)).unwrap();
    let original = fs::read(fixture.file()).unwrap();
    assert!(migrate(&fixture.file(), &fixture.records).is_err());
    assert_eq!(fs::read(fixture.file()).unwrap(), original);
    assert!(!storage::v1_backup_path(&fixture.file()).exists());
}

#[test]
fn selecting_an_existing_project_keeps_both_environment_defaults() {
    let fixture = Fixture::new();
    let mut registry = Registry::default();
    let mut defaults = settings::Settings::default();
    defaults.codex.profile = "saved-codex".into();
    defaults.network = settings::Network::Offline;
    let codex = registry
        .register_with_defaults(&fixture.root.join("project"), &defaults)
        .unwrap();
    assert_eq!(codex.network, settings::Network::Offline);
    registry
        .ensure_environment(&codex.id, AgentKind::ClaudeCode, &defaults)
        .unwrap();
    defaults.codex.profile = "changed-codex".into();
    defaults.network = settings::Network::Isolated;
    let reopened = registry
        .register_with_defaults(&fixture.root.join("project"), &defaults)
        .unwrap();
    assert_eq!(reopened.environment_id, codex.environment_id);
    assert_eq!(reopened.reference(), codex.reference());
    assert_eq!(reopened.network, settings::Network::Offline);
    assert_eq!(registry.environments(&codex.id).unwrap().iter().count(), 2);
}

#[test]
fn missing_records_marker_or_disconnected_records_do_not_start_migration() {
    let fixture = Fixture::new();
    let (registry, _) = fixture.registry();
    storage::write(&fixture.file(), &legacy_registry(&registry)).unwrap();
    let original = fs::read(fixture.file()).unwrap();
    fs::remove_file(fixture.root.join("records/.tugboat-data.json")).unwrap();
    assert!(migrate(&fixture.file(), &fixture.records).is_err());
    assert_eq!(fs::read(fixture.file()).unwrap(), original);
    assert!(!storage::v1_backup_path(&fixture.file()).exists());
    fs::rename(
        fixture.root.join("records"),
        fixture.root.join("disconnected-records"),
    )
    .unwrap();
    assert!(migrate(&fixture.file(), &fixture.records).is_err());
    assert!(!fixture.root.join("records").exists());
}

#[test]
fn concurrent_migration_commits_once_and_returns_the_same_environment_id() {
    let fixture = Fixture::new();
    let (registry, project) = fixture.registry();
    storage::write(&fixture.file(), &legacy_registry(&registry)).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let barrier = barrier.clone();
            let records = fixture.records.clone();
            let file = fixture.file();
            std::thread::spawn(move || {
                barrier.wait();
                migrate(&file, &records).unwrap()
            })
        })
        .collect();
    let commits = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .filter(|committed| *committed)
        .count();
    assert_eq!(commits, 1);
    assert_eq!(
        Registry::load(&fixture.file())
            .unwrap()
            .project(&project.id)
            .unwrap()
            .environment_id,
        project.environment_id
    );
}

#[test]
fn a_new_claude_project_reopens_with_claude_and_can_add_codex_later() {
    let fixture = Fixture::new();
    let defaults = settings::Settings::default();
    let mut registry = Registry::default();
    let project = registry
        .register_for_agent(
            &fixture.root.join("project"),
            &defaults,
            Some(AgentKind::ClaudeCode),
        )
        .unwrap();
    assert_eq!(project.agent_kind, AgentKind::ClaudeCode);
    assert_eq!(
        registry.environments(&project.id).unwrap().iter().count(),
        1
    );
    assert!(registry
        .environments(&project.id)
        .unwrap()
        .by_kind(AgentKind::Codex)
        .is_err());
    storage::write(&fixture.file(), &registry).unwrap();
    let mut reopened = Registry::load(&fixture.file()).unwrap();
    assert_eq!(
        reopened.project(&project.id).unwrap().environment_id,
        project.environment_id
    );
    assert_eq!(
        reopened.project_views().unwrap()[0].agent_kind,
        AgentKind::ClaudeCode
    );
    let selected = reopened
        .register_for_agent(&fixture.root.join("project"), &defaults, None)
        .unwrap();
    assert_eq!(selected.environment_id, project.environment_id);
    let codex = reopened
        .ensure_environment(&project.id, AgentKind::Codex, &defaults)
        .unwrap();
    assert_eq!(codex.stage, "selected");
    assert_eq!(
        reopened.environments(&project.id).unwrap().iter().count(),
        2
    );
    assert_eq!(
        reopened
            .environment(&project.id, &project.environment_id)
            .unwrap()
            .reference(),
        project.reference()
    );
}

#[test]
fn choosing_claude_for_an_existing_codex_project_preserves_its_environment() {
    let fixture = Fixture::new();
    let (mut registry, mut codex) = fixture.registry();
    codex.stage = crate::projects::ProjectStage::EnvironmentReady;
    codex.environment_verified = Some(true);
    codex.policy = Some(Policy {
        name: "saved-codex-policy".into(),
        owned: true,
        fingerprint: Some("saved-policy-fingerprint".into()),
    });
    codex.owns_binding = true;
    registry.update(codex.clone()).unwrap();
    let before = serde_json::to_value(
        registry
            .environments(&codex.id)
            .unwrap()
            .by_kind(AgentKind::Codex)
            .unwrap(),
    )
    .unwrap();
    let mut defaults = settings::Settings::default();
    defaults.codex.profile = "different-default-codex".into();
    let claude = registry
        .register_for_agent(
            &fixture.root.join("project"),
            &defaults,
            Some(AgentKind::ClaudeCode),
        )
        .unwrap();
    assert_eq!(claude.agent_kind, AgentKind::ClaudeCode);
    assert_eq!(registry.environments(&codex.id).unwrap().iter().count(), 2);
    assert_eq!(
        serde_json::to_value(
            registry
                .environments(&codex.id)
                .unwrap()
                .by_kind(AgentKind::Codex)
                .unwrap()
        )
        .unwrap(),
        before
    );
    assert_eq!(
        registry.project(&codex.id).unwrap().environment_id,
        codex.environment_id
    );
}

#[test]
fn registration_without_an_agent_keeps_the_legacy_codex_default() {
    let fixture = Fixture::new();
    let mut defaults = settings::Settings::default();
    defaults.codex.profile = "configured-codex".into();
    defaults.network = settings::Network::Offline;
    let mut registry = Registry::default();
    let project = registry
        .register_for_agent(&fixture.root.join("project"), &defaults, None)
        .unwrap();
    assert_eq!(project.agent_kind, AgentKind::Codex);
    assert_eq!(project.profile, "configured-codex");
    assert_eq!(project.network, settings::Network::Offline);
    assert_eq!(
        registry.environments(&project.id).unwrap().iter().count(),
        1
    );
}

#[test]
fn invalid_agent_defaults_do_not_leave_a_partial_project_registration() {
    let fixture = Fixture::new();
    let mut defaults = settings::Settings::default();
    defaults.claude_code.profile = String::new();
    let mut registry = Registry::default();
    assert!(registry
        .register_for_agent(
            &fixture.root.join("project"),
            &defaults,
            Some(AgentKind::ClaudeCode)
        )
        .is_err());
    assert!(registry.projects.is_empty());
}
