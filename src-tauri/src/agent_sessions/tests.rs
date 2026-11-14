use super::*;
use crate::{desktop::settings::Settings, environment::CheckedProject, platform::brig::Brig};
use serde_json::json;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const SESSION: &str = "1a5bc5b5-af21-432d-bf07-e2218dd9f9bf";
const OTHER_SESSION: &str = "2a5bc5b5-af21-432d-bf07-e2218dd9f9bf";
const HELP: &str = "  --session-id <uuid>  New chat\n  --resume <uuid>  Resume chat\n  --permission-mode <mode>  Mode (choices: \"manual\", \"plan\")\n  --other  Something else\n";
static NEXT: AtomicU64 = AtomicU64::new(1);

#[test]
fn terminal_status_serialization_matches_the_interface_chat_fixture() {
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/terminal-status.json")).unwrap();
    for (key, action, identifier, conversation) in [
        (
            "claude-new-chat",
            Action::NewChat,
            1,
            Some("00000000-0000-4000-8000-000000000001"),
        ),
        ("claude-resume", Action::Resume, 2, Some(SESSION)),
    ] {
        let status = terminal::TerminalStatus {
            identifier,
            project_id: "project-a".into(),
            environment_id: Some(
                serde_json::from_value(json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")).unwrap(),
            ),
            agent_kind: Some(AgentKind::ClaudeCode),
            purpose: action.purpose(),
            running: true,
            exit_code: None,
            conversation_id: conversation.map(String::from),
            busy: false,
        };
        assert_eq!(serde_json::to_value(status).unwrap(), fixtures[key]);
    }
}

struct Fixture {
    root: PathBuf,
    checked: CheckedProject,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "tugboat-claude-sessions-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("project")).unwrap();
        fs::create_dir_all(root.join("home/.claude/projects/-work-p")).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let mut registry = Registry::default();
        let codex = registry.register(&root.join("project"), "codex").unwrap();
        let settings = Settings::default();
        let environment = registry
            .ensure_environment(&codex.id, AgentKind::ClaudeCode, &settings)
            .unwrap();
        let mut project = registry
            .environment(&codex.id, &environment.environment_id)
            .unwrap();
        project.stage = crate::projects::ProjectStage::EnvironmentReady;
        let executable = root.join("brig");
        fs::write(
            &executable,
            r#"#!/bin/sh
printf '%s\n' "$@" >> "$CALLS"
case "$4 $5" in
  "auth status")
    if [ "$AUTH_TERMINAL" = 1 ]; then
      printf '\0337\033[r\0338\033[?25h{\n\033[3G"loggedIn":\033[15Gfalse\n}\n'
      exit 1
    fi
    printf '%s' "$AUTH_STATUS"; exit "$AUTH_EXIT" ;;
  "claude auth") printf '%s' "$AUTH_STATUS"; exit "$AUTH_EXIT" ;;
  "claude --help") printf '%s' "$HELP" ;;
  "auth logout") exit "$LOGOUT_EXIT" ;;
  "--help ") printf '%s' "$HELP" ;;
  *) exit 7 ;;
