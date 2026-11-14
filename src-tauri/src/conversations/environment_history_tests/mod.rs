use super::*;
use serde_json::json;
use std::io::Write;
use std::os::unix::fs::symlink;

mod sqlite_usage_tests;

const CHAT: &str = "1a5bc5b5-af21-432d-bf07-e2218dd9f9bf";
const PROJECT: &str = "11111111111111111111111111111111";

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("tugboat-env-history-{name}-{}", std::process::id()));
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn watch(&self, kind: AgentKind) -> Watch {
        let name = kind.binary();
        Watch {
            target: Some(environment_history::Target {
                project_id: PROJECT.into(),
                environment_id: projects::EnvironmentId::for_agent(PROJECT, kind),
                agent_kind: kind,
            }),
            records_directory: None,
            store: None,
            home: self.0.join(name),
            project: "/work/p".into(),
            ledger: self.0.join(format!("{name}-ledger.json")),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn claude_file(watch: &Watch, id: &str) -> PathBuf {
    let directory = watch.home.join(".claude/projects/-work-p");
    fs::create_dir_all(&directory).unwrap();
    directory.join(format!("{id}.jsonl"))
}
fn claude_message(id: &str, cwd: &str, sidechain: bool, text: &str, timestamp: &str) -> Value {
    json!({"type":"user","sessionId":id,"cwd":cwd,"isSidechain":sidechain,
        "version":"2.1.318","timestamp":timestamp,"message":{"content":text}})
}
fn write_claude(watch: &Watch) -> PathBuf {
    let file = claude_file(watch, CHAT);
    fs::write(
        &file,
        include_str!("../../../../tests/fixtures/claude-code/v2.1.318/main.jsonl"),
    )
    .unwrap();
    file
}
fn write_codex(watch: &Watch) -> PathBuf {
    let directory = watch.home.join(".codex/sessions");
    fs::create_dir_all(&directory).unwrap();
    let file = directory.join("rollout-fixture.jsonl");
    let documents = [
        json!({"type":"session_meta","payload":{"id":CHAT,"cwd":"/work/p","source":"cli","cli_version":"0.167.0","model_provider":"openai"}}),
        json!({"type":"turn_context","payload":{"turn_id":"aaaaaaaa-0000-4000-8000-000000000001","model":"gpt-6.1-sol","root_turn_id":"aaaaaaaa-0000-4000-8000-000000000001"}}),
        json!({"type":"token_usage_record","timestamp":"2026-11-12T00:00:00Z","payload":{"thread_id":CHAT,"turn_id":"aaaaaaaa-0000-4000-8000-000000000001","root_turn_id":"aaaaaaaa-0000-4000-8000-000000000001","response_id":"response-1","usage":{"input_tokens":100,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":10,"total_tokens":110},"thread_token_usage":{"input_tokens":100,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":10,"total_tokens":110}}}),
    ];
    fs::write(
        &file,
        documents
            .iter()
            .map(|document| format!("{document}\n"))
            .collect::<String>(),
    )
    .unwrap();
    file
}

#[test]
fn two_agents_with_the_same_uuid_keep_independent_archive_and_usage() {
    let fixture = Fixture::new("same-uuid");
    let codex = fixture.watch(AgentKind::Codex);
    let claude = fixture.watch(AgentKind::ClaudeCode);
    write_codex(&codex);
    write_claude(&claude);
    let codex_dashboard = synchronize(&codex).unwrap().0;
    let claude_dashboard = synchronize(&claude).unwrap().0;
    assert_eq!(
        codex_dashboard.conversations[0].id,
        claude_dashboard.conversations[0].id
    );
    assert_ne!(
        codex_dashboard.conversations[0].environment_id,
        claude_dashboard.conversations[0].environment_id
    );
    assert!(codex_dashboard.usage.requests > 0);
    assert_eq!(claude_dashboard.usage.requests, 5);
    assert!(claude_dashboard.usage_available);
    let mut expected_usage = codex_dashboard.usage.clone();
    expected_usage.add(&claude_dashboard.usage);
    let merged =
        environment_history::combine([codex_dashboard.clone(), claude_dashboard].into_iter());
    assert_eq!(merged.conversations.len(), 2);
    assert_eq!(merged.usage, expected_usage);
    assert_eq!(merged.coverage, usage::Coverage::Partial);
    assert!(merged.usage_available);
    change_state(&claude, CHAT, SessionState::Archived, |_| false).unwrap();
    assert_eq!(
        synchronize(&claude).unwrap().0.conversations[0].state,
        SessionState::Archived
    );
    assert_eq!(
        synchronize(&codex).unwrap().0.conversations[0].state,
        SessionState::Active
    );
    change_state(&claude, CHAT, SessionState::Deleted, |_| false).unwrap();
    change_state(&claude, CHAT, SessionState::Active, |_| false).unwrap();
    assert_eq!(
        synchronize(&claude).unwrap().0.conversations[0].state,
        SessionState::Active
    );
}

#[test]
fn real_claude_sample_measures_each_message_once_with_additive_cache_input() {
    let fixture = Fixture::new("sample");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    write_claude(&watch);
    let scan = claude::history::scan(&watch.home, &watch.project).unwrap();
    assert_eq!(scan.sessions.len(), 1);
    let row = &scan.sessions[0];
    assert!(row.title.starts_with("Acceptance test. Remember the code"));
    assert_eq!(row.cwd, Path::new("/work/p"));
    assert_eq!(row.lineage.kind, lineage::Kind::Primary);
    assert_eq!(row.lineage.source_version.as_deref(), Some("2.1.318"));
    assert!(row.updated_at >= 1_794_511_738_332);
    assert_eq!(row.usage.requests, 5);
    assert_eq!(row.usage.input, 82_539);
    assert_eq!(row.usage.cached, 76_065);
    assert_eq!(row.usage.cache_write, 6_458);
    assert_eq!(
        row.usage.input - row.usage.cached - row.usage.cache_write,
        16
    );
    assert_eq!(row.usage.output, 409);
    assert_eq!(row.usage.total, 82_948);
    assert_eq!(row.usage.unpriced_requests, 0);
    assert!((row.usage.cost_usd - 0.055747).abs() < 1e-12);
    assert_eq!(row.usage.unmeasured_turns, 0);
    assert!(row.usage_available);
    assert!(scan.warnings.is_empty());
    let first = synchronize(&watch).unwrap().0;
    assert!(first.usage_available);
    assert_eq!(first.coverage, usage::Coverage::Partial);
    let details = usage_details::from_ledger(&load_ledger(&watch.ledger).unwrap(), CHAT).unwrap();
    assert_eq!(details.combined_usage, row.usage);
    assert_eq!(details.user_prompt_count, None);
    assert_eq!(details.agent_turn_count, 5);
    assert!(details.prompts.is_empty());
    assert_eq!(details.unassigned.request_refs.len(), 5);
    assert!(details
        .warnings
        .iter()
        .any(|warning| warning.contains("discovered subagent messages")));
    for request in details
        .agents
        .iter()
        .flat_map(|agent| &agent.turns)
        .flat_map(|turn| &turn.requests)
    {
        assert_eq!(request.source.as_ref().unwrap().model_provider, "anthropic");
        assert!(request.rates.is_some() && request.cost_usd.is_some());
        assert!(request
            .rates
            .as_ref()
            .unwrap()
            .anthropic
            .as_ref()
            .unwrap()
            .assumptions
            .iter()
            .any(|assumption| assumption.starts_with("5-minute")));
        assert!(request.tokens.write_12h.is_none());
    }
    for _ in 0..3 {
        let (repeated, changed) = synchronize(&watch).unwrap();
        assert!(!changed);
        assert_eq!(repeated.usage, first.usage);
        assert_eq!(
            repeated.conversations[0].usage_revision,
            first.conversations[0].usage_revision
        );
    }
    let saved = environment_history::saved(watch.target.as_ref(), &watch.ledger).unwrap();
    assert!(saved.usage_available && saved.conversations[0].usage_available);
    assert_eq!(saved.usage, first.usage);
    if let Ok(directory) = std::env::var("TUGBOAT_CLAUDE_USAGE_EXPORT") {
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            Path::new(&directory).join("genuine-main-dashboard.json"),
            serde_json::to_vec_pretty(&first).unwrap(),
        )
        .unwrap();
        fs::write(
            Path::new(&directory).join("genuine-main-details.json"),
            serde_json::to_vec_pretty(&details).unwrap(),
        )
        .unwrap();
    }
    println!(
        "CLAUDE_FIXTURE_USAGE={}",
        json!({"summary": first.usage, "dashboard": first, "details": details})
    );
}

mod subagents;

mod costs;

#[test]
fn claude_waits_for_newline_and_accepts_project_subdirectories() {
    let fixture = Fixture::new("partial");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    let path = claude_file(&watch, CHAT);
    let first = claude_message(
        CHAT,
        "/work/p/sub",
        false,
        "First title",
        "2026-11-12T00:00:00Z",
    );
    let next = claude_message(
        CHAT,
        "/work/p/sub",
        false,
        "Second prompt",
        "2026-11-12T01:00:00Z",
    );
    fs::write(&path, format!("{first}\n{next}")).unwrap();
    let before = synchronize(&watch).unwrap().0;
    assert_eq!(before.conversations[0].title, "First title");
    let mut file = OpenOptions::new().append(true).open(path).unwrap();
    writeln!(file).unwrap();
    let after = synchronize(&watch).unwrap().0;
    assert_eq!(
        after.conversations[0].updated_at - before.conversations[0].updated_at,
        3_600_000
    );
}

#[test]
fn claude_skips_subagents_foreign_paths_and_wrong_session_ids() {
    let fixture = Fixture::new("ownership");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    let path = claude_file(&watch, CHAT);
    for (cwd, sidechain) in [
        ("/work/p", true),
        ("/work/p-other", false),
        ("/work/p/../other", false),
    ] {
        let document = claude_message(CHAT, cwd, sidechain, "Excluded", "2026-11-12T00:00:00Z");
        fs::write(&path, format!("{document}\n")).unwrap();
        assert!(claude::history::scan(&watch.home, &watch.project)
            .unwrap()
            .sessions
            .is_empty());
    }
    let wrong = claude_message(
        "aaaaaaaa-0000-4000-8000-000000000001",
        "/work/p",
        false,
        "Wrong ID",
        "2026-11-12T00:00:00Z",
    );
    fs::write(&path, format!("{wrong}\n")).unwrap();
    let scan = claude::history::scan(&watch.home, &watch.project).unwrap();
    assert!(scan.sessions.is_empty());
    assert_eq!(scan.problems.len(), 1);
    fs::remove_file(path).unwrap();
    let directory = watch.home.join(".claude/projects/-work-p/subagents");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join(format!("{CHAT}.jsonl")),
        format!("{wrong}\n"),
    )
    .unwrap();
    assert!(claude::history::scan(&watch.home, &watch.project)
        .unwrap()
        .sessions
        .is_empty());
}

