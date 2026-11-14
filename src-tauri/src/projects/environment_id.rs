use crate::agents::AgentKind;
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct EnvironmentId(String);

impl EnvironmentId {
    pub fn for_agent(project: &str, kind: AgentKind) -> Self {
        let kind = match kind {
            AgentKind::Codex => "codex",
            AgentKind::ClaudeCode => "claude-code",
        };
        let digest = Sha256::digest(format!("tugboat-environment:{project}:{kind}"));
        Self(crate::hash::hex(digest)[..32].into())
    }

    #[cfg(test)]
    pub(super) fn filename(&self) -> String {
        format!("{}.json", self.0)
    }
}

impl std::fmt::Display for EnvironmentId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for EnvironmentId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(serde::de::Error::custom(
                "Invalid agent environment identity.",
            ));
        }
        Ok(Self(value))
    }
}
