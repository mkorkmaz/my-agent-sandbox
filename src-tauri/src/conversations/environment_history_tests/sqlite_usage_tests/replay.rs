use super::*;

const CHILD: &str = "bbbbbbbb-0000-4000-8000-000000000002";

fn codex_request(response: &str, cumulative: u64) -> Value {
    let tokens = |count: u64| json!({"input_tokens":100*count,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":10*count,"total_tokens":110*count});
    json!({"type":"token_usage_record","timestamp":format!("2026-11-13T00:00:{cumulative:02}Z"),"payload":{"thread_id":CHAT,"turn_id":"aaaaaaaa-0000-4000-8000-000000000001","root_turn_id":"aaaaaaaa-0000-4000-8000-000000000001","response_id":response,"usage":tokens(1),"thread_token_usage":tokens(cumulative)}})
}

fn append_codex(file: &Path, response: &str, cumulative: u64) {
    let mut output = OpenOptions::new().append(true).open(file).unwrap();
    writeln!(output, "{}", codex_request(response, cumulative)).unwrap();
}

#[test]
fn codex_guest_switches_preserve_both_records_requests_cursors_and_states() {
    let fixture = Fixture::new("sqlite-live-guest-slots");
    let legacy = fixture.watch(AgentKind::Codex);
    write_codex(&legacy);
    synchronize(&legacy).unwrap();
    let (first, original) = imported(&fixture, &legacy, &legacy.selected_ledger().unwrap());
    let original_bytes = fs::read(&original).unwrap();
    let second = Watch {
        home: fixture.0.join("second-codex"),
        ..first.clone()
    };
    write_codex(&second);
    let first_namespace = first.namespace();
    let second_namespace = second.namespace();
    assert_ne!(first_namespace, second_namespace);
    let from_second = synchronize(&second).unwrap().0;
    assert_eq!(from_second.usage.requests, 2);
    change_state(&second, CHAT, SessionState::Archived, |_| false).unwrap();
    for _ in 0..3 {
        let from_first = synchronize(&first).unwrap().0;
        assert_eq!(from_first.usage.requests, 2);
        assert_eq!(from_first.conversations[0].usage.requests, 1);
        assert_eq!(from_first.conversations[0].state, SessionState::Active);
        let from_second = synchronize(&second).unwrap().0;
        assert_eq!(from_second.usage.requests, 2);
        assert_eq!(from_second.conversations[0].usage.requests, 1);
        assert_eq!(from_second.conversations[0].state, SessionState::Archived);
    }
    let saved = second.selected_ledger().unwrap();
    assert_eq!(
        saved.sessions[CHAT].guest_namespace.as_deref(),
        Some(second_namespace.as_str())
    );
    assert_eq!(
        saved.variants[CHAT][&first_namespace]
            .guest_namespace
            .as_deref(),
        Some(first_namespace.as_str())
    );
    assert!(!saved.sessions[CHAT].cursors.is_empty());
    assert!(!saved.variants[CHAT][&first_namespace].cursors.is_empty());
    assert_eq!(fs::read(&original).unwrap(), original_bytes);
}

#[test]
fn details_resolve_a_selected_legacy_variant_namespace_from_its_own_evidence() {
    let fixture = Fixture::new("sqlite-details-variant-namespace");
    let legacy = fixture.watch(AgentKind::Codex);
    write_codex(&legacy);
    synchronize(&legacy).unwrap();
    let mut ledger = legacy.selected_ledger().unwrap();
    let mut historical = ledger.sessions[CHAT].clone();
    historical.title = "Historical guest root".into();
    historical.guest_namespace = None;
    historical.cursors.clear();
    let namespace = "guest:fixture-historical-owner";
    for request in historical
        .turns
        .values_mut()
        .flat_map(|turn| &mut turn.requests)
    {
        request.source.as_mut().unwrap().guest_namespace = namespace.into();
    }
    let mut child = historical.clone();
    child.title = "Historical guest child".into();
    child.guest_namespace = Some(namespace.into());
    child.lineage.kind = lineage::Kind::Subagent;
    child.lineage.parent_session_id = Some(CHAT.into());
    child.source_root_id = Some(CHAT.into());
    for turn in child.turns.values_mut() {
        turn.origin_thread_id = Some(CHILD.into());
        for request in &mut turn.requests {
            let source = request.source.as_mut().unwrap();
            source.origin_thread_id = CHILD.into();
            source.response_id = "child-response".into();
        }
    }
    ledger.sessions.insert(CHILD.into(), child);
    ledger
        .variants
        .entry(CHAT.into())
        .or_default()
        .insert(namespace.into(), historical);
    let (sql, _) = imported(&fixture, &legacy, &ledger);
    let mut expected = ledger.clone();
    expected.select_guest(namespace);
    let expected = usage_details::from_ledger(&expected, CHAT).unwrap();
    assert_eq!(expected.combined_usage.requests, 2);
    let actual = sql_backend::details(
        sql.target.as_ref().unwrap(),
        sql.store.as_ref().unwrap(),
        CHAT,
        |_| Ok(Some(namespace.into())),
    )
    .unwrap();
    assert_eq!(value(&actual), value(&expected));
}

#[test]
fn an_aborted_cursor_write_rolls_back_request_rows_and_does_not_publish() {
    let fixture = Fixture::new("sqlite-request-cursor-rollback");
    let legacy = fixture.watch(AgentKind::Codex);
    let source = write_codex(&legacy);
    let dashboard = synchronize(&legacy).unwrap().0;
    let ledger = legacy.selected_ledger().unwrap();
    let (sql, original) = imported(&fixture, &legacy, &ledger);
    let original_bytes = fs::read(&original).unwrap();
    let store = sql.store.as_ref().unwrap();
    let environment = sql.target.as_ref().unwrap().usage_environment().unwrap();
    let history = History::default();
    history.record_published(PROJECT, &dashboard).unwrap();
    append_codex(&source, "response-2", 2);
    store.with(|store| store.connection.execute_batch("CREATE TEMP TRIGGER fail_cursor BEFORE UPDATE ON source_cursors BEGIN SELECT RAISE(ABORT,'injected cursor write failure'); END;").map_err(crate::usage_store::error)).unwrap();
    assert!(synchronize(&sql).is_err());
    let after = store
        .with(|store| store.load_environment(&environment))
        .unwrap();
    assert_eq!(value(&after), value(&ledger));
    assert!(!history.should_publish(PROJECT, &dashboard).unwrap());
    assert_eq!(
        value(history.published.lock().unwrap().get(PROJECT).unwrap()),
        value(&dashboard)
    );
    store
        .with(|store| {
            store
                .connection
                .execute_batch("DROP TRIGGER fail_cursor")
                .map_err(crate::usage_store::error)
        })
        .unwrap();
    let (committed, changed) = synchronize(&sql).unwrap();
    assert!(changed);
    assert_eq!(committed.usage.requests, 2);
    assert!(!synchronize(&sql).unwrap().1);
    assert_eq!(synchronize(&sql).unwrap().0.usage, committed.usage);
    assert_eq!(fs::read(&original).unwrap(), original_bytes);
}

#[test]
fn a_state_change_uses_the_requested_guest_and_preserves_the_other_guests_state() {
    let fixture = Fixture::new("sqlite-state-guest-scope");
    let legacy = fixture.watch(AgentKind::Codex);
    write_codex(&legacy);
    synchronize(&legacy).unwrap();
    let (first, _) = imported(&fixture, &legacy, &legacy.selected_ledger().unwrap());
    let second = Watch {
        home: fixture.0.join("second-codex"),
        ..first.clone()
    };
    write_codex(&second);
    synchronize(&second).unwrap();
    change_state(&first, CHAT, SessionState::Archived, |_| false).unwrap();
    let first_ledger = first.selected_ledger().unwrap();
    let second_ledger = second.selected_ledger().unwrap();
    assert_eq!(first_ledger.sessions[CHAT].state, SessionState::Archived);
    assert_eq!(second_ledger.sessions[CHAT].state, SessionState::Active);
}

#[test]
fn claude_copied_child_evidence_keeps_the_saved_owner_when_its_transcript_is_missing() {
    let fixture = Fixture::new("sqlite-claude-missing-copy-owner");
    let legacy = fixture.watch(AgentKind::ClaudeCode);
    family(&legacy, "priced-subagents");
    let initial = synchronize(&legacy).unwrap().0;
    let (sql, original) = imported(&fixture, &legacy, &legacy.selected_ledger().unwrap());
    let original_bytes = fs::read(&original).unwrap();
    let root = claude_file(&legacy, CHAT);
    let alpha = root.with_extension("").join("subagents/agent-alpha.jsonl");
    let response: Value = fs::read_to_string(&alpha)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|value| value["type"] == "assistant")
        .unwrap();
    let mut output = OpenOptions::new().append(true).open(&root).unwrap();
    writeln!(output, "{response}").unwrap();
    drop(output);
    fs::remove_file(&alpha).unwrap();
    let expected = synchronize(&legacy).unwrap().0;
    let actual = synchronize(&sql).unwrap().0;
    assert_eq!(actual.usage, initial.usage);
    assert_eq!(actual.usage.requests, 4);
    assert_eq!(value(&actual), value(&expected));
    let child = claude::subagents::session_id(CHAT, "alpha");
    let facts = sql.selected_ledger().unwrap();
    assert_eq!(usage::summarize(&facts.sessions[&child].turns).requests, 1);
    assert!(!facts.sessions[&child].available);
    assert_eq!(usage::summarize(&facts.sessions[CHAT].turns).requests, 1);
    for _ in 0..3 {
        assert!(!synchronize(&sql).unwrap().1);
        assert_eq!(synchronize(&sql).unwrap().0.usage, initial.usage);
    }
    assert_eq!(fs::read(&original).unwrap(), original_bytes);
}

#[test]
fn failed_live_scans_keep_the_previous_committed_usage_in_the_history_cache() {
    let fixture = Fixture::new("sqlite-cache-after-commit-failure");
    let legacy = fixture.watch(AgentKind::Codex);
    let source = write_codex(&legacy);
    let dashboard = synchronize(&legacy).unwrap().0;
    let (sql, _) = imported(&fixture, &legacy, &legacy.selected_ledger().unwrap());
    let history = History::default();
    history.record_result(PROJECT, dashboard.clone()).unwrap();
    history.record_published(PROJECT, &dashboard).unwrap();
    append_codex(&source, "response-2", 2);
    sql.store.as_ref().unwrap().with(|store| store.connection.execute_batch("CREATE TEMP TRIGGER fail_cursor_cache BEFORE UPDATE ON source_cursors BEGIN SELECT RAISE(ABORT,'injected cursor failure'); END;").map_err(crate::usage_store::error)).unwrap();
    history.refresh(PROJECT, &sql).unwrap();
    let latest = history.latest.lock().unwrap();
    let latest = latest.get(PROJECT).unwrap();
    assert_eq!(latest.usage, dashboard.usage);
    assert_eq!(latest.usage.requests, 1);
    assert!(latest
        .warnings
        .iter()
        .any(|warning| warning.contains("injected cursor failure")));
    assert!(latest
        .conversations
        .iter()
        .all(|conversation| !conversation.available));
    assert_eq!(
        history.published.lock().unwrap()[PROJECT].usage,
        dashboard.usage
    );
}

#[test]
fn partial_codex_lines_rotation_truncation_and_source_loss_keep_recorded_facts() {
    let fixture = Fixture::new("sqlite-partial-rotation-replay");
    let legacy = fixture.watch(AgentKind::Codex);
    let source = write_codex(&legacy);
    synchronize(&legacy).unwrap();
    let initial_source = fs::read(&source).unwrap();
    let (sql, original) = imported(&fixture, &legacy, &legacy.selected_ledger().unwrap());
    let original_bytes = fs::read(&original).unwrap();
    let mut output = OpenOptions::new().append(true).open(&source).unwrap();
    write!(output, "{}", codex_request("response-2", 2)).unwrap();
    drop(output);
    let partial = synchronize(&sql).unwrap().0;
    assert_eq!(partial.usage.requests, 1);
    assert_eq!(value(&partial), value(&synchronize(&legacy).unwrap().0));
    let mut output = OpenOptions::new().append(true).open(&source).unwrap();
    writeln!(output).unwrap();
    drop(output);
    let complete = synchronize(&sql).unwrap().0;
    assert_eq!(complete.usage.requests, 2);
    assert_eq!(value(&complete), value(&synchronize(&legacy).unwrap().0));
    assert!(!synchronize(&sql).unwrap().1);
    fs::rename(&source, source.with_extension("old-fixture")).unwrap();
    fs::write(&source, &initial_source).unwrap();
    let rotated = synchronize(&sql).unwrap().0;
    assert_eq!(rotated.usage, complete.usage);
    assert_eq!(value(&rotated), value(&synchronize(&legacy).unwrap().0));
    let header = String::from_utf8(initial_source)
        .unwrap()
        .lines()
        .take(2)
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    fs::write(&source, header).unwrap();
    let truncated = synchronize(&sql).unwrap().0;
    assert_eq!(truncated.usage, complete.usage);
    assert_eq!(value(&truncated), value(&synchronize(&legacy).unwrap().0));
    fs::remove_dir_all(&legacy.home).unwrap();
    let missing = synchronize(&sql).unwrap().0;
    assert_eq!(missing.usage, complete.usage);
    assert_eq!(value(&missing), value(&synchronize(&legacy).unwrap().0));
    assert_eq!(fs::read(&original).unwrap(), original_bytes);
}

#[test]
fn failed_saved_claude_preparation_keeps_facts_and_both_history_caches_unchanged() {
    let fixture = Fixture::new("sqlite-price-failure-cache-guard");
    let legacy = fixture.watch(AgentKind::ClaudeCode);
    write_claude(&legacy);
    synchronize(&legacy).unwrap();
    let raw = unpriced(legacy.selected_ledger().unwrap());
    let before = dashboard_from_ledger(raw.clone(), HashMap::new()).unwrap();
    let (sql, _) = imported(&fixture, &legacy, &raw);
    let target = sql.target.as_ref().unwrap();
    let environment = target.usage_environment().unwrap();
    let store = sql.store.as_ref().unwrap();
    let history = History::default();
    history.record_result(PROJECT, before.clone()).unwrap();
    history.record_published(PROJECT, &before).unwrap();
    store.with(|store| store.connection.execute_batch("CREATE TEMP TRIGGER fail_policy_cache BEFORE UPDATE OF claude_pricing_version ON usage_environments BEGIN SELECT RAISE(ABORT,'injected pricing checkpoint failure'); END;").map_err(crate::usage_store::error)).unwrap();
    assert!(history.refresh(PROJECT, &sql).is_err());
    assert!(history
        .saved_result(PROJECT, target, store, "offline fixture".into())
        .is_err());
    assert_eq!(
        value(history.latest.lock().unwrap().get(PROJECT).unwrap()),
        value(&before)
    );
    assert_eq!(
        value(history.published.lock().unwrap().get(PROJECT).unwrap()),
        value(&before)
    );
    assert_eq!(
        value(
            &store
                .with(|store| store.load_environment(&environment))
                .unwrap()
        ),
        value(&raw)
    );
    let pending = store
        .with(|store| {
            store
                .connection
                .query_row(
                    "SELECT claude_pricing_version FROM usage_environments",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(crate::usage_store::error)
        })
        .unwrap();
    assert_eq!(pending, 0);
}

#[test]
fn unchanged_codex_scan_loads_no_raw_records_and_commits_no_data_change() {
    let fixture = Fixture::new("sqlite-quiet-scan-structure");
    let legacy = fixture.watch(AgentKind::Codex);
    write_codex(&legacy);
    synchronize(&legacy).unwrap();
    let (sql, _) = imported(&fixture, &legacy, &legacy.selected_ledger().unwrap());
    // The first live open prepares saved provider corrections once per connection.
    synchronize(&sql).unwrap();
    let store = sql.store.as_ref().unwrap();
    let before = store
        .with(|store| Ok(store.connection.total_changes()))
        .unwrap();
    crate::usage_store::record_reads::begin();
    let result = synchronize(&sql);
    let reads = crate::usage_store::record_reads::finish();
    let (_, changed) = result.unwrap();
    assert!(!changed);
    assert!(reads.is_empty(), "quiet scan loaded records: {reads:?}");
    assert_eq!(
        before,
        store
            .with(|store| Ok(store.connection.total_changes()))
            .unwrap()
    );
}

#[test]
fn a_current_guest_state_change_updates_one_row_without_loading_raw_records() {
    let fixture = Fixture::new("sqlite-one-state-row-structure");
    let legacy = fixture.watch(AgentKind::Codex);
    write_codex(&legacy);
    synchronize(&legacy).unwrap();
    let (sql, _) = imported(&fixture, &legacy, &legacy.selected_ledger().unwrap());
    let store = sql.store.as_ref().unwrap();
    let before = store
        .with(|store| Ok(store.connection.total_changes()))
        .unwrap();
    crate::usage_store::record_reads::begin();
    let result = change_state(&sql, CHAT, SessionState::Archived, |_| false);
    let reads = crate::usage_store::record_reads::finish();
    result.unwrap();
    assert!(reads.is_empty(), "state change loaded records: {reads:?}");
    assert_eq!(
        store
            .with(|store| Ok(store.connection.total_changes()))
            .unwrap()
            - before,
        1
    );
    let state = store
        .with(|store| {
            store
                .connection
                .query_row(
                    "SELECT state FROM session_records WHERE variant_key IS NULL",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .map_err(crate::usage_store::error)
        })
        .unwrap();
    assert_eq!(state, "archived");
}

#[test]
fn historical_details_load_only_the_chosen_slot_without_hidden_or_unrelated_records() {
    let fixture = Fixture::new("sqlite-selected-slot-details-structure");
    let legacy = fixture.watch(AgentKind::Codex);
    write_codex(&legacy);
    synchronize(&legacy).unwrap();
    let mut ledger = legacy.selected_ledger().unwrap();
    let namespace = "guest:chosen-historical-slot";
    let mut chosen = ledger.sessions[CHAT].clone();
    chosen.title = "Chosen historical root".into();
    chosen.guest_namespace = Some(namespace.into());
    ledger
        .variants
        .entry(CHAT.into())
        .or_default()
        .insert(namespace.into(), chosen);
    let mut unrelated = ledger.sessions[CHAT].clone();
    unrelated.title = "Unrelated root".into();
    unrelated.guest_namespace = Some(namespace.into());
    ledger.sessions.insert(CHILD.into(), unrelated);
    let (sql, _) = imported(&fixture, &legacy, &ledger);
    let store = sql.store.as_ref().unwrap();
    let other_project = "2".repeat(32);
    let other = crate::usage_store::Environment::new(
        other_project.clone(),
        projects::EnvironmentId::for_agent(&other_project, AgentKind::Codex),
        AgentKind::Codex,
    )
    .unwrap();
    store
        .with(|store| store.save_environment(&other, &ledger).map(|_| ()))
        .unwrap();
    let expected_id = store.with(|store| store.connection.query_row("SELECT r.record_id FROM usage_environments e JOIN session_groups g ON g.environment_key=e.environment_key JOIN session_records r ON r.session_key=g.session_key WHERE e.project_id=?1 AND g.native_session_id=?2 AND r.variant_key=?3", rusqlite::params![PROJECT, CHAT, namespace], |row| row.get::<_, i64>(0)).map_err(crate::usage_store::error)).unwrap();
    crate::usage_store::record_reads::begin();
    let result = sql_backend::details(sql.target.as_ref().unwrap(), store, CHAT, |_| {
        Ok(Some(namespace.into()))
    });
    let reads = crate::usage_store::record_reads::finish();
    let details = result.unwrap();
    assert_eq!(details.agents.len(), 1);
    assert_eq!(details.agents[0].title, "Chosen historical root");
    assert_eq!(reads, vec![expected_id]);
}

#[test]
fn a_mixed_v1_codex_record_splits_guests_without_losing_unattributed_spending() {
    let fixture = Fixture::new("sqlite-mixed-v1-native-owners");
    let first = fixture.watch(AgentKind::Codex);
    let second = Watch {
        home: fixture.0.join("second-codex"),
        ledger: fixture.0.join("second-native-ledger.json"),
        ..first.clone()
    };
    for watch in [&first, &second] {
        write_codex(watch);
        synchronize(watch).unwrap();
    }
    let mut raw = first.selected_ledger().unwrap();
    let mut other = second
        .selected_ledger()
        .unwrap()
        .sessions
        .remove(CHAT)
        .unwrap();
    raw.schema_version = 1;
    let record = raw.sessions.get_mut(CHAT).unwrap();
    record.guest_namespace = None;
    record.state = SessionState::Archived;
    record.cursors.append(&mut other.cursors);
    let bucket = record.turns.keys().next().unwrap().clone();
    let requests = &mut record.turns.get_mut(&bucket).unwrap().requests;
    requests.append(&mut other.turns.get_mut(&bucket).unwrap().requests);
    let mut unattributed = requests[0].clone();
    unattributed.key = "legacy-unattributed".into();
    unattributed.source = None;
    unattributed.cost_usd = Some(1.25);
    requests.push(unattributed);
    let (sql_first, original) = imported(&fixture, &first, &raw);
    let original_bytes = fs::read(&original).unwrap();
    let backup_bytes = fs::read(storage::v1_backup_path(&original)).unwrap();
    storage::write(&first.ledger, &raw).unwrap();
    let legacy_second = Watch {
        ledger: first.ledger.clone(),
        ..second.clone()
    };
    let sql_second = Watch {
        home: second.home,
        ..sql_first.clone()
    };
    for (actual, expected) in [
        (&sql_first, &first),
        (&sql_second, &legacy_second),
        (&sql_first, &first),
    ] {
        let expected_dashboard = synchronize(expected).unwrap().0;
        let actual_dashboard = synchronize(actual).unwrap().0;
        assert_eq!(value(&actual_dashboard), value(&expected_dashboard));
        assert_eq!(actual_dashboard.usage.requests, 3);
        assert_eq!(actual_dashboard.usage.total, 330);
        assert_eq!(
            actual_dashboard.conversations[0].state,
            SessionState::Active
        );
        let facts = actual.selected_ledger().unwrap();
        assert_eq!(
            usage_details::from_ledger(&facts, CHAT)
                .unwrap()
                .combined_usage
                .requests,
            1
        );
        assert_eq!(facts.variants[CHAT][""].state, SessionState::Archived);
        assert_eq!(
            facts.variants[CHAT][""].turns[&bucket].requests[0].cost_usd,
            Some(1.25)
        );
    }
    assert_eq!(fs::read(&original).unwrap(), original_bytes);
    assert_eq!(
        fs::read(storage::v1_backup_path(&original)).unwrap(),
        backup_bytes
    );
}
