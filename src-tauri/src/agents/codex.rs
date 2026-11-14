use super::{AgentCommand, GuestCommand};

#[derive(Clone, Copy)]
pub enum Boundary {
    Brig,
    Workspace,
}

pub fn command(command: AgentCommand<'_>, boundary: Boundary) -> Result<GuestCommand, String> {
    let arguments = match command {
        AgentCommand::Version => vec!["--version".into()],
        AgentCommand::Login => account_arguments("--device-auth"),
        AgentCommand::LoginStatus => account_arguments("status"),
        AgentCommand::Logout => vec![
            "-c".into(),
            "cli_auth_credentials_store=\"file\"".into(),
            "logout".into(),
        ],
        AgentCommand::NewChat(None) => chat_arguments(Vec::new(), boundary),
        AgentCommand::NewChat(Some(_)) => {
            return Err("Codex assigns its own new session ID.".into())
        }
        AgentCommand::Resume(session) => {
            chat_arguments(vec!["resume".into(), session.argument()], boundary)
        }
    };
    Ok(GuestCommand::new("codex", arguments))
}

fn account_arguments(action: &str) -> Vec<String> {
    vec![
        "-c".into(),
        "cli_auth_credentials_store=\"file\"".into(),
        "login".into(),
        action.into(),
    ]
}

fn chat_arguments(session: Vec<String>, boundary: Boundary) -> Vec<String> {
    // A shared background daemon can retain credentials after its terminal exits.
    let mut arguments = vec!["--no-daemon".into()];
    arguments.extend(session);
    arguments.extend([
        "-c".into(),
        "cli_auth_credentials_store=\"file\"".into(),
        "-c".into(),
        "tui.terminal_title=[\"session-id\",\"status\"]".into(),
        "--sandbox".into(),
        match boundary {
            Boundary::Brig => "danger-full-access".into(),
            Boundary::Workspace => "workspace-write".into(),
        },
        "--ask-for-approval".into(),
        "never".into(),
    ]);
    if matches!(boundary, Boundary::Workspace) {
        arguments.extend([
            "-c".into(),
            "sandbox_workspace_write.network_access=false".into(),
        ]);
    }
    arguments
}