#[test]
fn claude_rejects_directory_and_transcript_links_without_reading_the_target() {
    let fixture = Fixture::new("links");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    let path = claude_file(&watch, CHAT);
    let outside = fixture.0.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.jsonl"), "sensitive fixture content").unwrap();
    symlink(outside.join("secret.jsonl"), &path).unwrap();
    let scan = claude::history::scan(&watch.home, &watch.project).unwrap();
    assert!(scan.sessions.is_empty());
    assert_eq!(scan.problems.len(), 1);
    assert!(!scan
        .problems
        .values()
        .any(|message| message.contains("sensitive")));
    fs::remove_dir_all(watch.home.join(".claude/projects")).unwrap();
    symlink(&outside, watch.home.join(".claude/projects")).unwrap();
    assert!(claude::history::scan(&watch.home, &watch.project)
        .err()
        .unwrap()
        .contains("Symbolic links"));
}

#[test]
fn one_failed_environment_keeps_the_other_current_result_and_last_saved_rows() {
    let fixture = Fixture::new("failed-env");
    let codex = fixture.watch(AgentKind::Codex);
    let claude = fixture.watch(AgentKind::ClaudeCode);
    write_codex(&codex);
    write_claude(&claude);
    let history = History::default();
    let codex_key = codex.target.as_ref().unwrap().key();
    let claude_key = claude.target.as_ref().unwrap().key();
    history.refresh(&codex_key, &codex).unwrap();
    history.refresh(&claude_key, &claude).unwrap();
    let before = DashboardUpdate::new(history.merged(PROJECT).unwrap());
    fs::remove_dir_all(claude.home.join(".claude")).unwrap();
    symlink(&codex.home, claude.home.join(".claude")).unwrap();
    let mut current = synchronize(&codex).unwrap().0;
    current.conversations[0].title = "Fresh Codex result".into();
    history.record_result(&codex_key, current.clone()).unwrap();
    for _ in 0..3 {
        history.refresh(&claude_key, &claude).unwrap();
    }
    let after = DashboardUpdate::new(history.merged(PROJECT).unwrap());
    assert!(after.sequence > before.sequence);
    assert_eq!(after.dashboard.conversations.len(), 2);
    assert!(after
        .dashboard
        .conversations
        .iter()
        .any(|row| row.title == "Fresh Codex result"));
    let saved = after
        .dashboard
        .conversations
        .iter()
        .find(|row| row.agent_kind == AgentKind::ClaudeCode)
        .unwrap();
    assert!(!saved.available);
    assert_eq!(after.dashboard.usage, before.dashboard.usage);
    assert_eq!(
        history.latest.lock().unwrap()[&claude_key].warnings.len(),
        1
    );
}

