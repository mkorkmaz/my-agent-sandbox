mod claude;
pub(crate) mod codex;
mod command;
mod session_id;

pub use command::GuestCommand;
use serde::{Deserialize, Serialize};
pub use session_id::SessionId;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    Codex,
    ClaudeCode,
}

pub enum AgentCommand<'a> {
    Version,
    LoginStatus,
    #[cfg(test)]
    Logout,
    NewChat(Option<&'a SessionId>),
    Resume(&'a SessionId),
}

impl AgentKind {
    pub fn binary(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude",
        }
    }

    pub fn command(self, command: AgentCommand<'_>) -> Result<GuestCommand, String> {
        match self {
            Self::Codex => codex::command(command, codex::Boundary::Brig),
            Self::ClaudeCode => claude::command(command),
        }
    }

    #[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
mod tests;
