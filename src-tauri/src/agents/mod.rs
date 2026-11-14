mod claude;
pub(crate) mod codex;
mod command;
mod session_id;

pub use command::GuestCommand;
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub use session_id::SessionId;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    Codex,
    ClaudeCode,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentOperation {
    Login,
    LoginStatus,
    Logout,
    NewChat,
    Resume,
}

pub enum AgentCommand<'a> {
    Version,
    Login,
    LoginStatus,
    Logout,
    NewChat(Option<&'a SessionId>),
    Resume(&'a SessionId),
}

impl AgentOperation {
    pub fn command(self, session: Option<&SessionId>) -> Result<AgentCommand<'_>, String> {
        if session.is_some() && !matches!(self, Self::NewChat | Self::Resume) {
            return Err("This agent operation does not accept a session ID.".into());
        }
        match self {
            Self::Login => Ok(AgentCommand::Login),
            Self::LoginStatus => Ok(AgentCommand::LoginStatus),
            Self::Logout => Ok(AgentCommand::Logout),
            Self::NewChat => Ok(AgentCommand::NewChat(session)),
            Self::Resume => Ok(AgentCommand::Resume(
                session.ok_or("Choose a saved agent session to resume.")?,
            )),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDefinition {
    kind: AgentKind,
    operations: [AgentOperation; 5],
}

impl AgentKind {
    pub fn binary(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude",
        }
    }

    pub fn validate_profile(self, name: &str, profile: &Value) -> Result<(), String> {
        if profile["name"] != name
            || profile["kind"] != "agent"
            || profile["binary"] != self.binary()
        {
            return Err(format!(
                "Select an agent profile that runs {}.",
                self.binary()
            ));
        }
        if self == Self::ClaudeCode {
            claude::validate_profile(profile)?;
        }
        Ok(())
    }

    pub fn command(self, command: AgentCommand<'_>) -> Result<GuestCommand, String> {
        match self {
            Self::Codex => codex::command(command, codex::Boundary::Brig),
            Self::ClaudeCode => claude::command(command),
        }
    }

    pub fn validate_version(self, output: &str) -> Result<(), String> {
        let version = match self {
            Self::Codex => output.trim().strip_prefix("codex-cli "),
            Self::ClaudeCode => output.trim().strip_suffix(" (Claude Code)"),
        };
        if !version.is_some_and(valid_version) {
            return Err(format!(
                "Guest {} returned an unsupported version response.",
                self.binary()
            ));
        }
        Ok(())
    }
}

fn valid_version(version: &str) -> bool {
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let parts: Vec<_> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-+".contains(&byte))
}

#[tauri::command]
pub fn agent_definitions() -> [AgentDefinition; 2] {
    [AgentKind::Codex, AgentKind::ClaudeCode].map(|kind| AgentDefinition {
        kind,
        operations: [
            AgentOperation::Login,
            AgentOperation::LoginStatus,
            AgentOperation::Logout,
            AgentOperation::NewChat,
            AgentOperation::Resume,
        ],
    })
}

#[cfg(test)]
mod tests;