#[test]
fn removing_one_environment_flushes_it_and_keeps_other_watchers() {
    let fixture = Fixture::new("remove-one");
    let codex = fixture.watch(AgentKind::Codex);
    let claude = fixture.watch(AgentKind::ClaudeCode);
    write_codex(&codex);
    write_claude(&claude);
    let history = History::default();
    for watch in [codex.clone(), claude.clone()] {
        let key = watch.target.as_ref().unwrap().key();
        history
            .watches
            .lock()
            .unwrap()
            .insert(key.clone(), watch.clone());
        history.refresh(&key, &watch).unwrap();
    }
    history
        .remove(
            PROJECT,
            Some(&claude.target.as_ref().unwrap().environment_id),
            true,
        )
        .unwrap();
    assert_eq!(history.watches.lock().unwrap().len(), 1);
    assert_eq!(history.merged(PROJECT).unwrap().conversations.len(), 1);
    assert!(claude.ledger.is_file());
    history.flush().unwrap();
    assert!(codex.ledger.is_file());
    history.unobserve(PROJECT).unwrap();
    assert!(history.watches.lock().unwrap().is_empty());
}

#[test]
fn missing_claude_home_keeps_measured_saved_rows_and_both_agents_usage() {
    let fixture = Fixture::new("missing-home");
    let codex = fixture.watch(AgentKind::Codex);
    let claude = fixture.watch(AgentKind::ClaudeCode);
    write_codex(&codex);
    write_claude(&claude);
    let codex_before = synchronize(&codex).unwrap().0;
    let claude_before = synchronize(&claude).unwrap().0;
    fs::remove_dir_all(&claude.home).unwrap();
    let claude_missing = synchronize(&claude).unwrap().0;
    assert_eq!(claude_missing.conversations.len(), 1);
    assert!(!claude_missing.conversations[0].available);
    assert!(claude_missing.conversations[0].usage_available);
    assert_eq!(claude_missing.usage, claude_before.usage);
    assert_eq!(
        claude_missing.conversations[0].coverage,
        usage::Coverage::Partial
    );
    let combined =
        environment_history::combine([synchronize(&codex).unwrap().0, claude_missing].into_iter());
    assert_eq!(combined.conversations.len(), 2);
    let mut expected_usage = codex_before.usage;
    expected_usage.add(&claude_before.usage);
    assert_eq!(combined.usage, expected_usage);
    assert!(combined.usage_available);
    assert_eq!(combined.coverage, usage::Coverage::Partial);
}

