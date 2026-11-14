use super::*;

fn native_codex_family(watch: &Watch) {
    let directory = watch.home.join(".codex/sessions");
    fs::create_dir_all(&directory).unwrap();
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/codex-subagents/v0.167.0");
    for name in [
        "primary.jsonl",
        "child-alpha.jsonl",
        "child-beta.jsonl",
        "grandchild-gamma.jsonl",
        "normal-fork.jsonl",
        "p02/history-primary.jsonl",
        "p02/history-child.jsonl",
    ] {
        fs::copy(
            source.join(name),
            directory.join(format!(
                "rollout-{}",
                Path::new(name).file_name().unwrap().to_str().unwrap()
            )),
        )
        .unwrap();
    }
}

#[test]
fn full_api_event_resume_and_quiet_contracts_match_on_both_agents_native_families() {
    for name in [
        "codex-family",
        "claude-main",
        "priced-subagents",
        "subagents",
    ] {
        let fixture = Fixture::new(&format!("sqlite-acceptance-api-{name}"));
        let kind = if name == "codex-family" {
            AgentKind::Codex
        } else {
            AgentKind::ClaudeCode
        };
        let mut legacy = fixture.watch(kind);
        let root_id = if kind == AgentKind::Codex {
            "00000000-0000-4000-8000-000000000001"
        } else {
            CHAT
        };
        if kind == AgentKind::Codex {
            legacy.project = "/work/fixture-project".into();
            native_codex_family(&legacy);
        }
        if name == "claude-main" {
            write_claude(&legacy);
        }
        if ["priced-subagents", "subagents"].contains(&name) {
            family(&legacy, name);
        }
        synchronize(&legacy).unwrap();
        let raw = legacy.selected_ledger().unwrap();
        let expected = synchronize(&legacy).unwrap().0;
        let expected_details =
            usage_details::from_ledger(&legacy.selected_ledger().unwrap(), root_id).unwrap();
        let (sql, original) = imported(&fixture, &legacy, &raw);
        let original_bytes = fs::read(&original).unwrap();
        let actual = synchronize(&sql).unwrap().0;
        assert_eq!(value(&actual), value(&expected), "{name} dashboard");
        let actual_details = sql_backend::details(
            sql.target.as_ref().unwrap(),
            sql.store.as_ref().unwrap(),
            root_id,
            |_| Ok(Some(sql.namespace())),
        )
        .unwrap();
        assert_eq!(
            value(&actual_details),
            value(&expected_details),
            "{name} details"
        );
        let legacy_update = DashboardUpdate {
            dashboard: expected.clone(),
            sequence: 1,
        };
        let sql_update = DashboardUpdate {
            dashboard: actual.clone(),
            sequence: 1,
        };
        assert_eq!(value(&legacy_update), value(&sql_update));
        let legacy_event = value(&(PROJECT, &legacy_update));
        let sql_event = value(&(PROJECT, &sql_update));
        assert_eq!(legacy_event, sql_event);
        let history = History::default();
        history.record_published(PROJECT, &actual).unwrap();
        let quiet = synchronize(&sql).unwrap();
        assert!(!quiet.1, "{name} quiet");
        assert!(!history.should_publish(PROJECT, &quiet.0).unwrap());
        assert!(resume_dashboard(&legacy, root_id).is_ok());
        assert!(resume_dashboard(&sql, root_id).is_ok());
        for row in &actual.conversations {
            assert_eq!(
                resume_dashboard(&legacy, &row.id).is_ok(),
                resume_dashboard(&sql, &row.id).is_ok(),
                "{name} {} resume",
                row.id
            );
        }
        let descendants = ledger_graph(&raw).descendants(root_id);
        for id in &descendants {
            assert!(resume_dashboard(&legacy, id).is_err());
            assert!(resume_dashboard(&sql, id).is_err());
        }
        let before = sql.load().unwrap();
        assert!(change_state(&sql, root_id, SessionState::Archived, |ids| {
            assert!(ids.contains(&root_id.to_owned()));
            true
        })
        .is_err());
        assert_eq!(value(&sql.load().unwrap()), value(&before));
        for state in [
            SessionState::Archived,
            SessionState::Deleted,
            SessionState::Active,
        ] {
            change_state(&legacy, root_id, state, |_| false).unwrap();
            change_state(&sql, root_id, state, |_| false).unwrap();
            let expected = synchronize(&legacy).unwrap().0;
            let actual = synchronize(&sql).unwrap().0;
            assert_eq!(value(&actual), value(&expected), "{name} state");
            assert_eq!(actual.usage, quiet.0.usage);
        }
        if kind == AgentKind::ClaudeCode {
            assert_eq!(actual.coverage, usage::Coverage::Partial);
            assert!(actual_details.user_prompt_count.is_none());
        }
        assert_eq!(actual.pricing_date, "2026-11-10");
        assert_eq!(actual.anthropic_pricing_date, "2026-11-13");
        for row in &actual.conversations {
            assert_eq!(row.usage_revision.len(), 64);
            assert!(row.usage_revision.bytes().any(|byte| byte != b'0'));
        }
        emit(
            &format!("api-{name}"),
            &json!({"dashboard":actual,"details":actual_details,"update":sql_update,"event":sql_event,"typedLedger":sql.load().unwrap(),"dashboardSha256":crate::hash::hex(Sha256::digest(serde_json::to_vec(&actual).unwrap())),"detailsSha256":crate::hash::hex(Sha256::digest(serde_json::to_vec(&actual_details).unwrap())),"eventSha256":crate::hash::hex(Sha256::digest(serde_json::to_vec(&sql_event).unwrap())),"resumeRoot":true,"resumeDescendantsRejected":descendants,"quietEventSuppressed":true,"runningFamilyArchiveRejected":true}),
        );
        assert_eq!(fs::read(&original).unwrap(), original_bytes);
    }
}

