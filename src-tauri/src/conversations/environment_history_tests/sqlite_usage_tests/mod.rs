use super::*;
use crate::usage_store::{bootstrap, SharedStore, Store};

fn family(watch: &Watch, name: &str) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/claude-code")
        .join(name);
    let root = claude_file(watch, CHAT);
    fs::copy(source.join("root.jsonl"), &root).unwrap();
    for agent in ["alpha", "beta", "nested"] {
        let directory = if agent == "nested" {
            root.with_extension("").join("subagents/workflows/run-1")
        } else {
            root.with_extension("").join("subagents")
        };
        fs::create_dir_all(&directory).unwrap();
        for extension in ["jsonl", "meta.json"] {
            fs::copy(
                source.join(format!("agent-{agent}.{extension}")),
                directory.join(format!("agent-{agent}.{extension}")),
            )
            .unwrap();
        }
    }
}

fn seeded(fixture: &Fixture, watch: &Watch, ledger: &Ledger) -> (Watch, PathBuf) {
    let directory = fixture.0.join("records");
    fs::create_dir_all(&directory).unwrap();
    let records = crate::storage::data_location::RecordsDirectory::fixture(&directory);
    let target = watch.target.as_ref().unwrap();
    let path = if target.agent_kind == AgentKind::Codex {
        directory
            .join("conversation-ledgers")
            .join(format!("{PROJECT}.json"))
    } else {
        directory
            .join("conversation-ledgers")
            .join(PROJECT)
            .join(format!("{}.json", target.environment_id))
    };
    storage::write(&path, ledger).unwrap();
    let mut store = bootstrap::open_or_initialize(&records).unwrap();
    let environment = target.usage_environment().unwrap();
    store.save_environment(&environment, ledger).unwrap();
    if target.agent_kind == AgentKind::ClaudeCode {
        reset_fixture_pricing(&store, &environment);
    }
    let mut sql = watch.clone();
    sql.records_directory = Some(records);
    sql.store = Some(SharedStore::new(store));
    (sql, path)
}

fn reset_fixture_pricing(store: &Store, environment: &crate::usage_store::Environment) {
    let key = crate::usage_store::codecs::environment_key(&store.connection, environment)
        .unwrap()
        .unwrap();
    store
        .connection
        .execute(
            "UPDATE usage_environments SET claude_pricing_version=0 WHERE environment_key=?1",
            [key],
        )
        .unwrap();
}

fn value(value: &impl Serialize) -> Value {
    serde_json::to_value(value).unwrap()
}

#[test]
fn claude_history_refresh_corrects_a_saved_command_title() {
    let fixture = Fixture::new("sqlite-claude-command-title");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    let path = write_claude(&watch);
    let mut file = OpenOptions::new().append(true).open(path).unwrap();
    writeln!(
        file,
        "{}",
        json!({"type":"ai-title","sessionId":CHAT,"aiTitle":"Test the chat history"})
    )
    .unwrap();
    let usage = synchronize(&watch).unwrap().0.usage;
    let mut ledger = watch.selected_ledger().unwrap();
    let record = ledger.sessions.get_mut(CHAT).unwrap();
    record.title = "<local-command-caveat>The command below was run directly in Claude Code".into();
    record.state = SessionState::Archived;
    let (sql, _) = seeded(&fixture, &watch, &ledger);

    synchronize(&sql).unwrap();
    let saved = sql.saved_dashboard().unwrap();
    assert_eq!(saved.conversations.len(), 1);
    assert_eq!(saved.conversations[0].title, "Test the chat history");
    assert_eq!(saved.conversations[0].state, SessionState::Archived);
    assert_eq!(saved.usage, usage);
}

