use super::Action;
use crate::{
    agents::{AgentCommand, AgentKind, SessionId},
    environment::{self, CheckedProject},
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Authentication {
    logged_in: bool,
}

pub(super) fn signed_in(checked: &CheckedProject) -> Result<bool, String> {
    // Brig 0.5.2 keeps `exec` for plain output; `sh` allocates a guest terminal.
    let mut arguments = vec![
        "exec".into(),
        checked.project.reference(),
        "--".into(),
        "claude".into(),
    ];
    arguments.extend(
        AgentKind::ClaudeCode
            .command(AgentCommand::LoginStatus)?
            .in_profile(),
    );
    let output = checked.brig.run(&arguments, Some(&checked.project.path))?;
    read_authentication(&output)
}

fn read_authentication(output: &crate::platform::command::CommandOutput) -> Result<bool, String> {
    if output.timed_out || output.cancelled || output.output_exceeded {
        return Err(
            "Claude sign-in status could not be verified. Check this environment and try again."
                .into(),
        );
    }
    let authentication: Authentication = serde_json::from_str(&output.stdout)
        .map_err(|_| "Claude returned an unsupported sign-in response.")?;
    if output.exit_code != Some(i32::from(!authentication.logged_in)) {
        return Err("Claude sign-in status failed. Check this environment and try again.".into());
    }
    Ok(authentication.logged_in)
}

fn supports_manual(help: &str) -> bool {
    let Some((_, options)) = help.split_once("--permission-mode") else {
        return false;
    };
    options
        .lines()
        .take_while(|line| !line.trim_start().starts_with('-'))
        .flat_map(str::split_whitespace)
        .any(|word| word.trim_matches(['"', '\'', ',', '[', ']', '(', ')']) == "manual")
}

pub(super) fn arguments(
    checked: &CheckedProject,
    action: Action,
    session: Option<&SessionId>,
    use_brig_boundary: Option<bool>,
) -> Result<Vec<String>, String> {
    use crate::agent_sessions::launch::{Admission, LaunchMode};
    let mode = match action {
        Action::Shell => LaunchMode::Shell,
        Action::Login => LaunchMode::Login,
        _ => LaunchMode::Chat,
    };
    let mut admission = Admission::begin(&checked.project, AgentKind::ClaudeCode, mode,
        "Prepare the selected Claude environment first.")?;
    if action == Action::Shell {
        return admission.prepare(vec!["sh".into(), checked.project.reference()]);
    }
    admission.online(&checked.settings.network, "Claude sign-in and chats are unavailable in offline mode.")?;
    if use_brig_boundary == Some(false) {
        return Err(
            "Claude uses the Brig boundary. Enable it before starting this session.".into(),
        );
    }
    if action == Action::Login {
        admission.capabilities()?;
        return admission.prepare(AgentKind::ClaudeCode
            .command(AgentCommand::Login)?
            .in_guest(checked.project.reference()));
    }
    admission.authorize(signed_in(checked)?, "Sign in with Claude in Settings, Providers.")?;
    let help = checked.brig.checked(
        &[
            "exec".into(),
            checked.project.reference(),
            "--".into(),
            "claude".into(),
            "--help".into(),
        ],
        None,
    )?;
    if !help.stdout.contains("--session-id")
        || !help.stdout.contains("--resume")
        || !supports_manual(&help.stdout)
    {
        return Err(
            "Guest Claude does not support the required session and manual permission options."
                .into(),
        );
    }
    let session = session.ok_or("Choose a Claude chat session ID.")?;
    let command = match action {
        Action::NewChat => AgentCommand::NewChat(Some(session)),
        Action::Resume => AgentCommand::Resume(session),
        _ => return Err("Unsupported Claude chat operation.".into()),
    };
    admission.capabilities()?;
    admission.prepare(environment::run_arguments(
        checked,
        &AgentKind::ClaudeCode.command(command)?.in_profile(),
    ))
}

#[cfg(test)]
pub(super) fn logout(checked: &CheckedProject) -> Result<bool, String> {
    let arguments = AgentKind::ClaudeCode
        .command(AgentCommand::Logout)?
        .in_guest(checked.project.reference());
    let output = checked.brig.run(&arguments, Some(&checked.project.path))?;
    if output.exit_code != Some(0) || output.timed_out || output.cancelled || output.output_exceeded
    {
        return Err("Claude sign-out failed. Check this environment and try again.".into());
    }
    if signed_in(checked)? {
        return Err("Claude is still signed in. Refresh its status before trying again.".into());
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_authentication_commands_do_not_publish_a_signed_in_status() {
        for interrupted in [0, 1, 2] {
            let output = crate::platform::command::CommandOutput {
                stdout: "{\"loggedIn\":true}".into(),
                stderr: "synthetic-private-detail".into(),
                exit_code: Some(0),
                timed_out: interrupted == 0,
                cancelled: interrupted == 1,
                output_exceeded: interrupted == 2,
            };
            let error = read_authentication(&output).unwrap_err();
            assert!(error.contains("could not be verified"));
            assert!(!error.contains("synthetic-private-detail"));
        }
    }
}
