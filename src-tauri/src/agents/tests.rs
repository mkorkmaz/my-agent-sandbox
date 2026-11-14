use super::*;
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const SESSION: &str = "1a5bc5b5-af21-432d-bf07-e2218dd9f9bf";

#[test]
fn agent_kinds_and_supported_operations_have_stable_wire_names() {
    assert_eq!(serde_json::to_value(AgentKind::Codex).unwrap(), "codex");
    assert_eq!(
        serde_json::to_value(AgentKind::ClaudeCode).unwrap(),
        "claude-code"
    );
    assert_eq!(
        serde_json::from_str::<AgentKind>("\"claude-code\"").unwrap(),
        AgentKind::ClaudeCode
    );
    for unsupported in ["ClaudeCode", "openai", "anthropic", "shell", ""] {
        assert!(serde_json::from_value::<AgentKind>(json!(unsupported)).is_err());
    }
    let definitions = serde_json::to_value(agent_definitions()).unwrap();
    assert_eq!(
        definitions,
        json!([
            {"kind":"codex","operations":["login","login-status","logout","new-chat","resume"]},
            {"kind":"claude-code","operations":["login","login-status","logout","new-chat","resume"]}
        ])
    );
}

#[test]
fn custom_profile_names_work_but_program_kind_and_identity_must_match() {
    for (kind, binary, name) in [
        (AgentKind::Codex, "codex", "team-codex"),
        (AgentKind::ClaudeCode, "claude", "tugboat-claude"),
    ] {
        let profile = json!({"name":name,"kind":"agent","binary":binary});
        assert!(kind.validate_profile(name, &profile).is_ok());
        for (field, value) in [
            ("name", json!("another-profile")),
            ("kind", json!("service")),
            ("binary", json!("sh")),
            ("binary", json!(null)),
            ("name", json!(42)),
        ] {
            let mut invalid = profile.clone();
            invalid[field] = value;
            assert!(kind.validate_profile(name, &invalid).is_err(), "{field}");
        }
        assert!(kind.validate_profile(name, &json!({})).is_err());
    }
}

#[test]
fn literal_claude_credentials_are_rejected_without_exposing_the_value() {
    for name in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "GH_TOKEN",
        "GITHUB_TOKEN",
    ] {
        let mut profile = json!({"name":"custom-claude","kind":"agent","binary":"claude","env":[{"name":name,"value":"synthetic-private-value"}]});
        let error = AgentKind::ClaudeCode
            .validate_profile("custom-claude", &profile)
            .unwrap_err();
        assert!(error.contains("literal host credential"));
        assert!(!error.contains("synthetic-private-value"));
        profile["deny"] = json!([name]);
        assert!(AgentKind::ClaudeCode
            .validate_profile("custom-claude", &profile)
            .is_ok());
        profile["deny"] = json!([]);
        profile["env"][0]["value"] = "".into();
        assert!(AgentKind::ClaudeCode
            .validate_profile("custom-claude", &profile)
            .is_ok());
    }
}

#[test]
fn operation_identifiers_accept_only_their_required_session_parameters() {
    let session = SessionId::new(SESSION).unwrap();
    for operation in [
        AgentOperation::Login,
        AgentOperation::LoginStatus,
        AgentOperation::Logout,
    ] {
        assert!(operation.command(None).is_ok());
        assert!(operation.command(Some(&session)).is_err());
    }
    assert!(AgentOperation::Resume.command(None).is_err());
    assert!(AgentOperation::Resume.command(Some(&session)).is_ok());
    assert!(AgentOperation::NewChat.command(None).is_ok());
    assert!(AgentOperation::NewChat.command(Some(&session)).is_ok());
    assert!(serde_json::from_value::<AgentOperation>(json!("run-shell")).is_err());
}