#[test]
fn saved_project_snapshot_uses_only_recorded_usage_and_keeps_sessions_unavailable() {
    for kind in [AgentKind::Codex, AgentKind::ClaudeCode] {
        let fixture = Fixture::new(&format!("sqlite-opening-snapshot-{kind:?}"));
        let legacy = fixture.watch(kind);
        match kind {
            AgentKind::Codex => {
                write_codex(&legacy);
            }
            AgentKind::ClaudeCode => {
                write_claude(&legacy);
            }
        }
        let current = synchronize(&legacy).unwrap().0;
        let ledger = legacy.selected_ledger().unwrap();
        let (sql, _) = seeded(&fixture, &legacy, &ledger);
        let store = sql.store.as_ref().unwrap();
        let target = sql.target.as_ref().unwrap().clone();
        fs::remove_dir_all(&legacy.home).unwrap();

        let saved = environment_history::saved_snapshot(&[target.clone()], store).unwrap();
        assert_eq!(saved.usage, current.usage);
        assert!(!saved.conversations.is_empty());
        assert!(saved.conversations.iter().all(|entry| !entry.available));
        assert!(saved
            .conversations
            .iter()
            .all(|entry| entry.environment_id.as_ref() == Some(&target.environment_id)));
        assert!(saved
            .warnings
            .iter()
            .any(|warning| warning.contains("Showing saved sessions")));

        let other_id = "f".repeat(32);
        let other = environment_history::Target {
            environment_id: projects::EnvironmentId::for_project(&other_id),
            project_id: other_id,
            agent_kind: kind,
        };
        assert!(environment_history::saved_snapshot(&[other], store)
            .unwrap()
            .conversations
            .is_empty());
        store.close().unwrap();
    }
}

#[test]
fn verified_original_folder_rename_preserves_both_agents_and_usage() {
    let fixture = Fixture::new("sqlite-folder-rename");
    let original_path = fixture.0.join("p");
    let renamed_path = fixture.0.join("renamed project");
    fs::create_dir(&original_path).unwrap();
    let mut registry = projects::Registry::default();
    let original = registry.register(&original_path).unwrap();
    let records_path = fixture.0.join("records");
    fs::create_dir(&records_path).unwrap();
    let records = crate::storage::data_location::RecordsDirectory::fixture(&records_path);
    let store = SharedStore::new(bootstrap::open_or_initialize(&records).unwrap());
    let mut watches = [AgentKind::Codex, AgentKind::ClaudeCode].map(|kind| {
        let mut watch = fixture.watch(kind);
        watch.target = Some(environment_history::Target {
            project_id: original.id.clone(),
            environment_id: projects::EnvironmentId::for_project(&original.id),
            agent_kind: kind,
        });
        watch.records_directory = Some(records.clone());
        watch.store = Some(store.clone());
        watch
    });
    write_codex(&watches[0]);
    write_claude(&watches[1]);
    let original_dashboard =
        environment_history::combine(watches.iter().map(|watch| synchronize(watch).unwrap().0));
    assert_eq!(original_dashboard.conversations.len(), 2);
    assert!(original_dashboard.usage.total > 0);

    fs::rename(&original_path, &renamed_path).unwrap();
    let renamed = registry.register(&renamed_path).unwrap();
    assert_eq!(renamed.id, original.id);
    assert_eq!(renamed.reference(), original.reference());
    fs::create_dir(&original_path).unwrap();
    let replacement = registry.register(&original_path).unwrap();
    assert_ne!(replacement.id, original.id);
    let registry_file = records_path.join("projects.json");
    storage::write(&registry_file, &registry).unwrap();
    let registry = projects::Registry::load(&registry_file).unwrap();
    for watch in &mut watches {
        let target = watch.target.as_ref().unwrap();
        let environment = registry
            .environment(&original.id, &target.environment_id)
            .unwrap();
        let guest_project = Path::new("/work").join(environment.path.file_name().unwrap());
        // A previous refresh can have excluded the original sessions.
        watch.project = guest_project.clone().into();
        assert!(synchronize(watch).unwrap().0.conversations.is_empty());
        watch.project = environment.guest_project_scope(&guest_project);
        assert!(watch.project.contains(Path::new("/work/p/subdirectory")));
        assert!(!watch.project.contains(Path::new("/work/p-other")));
    }

    let refreshed =
        environment_history::combine(watches.iter().map(|watch| synchronize(watch).unwrap().0));
    assert_eq!(refreshed.usage, original_dashboard.usage);
    assert_eq!(refreshed.conversations, original_dashboard.conversations);
    let targets = watches.map(|watch| watch.target.unwrap());
    let saved = environment_history::recorded_snapshot(&targets, &store).unwrap();
    assert_eq!(saved.usage, original_dashboard.usage);
    assert_eq!(saved.conversations.len(), 2);
    store.close().unwrap();
}

