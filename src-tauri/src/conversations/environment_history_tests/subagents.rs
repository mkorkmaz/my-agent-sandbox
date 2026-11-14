use super::*;

fn family(watch: &Watch) -> PathBuf {
    family_named(watch, "subagents")
}

fn family_named(watch: &Watch, fixture: &str) -> PathBuf {
    let root = claude_file(watch, CHAT);
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../tests/fixtures/claude-code/{fixture}/root.jsonl"
        )),
        &root,
    )
    .unwrap();
    let directory = root.with_extension("").join("subagents");
    for agent in ["alpha", "beta", "nested"] {
        let destination = if agent == "nested" {
            directory.join("workflows/run-1")
        } else {
            directory.clone()
        };
        fs::create_dir_all(&destination).unwrap();
        for extension in ["jsonl", "meta.json"] {
            fs::copy(
                Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
                    "../tests/fixtures/claude-code/{fixture}/agent-{agent}.{extension}"
                )),
                destination.join(format!("agent-{agent}.{extension}")),
            )
            .unwrap();
        }
    }
    directory
}

fn child(agent: &str) -> String {
    claude::subagents::session_id(CHAT, agent)
}
fn row<'a>(dashboard: &'a Dashboard, id: &str) -> &'a Conversation {
    dashboard
        .conversations
        .iter()
        .find(|row| row.id == id)
        .unwrap()
}
fn details(watch: &Watch, id: &str) -> ConversationUsageDetails {
    usage_details::from_ledger(&watch.selected_ledger().unwrap(), id).unwrap()
}