#[test]
fn session_ids_reject_flags_paths_wrong_types_and_invalid_uuid_shapes() {
    let session = SessionId::new(SESSION).unwrap();
    assert_eq!(serde_json::to_value(&session).unwrap(), SESSION);
    assert_eq!(
        serde_json::from_value::<SessionId>(json!(SESSION)).unwrap(),
        session
    );
    for invalid in [
        "",
        "../history",
        "--resume",
        "1a5bc5b5-af21-432d-bf07-e2218dd9f9bf; touch /tmp/probe",
        "1a5bc5b5-af21-432d-bf07-e2218dd9f9bg",
        "1a5bc5b5af21432dbf07e2218dd9f9bf",
    ] {
        assert!(SessionId::new(invalid).is_err());
        assert!(serde_json::from_value::<SessionId>(json!(invalid)).is_err());
    }
    for invalid in [json!(null), json!(true), json!(12), json!({"id":SESSION})] {
        assert!(serde_json::from_value::<SessionId>(invalid).is_err());
    }
}

#[test]
fn codex_account_and_version_commands_keep_the_file_store_and_device_login() {
    let cases = [
        (
            AgentCommand::Version,
            vec!["sh", "custom@a", "codex", "--version"],
        ),
        (
            AgentCommand::Login,
            vec![
                "sh",
                "custom@a",
                "codex",
                "-c",
                "cli_auth_credentials_store=\"file\"",
                "login",
                "--device-auth",
            ],
        ),
        (
            AgentCommand::LoginStatus,
            vec![
                "sh",
                "custom@a",
                "codex",
                "-c",
                "cli_auth_credentials_store=\"file\"",
                "login",
                "status",
            ],
        ),
        (
            AgentCommand::Logout,
            vec![
                "sh",
                "custom@a",
                "codex",
                "-c",
                "cli_auth_credentials_store=\"file\"",
                "logout",
            ],
        ),
    ];
    for (request, expected) in cases {
        assert_eq!(
            AgentKind::Codex
                .command(request)
                .unwrap()
                .in_guest("custom@a".into()),
            expected
        );
    }
}

#[test]
fn codex_new_and_resumed_chats_preserve_current_arguments_in_both_boundary_modes() {
    let session = SessionId::new(SESSION).unwrap();
    for (boundary, sandbox) in [
        (codex::Boundary::Brig, "danger-full-access"),
        (codex::Boundary::Workspace, "workspace-write"),
    ] {
        let new = codex::command(AgentCommand::NewChat(None), boundary)
            .unwrap()
            .in_profile();
        let resumed = codex::command(AgentCommand::Resume(&session), boundary)
            .unwrap()
            .in_profile();
        assert_eq!(new[0], "--no-daemon");
        assert_eq!(&resumed[..3], &["--no-daemon", "resume", SESSION]);
        assert_eq!(&new[1..], &resumed[3..]);
        assert_eq!(
            &new[1..9],
            &[
                "-c",
                "cli_auth_credentials_store=\"file\"",
                "-c",
                "tui.terminal_title=[\"session-id\",\"status\"]",
                "--sandbox",
                sandbox,
                "--ask-for-approval",
                "never"
            ]
        );
        let expected_tail: &[&str] = match boundary {
            codex::Boundary::Brig => &[],
            codex::Boundary::Workspace => &["-c", "sandbox_workspace_write.network_access=false"],
        };
        assert_eq!(&new[9..], expected_tail);
    }
    assert!(AgentKind::Codex
        .command(AgentCommand::NewChat(Some(&session)))
        .is_err());
}