#[test]
fn recorded_snapshot_reflects_an_external_database_commit() {
    let fixture = Fixture::new("sqlite-recorded-external-change");
    let legacy = fixture.watch(AgentKind::Codex);
    write_codex(&legacy);
    synchronize(&legacy).unwrap();
    let (sql, _) = seeded(&fixture, &legacy, &legacy.selected_ledger().unwrap());
    let store = sql.store.as_ref().unwrap();
    let target = sql.target.as_ref().unwrap().clone();
    let before = environment_history::recorded_snapshot(&[target.clone()], store).unwrap();
    assert_eq!(before.conversations[0].state, SessionState::Active);
    let external = rusqlite::Connection::open(fixture.0.join("records/usage.sqlite3")).unwrap();
    external
        .execute("UPDATE session_records SET state='archived'", [])
        .unwrap();
    let after = environment_history::recorded_snapshot(&[target], store).unwrap();
    assert_eq!(after.conversations[0].state, SessionState::Archived);
    let mut unverified = after.clone();
    unverified.conversations[0].available = true;
    environment_history::retain_verified_availability(&mut unverified, None);
    assert!(!unverified.conversations[0].available);
    let mut verified = after.clone();
    verified.conversations[0].available = true;
    let mut current = verified.clone();
    environment_history::retain_verified_availability(&mut current, Some(&verified));
    assert!(current.conversations[0].available);
    current.conversations[0].environment_id =
        Some(crate::projects::EnvironmentId::for_project("other-project"));
    environment_history::retain_verified_availability(&mut current, Some(&verified));
    assert!(!current.conversations[0].available);
    let update = RecordedDashboardUpdate {
        update: DashboardUpdate::new(after),
        availability_verified: true,
    };
    let payload = serde_json::to_value(update).unwrap();
    assert_eq!(payload["availabilityVerified"], true);
    assert!(payload["sequence"].as_u64().is_some());
    assert!(payload["conversations"].is_array());
    drop(external);
    store.close().unwrap();
}

#[test]
fn saved_project_snapshot_combines_shared_environment_usage_without_merging_agent_sessions() {
    let fixture = Fixture::new("sqlite-opening-snapshot-agents");
    let codex = fixture.watch(AgentKind::Codex);
    let mut claude = fixture.watch(AgentKind::ClaudeCode);
    claude.home = codex.home.clone();
    write_codex(&codex);
    write_claude(&claude);
    let codex_dashboard = synchronize(&codex).unwrap().0;
    let claude_dashboard = synchronize(&claude).unwrap().0;
    let (sql, _) = seeded(&fixture, &codex, &codex.selected_ledger().unwrap());
    let store = sql.store.as_ref().unwrap();
    let claude_target = claude.target.as_ref().unwrap().clone();
    store
        .with(|store| {
            store.save_environment(
                &claude_target.usage_environment()?,
                &claude.selected_ledger()?,
            )
        })
        .unwrap();

    let targets = [sql.target.as_ref().unwrap().clone(), claude_target];
    let saved = environment_history::saved_snapshot(&targets, store).unwrap();
    let mut expected_usage = codex_dashboard.usage;
    expected_usage.add(&claude_dashboard.usage);
    assert_eq!(saved.usage, expected_usage);
    assert_eq!(saved.conversations.len(), 2);
    assert_eq!(saved.conversations[0].id, saved.conversations[1].id);
    assert_eq!(
        saved.conversations[0].environment_id,
        saved.conversations[1].environment_id
    );
    assert_ne!(
        saved.conversations[0].agent_kind,
        saved.conversations[1].agent_kind
    );
    assert!(saved.conversations.iter().all(|entry| !entry.available));
    store.close().unwrap();
}