#[test]
fn priced_claude_family_keeps_one_dated_cost_per_message_across_replay_and_source_loss() {
    let fixture = Fixture::new("priced-claude-family");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    let directory = family_named(&watch, "priced-subagents");
    let first = synchronize(&watch).unwrap().0;
    assert_eq!(first.usage.requests, 4);
    assert_eq!(first.usage.total, 100_976);
    assert_eq!(first.usage.unpriced_requests, 0);
    assert!((first.usage.cost_usd - 0.0599405375).abs() < 1e-12);
    let root = details(&watch, CHAT);
    assert!((root.own_usage.cost_usd - 0.001397).abs() < 1e-12);
    assert!((root.descendants_usage.cost_usd - 0.0585435375).abs() < 1e-12);
    assert_eq!(root.combined_usage, first.usage);
    assert_eq!(details(&watch, &child("alpha")).combined_usage.requests, 2);
    for request in root
        .agents
        .iter()
        .flat_map(|agent| &agent.turns)
        .flat_map(|turn| &turn.requests)
    {
        let rate = request.rates.as_ref().unwrap();
        assert_eq!(rate.date, usage::anthropic::PRICING_DATE);
        assert_eq!(rate.source, usage::anthropic::PRICING_SOURCE);
        assert!(rate.anthropic.as_ref().unwrap().assumptions.is_empty());
    }
    let copy = response(&directory.join("agent-alpha.jsonl"));
    append(&claude_file(&watch, CHAT), &copy);
    assert_eq!(synchronize(&watch).unwrap().0.usage, first.usage);
    let before = fs::read(&watch.ledger).unwrap();
    assert!(!synchronize(&watch).unwrap().1);
    assert_eq!(fs::read(&watch.ledger).unwrap(), before);
    if let Ok(directory) = std::env::var("TUGBOAT_CLAUDE_USAGE_EXPORT") {
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            Path::new(&directory).join("priced-family-dashboard.json"),
            serde_json::to_vec_pretty(&first).unwrap(),
        )
        .unwrap();
        fs::write(
            Path::new(&directory).join("priced-family-details.json"),
            serde_json::to_vec_pretty(&root).unwrap(),
        )
        .unwrap();
    }
    fs::remove_dir_all(&watch.home).unwrap();
    let saved = environment_history::saved(watch.target.as_ref(), &watch.ledger).unwrap();
    assert_eq!(saved.usage, first.usage);
    assert_eq!(details(&watch, CHAT).combined_usage, first.usage);
}
fn append(path: &Path, value: &Value) {
    writeln!(
        OpenOptions::new().append(true).open(path).unwrap(),
        "{value}"
    )
    .unwrap();
}
fn response(path: &Path) -> Value {
    serde_json::from_str(
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .find(|line| line.contains("\"type\":\"assistant\""))
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn measured_claude_family_reuses_the_graph_details_and_root_state_contract() {
    let fixture = Fixture::new("claude-family");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    family(&watch);
    let dashboard = synchronize(&watch).unwrap().0;
    assert_eq!(dashboard.usage.total, 1350);
    assert_eq!(dashboard.usage.requests, 4);
    assert_eq!(dashboard.usage.input, 1250);
    assert_eq!(dashboard.usage.cached, 200);
    assert_eq!(dashboard.usage.cache_write, 50);
    assert_eq!(row(&dashboard, CHAT).usage.total, 135);
    assert_eq!(row(&dashboard, CHAT).family_usage.total, 1350);
    assert_eq!(row(&dashboard, CHAT).subagent_count, 3);
    assert_eq!(row(&dashboard, CHAT).relation_status, graph::Status::Root);
    assert_eq!(row(&dashboard, &child("alpha")).family_usage.total, 675);
    assert_eq!(
        row(&dashboard, &child("nested"))
            .lineage
            .parent_session_id
            .as_deref(),
        Some(child("alpha").as_str())
    );
    for agent in ["alpha", "beta", "nested"] {
        let agent = row(&dashboard, &child(agent));
        assert_eq!(agent.relation_status, graph::Status::Resolved);
        assert_eq!(agent.root_session_id.as_deref(), Some(CHAT));
        assert!(resume_dashboard(&watch, &agent.id)
            .err()
            .unwrap()
            .contains("root chat"));
        assert!(
            change_state(&watch, &agent.id, SessionState::Archived, |_| false)
                .unwrap_err()
                .contains("root chat")
        );
    }
    let root_details = details(&watch, CHAT);
    assert_eq!(root_details.own_usage.total, 135);
    assert_eq!(root_details.descendants_usage.total, 1215);
    assert_eq!(root_details.combined_usage, dashboard.usage);
    assert_eq!(root_details.agents.len(), 4);
    assert_eq!(root_details.user_prompt_count, None);
    assert!(root_details.prompts.is_empty());
    assert_eq!(root_details.unassigned.request_refs.len(), 4);
    let subtree = details(&watch, &child("alpha"));
    assert_eq!(subtree.agents.len(), 2);
    assert_eq!(subtree.own_usage.total, 270);
    assert_eq!(subtree.descendants_usage.total, 405);
    assert_eq!(subtree.combined_usage.total, 675);
    if let Ok(directory) = std::env::var("TUGBOAT_CLAUDE_USAGE_EXPORT") {
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            Path::new(&directory).join("subagent-dashboard.json"),
            serde_json::to_vec_pretty(&dashboard).unwrap(),
        )
        .unwrap();
        fs::write(
            Path::new(&directory).join("subagent-details.json"),
            serde_json::to_vec_pretty(&root_details).unwrap(),
        )
        .unwrap();
    }
    change_state(&watch, CHAT, SessionState::Archived, |_| false).unwrap();
    assert_eq!(
        row(&synchronize(&watch).unwrap().0, CHAT).state,
        SessionState::Archived
    );
    assert_eq!(
        watch.selected_ledger().unwrap().sessions[&child("alpha")].state,
        SessionState::Active
    );
}

#[test]
fn duplicate_copied_responses_and_tool_summaries_do_not_multiply_family_usage() {
    let fixture = Fixture::new("claude-family-dedup");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    let directory = family(&watch);
    let mut copied = response(&directory.join("agent-alpha.jsonl"));
    copied["isSidechain"] = json!(false);
    copied.as_object_mut().unwrap().remove("agentId");
    append(&claude_file(&watch, CHAT), &copied);
    let first = synchronize(&watch).unwrap().0;
    assert_eq!(first.usage.total, 1350);
    assert_eq!(row(&first, CHAT).usage.total, 135);
    for _ in 0..3 {
        let (replayed, changed) = synchronize(&watch).unwrap();
        assert!(!changed);
        assert_eq!(replayed.usage, first.usage);
        assert_eq!(details(&watch, CHAT).combined_usage, first.usage);
    }
    fs::remove_file(directory.join("agent-alpha.jsonl")).unwrap();
    let missing = synchronize(&watch).unwrap().0;
    assert_eq!(missing.usage, first.usage);
    assert_eq!(row(&missing, CHAT).family_usage, first.usage);
    assert!(!row(&missing, &child("alpha")).available);
    let saved = environment_history::saved(watch.target.as_ref(), &watch.ledger).unwrap();
    assert_eq!(saved.usage, first.usage);
    assert_eq!(details(&watch, CHAT).agents.len(), 4);
}

#[test]
fn child_only_growth_changes_its_revision_and_the_root_detail_subtotal() {
    let fixture = Fixture::new("claude-family-growth");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    let directory = family(&watch);
    let first = synchronize(&watch).unwrap().0;
    let path = directory.join("agent-alpha.jsonl");
    let mut next = response(&path);
    next["message"]["id"] = json!("msg_family_alpha_next");
    next["message"]["content"] = json!([{ "type":"text", "text":"Another synthetic reply." }]);
    next["timestamp"] = json!("2026-11-13T00:01:00Z");
    append(&path, &next);
    let grown = synchronize(&watch).unwrap().0;
    assert_eq!(grown.usage.total, 1620);
    assert_eq!(row(&grown, CHAT).family_usage.total, 1620);
    assert_ne!(
        row(&grown, &child("alpha")).usage_revision,
        row(&first, &child("alpha")).usage_revision
    );
    assert_eq!(details(&watch, CHAT).descendants_usage.total, 1485);
    assert_eq!(details(&watch, CHAT).agent_turn_count, 5);
}

#[test]
fn standalone_roots_keep_their_own_rows_and_do_not_adopt_another_roots_agent_ids() {
    let fixture = Fixture::new("claude-family-standalone");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    family(&watch);
    let standalone = "bbbbbbbb-0000-4000-8000-000000000001";
    let text = include_str!("../../../../tests/fixtures/claude-code/subagents/root.jsonl")
        .replace(CHAT, standalone)
        .replace("msg_family_root", "msg_standalone_root");
    fs::write(claude_file(&watch, standalone), text).unwrap();
    let dashboard = synchronize(&watch).unwrap().0;
    assert_eq!(
        dashboard
            .conversations
            .iter()
            .filter(|session| session.lineage.kind == lineage::Kind::Primary)
            .count(),
        2
    );
    assert_eq!(row(&dashboard, standalone).subagent_count, 0);
    assert_eq!(row(&dashboard, standalone).family_usage.total, 135);
    assert_eq!(row(&dashboard, CHAT).subagent_count, 3);
    assert_eq!(row(&dashboard, CHAT).family_usage.total, 1350);
    assert_eq!(dashboard.usage.total, 1485);
}

#[test]
fn three_or_more_ancestor_nested_and_sibling_copies_have_a_stable_canonical_owner() {
    for order in [
        ["root", "alpha", "beta"],
        ["beta", "root", "alpha"],
        ["alpha", "beta", "root"],
    ] {
        let fixture = Fixture::new(&format!("claude-family-many-copies-{}", order[0]));
        let watch = fixture.watch(AgentKind::ClaudeCode);
        let directory = family(&watch);
        let original = response(&directory.join("workflows/run-1/agent-nested.jsonl"));
        for owner in order {
            let mut copied = original.clone();
            copied["isSidechain"] = json!(owner != "root");
            if owner == "root" {
                copied.as_object_mut().unwrap().remove("agentId");
            }
            if owner != "root" {
                copied["agentId"] = json!(owner);
            }
            let path = if owner == "root" {
                claude_file(&watch, CHAT)
            } else {
                directory.join(format!("agent-{owner}.jsonl"))
            };
            append(&path, &copied);
        }
        let first = synchronize(&watch).unwrap().0;
        assert_eq!(first.usage.total, 1350);
        assert_eq!(first.usage.requests, 4);
        assert_eq!(row(&first, &child("nested")).usage.total, 405);
        assert_eq!(row(&first, &child("alpha")).usage.total, 270);
        assert_eq!(row(&first, &child("beta")).usage.total, 540);
        assert!(row(&first, &child("nested"))
            .warning
            .as_ref()
            .unwrap()
            .contains("counted once"));
        let replayed = synchronize(&watch).unwrap();
        assert!(!replayed.1);
        assert_eq!(replayed.0.usage, first.usage);
    }
}

#[test]
fn later_child_ownership_keeps_the_first_saved_measurement_when_copies_conflict() {
    let fixture = Fixture::new("claude-family-first-measurement");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    let directory = family(&watch);
    let path = directory.join("agent-alpha.jsonl");
    let original = fs::read_to_string(&path)
        .unwrap()
        .replace("claude-fixture", "claude-opus-5-5");
    fs::write(&path, &original).unwrap();
    let mut copied = response(&path);
    copied["isSidechain"] = json!(false);
    copied.as_object_mut().unwrap().remove("agentId");
    append(&claude_file(&watch, CHAT), &copied);
    fs::remove_file(&path).unwrap();
    synchronize(&watch).unwrap();
    let mut ledger = watch.selected_ledger().unwrap();
    let request = &mut ledger
        .sessions
        .get_mut(CHAT)
        .unwrap()
        .turns
        .get_mut("msg_family_alpha")
        .unwrap()
        .requests[0];
    // Synthetic dated Anthropic tariff proves ownership recovery does not reprice history.
    let rate = request.rates.as_mut().unwrap();
    rate.date = "2026-10-01".into();
    rate.input *= 0.5;
    rate.cached *= 0.5;
    rate.output *= 0.5;
    rate.anthropic.as_mut().unwrap().write_5m *= 0.5;
    rate.anthropic.as_mut().unwrap().write_1h *= 0.5;
    request.cost_usd = Some(0.000629);
    request.unpriced_reason = None;
    let saved_rate = serde_json::to_value(&request.rates).unwrap();
    write_ledger(&watch.ledger, &mut ledger).unwrap();
    let first = synchronize(&watch).unwrap().0;
    assert_eq!(first.usage.total, 1350);
    fs::write(&path, original).unwrap();
    fs::write(
        &path,
        fs::read_to_string(&path)
            .unwrap()
            .replace("\"input_tokens\":200", "\"input_tokens\":999"),
    )
    .unwrap();
    let recovered = synchronize(&watch).unwrap().0;
    assert_eq!(recovered.usage, first.usage);
    assert_eq!(row(&recovered, &child("alpha")).usage.total, 270);
    assert_eq!(row(&recovered, CHAT).usage.total, 135);
    assert_eq!(row(&recovered, CHAT).family_usage.total, 1350);
    let recovered_ledger = watch.selected_ledger().unwrap();
    let request = &recovered_ledger.sessions[&child("alpha")].turns["msg_family_alpha"].requests[0];
    assert_eq!(serde_json::to_value(&request.rates).unwrap(), saved_rate);
    assert_eq!(request.cost_usd, Some(0.000629));
    assert!(row(&recovered, &child("alpha"))
        .warning
        .as_ref()
        .unwrap()
        .contains("first saved measurement"));
    let replayed = synchronize(&watch).unwrap().0;
    assert_eq!(replayed.usage, first.usage);
    assert_eq!(details(&watch, CHAT).combined_usage, first.usage);
}

#[test]
fn child_transcript_failures_retain_saved_counts_and_known_family_links() {
    let fixture = Fixture::new("claude-family-failures");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    let directory = family(&watch);
    let path = directory.join("agent-alpha.jsonl");
    let original = fs::read(&path).unwrap();
    let first = synchronize(&watch).unwrap().0;
    change_state(&watch, CHAT, SessionState::Deleted, |_| false).unwrap();
    for malformed in [
        b"not json\n".to_vec(),
        vec![b' '; 1_048_577],
        b"{}\n".repeat(131_073),
    ] {
        fs::write(&path, malformed).unwrap();
        let current = synchronize(&watch).unwrap().0;
        assert_eq!(current.usage, first.usage);
        assert_eq!(row(&current, CHAT).family_usage, first.usage);
        assert_eq!(row(&current, CHAT).state, SessionState::Deleted);
        assert!(!row(&current, &child("alpha")).available);
        assert_eq!(current.coverage, usage::Coverage::Partial);
    }
    let file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    file.set_len(64 * 1024 * 1024 + 1).unwrap();
    assert_eq!(synchronize(&watch).unwrap().0.usage, first.usage);
    fs::write(&path, &original).unwrap();
    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"type\":\"assistant\"")
        .unwrap();
    let incomplete = synchronize(&watch).unwrap().0;
    assert_eq!(incomplete.usage, first.usage);
    assert!(row(&incomplete, &child("alpha"))
        .warning
        .as_ref()
        .unwrap()
        .contains("incomplete"));
    fs::write(&path, &original).unwrap();
    fs::remove_file(directory.join("agent-alpha.meta.json")).unwrap();
    let absent_metadata = synchronize(&watch).unwrap().0;
    assert_eq!(row(&absent_metadata, CHAT).subagent_count, 3);
    assert_eq!(absent_metadata.usage, first.usage);
    fs::write(
        claude_file(&watch, CHAT),
        include_str!("../../../../tests/fixtures/claude-code/subagents/root.jsonl")
            .lines()
            .next()
            .unwrap()
            .to_owned()
            + "\n",
    )
    .unwrap();
    let rotated = synchronize(&watch).unwrap().0;
    assert_eq!(rotated.usage, first.usage);
    assert_eq!(row(&rotated, CHAT).family_usage, first.usage);
}

