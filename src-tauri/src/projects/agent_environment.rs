use super::{EnvironmentAttempt, EnvironmentId, Policy, Project, ProjectStage, Removal};
use crate::{agents::AgentKind, desktop::settings::Network};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentEnvironment {
    pub environment_id: EnvironmentId,
    pub agent_kind: AgentKind,
    pub profile: String,
    pub label: String,
    pub network: Network,
    pub policy: Option<Policy>,
    pub stage: ProjectStage,
    pub owns_session: bool,
    pub owns_binding: bool,
    pub policy_detached: bool,
    pub environment_verified: Option<bool>,
    pub environment_attempt: Option<EnvironmentAttempt>,
    pub removal: Option<Removal>,
}

impl AgentEnvironment {
    pub(super) fn from_project(project: &Project) -> Self {
        Self {
            environment_id: project.environment_id.clone(),
            agent_kind: project.agent_kind,
            profile: project.profile.clone(),
            label: project.label.clone(),
            network: project.network.clone(),
            policy: project.policy.clone(),
            stage: project.stage.clone(),
            owns_session: project.owns_session,
            owns_binding: project.owns_binding,
            policy_detached: project.policy_detached,
            environment_verified: project.environment_verified,
            environment_attempt: project.environment_attempt.clone(),
            removal: project.removal.clone(),
        }
    }

    pub fn reference(&self) -> String {
        format!("{}@{}", self.profile, self.label)
    }

    pub(super) fn invalidate_preparation(&mut self) {
        self.environment_verified = Some(false);
        self.environment_attempt = None;
        self.stage = self.stage.invalidate_preparation();
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(try_from = "Vec<AgentEnvironment>", into = "Vec<AgentEnvironment>")]
pub struct Environments(Vec<AgentEnvironment>);

pub enum Membership<'a> {
    Absent,
    Present(&'a AgentEnvironment),
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum RemovalEffect {
    KeepProject,
    RemoveProject,
}

impl Environments {
    pub fn membership(&self, kind: AgentKind) -> Membership<'_> {
        match self.0.iter().find(|environment| environment.agent_kind == kind) {
            Some(environment) => Membership::Present(environment),
            None => Membership::Absent,
        }
    }

    pub fn removal_effect(&self, id: &EnvironmentId) -> Result<RemovalEffect, String> {
        self.by_id(id)?;
        if self.0.len() == 1 {
            return Ok(RemovalEffect::RemoveProject);
        }
        Ok(RemovalEffect::KeepProject)
    }

    pub(super) fn one(environment: AgentEnvironment) -> Self {
        Self(vec![environment])
    }

    pub fn by_kind(&self, kind: AgentKind) -> Result<&AgentEnvironment, String> {
        self.0
            .iter()
            .find(|environment| environment.agent_kind == kind)
            .ok_or("The project has no environment for this agent.".into())
    }

    pub fn by_id(&self, id: &EnvironmentId) -> Result<&AgentEnvironment, String> {
        self.0
            .iter()
            .find(|environment| &environment.environment_id == id)
            .ok_or("This environment does not belong to the project.".into())
    }

    pub fn iter(&self) -> impl Iterator<Item = &AgentEnvironment> {
        self.0.iter()
    }

    pub(super) fn replace(&mut self, environment: AgentEnvironment) -> Result<(), String> {
        let current = self
            .0
            .iter_mut()
            .find(|current| current.environment_id == environment.environment_id)
            .ok_or("This environment does not belong to the project.")?;
        if current.agent_kind != environment.agent_kind {
            return Err("The environment agent changed.".into());
        }
        *current = environment;
        Ok(())
    }

    pub(super) fn add(&mut self, environment: AgentEnvironment) -> Result<(), String> {
        let mut proposed = self.0.clone();
        proposed.push(environment);
        *self = Self::try_from(proposed)?;
        Ok(())
    }

    pub(super) fn invalidate_others(&mut self, selected: &EnvironmentId) {
        self.0
            .iter_mut()
            .filter(|environment| &environment.environment_id != selected)
            .for_each(AgentEnvironment::invalidate_preparation);
    }
}

impl TryFrom<Vec<AgentEnvironment>> for Environments {
    type Error = String;
    fn try_from(environments: Vec<AgentEnvironment>) -> Result<Self, String> {
        let mut identities = HashSet::new();
        let mut kinds = HashSet::new();
        if environments.is_empty() || environments.len() > 2 {
            return Err("A project must have one environment per supported agent.".into());
        }
        for environment in &environments {
            if !identities.insert(environment.environment_id.clone())
                || !kinds.insert(environment.agent_kind)
            {
                return Err("Duplicate agent environment found.".into());
            }
        }
        Ok(Self(environments))
    }
}

impl From<Environments> for Vec<AgentEnvironment> {
    fn from(environments: Environments) -> Self {
        environments.0
    }
}