#[test]
fn sql_public_contracts_match_both_agents_and_three_claude_fixtures() {
    for name in ["codex", "main", "priced-subagents", "subagents"] {
        let fixture = Fixture::new(&format!("sqlite-contract-{name}"));
        let kind = if name == "codex" {
            AgentKind::Codex
        } else {
            AgentKind::ClaudeCode
        };
        let legacy = fixture.watch(kind);
        match name {
            "codex" => {
                write_codex(&legacy);
            }
            "main" => {
                write_claude(&legacy);
            }
            _ => family(&legacy, name),
        }
        let expected = synchronize(&legacy).unwrap().0;
        let ledger = legacy.selected_ledger().unwrap();
        let expected_details = usage_details::from_ledger(&ledger, CHAT).unwrap();
        let (sql, source) = seeded(&fixture, &legacy, &ledger);
        let source_bytes = fs::read(&source).unwrap();
        let store = sql.store.as_ref().unwrap();
        let target = sql.target.as_ref().unwrap();
        assert_eq!(
            value(&sql_backend::dashboard(target, store, false).unwrap()),
            value(&expected),
            "{name} dashboard"
        );
        let details =
            sql_backend::details(target, store, CHAT, |_| Ok(Some(sql.namespace()))).unwrap();
        assert_eq!(value(&details), value(&expected_details), "{name} details");
        assert_eq!(
            value(&synchronize(&sql).unwrap().0),
            value(&expected),
            "{name} scan"
        );
        let changed_before = store
            .with(|store| Ok(store.connection.total_changes()))
            .unwrap();
        assert!(!synchronize(&sql).unwrap().1, "{name} quiet scan");
        assert_eq!(
            changed_before,
            store
                .with(|store| Ok(store.connection.total_changes()))
                .unwrap()
        );
        let environment = target.usage_environment().unwrap();
        assert_eq!(
            value(
                &store
                    .with(|store| store.load_environment(&environment))
                    .unwrap()
            ),
            value(&ledger)
        );
        assert_eq!(fs::read(&source).unwrap(), source_bytes);
        let offline_expected =
            environment_history::saved(legacy.target.as_ref(), &legacy.ledger).unwrap();
        assert_eq!(
            value(&sql_backend::dashboard(target, store, true).unwrap()),
            value(&offline_expected),
            "{name} offline"
        );
        assert_eq!(expected.pricing_date, "2026-11-17");
        assert_eq!(expected.anthropic_pricing_date, "2026-11-17");
        if name == "main" {
            assert_eq!(expected.usage.requests, 5);
            assert_eq!(expected.usage.total, 82_948);
            assert!((expected.usage.cost_usd - 0.055747).abs() < 1e-12);
        }
        if name == "priced-subagents" {
            assert_eq!(expected.usage.requests, 4);
            assert_eq!(expected.usage.total, 100_976);
            assert!((expected_details.own_usage.cost_usd - 0.001397).abs() < 1e-12);
            assert!((expected_details.descendants_usage.cost_usd - 0.0585435375).abs() < 1e-12);
        }
        if name == "subagents" {
            assert_eq!(expected.usage.requests, 4);
            assert_eq!(expected.usage.total, 1_350);
            assert_eq!(expected.usage.unpriced_requests, 4);
        }
        if kind == AgentKind::ClaudeCode {
            assert_eq!(expected.coverage, usage::Coverage::Partial);
            assert!(expected_details.user_prompt_count.is_none());
        }
        store.close().unwrap();
    }
}

fn unpriced(mut ledger: Ledger) -> Ledger {
    for record in ledger.sessions.values_mut() {
        for request in record
            .turns
            .values_mut()
            .flat_map(|turn| &mut turn.requests)
        {
            request.anthropic = None;
            request.rates = None;
            request.cost_usd = None;
            request.anthropic_corrections.clear();
            request.unpriced_reason = Some("Legacy fixture price is unavailable".into());
        }
    }
    let historical = ledger.sessions[CHAT].clone();
    ledger
        .variants
        .entry(CHAT.into())
        .or_default()
        .insert("historical-guest".into(), historical);
    ledger
}