#[test]
fn claude_commands_match_the_live_cli_contract_and_require_a_new_session_id() {
    let session = SessionId::new(SESSION).unwrap();
    for (request, expected) in [
        (AgentCommand::Version, vec!["--version"]),
        (AgentCommand::Login, vec!["auth", "login"]),
        (AgentCommand::LoginStatus, vec!["auth", "status", "--json"]),
        (AgentCommand::Logout, vec!["auth", "logout"]),
        (
            AgentCommand::NewChat(Some(&session)),
            vec!["--session-id", SESSION, "--permission-mode", "manual"],
        ),
        (
            AgentCommand::Resume(&session),
            vec!["--resume", SESSION, "--permission-mode", "manual"],
        ),
    ] {
        let command = AgentKind::ClaudeCode
            .command(request)
            .unwrap()
            .in_guest("tugboat-claude@a".into());
        assert_eq!(&command[..3], &["sh", "tugboat-claude@a", "claude"]);
        assert_eq!(&command[3..], expected);
        assert!(!command
            .iter()
            .any(|argument| argument.contains("bypass") || argument == "--fork-session"));
    }
    assert!(AgentKind::ClaudeCode
        .command(AgentCommand::NewChat(None))
        .is_err());
}

#[test]
fn version_checks_accept_captured_program_responses_and_reject_wrong_programs() {
    assert!(AgentKind::Codex
        .validate_version("codex-cli 0.167.0\n")
        .is_ok());
    assert!(AgentKind::Codex
        .validate_version("codex-cli 0.167.0-alpha.1")
        .is_ok());
    assert!(AgentKind::ClaudeCode
        .validate_version("2.1.318 (Claude Code)\n")
        .is_ok());
    for invalid in [
        "",
        "not a version",
        "codex-cli",
        "codex-cli 1.2",
        "codex-cli 1.2.3\nother output",
        "2.1 (Claude Code)",
        "2.1.318 (Another CLI)",
    ] {
        assert!(AgentKind::Codex.validate_version(invalid).is_err());
        assert!(AgentKind::ClaudeCode.validate_version(invalid).is_err());
    }
    assert!(AgentKind::Codex
        .validate_version("2.1.318 (Claude Code)")
        .is_err());
    assert!(AgentKind::ClaudeCode
        .validate_version("codex-cli 0.167.0")
        .is_err());
}

struct StubBrig {
    directory: PathBuf,
    brig: crate::platform::brig::Brig,
}

impl StubBrig {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("agent-contract-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let executable = directory.join("brig");
        fs::write(
            &executable,
            r#"#!/bin/sh
case "$1" in
agent) cat "$0.profile" ;;
sh) cat "$0.version"; exit "$(cat "$0.exit")" ;;
*) exit 2 ;;
esac
"#,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            directory,
            brig: crate::platform::brig::Brig {
                executable,
                environment: crate::platform::command::environment(),
            },
        }
    }
}

impl Drop for StubBrig {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.directory).unwrap();
    }
}

#[test]
fn brig_boundary_checks_the_profile_and_the_actual_guest_version_response() {
    let fixture = StubBrig::new();
    fs::write(
        fixture.directory.join("brig.profile"),
        r#"{"name":"custom","kind":"agent","binary":"claude"}"#,
    )
    .unwrap();
    assert!(fixture
        .brig
        .validate_agent_profile("custom", AgentKind::ClaudeCode)
        .is_ok());
    assert!(fixture
        .brig
        .validate_agent_profile("custom", AgentKind::Codex)
        .is_err());
    fs::write(fixture.directory.join("brig.profile"), "invalid JSON").unwrap();
    assert!(fixture
        .brig
        .validate_agent_profile("custom", AgentKind::ClaudeCode)
        .is_err());
    fs::write(
        fixture.directory.join("brig.version"),
        "2.1.318 (Claude Code)\n",
    )
    .unwrap();
    fs::write(fixture.directory.join("brig.exit"), "0").unwrap();
    assert!(fixture
        .brig
        .verify_agent_version(AgentKind::ClaudeCode, "custom@a".into())
        .is_ok());
    assert!(fixture
        .brig
        .verify_agent_version(AgentKind::Codex, "custom@a".into())
        .is_err());
    fs::write(fixture.directory.join("brig.exit"), "1").unwrap();
    assert!(fixture
        .brig
        .verify_agent_version(AgentKind::ClaudeCode, "custom@a".into())
        .is_err());
}
