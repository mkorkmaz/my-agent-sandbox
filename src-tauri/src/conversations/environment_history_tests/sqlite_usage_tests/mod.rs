use super::*;
use crate::usage_store::{importer, SharedStore, Store};

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

fn imported(fixture: &Fixture, watch: &Watch, ledger: &Ledger) -> (Watch, PathBuf) {
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
    let before = fs::read(&path).unwrap();
    if target.agent_kind == AgentKind::ClaudeCode {
        fs::write(
            storage::v1_backup_path(&path),
            b"immutable previous v1 data\n",
        )
        .unwrap();
    }
    importer::activate(&records).unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    let mut sql = watch.clone();
    sql.records_directory = Some(records.clone());
    sql.store = Some(SharedStore::new(Store::open(&records).unwrap()));
    (sql, path)
}

fn value(value: &impl Serialize) -> Value {
    serde_json::to_value(value).unwrap()
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
        let (sql, _) = imported(&fixture, &legacy, &ledger);
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
            environment_id: projects::EnvironmentId::for_agent(&other_id, kind),
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
fn recorded_snapshot_reflects_an_external_database_commit() {
    let fixture = Fixture::new("sqlite-recorded-external-change");
    let legacy = fixture.watch(AgentKind::Codex);
    write_codex(&legacy);
    synchronize(&legacy).unwrap();
    let (sql, _) = imported(&fixture, &legacy, &legacy.selected_ledger().unwrap());
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
    current.conversations[0].environment_id = Some(crate::projects::EnvironmentId::for_agent(
        "other-project",
        AgentKind::Codex,
    ));
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
fn saved_project_snapshot_combines_agent_environments_without_merging_their_sessions() {
    let fixture = Fixture::new("sqlite-opening-snapshot-agents");
    let codex = fixture.watch(AgentKind::Codex);
    let claude = fixture.watch(AgentKind::ClaudeCode);
    write_codex(&codex);
    write_claude(&claude);
    let codex_dashboard = synchronize(&codex).unwrap().0;
    let claude_dashboard = synchronize(&claude).unwrap().0;
    let (sql, _) = imported(&fixture, &codex, &codex.selected_ledger().unwrap());
    let store = sql.store.as_ref().unwrap();
    let claude_target = claude.target.as_ref().unwrap().clone();
    store
        .with(|store| {
            store.save_environment(&claude_target.usage_environment()?, &claude.selected_ledger()?)
        })
        .unwrap();

    let targets = [sql.target.as_ref().unwrap().clone(), claude_target];
    let saved = environment_history::saved_snapshot(&targets, store).unwrap();
    let mut expected_usage = codex_dashboard.usage;
    expected_usage.add(&claude_dashboard.usage);
    assert_eq!(saved.usage, expected_usage);
    assert_eq!(saved.conversations.len(), 2);
    assert_eq!(saved.conversations[0].id, saved.conversations[1].id);
    assert_ne!(
        saved.conversations[0].environment_id,
        saved.conversations[1].environment_id
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
        let (sql, source) = imported(&fixture, &legacy, &ledger);
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
        assert_eq!(expected.pricing_date, "2026-11-10");
        assert_eq!(expected.anthropic_pricing_date, "2026-11-13");
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
        let (sql, original) = imported(&fixture, &legacy, &raw);
        let original_bytes = fs::read(&original).unwrap();
        let backup = storage::v1_backup_path(&original);
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
        assert_eq!(fs::read(&backup).unwrap(), b"immutable previous v1 data\n");
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