#[test]
fn sql_offline_claude_preparation_runs_before_dashboard_or_details_once_and_rolls_back_on_failure()
{
    for first_details in [false, true] {
        let fixture = Fixture::new(&format!("sqlite-offline-{first_details}"));
        let legacy = fixture.watch(AgentKind::ClaudeCode);
        write_claude(&legacy);
        synchronize(&legacy).unwrap();
        let mut raw = unpriced(legacy.selected_ledger().unwrap());
        let historical = raw
            .variants
            .get_mut(CHAT)
            .unwrap()
            .get_mut("historical-guest")
            .unwrap();
        historical.available = false;
        historical.project_exclusion = Some(ProjectExclusion {
            root_session_id: CHAT.into(),
            root_cwd: "/work/other".into(),
        });
        let (sql, original) = seeded(&fixture, &legacy, &raw);
        let original_bytes = fs::read(&original).unwrap();
        let store = sql.store.as_ref().unwrap();
        let target = sql.target.as_ref().unwrap();
        let environment = target.usage_environment().unwrap();
        assert_eq!(
            value(
                &store
                    .with(|store| store.load_environment(&environment))
                    .unwrap()
            ),
            value(&raw)
        );
        storage::write(&legacy.ledger, &raw).unwrap();
        let expected_records = crate::storage::data_location::RecordsDirectory::fixture(&fixture.0);
        let expected = environment_history::load_saved_ledger(
            AgentKind::ClaudeCode,
            &legacy.ledger,
            &expected_records,
        )
        .unwrap();
        let expected_dashboard =
            environment_history::saved(legacy.target.as_ref(), &legacy.ledger).unwrap();
        fs::remove_dir_all(&legacy.home).unwrap();
        store.with(|store| {
            store.connection.execute_batch("CREATE TEMP TRIGGER fail_preparation BEFORE UPDATE OF claude_pricing_version ON usage_environments BEGIN SELECT RAISE(ABORT,'injected policy commit failure'); END;").map_err(crate::usage_store::error)
        }).unwrap();
        assert!(sql_backend::dashboard(target, store, true).is_err());
        assert_eq!(
            value(
                &store
                    .with(|store| store.load_environment(&environment))
                    .unwrap()
            ),
            value(&raw)
        );
        store
            .with(|store| {
                store
                    .connection
                    .execute_batch("DROP TRIGGER fail_preparation")
                    .map_err(crate::usage_store::error)
            })
            .unwrap();
        if first_details {
            // Pricing occurs before the existing ambiguous-guest details guard.
            assert!(sql_backend::details(target, store, CHAT, |variants| {
                assert!(variants);
                Err("Guest is unavailable".into())
            })
            .is_err());
        } else {
            assert_eq!(
                value(&sql_backend::dashboard(target, store, true).unwrap()),
                value(&expected_dashboard)
            );
        }
        let actual = store
            .with(|store| store.load_environment(&environment))
            .unwrap();
        assert_eq!(value(&actual), value(&expected));
        assert!(actual.sessions[CHAT].available);
        for record in actual.sessions.values().chain(
            actual
                .variants
                .values()
                .flat_map(|variants| variants.values()),
        ) {
            assert_eq!(usage::summarize(&record.turns).requests, 5);
            assert!(record
                .turns
                .values()
                .flat_map(|turn| &turn.requests)
                .all(|request| request.anthropic_corrections.len() == 1));
        }
        let changes = store
            .with(|store| Ok(store.connection.total_changes()))
            .unwrap();
        sql_backend::dashboard(target, store, true).unwrap();
        assert!(!store
            .with(|store| store.prepare_claude(&environment))
            .unwrap());
        assert_eq!(
            changes,
            store
                .with(|store| Ok(store.connection.total_changes()))
                .unwrap()
        );
        assert_eq!(fs::read(&original).unwrap(), original_bytes);
        store.close().unwrap();
        let reopened =
            SharedStore::new(Store::open(sql.records_directory.as_ref().unwrap()).unwrap());
        assert_eq!(
            value(&sql_backend::dashboard(target, &reopened, true).unwrap()),
            value(&expected_dashboard)
        );
        reopened.close().unwrap();
    }
}

mod replay;

mod acceptance;
