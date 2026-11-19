use super::*;
use serde_json::json;

const SESSION: &str = "1a5bc5b5-af21-432d-bf07-e2218dd9f9bf";

#[test]
fn agent_kinds_have_stable_wire_names() {
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
fn codex_account_status_logout_and_version_commands_keep_the_file_store() {
    let cases = [
        (
            AgentCommand::Version,
            vec![
                "sh",
                "custom@a",
                crate::environment::shared_runtime::program(AgentKind::Codex),
                "--version",
            ],
        ),
        (
            AgentCommand::LoginStatus,
            vec![
                "sh",
                "custom@a",
                crate::environment::shared_runtime::program(AgentKind::Codex),
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
                crate::environment::shared_runtime::program(AgentKind::Codex),
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
        assert_eq!(
            &command[..3],
            &[
                "sh",
                "tugboat-claude@a",
                crate::environment::shared_runtime::program(AgentKind::ClaudeCode)
            ]
        );
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