esac
"#,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let environment = BTreeMap::from([
            ("CALLS".into(), root.join("calls").to_string_lossy().into()),
            ("AUTH_STATUS".into(), "{\"loggedIn\":true}".into()),
            ("AUTH_EXIT".into(), "0".into()),
            ("LOGOUT_EXIT".into(), "0".into()),
            ("HELP".into(), HELP.into()),
        ]);
        let plan =
            json!({"data":{"home":{"host":root.join("home")},"project":{"guest":"/work/p"}}});
        let checked = CheckedProject {
            project,
            settings,
            brig: Brig {
                executable,
                environment,
            },
            paths: crate::platform::brig::PlanPaths::from_plan(&plan).unwrap(),
            plan,
            fingerprint: "fixture-fingerprint".into(),
        };
        Self { root, checked }
    }

    fn transcript(&self) -> PathBuf {
        self.root
            .join(format!("home/.claude/projects/-work-p/{SESSION}.jsonl"))
    }

    fn header(&self) -> serde_json::Value {
        json!({"type":"user","sessionId":SESSION,"cwd":"/work/p","isSidechain":false,"message":{"content":"Test message"}})
    }

    fn write_header(&self, value: &serde_json::Value) {
        fs::write(self.transcript(), format!("{value}\n")).unwrap();
    }

    fn set(&mut self, name: &str, value: &str) {
        self.checked
            .brig
            .environment
            .insert(name.into(), value.into());
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn new_session_ids_are_unique_rfc4122_version_four_ids_and_only_resume_accepts_supplied_ids() {
    let mut generated = HashSet::new();
    for _ in 0..128 {
        let session = Action::NewChat.session(None).unwrap().unwrap();
        let value = session.argument();
        assert_eq!(value.as_bytes()[14], b'4');
        assert!(b"89ab".contains(&value.as_bytes()[19]));
        assert_eq!(
            serde_json::from_value::<SessionId>(json!(&value)).unwrap(),
            session
        );
        assert!(generated.insert(value));
    }
    for action in [Action::Login, Action::Shell, Action::NewChat] {
        assert!(action.session(Some(SESSION)).is_err());
    }
    assert!(Action::Login.session(None).unwrap().is_none());
    assert!(Action::Shell.session(None).unwrap().is_none());
    assert!(Action::Resume.session(None).is_err());
    assert!(Action::Resume.session(Some("--resume")).is_err());
    assert_eq!(
        Action::Resume
            .session(Some(SESSION))
            .unwrap()
            .unwrap()
            .argument(),
        SESSION
    );
    assert_eq!(
        serde_json::from_value::<Action>(json!("new-chat")).unwrap(),
        Action::NewChat
    );
    assert!(matches!(Action::NewChat.codex(), codex::Action::Codex));
    assert!(matches!(Action::Resume.codex(), codex::Action::Resume));
    assert!(matches!(Action::Login.codex(), codex::Action::Login));
    assert!(matches!(Action::Shell.codex(), codex::Action::Shell));
    assert!(serde_json::from_value::<Action>(json!("codex")).is_err());
}

#[test]
fn login_results_are_matched_to_both_environment_and_terminal_attempt() {
    let fixture = Fixture::new();
    let project = &fixture.checked.project;
    let mut codex = project.clone();
    codex.agent_kind = AgentKind::Codex;
    codex.environment_id = EnvironmentId::for_agent(&project.id, AgentKind::Codex);
    let accounts = Accounts::default();
    let status = accounts.begin(project, 10, "first-plan").unwrap();
    assert_eq!(status.state, State::SigningIn);
    assert_eq!(status.terminal_identifier, Some(10));
    assert_eq!(
        accounts
            .pending_fingerprint(project, 10)
            .unwrap()
            .as_deref(),
        Some("first-plan")
    );
    assert!(accounts.pending_fingerprint(project, 11).unwrap().is_none());
    assert!(accounts.pending_fingerprint(&codex, 10).unwrap().is_none());
    assert!(accounts.finish(&codex, 10, Ok(true)).unwrap().is_none());
    assert!(accounts.finish(project, 11, Ok(true)).unwrap().is_none());
    assert!(accounts.begin(project, 11, "second-plan").is_err());
    let pending = accounts
        .publish(project, Status::verified(project, false))
        .unwrap();
    assert_eq!(pending.state, State::SigningIn);
    assert_eq!(
        accounts
            .finish(project, 10, Ok(true))
            .unwrap()
            .unwrap()
            .state,
        State::SignedIn
    );
    assert!(accounts.pending(project).unwrap().is_none());
    accounts.begin(project, 11, "second-plan").unwrap();
    assert!(accounts.finish(project, 10, Ok(false)).unwrap().is_none());
    let failure = accounts
        .finish(project, 11, Err("The environment changed.".into()))
        .unwrap()
        .unwrap();
    assert_eq!(failure.state, State::Unknown);
    assert_eq!(failure.terminal_identifier, Some(11));
    assert_eq!(failure.message, "The environment changed.");
}

#[test]
fn environment_account_locks_and_invalidation_do_not_cross_project_or_environment_boundaries() {
    let fixture = Fixture::new();
    let project = &fixture.checked.project;
    let mut other = project.clone();
    other.agent_kind = AgentKind::Codex;
    other.environment_id = EnvironmentId::for_agent(&project.id, AgentKind::Codex);
    let accounts = Accounts::default();
    let first = accounts.operation(project).unwrap();
    assert!(Arc::ptr_eq(&first, &accounts.operation(project).unwrap()));
    let guard = first.try_lock().unwrap();
    assert!(accounts.operation(project).unwrap().try_lock().is_err());
    assert!(accounts.operation(&other).unwrap().try_lock().is_ok());
    other.id = "f".repeat(32);
    other.environment_id = EnvironmentId::for_agent(&other.id, AgentKind::ClaudeCode);
    other.agent_kind = AgentKind::ClaudeCode;
    assert!(accounts.operation(&other).unwrap().try_lock().is_ok());
    accounts.begin(project, 1, "plan-one").unwrap();
    accounts.begin(&other, 2, "plan-two").unwrap();
    assert_eq!(
        accounts.invalidate(project).unwrap().state,
        State::EnvironmentStopped
    );
    assert!(accounts.pending(project).unwrap().is_none());
    assert!(accounts.finish(project, 1, Ok(true)).unwrap().is_none());
    assert!(accounts.pending(&other).unwrap().is_some());
    drop(guard);
}

#[test]
fn terminal_identity_requires_the_project_environment_and_agent_to_match() {
    let fixture = Fixture::new();
    let project = &fixture.checked.project;
    let mut status = terminal::TerminalStatus {
        identifier: 1,
        project_id: project.id.clone(),
        environment_id: Some(project.environment_id.clone()),
        agent_kind: Some(AgentKind::ClaudeCode),
        purpose: "claude",
        running: true,
        exit_code: None,
        conversation_id: Some(SESSION.into()),
        busy: false,
    };
    assert!(status.belongs_to(project));
    status.environment_id = Some(EnvironmentId::for_agent(&project.id, AgentKind::Codex));
    assert!(!status.belongs_to(project));
    status.environment_id = Some(project.environment_id.clone());
    status.agent_kind = Some(AgentKind::Codex);
    assert!(!status.belongs_to(project));
    status.agent_kind = Some(AgentKind::ClaudeCode);
    status.project_id = "f".repeat(32);
    assert!(!status.belongs_to(project));
    status.project_id = project.id.clone();
    status.environment_id = None;
    status.agent_kind = None;
    assert!(!status.belongs_to(project));
}

#[test]
fn claude_account_and_chat_commands_use_only_the_selected_guest_reference() {
    let mut fixture = Fixture::new();
    let checked = &fixture.checked;
    let session = SessionId::new(SESSION).unwrap();
    assert_eq!(
        claude::arguments(checked, Action::Login, None, None).unwrap(),
        vec![
            "sh",
            &checked.project.reference(),
            "claude",
            "auth",
            "login"
        ]
    );
    assert!(fixture.calls().is_empty());
    assert_eq!(
        claude::arguments(checked, Action::Shell, None, None).unwrap(),
        vec!["sh", &checked.project.reference()]
    );
    for (action, option) in [
        (Action::NewChat, "--session-id"),
        (Action::Resume, "--resume"),
    ] {
        let arguments = claude::arguments(checked, action, Some(&session), Some(true)).unwrap();
        let separator = arguments.iter().position(|value| value == "--").unwrap();
        assert_eq!(
            &arguments[separator - 2..separator],
            &[
                checked.project.reference(),
                checked.project.path.to_string_lossy().into()
            ]
        );
        assert_eq!(
            &arguments[separator + 1..],
            &[option, SESSION, "--permission-mode", "manual"]
        );
    }
    let calls = fixture.calls();
    assert_eq!(
        calls,
        [
            "exec",
            &checked.project.reference(),
            "--",
            "claude",
            "auth",
            "status",
            "--json",
            "exec",
            &checked.project.reference(),
            "--",
            "claude",
            "--help",
            "exec",
            &checked.project.reference(),
            "--",
            "claude",
            "auth",
            "status",
            "--json",
            "exec",
            &checked.project.reference(),
            "--",
            "claude",
            "--help"
        ]
    );
    fixture.set("AUTH_STATUS", "{\"loggedIn\":false}");
    fixture.set("AUTH_EXIT", "1");
    assert!(!claude::logout(&fixture.checked).unwrap());
    assert_eq!(
        &fixture.calls()[calls.len()..],
        &[
            "sh",
            &fixture.checked.project.reference(),
            "claude",
            "auth",
            "logout",
            "exec",
            &fixture.checked.project.reference(),
            "--",
            "claude",
            "auth",
            "status",
            "--json"
        ]
    );
}

#[test]
fn account_status_rejects_malformed_or_contradictory_cli_responses_without_exposing_them() {
    let mut fixture = Fixture::new();
    for (response, exit_code) in [
        ("{\"loggedIn\":false}", "0"),
        ("{\"loggedIn\":true}", "1"),
        ("{\"loggedIn\":true}", "2"),
        ("{\"loggedIn\":\"true\"}", "0"),
        ("{\"loggedIn\":null}", "0"),
        ("{}", "0"),
        ("synthetic-private-detail", "0"),
        ("\u{1b}[3G{\"loggedIn\":false}", "1"),
    ] {
        fixture.set("AUTH_STATUS", response);
        fixture.set("AUTH_EXIT", exit_code);
        let error = claude::signed_in(&fixture.checked).unwrap_err();
        assert!(!error.contains(response));
        assert!(!error.contains("synthetic-private-detail"));
    }
    fixture.set(
        "AUTH_STATUS",
        "{\"loggedIn\":false,\"authMethod\":\"none\"}",
    );
    fixture.set("AUTH_EXIT", "1");
    assert!(!claude::signed_in(&fixture.checked).unwrap());
    fixture.set("LOGOUT_EXIT", "1");
    assert!(claude::logout(&fixture.checked).is_err());
    fixture.set("LOGOUT_EXIT", "0");
    fixture.set("AUTH_STATUS", "{\"loggedIn\":true}");
    fixture.set("AUTH_EXIT", "0");
    assert!(claude::logout(&fixture.checked)
        .unwrap_err()
        .contains("still signed in"));
}

#[test]
fn claude_status_uses_machine_output_without_terminal_cursor_sequences() {
    let mut fixture = Fixture::new();
    fixture.set("AUTH_TERMINAL", "1");
    fixture.set(
        "AUTH_STATUS",
        r#"{
  "loggedIn": false,
  "authMethod": "none",
  "apiProvider": "firstParty",
  "analyticsDisabled": false,
  "projectsDirectory": "/brig/claude/projects",
  "configDirectory": "/brig/claude",
  "loginExpiresAt": null
}
"#,
    );
    fixture.set("AUTH_EXIT", "1");
    assert!(!claude::signed_in(&fixture.checked).unwrap());
    assert_eq!(
        fixture.calls(),
        [
            "exec",
            &fixture.checked.project.reference(),
            "--",
            "claude",
            "auth",
            "status",
            "--json"
        ]
    );
}

