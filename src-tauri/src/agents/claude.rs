use super::{AgentCommand, GuestCommand};
use serde_json::Value;

pub fn validate_profile(profile: &Value) -> Result<(), String> {
    // Brig v0.5.2 omits literal environment bindings from plan.credentials.
    let credential = profile["env"].as_array().is_some_and(|bindings| {
        bindings.iter().any(|binding| {
            let name = binding["name"].as_str().unwrap_or_default();
            matches!(
                name,
                "ANTHROPIC_API_KEY"
                    | "ANTHROPIC_AUTH_TOKEN"
                    | "CLAUDE_CODE_OAUTH_TOKEN"
                    | "GH_TOKEN"
                    | "GITHUB_TOKEN"
            ) && binding
                .get("value")
                .is_some_and(|value| value.as_str() != Some(""))
                && !profile["deny"]
                    .as_array()
                    .is_some_and(|denied| denied.iter().any(|denied| denied == name))
        })
    });
    if credential {
        return Err("The Claude profile contains a literal host credential. Remove it and sign in from Settings, Providers.".into());
    }
    Ok(())
}

pub fn command(command: AgentCommand<'_>) -> Result<GuestCommand, String> {
    let arguments = match command {
        AgentCommand::Version => vec!["--version".into()],
        AgentCommand::Login => vec!["auth".into(), "login".into()],
        AgentCommand::LoginStatus => vec!["auth".into(), "status".into(), "--json".into()],
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
    Ok(GuestCommand::new("claude", arguments))
}