fn claude_response(message: &str, usage: Value, complete: bool) -> Value {
    json!({"type":"assistant","sessionId":CHAT,"cwd":"/work/p","isSidechain":false,
        "uuid":"aaaaaaaa-0000-4000-8000-000000000001","version":"2.1.318",
        "timestamp":"2026-11-12T01:00:00Z","message":{"id":message,"model":"claude-fixture",
        "stop_reason":if complete { Some("end_turn") } else { None },"usage":usage}})
}

fn claude_counts(output: u64) -> Value {
    json!({"input_tokens":2,"cache_read_input_tokens":10,"cache_creation_input_tokens":1,"output_tokens":output})
}

#[test]
fn claude_streams_replays_and_incomplete_duplicates_keep_one_saved_request() {
    let fixture = Fixture::new("stream-replay");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    let path = claude_file(&watch, CHAT);
    let mut initial = claude_response("msg_one", claude_counts(1), false);
    initial["message"].as_object_mut().unwrap().remove("model");
    fs::write(&path, format!("{initial}\n")).unwrap();
    let first = synchronize(&watch).unwrap().0;
    assert_eq!(first.usage.total, 14);
    let completed = claude_response("msg_one", claude_counts(15), true);
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    write!(file, "{completed}").unwrap();
    assert_eq!(synchronize(&watch).unwrap().0.usage, first.usage);
    writeln!(file).unwrap();
    let measured = synchronize(&watch).unwrap().0;
    assert_eq!(measured.usage.requests, 1);
    assert_eq!(measured.usage.input, 13);
    assert_eq!(measured.usage.output, 15);
    assert_eq!(measured.usage.completed_turns, 1);
    assert_ne!(
        measured.conversations[0].usage_revision,
        first.conversations[0].usage_revision
    );
    let mut missing = completed.clone();
    missing["message"].as_object_mut().unwrap().remove("usage");
    let mut invalid = completed.clone();
    invalid["message"]["usage"]["output_tokens"] = json!(-1);
    writeln!(file, "{initial}\n{completed}\n{missing}\n{invalid}").unwrap();
    let repeated = synchronize(&watch).unwrap().0;
    assert_eq!(repeated.usage, measured.usage);
    assert_eq!(repeated.usage.unmeasured_turns, 0);
    let stored = load_ledger(&watch.ledger).unwrap();
    assert_eq!(stored.sessions[CHAT].turns.len(), 1);
    let request = &stored.sessions[CHAT].turns["msg_one"].requests[0];
    assert_eq!(request.model.as_deref(), Some("claude-fixture"));
    assert!(!request.partial);
    fs::write(&path, format!("{initial}\n")).unwrap();
    assert_eq!(synchronize(&watch).unwrap().0.usage, measured.usage);
    assert_eq!(
        environment_history::saved(watch.target.as_ref(), &watch.ledger)
            .unwrap()
            .usage,
        measured.usage
    );
    let mut conflict = completed;
    conflict["message"]["model"] = json!("claude-other-model");
    conflict["message"]["usage"]["output_tokens"] = json!(30);
    fs::write(&path, format!("{conflict}\n")).unwrap();
    let conflicted = synchronize(&watch).unwrap().0;
    assert_eq!(conflicted.usage, measured.usage);
    assert!(conflicted.conversations[0]
        .warning
        .as_ref()
        .unwrap()
        .contains("Conflicting"));
}