#[test]
fn graph_orphan_unknown_fork_exclusion_and_empty_variant_shapes_match() {
    let (fixture, sql, _native, _original, baseline) = generated();
    let environment = environment(&sql);
    let mut ledger = baseline.clone();
    let root = ledger.sessions[CHAT].clone();
    for (id, kind, parent) in [
        (
            "00000000-0000-4000-8000-000000000011",
            lineage::Kind::Subagent,
            Some("00000000-0000-4000-8000-000000000099"),
        ),
        (
            "00000000-0000-4000-8000-000000000012",
            lineage::Kind::Unknown,
            None,
        ),
        (
            "00000000-0000-4000-8000-000000000013",
            lineage::Kind::Primary,
            None,
        ),
        (
            "00000000-0000-4000-8000-000000000014",
            lineage::Kind::Subagent,
            Some(CHAT),
        ),
        (
            "00000000-0000-4000-8000-000000000016",
            lineage::Kind::Subagent,
            None,
        ),
        (
            "00000000-0000-4000-8000-000000000017",
            lineage::Kind::Subagent,
            Some(CHAT),
        ),
    ] {
        let mut record = root.clone();
        record.title = format!("shape {id}");
        record.lineage.kind = kind;
        record.lineage.parent_session_id = parent.map(str::to_owned);
        for request in record
            .turns
            .values_mut()
            .flat_map(|turn| &mut turn.requests)
        {
            request.source.as_mut().unwrap().origin_thread_id = id.into();
            request.source.as_mut().unwrap().response_id = id.into();
        }
        if id.ends_with("017") {
            record.lineage_problem = Some(lineage::Problem::Malformed);
        }
        if id.ends_with("014") {
            record.project_exclusion = Some(ProjectExclusion {
                root_session_id: CHAT.into(),
                root_cwd: "/work/other-project".into(),
            });
        }
        ledger.sessions.insert(id.into(), record);
    }
    ledger.variants.insert(
        "00000000-0000-4000-8000-000000000015".into(),
        BTreeMap::new(),
    );
    sql.store
        .as_ref()
        .unwrap()
        .with(|store| store.save_environment(&environment, &ledger))
        .unwrap();
    let mut expected = dashboard_from_ledger(ledger.clone(), HashMap::new()).unwrap();
    sql.target.as_ref().unwrap().annotate(&mut expected);
    let actual = sql_backend::dashboard(
        sql.target.as_ref().unwrap(),
        sql.store.as_ref().unwrap(),
        false,
    )
    .unwrap();
    assert_eq!(value(&actual), value(&expected));
    assert_eq!(actual.conversations.len(), 6);
    assert!(actual
        .conversations
        .iter()
        .any(|row| row.relation_status == graph::Status::Orphan));
    assert!(actual
        .conversations
        .iter()
        .all(|row| !row.id.ends_with("014")));
    assert!(sql.load().unwrap().variants["00000000-0000-4000-8000-000000000015"].is_empty());
    emit(
        "scope-lineage-shapes",
        &json!({"dashboard":actual,"ledger":sql.load().unwrap(),"fixture":fixture.0}),
    );
}

