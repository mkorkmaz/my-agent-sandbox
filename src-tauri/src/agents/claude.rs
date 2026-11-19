use super::{AgentCommand, GuestCommand};
pub fn command(command: AgentCommand<'_>) -> Result<GuestCommand, String> {
    let arguments = match command {
        AgentCommand::Version => vec!["--version".into()],
        AgentCommand::LoginStatus => vec!["auth".into(), "status".into(), "--json".into()],
        #[cfg(test)]
        AgentCommand::Logout => vec!["auth".into(), "logout".into()],
        AgentCommand::NewChat(Some(session)) => vec![
            "--session-id".into(),
            session.argument(),
            "--permission-mode".into(),
            "manual".into(),
        ],
        AgentCommand::NewChat(None) => {
            return Err("Choose a UUID for the new Claude Code session.".into())
        }
        AgentCommand::Resume(session) => vec![
            "--resume".into(),
            session.argument(),
            "--permission-mode".into(),
            "manual".into(),
        ],
    };
    Ok(GuestCommand::new(
        crate::environment::shared_runtime::program(super::AgentKind::ClaudeCode),
        arguments,
    ))
}