#[test]
fn unprepared_offline_unsigned_and_unsupported_guests_do_not_start_claude_chats() {
    let mut fixture = Fixture::new();
    let session = SessionId::new(SESSION).unwrap();
    fixture.checked.project.stage = crate::projects::ProjectStage::Ready;
    assert!(claude::arguments(&fixture.checked, Action::Login, None, None).is_err());
    fixture.checked.project.stage = crate::projects::ProjectStage::EnvironmentReady;
    fixture.checked.settings.network = crate::desktop::settings::Network::Offline;
    for action in [Action::Login, Action::NewChat, Action::Resume] {
        assert!(
            claude::arguments(&fixture.checked, action, Some(&session), None)
                .unwrap_err()
                .contains("offline")
        );
    }
    assert!(claude::arguments(&fixture.checked, Action::Shell, None, None).is_ok());
    assert!(fixture.calls().is_empty());
    fixture.checked.settings.network = crate::desktop::settings::Network::Isolated;
    assert!(claude::arguments(
        &fixture.checked,
        Action::NewChat,
        Some(&session),
        Some(false)
    )
    .unwrap_err()
    .contains("Brig boundary"));
    fixture.set("AUTH_STATUS", "{\"loggedIn\":false}");
    fixture.set("AUTH_EXIT", "1");
    assert!(
        claude::arguments(&fixture.checked, Action::NewChat, Some(&session), None)
            .unwrap_err()
            .contains("Sign in")
    );
    fixture.set("AUTH_STATUS", "{\"loggedIn\":true}");
    fixture.set("AUTH_EXIT", "0");
    for help in [
        "--session-id <uuid> --resume <uuid>",
        "--session-id <uuid> --resume <uuid> --permission-mode <mode> choices: default\n --other manual",
        "--session-id <uuid> --resume <uuid> --permission-mode <mode> choices: manual-beta",
    ] {
        fixture.set("HELP", help);
        assert!(claude::arguments(&fixture.checked, Action::NewChat, Some(&session), None).unwrap_err().contains("required session and manual"));
    }
}