#[test]
fn equal_native_ids_stay_scoped_to_project_environment_and_guest() {
    let (fixture, sql, _native, _original, baseline) = generated();
    let store = sql.store.as_ref().unwrap();
    let mut first = baseline.clone();
    let mut historical = first.sessions[CHAT].clone();
    historical.title = "Historical guest".into();
    historical.guest_namespace = Some("guest:historical-field".into());
    for request in historical
        .turns
        .values_mut()
        .flat_map(|turn| &mut turn.requests)
    {
        request.source.as_mut().unwrap().guest_namespace = "guest:historical-field".into();
        request.cost_usd = Some(0.25);
        let rate = request.rates.as_mut().unwrap();
        rate.date = "2025-01-02".into();
        rate.input = 2000.0;
        rate.output = 5000.0;
    }
    first
        .variants
        .entry(CHAT.into())
        .or_default()
        .insert("guest:historical-slot".into(), historical);
    store
        .with(|store| store.save_environment(&environment(&sql), &first))
        .unwrap();
    let other_project = "2".repeat(32);
    let other = crate::usage_store::Environment::new(
        other_project.clone(),
        projects::EnvironmentId::for_agent(&other_project, AgentKind::Codex),
        AgentKind::Codex,
    )
    .unwrap();
    let mut second = baseline.clone();
    second.sessions.get_mut(CHAT).unwrap().title = "Other project".into();
    store
        .with(|store| store.save_environment(&other, &second))
        .unwrap();
    let claude = fixture.watch(AgentKind::ClaudeCode);
    write_claude(&claude);
    synchronize(&claude).unwrap();
    let claude_environment = environment(&claude);
    let claude_raw = claude.selected_ledger().unwrap();
    store
        .with(|store| store.save_environment(&claude_environment, &claude_raw))
        .unwrap();
    assert_eq!(
        store
            .with(|store| store.load_environment(&other))
            .unwrap()
            .sessions[CHAT]
            .title,
        "Other project"
    );
    let a = sql_backend::dashboard(sql.target.as_ref().unwrap(), store, false).unwrap();
    let b = sql_backend::dashboard(
        &environment_history::Target {
            project_id: other_project,
            environment_id: other.environment_id.clone(),
            agent_kind: AgentKind::Codex,
        },
        store,
        false,
    )
    .unwrap();
    let c = sql_backend::dashboard(claude.target.as_ref().unwrap(), store, false).unwrap();
    assert_eq!(a.usage.requests, 2);
    assert_eq!(a.conversations.len(), 1);
    assert_eq!(a.conversations[0].usage.requests, 1);
    assert_eq!(b.usage.requests, 1);
    assert_eq!(c.usage.requests, 5);
    assert!((a.usage.cost_usd - 0.2503).abs() < 1e-12);
    let combined = environment_history::combine([a.clone(), c.clone()].into_iter());
    assert_eq!(combined.usage.requests, 7);
    emit(
        "scope-collisions",
        &json!({"projectACodex":a,"projectBCodex":b,"projectAClaude":c,"projectACombined":combined}),
    );
}

#[test]
fn registry_environment_removal_and_forget_leave_all_saved_sql_facts() {
    let fixture = Fixture::new("sqlite-acceptance-registry-retention");
    let project_path = fixture.0.join("registered-project");
    fs::create_dir(&project_path).unwrap();
    let mut registry = projects::Registry::default();
    let project = registry.register(&project_path, "fixture-profile").unwrap();
    registry
        .ensure_environment(
            &project.id,
            AgentKind::ClaudeCode,
            &crate::desktop::settings::Settings::default(),
        )
        .unwrap();
    let before_registry = registry.clone();
    let directory = fixture.0.join("records");
    fs::create_dir(&directory).unwrap();
    let records = RecordsDirectory::fixture(&directory);
    importer::activate(&records).unwrap();
    let store = SharedStore::new(Store::open(&records).unwrap());
    let mut watch = fixture.watch(AgentKind::Codex);
    watch.target = Some(environment_history::Target::for_project(&project));
    watch.records_directory = Some(records.clone());
    watch.store = Some(store.clone());
    watch.project = project_path.clone();
    let source = write_codex(&watch);
    let contents = fs::read_to_string(&source)
        .unwrap()
        .replace("/work/p", project_path.to_str().unwrap());
    fs::write(&source, contents).unwrap();
    synchronize(&watch).unwrap();
    let before = watch.load().unwrap();
    let key = watch.target.as_ref().unwrap().key();
    let history = History::default();
    history.watches.lock().unwrap().insert(key, watch.clone());
    history
        .remove(&project.id, Some(&project.environment_id), true)
        .unwrap();
    registry
        .remove_environment(&project.id, &project.environment_id)
        .unwrap();
    storage::write(&directory.join("projects.json"), &registry).unwrap();
    fs::remove_dir_all(&watch.home).unwrap();
    assert_eq!(value(&watch.load().unwrap()), value(&before));
    let mut forgotten = before_registry;
    fs::rename(&project_path, fixture.0.join("unavailable-project")).unwrap();
    projects::forget_fixture_project(&mut forgotten, &project.id).unwrap();
    storage::write(&directory.join("projects.json"), &forgotten).unwrap();
    history.unobserve(&project.id).unwrap();
    assert!(forgotten.projects.is_empty());
    assert_eq!(value(&watch.load().unwrap()), value(&before));
    emit(
        "registry-retention",
        &json!({"forgottenProjectId":project.id,"environmentRemoved":project.environment_id,"savedLedgerSha256":crate::hash::hex(Sha256::digest(serde_json::to_vec(&before).unwrap())),"savedRequests":usage::summarize(&before.sessions[CHAT].turns).requests,"registryEmpty":true}),
    );
}