#[test]
fn claude_unmeasured_response_recovers_and_valid_boundaries_remain_measured() {
    let fixture = Fixture::new("usage-recovery");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    let path = claude_file(&watch, CHAT);
    let missing = claude_response("msg_recovered", json!({"input_tokens":0}), false);
    fs::write(&path, format!("{missing}\n")).unwrap();
    let unknown = synchronize(&watch).unwrap().0;
    assert_eq!(unknown.usage.requests, 0);
    assert_eq!(unknown.usage.unmeasured_turns, 1);
    let zero = claude_response(
        "msg_recovered",
        json!({"input_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":0}),
        true,
    );
    fs::write(&path, format!("{missing}\n{zero}\n")).unwrap();
    let measured = synchronize(&watch).unwrap().0;
    assert_eq!(measured.usage.requests, 1);
    assert_eq!(measured.usage.unmeasured_turns, 0);
    assert_eq!(measured.usage.total, 0);
    let boundary = claude_response(
        "msg_boundary",
        json!({"input_tokens":0,"cache_read_input_tokens":999_999_999_100_u64,"cache_creation_input_tokens":900,"output_tokens":1_000_000_000_000_u64,"cache_creation":{"ephemeral_1h_input_tokens":900}}),
        true,
    );
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(file, "{boundary}").unwrap();
    let full = synchronize(&watch).unwrap().0;
    assert_eq!(full.usage.requests, 2);
    assert_eq!(full.usage.input, 1_000_000_000_000);
    assert_eq!(full.usage.output, 1_000_000_000_000);
    assert_eq!(full.usage.total, 2_000_000_000_000);
    let ledger = load_ledger(&watch.ledger).unwrap();
    assert!(ledger.sessions[CHAT].turns["msg_boundary"].requests[0]
        .tokens
        .write_12h
        .is_none());
}