#[test]
fn resume_accepts_a_main_chat_or_subdirectory_and_ignores_an_unfinished_last_line() {
    let fixture = Fixture::new();
    let session = SessionId::new(SESSION).unwrap();
    let mut header = fixture.header();
    header["cwd"] = json!("/work/p/src");
    fs::write(
        fixture.transcript(),
        format!(
            "{}\n{header}\n{{\"type\":",
            json!({"type":"queue-operation","sessionId":SESSION})
        ),
    )
    .unwrap();
    assert!(resume::require(&fixture.checked, &session).is_ok());
}

#[test]
fn resume_rejects_foreign_sessions_subagents_and_invalid_or_missing_ownership() {
    let fixture = Fixture::new();
    let session = SessionId::new(SESSION).unwrap();
    for (field, value) in [
        ("sessionId", json!(OTHER_SESSION)),
        ("sessionId", json!(null)),
        ("isSidechain", json!(true)),
        ("isSidechain", json!(null)),
        ("agentId", json!("worker")),
        ("cwd", json!("/work/project")),
        ("cwd", json!("/work/p/../../other")),
        ("cwd", json!("work/p")),
        ("cwd", json!(null)),
    ] {
        let mut header = fixture.header();
        header[field] = value;
        fixture.write_header(&header);
        assert!(
            resume::require(&fixture.checked, &session).is_err(),
            "{field}"
        );
    }
    for invalid in ["{\n", "{\"type\":\"user\"", "{}\n"] {
        fs::write(fixture.transcript(), invalid).unwrap();
        assert!(resume::require(&fixture.checked, &session).is_err());
    }
    fs::remove_file(fixture.transcript()).unwrap();
    assert!(resume::require(&fixture.checked, &session).is_err());
}

