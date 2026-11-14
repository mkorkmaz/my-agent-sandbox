use super::{AgentEnvironment, EnvironmentId, Environments, Project, ProjectOptions};
use crate::{agents::AgentKind, desktop::settings};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Resources {
    cpus: u16,
    memory_mb: u32,
    verification: settings::Verification,
}

#[derive(Eq, PartialEq)]
enum ResourceChange {
    Unchanged,
    InvalidatePreparation,
}

impl ResourceChange {
    fn between(previous: &Option<Resources>, proposed: &Option<Resources>) -> Self {
        if previous == proposed {
            return Self::Unchanged;
        }
        Self::InvalidatePreparation
    }
}

impl Resources {
    fn from_options(options: &ProjectOptions) -> Self {
        Self {
            cpus: options.cpus,
            memory_mb: options.memory_mb,
            verification: options.verification.clone(),
        }
    }
    fn for_network(&self, network: settings::Network) -> ProjectOptions {
        ProjectOptions {
            cpus: self.cpus,
            memory_mb: self.memory_mb,
            verification: self.verification.clone(),
            network,
        }
    }
}

// Storage owns the environments. Project is the existing API's resolved environment view.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectRecord {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub(super) device: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) volume_uuid: Option<[u8; 16]>,
    inode: u64,
    created: u128,
    pub updated_at: u64,
    pub last_opened_at: Option<u64>,
    options: Option<Resources>,
    pub environments: Environments,
}

impl From<Project> for ProjectRecord {
    fn from(project: Project) -> Self {
        let environments = Environments::one(AgentEnvironment::from_project(&project));
        Self {
            id: project.id,
            name: project.name,
            path: project.path,
            device: project.device,
            volume_uuid: project.volume_uuid,
            inode: project.inode,
            created: project.created,
            updated_at: project.updated_at,
            last_opened_at: project.last_opened_at,
            options: project.options.as_ref().map(Resources::from_options),
            environments,
        }
    }
}

impl ProjectRecord {
    pub fn project(&self, id: &EnvironmentId) -> Result<Project, String> {
        let environment = self.environments.by_id(id)?;
        Ok(Project {
            id: self.id.clone(),
            name: self.name.clone(),
            path: self.path.clone(),
            device: self.device,
            volume_uuid: self.volume_uuid,
            inode: self.inode,
            created: self.created,
            updated_at: self.updated_at,
            last_opened_at: self.last_opened_at,
            options: self
                .options
                .as_ref()
                .map(|options| options.for_network(environment.network.clone())),
            environment_id: environment.environment_id.clone(),
            agent_kind: environment.agent_kind,
            network: environment.network.clone(),
            profile: environment.profile.clone(),
            label: environment.label.clone(),
            policy: environment.policy.clone(),
            stage: environment.stage.clone(),
            owns_session: environment.owns_session,
            owns_binding: environment.owns_binding,
            policy_detached: environment.policy_detached,
            environment_verified: environment.environment_verified,
            environment_attempt: environment.environment_attempt.clone(),
            removal: environment.removal.clone(),
        })
    }

    pub fn codex_project(&self) -> Result<Project, String> {
        self.project(&self.environments.by_kind(AgentKind::Codex)?.environment_id)
    }

    pub(super) fn default_project(&self) -> Result<Project, String> {
        if let Ok(project) = self.codex_project() {
            return Ok(project);
        }
        let environment = self
            .environments
            .iter()
            .next()
            .ok_or("Project has no environments.")?;
        self.project(&environment.environment_id)
    }

    pub(super) fn remove_environment(&mut self, id: &EnvironmentId) -> Result<(), String> {
        self.environments.by_id(id)?;
        self.environments = self
            .environments
            .iter()
            .filter(|environment| &environment.environment_id != id)
            .cloned()
            .collect::<Vec<_>>()
            .try_into()?;
        Ok(())
    }

    pub(super) fn update(&mut self, project: Project) -> Result<(), String> {
        let resources = project.options.as_ref().map(Resources::from_options);
        let mut environment = AgentEnvironment::from_project(&project);
        if let Some(options) = &project.options {
            environment.network = options.network.clone();
        }
        let mut proposed = self.clone();
        proposed.environments.replace(environment)?;
        if ResourceChange::between(&proposed.options, &resources) == ResourceChange::InvalidatePreparation {
            proposed
                .environments
                .invalidate_others(&project.environment_id);
        }
        proposed.options = resources;
        proposed.name = project.name;
        proposed.path = project.path;
        proposed.device = project.device;
        proposed.volume_uuid = project.volume_uuid;
        proposed.updated_at = project.updated_at;
        proposed.last_opened_at = project.last_opened_at;
        proposed.validate()?;
        *self = proposed;
        Ok(())
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        for environment in self.environments.iter() {
            if environment.environment_id
                != EnvironmentId::for_agent(&self.id, environment.agent_kind)
            {
                return Err("The agent environment identity does not match this project.".into());
            }
            self.project(&environment.environment_id)?.validate()?;
        }
        Ok(())
    }

    pub(super) fn matches_folder(
        &self,
        metadata: &std::fs::Metadata,
        volume: Option<[u8; 16]>,
        recover: bool,
    ) -> bool {
        self.environments
            .iter()
            .next()
            .and_then(|environment| self.project(&environment.environment_id).ok())
            .is_some_and(|project| project.matches_folder(metadata, volume, recover))
    }
}