#[test]
fn claude_missing_malformed_and_oversized_counts_remain_unknown() {
    let fixture = Fixture::new("invalid-counts");
    for (index, field) in [
        "input_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
        "output_tokens",
    ]
    .into_iter()
    .enumerate()
    {
        for (case, invalid) in [
            Value::Null,
            json!(-1),
            json!("1"),
            json!(1.5),
            json!(1_000_000_000_001_u64),
            json!(u64::MAX),
        ]
        .into_iter()
        .enumerate()
        {
            let mut watch = fixture.watch(AgentKind::ClaudeCode);
            watch.home = fixture.0.join(format!("guest-{index}-{case}"));
            watch.ledger = fixture.0.join(format!("ledger-{index}-{case}.json"));
            let mut counts = claude_counts(2);
            counts[field] = invalid;
            let response = claude_response("msg_unknown", counts, true);
            fs::write(claude_file(&watch, CHAT), format!("{response}\n")).unwrap();
            let dashboard = synchronize(&watch).unwrap().0;
            assert_eq!(dashboard.usage.requests, 0, "{field} case {case}");
            assert_eq!(dashboard.usage.unmeasured_turns, 1);
            assert_eq!(dashboard.coverage, usage::Coverage::Unknown);
            assert!(dashboard.conversations[0].usage_available);
        }
    }
    let watch = fixture.watch(AgentKind::ClaudeCode);
    let path = claude_file(&watch, CHAT);
    let mut counts = claude_counts(0);
    counts["input_tokens"] = json!(1_000_000_000_000_u64);
    let response = claude_response("msg_excess_total", counts, true);
    fs::write(&path, format!("{response}\n")).unwrap();
    assert_eq!(synchronize(&watch).unwrap().0.usage.requests, 0);
    let zero = claude_response(
        "msg_zero",
        json!({"input_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":0}),
        true,
    );
    fs::write(&path, format!("{zero}\n")).unwrap();
    let dashboard = synchronize(&watch).unwrap().0;
    assert_eq!(dashboard.usage.requests, 1);
    assert_eq!(dashboard.usage.total, 0);
    assert_eq!(dashboard.usage.unmeasured_turns, 1); // The earlier unknown response remains unknown.
}

#[test]
fn claude_known_subtotals_and_saved_states_survive_transcript_failure() {
    let fixture = Fixture::new("saved-partial");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    let path = write_claude(&watch);
    let known = synchronize(&watch).unwrap().0;
    change_state(&watch, CHAT, SessionState::Deleted, |_| false).unwrap();
    let unknown = claude_response(
        "msg_missing_cache",
        json!({"input_tokens":4,"output_tokens":10}),
        true,
    );
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(file, "{unknown}").unwrap();
    let partial = synchronize(&watch).unwrap().0;
    assert_eq!(partial.usage.total, known.usage.total);
    assert_eq!(partial.usage.unmeasured_turns, 1);
    assert_eq!(partial.conversations[0].state, SessionState::Deleted);
    writeln!(file, "invalid complete record").unwrap();
    let failed = synchronize(&watch).unwrap().0;
    assert_eq!(failed.usage, partial.usage);
    assert!(!failed.conversations[0].available);
    assert!(failed.conversations[0].usage_available);
    fs::remove_file(&path).unwrap();
    let missing = synchronize(&watch).unwrap().0;
    assert_eq!(missing.usage, partial.usage);
    let saved = environment_history::saved(watch.target.as_ref(), &watch.ledger).unwrap();
    assert_eq!(saved.usage, partial.usage);
    assert_eq!(saved.conversations[0].state, SessionState::Deleted);
    let details = usage_details::from_ledger(&load_ledger(&watch.ledger).unwrap(), CHAT).unwrap();
    assert_eq!(details.combined_usage, partial.usage);
    assert!(details
        .warnings
        .iter()
        .any(|warning| warning.contains("saved usage is retained")));
}