#[test]
fn unverified_orphan_cycle_and_conflicting_parents_remain_measured_recovery_rows() {
    for case in [
        "absent-meta",
        "bad-meta",
        "oversized-meta",
        "orphan",
        "cycle",
        "wrong-parent",
        "wrong-tool",
    ] {
        let fixture = Fixture::new(&format!("claude-family-{case}"));
        let watch = fixture.watch(AgentKind::ClaudeCode);
        let directory = family(&watch);
        let metadata = directory.join("agent-alpha.meta.json");
        match case {
            "absent-meta" => fs::remove_file(metadata).unwrap(),
            "bad-meta" => fs::write(metadata, "[]").unwrap(),
            "oversized-meta" => fs::write(metadata, vec![b' '; 65_537]).unwrap(),
            "orphan" => fs::remove_file(claude_file(&watch, CHAT)).unwrap(),
            "wrong-parent" => fs::write(
                metadata,
                r#"{"parentAgentId":"beta","toolUseId":"toolu_alpha"}"#,
            )
            .unwrap(),
            "wrong-tool" => fs::write(metadata, r#"{"toolUseId":"toolu_missing"}"#).unwrap(),
            "cycle" => {
                let path = directory.join("workflows/run-1/agent-nested.jsonl");
                let text = fs::read_to_string(&path).unwrap();
                let mut documents: Vec<Value> = text
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
                documents[1]["message"]["content"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"type":"tool_use","name":"Agent","id":"toolu_cycle_alpha"}));
                fs::write(
                    path,
                    documents
                        .iter()
                        .map(|value| format!("{value}\n"))
                        .collect::<String>(),
                )
                .unwrap();
                fs::write(
                    metadata,
                    r#"{"parentAgentId":"nested","toolUseId":"toolu_cycle_alpha"}"#,
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        let dashboard = synchronize(&watch).unwrap().0;
        assert_eq!(
            dashboard.usage.total,
            if case == "orphan" { 1215 } else { 1350 },
            "{case}"
        );
        let alpha = row(&dashboard, &child("alpha"));
        assert_ne!(alpha.relation_status, graph::Status::Resolved, "{case}");
        assert!(
            details(&watch, &alpha.id)
                .warnings
                .iter()
                .any(|warning| warning.contains("relationship")
                    || warning.contains("Parent session")),
            "{case}"
        );
        assert_eq!(alpha.usage.total, 270);
        if case == "cycle" {
            assert_eq!(alpha.relation_status, graph::Status::Invalid);
        }
        if case == "orphan" {
            assert_eq!(alpha.relation_status, graph::Status::Orphan);
        }
    }
}

#[test]
fn equal_root_agent_and_message_ids_keep_separate_environments_and_guest_variants() {
    let fixture = Fixture::new("claude-family-isolation");
    let first = fixture.watch(AgentKind::ClaudeCode);
    family(&first);
    synchronize(&first).unwrap();
    change_state(&first, CHAT, SessionState::Archived, |_| false).unwrap();
    let second = Watch {
        home: fixture.0.join("second-claude"),
        ..first.clone()
    };
    let directory = family(&second);
    let path = directory.join("agent-alpha.jsonl");
    fs::write(
        &path,
        fs::read_to_string(&path)
            .unwrap()
            .replace("\"output_tokens\":20", "\"output_tokens\":21"),
    )
    .unwrap();
    let second_result = synchronize(&second).unwrap().0;
    assert_eq!(details(&second, CHAT).combined_usage.total, 1351);
    assert_eq!(row(&second_result, CHAT).state, SessionState::Active);
    assert_eq!(row(&second_result, CHAT).subagent_count, 3);
    assert_eq!(second_result.usage.total, 2701);
    let first_result = synchronize(&first).unwrap().0;
    assert_eq!(details(&first, CHAT).combined_usage.total, 1350);
    assert_eq!(row(&first_result, CHAT).state, SessionState::Archived);
    let third = Watch {
        home: fixture.0.join("other-environment"),
        ledger: fixture.0.join("other-environment-ledger.json"),
        ..first.clone()
    };
    family(&third);
    let third_result = synchronize(&third).unwrap().0;
    assert_eq!(third_result.usage.total, 1350);
    assert_eq!(row(&third_result, CHAT).state, SessionState::Active);
    assert_ne!(
        details(&first, CHAT).agents[1].turns[0].requests[0]
            .source
            .as_ref()
            .unwrap()
            .guest_namespace,
        details(&third, CHAT).agents[1].turns[0].requests[0]
            .source
            .as_ref()
            .unwrap()
            .guest_namespace
    );
}

#[test]
fn foreign_roots_exclude_their_children_and_stay_excluded_after_the_root_disappears() {
    let fixture = Fixture::new("claude-family-project-scope");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    family(&watch);
    assert_eq!(synchronize(&watch).unwrap().0.usage.total, 1350);
    let path = claude_file(&watch, CHAT);
    let original = fs::read_to_string(&path).unwrap();
    fs::write(&path, original.replace("/work/p", "/work/foreign")).unwrap();
    let excluded = synchronize(&watch).unwrap().0;
    assert_eq!(excluded.usage.total, 0);
    assert!(excluded.conversations.is_empty());
    fs::remove_file(&path).unwrap();
    let still_excluded = synchronize(&watch).unwrap().0;
    assert_eq!(still_excluded.usage.total, 0);
    assert!(still_excluded.conversations.is_empty());
    fs::write(&path, original).unwrap();
    assert_eq!(synchronize(&watch).unwrap().0.usage.total, 1350);
    let other_project = Watch {
        project: "/work/foreign".into(),
        ledger: fixture.0.join("foreign-project-ledger.json"),
        ..watch.clone()
    };
    assert!(synchronize(&other_project)
        .unwrap()
        .0
        .conversations
        .is_empty());
}

#[test]
fn duplicate_workflow_agent_paths_and_conflicting_sidecar_parent_changes_retain_known_usage() {
    let fixture = Fixture::new("claude-family-duplicate-paths");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    let directory = family(&watch);
    let first = synchronize(&watch).unwrap().0;
    let duplicate = directory.join("workflows/run-2");
    fs::create_dir_all(&duplicate).unwrap();
    fs::copy(
        directory.join("agent-alpha.jsonl"),
        duplicate.join("agent-alpha.jsonl"),
    )
    .unwrap();
    fs::copy(
        directory.join("agent-alpha.meta.json"),
        duplicate.join("agent-alpha.meta.json"),
    )
    .unwrap();
    let conflict = synchronize(&watch).unwrap().0;
    assert_eq!(conflict.usage, first.usage);
    assert!(!row(&conflict, &child("alpha")).available);
    assert_eq!(
        row(&conflict, &child("alpha")).relation_status,
        graph::Status::Invalid
    );
    fs::remove_dir_all(duplicate).unwrap();
    synchronize(&watch).unwrap();
    let beta = directory.join("agent-beta.jsonl");
    let mut documents: Vec<Value> = fs::read_to_string(&beta)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    documents[1]["message"]["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"tool_use","name":"Agent","id":"toolu_changed_parent"}));
    fs::write(
        beta,
        documents
            .iter()
            .map(|value| format!("{value}\n"))
            .collect::<String>(),
    )
    .unwrap();
    fs::write(
        directory.join("agent-alpha.meta.json"),
        r#"{"parentAgentId":"beta","toolUseId":"toolu_changed_parent"}"#,
    )
    .unwrap();
    let changed_parent = synchronize(&watch).unwrap().0;
    assert_eq!(changed_parent.usage, first.usage);
    assert_eq!(
        row(&changed_parent, &child("alpha")).relation_status,
        graph::Status::Invalid
    );
    assert_eq!(
        watch.selected_ledger().unwrap().sessions[&child("alpha")]
            .lineage
            .parent_session_id
            .as_deref(),
        Some(CHAT)
    );
}

#[test]
fn child_identity_sidecar_identity_links_special_files_and_depth_limits_are_checked() {
    for case in [
        "agent-id",
        "session-id",
        "sidecar-id",
        "sidecar-session",
        "cwd",
        "symlink-file",
        "symlink-meta",
        "symlink-directory",
        "fifo",
        "deep-directory",
    ] {
        let fixture = Fixture::new(&format!("claude-family-source-{case}"));
        let watch = fixture.watch(AgentKind::ClaudeCode);
        let directory = family(&watch);
        let path = directory.join("agent-alpha.jsonl");
        let first = synchronize(&watch).unwrap().0;
        let outside = fixture.0.join("outside-source");
        fs::write(&outside, "not a transcript\n").unwrap();
        match case {
            "agent-id" => fs::write(
                &path,
                fs::read_to_string(&path)
                    .unwrap()
                    .replace("\"agentId\":\"alpha\"", "\"agentId\":\"other\""),
            )
            .unwrap(),
            "session-id" => fs::write(
                &path,
                fs::read_to_string(&path)
                    .unwrap()
                    .replace(CHAT, "bbbbbbbb-0000-4000-8000-000000000001"),
            )
            .unwrap(),
            "sidecar-id" => fs::write(
                directory.join("agent-alpha.meta.json"),
                r#"{"agentId":"other","toolUseId":"toolu_alpha"}"#,
            )
            .unwrap(),
            "sidecar-session" => fs::write(
                directory.join("agent-alpha.meta.json"),
                r#"{"sessionId":"bbbbbbbb-0000-4000-8000-000000000001","toolUseId":"toolu_alpha"}"#,
            )
            .unwrap(),
            "cwd" => {
                let mut copied = response(&path);
                copied["cwd"] = json!("/work/foreign");
                append(&path, &copied);
            }
            "symlink-file" => {
                fs::remove_file(&path).unwrap();
                symlink(&outside, &path).unwrap();
            }
            "symlink-meta" => {
                let meta = directory.join("agent-alpha.meta.json");
                fs::remove_file(&meta).unwrap();
                symlink(&outside, meta).unwrap();
            }
            "symlink-directory" => {
                fs::remove_dir_all(directory.join("workflows")).unwrap();
                symlink(&fixture.0, directory.join("workflows")).unwrap();
            }
            "fifo" => {
                fs::remove_file(&path).unwrap();
                let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            }
            "deep-directory" => {
                let deep = directory.join("d/d/d/d/d/d/d/d/d");
                fs::create_dir_all(&deep).unwrap();
                fs::rename(&path, deep.join("agent-alpha.jsonl")).unwrap();
            }
            _ => unreachable!(),
        }
        let current = synchronize(&watch).unwrap().0;
        assert_eq!(current.usage, first.usage, "{case}");
        assert_eq!(current.coverage, usage::Coverage::Partial);
        if matches!(case, "sidecar-id" | "sidecar-session" | "symlink-meta") {
            assert_eq!(
                row(&current, &child("alpha")).relation_status,
                graph::Status::Invalid,
                "{case}"
            );
        }
        if matches!(
            case,
            "agent-id" | "session-id" | "cwd" | "symlink-file" | "fifo" | "deep-directory"
        ) {
            assert!(!row(&current, &child("alpha")).available, "{case}");
        }
    }
}
