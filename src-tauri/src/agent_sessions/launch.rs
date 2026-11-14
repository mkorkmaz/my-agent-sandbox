use crate::{
    agents::AgentKind,
    desktop::settings::Network,
    projects::{Project, ProjectStage},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchMode {
    Shell,
    Login,
    Chat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AdmissionState {
    EnvironmentVerified,
    Online,
    Authorized,
    CapabilitiesVerified,
    Prepared,
}

pub struct Admission {
    mode: LaunchMode,
    state: AdmissionState,
}

impl Admission {
    pub fn begin(
        project: &Project,
        agent: AgentKind,
        mode: LaunchMode,
        error: &str,
    ) -> Result<Self, String> {
        if project.agent_kind != agent || project.stage != ProjectStage::EnvironmentReady {
            return Err(error.into());
        }
        Ok(Self {
            mode,
            state: AdmissionState::EnvironmentVerified,
        })
    }

    pub fn online(&mut self, network: &Network, error: &str) -> Result<(), String> {
        if *network == Network::Offline {
            return Err(error.into());
        }
        self.advance(AdmissionState::EnvironmentVerified, AdmissionState::Online)
    }

    pub fn authorize(&mut self, accepted: bool, error: &str) -> Result<(), String> {
        if !accepted {
            return Err(error.into());
        }
        self.advance(AdmissionState::Online, AdmissionState::Authorized)
    }

    pub fn capabilities(&mut self) -> Result<(), String> {
        let previous = match self.mode {
            LaunchMode::Login => AdmissionState::Online,
            _ => AdmissionState::Authorized,
        };
        self.advance(previous, AdmissionState::CapabilitiesVerified)
    }

    pub fn prepare(mut self, arguments: Vec<String>) -> Result<Vec<String>, String> {
        let previous = match self.mode {
            LaunchMode::Shell => AdmissionState::EnvironmentVerified,
            _ => AdmissionState::CapabilitiesVerified,
        };
        self.advance(previous, AdmissionState::Prepared)?;
        Ok(arguments)
    }

    fn advance(&mut self, expected: AdmissionState, next: AdmissionState) -> Result<(), String> {
        if self.state != expected {
            return Err("Agent launch checks are incomplete.".into());
        }
        self.state = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_chat_cannot_skip_authorization_or_capability_checks() {
        let mut admission = Admission {
            mode: LaunchMode::Chat,
            state: AdmissionState::EnvironmentVerified,
        };
        assert!(admission.capabilities().is_err());
        admission.online(&Network::Isolated, "offline").unwrap();
        assert!(admission.authorize(false, "unsigned").is_err());
        admission.authorize(true, "unsigned").unwrap();
        assert!(admission.prepare(vec![]).is_err());
    }
}