#[test]
fn claude_metadata_only_legacy_ledgers_are_unknown_and_keep_immutable_backup() {
    let fixture = Fixture::new("legacy-backup");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    let mut ledger = Ledger::default();
    ledger.schema_version = 1;
    ledger.sessions.insert(
        CHAT.into(),
        SessionRecord {
            title: "Saved Claude chat".into(),
            state: SessionState::Archived,
            lineage: lineage::Lineage {
                kind: lineage::Kind::Primary,
                ..Default::default()
            },
            ..Default::default()
        },
    );
    let original = serde_json::to_vec(&ledger).unwrap();
    fs::write(&watch.ledger, &original).unwrap();
    let offline = environment_history::saved(watch.target.as_ref(), &watch.ledger).unwrap();
    assert_eq!(offline.usage.unmeasured_turns, 1);
    assert_eq!(offline.usage.requests, 0);
    assert!(offline.usage_available && offline.conversations[0].usage_available);
    write_claude(&watch);
    let measured = synchronize(&watch).unwrap().0;
    assert_eq!(measured.usage.requests, 5);
    assert_eq!(measured.usage.unmeasured_turns, 0);
    assert_eq!(measured.conversations[0].state, SessionState::Archived);
    assert_eq!(
        fs::read(storage::v1_backup_path(&watch.ledger)).unwrap(),
        original
    );
}

#[test]
fn claude_guest_variants_keep_saved_states_usage_and_selected_detail_kind() {
    let fixture = Fixture::new("guest-variants");
    let first = fixture.watch(AgentKind::ClaudeCode);
    write_claude(&first);
    let first_usage = synchronize(&first).unwrap().0.usage;
    change_state(&first, CHAT, SessionState::Archived, |_| false).unwrap();
    let second = Watch {
        home: fixture.0.join("second-claude-guest"),
        ..first.clone()
    };
    let response = claude_response("msg_other_guest", claude_counts(2), true);
    fs::write(claude_file(&second, CHAT), format!("{response}\n")).unwrap();
    synchronize(&second).unwrap();
    let mut ledger = load_ledger(&first.ledger).unwrap();
    assert!(ledger.variants.contains_key(CHAT));
    ledger.select_guest(&first.namespace());
    let first_record = &ledger.sessions[CHAT];
    assert_eq!(first_record.state, SessionState::Archived);
    let first_key = first_record.turns.values().next().unwrap().requests[0]
        .source
        .as_ref()
        .unwrap()
        .physical_key();
    assert_eq!(usage::summarize(&first_record.turns), first_usage);
    ledger.select_guest(&second.namespace());
    let second_key = ledger.sessions[CHAT]
        .turns
        .values()
        .next()
        .unwrap()
        .requests[0]
        .source
        .as_ref()
        .unwrap()
        .physical_key();
    assert_ne!(first_key, second_key);
    let details = usage_details::from_ledger(&ledger, CHAT).unwrap();
    assert_eq!(details.combined_usage.total, 15);
    assert_eq!(details.agents[0].agent_kind, Some(AgentKind::ClaudeCode));
    assert_eq!(details.user_prompt_count, None);
    ledger
        .variants
        .get_mut(CHAT)
        .unwrap()
        .get_mut(&first.namespace())
        .unwrap()
        .agent_kind = None;
    ledger.select_guest(&first.namespace());
    for record in ledger.sessions.values_mut() {
        claude::usage::prepare(record);
    }
    let selected = usage_details::from_ledger(&ledger, CHAT).unwrap();
    assert_eq!(selected.combined_usage, first_usage);
    assert_eq!(selected.agents[0].agent_kind, Some(AgentKind::ClaudeCode));
    assert_eq!(selected.user_prompt_count, None);
}

#[test]
fn claude_duplicate_sources_and_transcript_bounds_preserve_known_usage() {
    let fixture = Fixture::new("duplicates-bounds");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    let path = write_claude(&watch);
    let known = synchronize(&watch).unwrap().0.usage;
    let duplicate = watch.home.join(".claude/projects/-work-p-other");
    fs::create_dir_all(&duplicate).unwrap();
    fs::copy(&path, duplicate.join(format!("{CHAT}.jsonl"))).unwrap();
    let dashboard = synchronize(&watch).unwrap().0;
    assert_eq!(dashboard.usage, known);
    assert!(!dashboard.conversations[0].available);
    assert!(dashboard
        .warnings
        .iter()
        .any(|warning| warning.contains("same session ID")));
    fs::remove_dir_all(duplicate).unwrap();
    fs::write(&path, vec![b' '; 1_048_577]).unwrap();
    assert_eq!(synchronize(&watch).unwrap().0.usage, known);
    let file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    file.set_len(64 * 1024 * 1024 + 1).unwrap();
    assert_eq!(synchronize(&watch).unwrap().0.usage, known);
}