#[test]
fn resume_rejects_duplicate_transcripts_and_does_not_follow_a_transcript_symlink() {
    let fixture = Fixture::new();
    let session = SessionId::new(SESSION).unwrap();
    fixture.write_header(&fixture.header());
    let duplicate = fixture.root.join("home/.claude/projects/duplicate");
    fs::create_dir(&duplicate).unwrap();
    fs::copy(
        fixture.transcript(),
        duplicate.join(format!("{SESSION}.jsonl")),
    )
    .unwrap();
    assert!(resume::require(&fixture.checked, &session)
        .unwrap_err()
        .contains("Multiple"));
    fs::remove_dir_all(duplicate).unwrap();
    let outside = fixture.root.join("outside.jsonl");
    fs::rename(fixture.transcript(), &outside).unwrap();
    symlink(outside, fixture.transcript()).unwrap();
    assert!(resume::require(&fixture.checked, &session)
        .unwrap_err()
        .contains("safely open"));
}

#[test]
fn resume_does_not_follow_a_linked_project_root_or_accept_unbounded_header_data() {
    let fixture = Fixture::new();
    let session = SessionId::new(SESSION).unwrap();
    fixture.write_header(&fixture.header());
    let projects = fixture.root.join("home/.claude/projects");
    let outside = fixture.root.join("outside-projects");
    fs::rename(&projects, &outside).unwrap();
    symlink(&outside, &projects).unwrap();
    assert!(resume::require(&fixture.checked, &session).is_err());
    fs::remove_file(&projects).unwrap();
    fs::rename(outside, &projects).unwrap();
    fs::write(fixture.transcript(), " ".repeat(1_048_578)).unwrap();
    assert!(resume::require(&fixture.checked, &session)
        .unwrap_err()
        .contains("supported size"));
    assert!(!Path::new("/work/project").starts_with("/work/p"));
}
