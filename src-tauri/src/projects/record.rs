use super::{
    EnvironmentAttempt, EnvironmentId, Policy, Project, ProjectOptions, ProjectStage, Removal,
};
use crate::{agents::AgentKind, desktop::settings::Network};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SharedEnvironment {
    environment_id: EnvironmentId,
    profile: String,
    label: String,
    image_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    image_update: Option<super::project_image_update::Pending>,
    network: Network,
    policy: Option<Policy>,
    stage: ProjectStage,
    owns_session: bool,
    owns_binding: bool,
    policy_detached: bool,
    environment_verified: Option<bool>,
    environment_attempt: Option<EnvironmentAttempt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    approved_plan: Option<String>,
    removal: Option<Removal>,
}

impl From<&Project> for SharedEnvironment {
    fn from(project: &Project) -> Self {
        Self {
            environment_id: project.environment_id.clone(),
            profile: project.profile.clone(),
            label: project.label.clone(),
            image_ref: project.image_ref.clone(),
            image_update: project.image_update.clone(),
            network: project.network.clone(),
            policy: project.policy.clone(),
            stage: project.stage,
            owns_session: project.owns_session,
            owns_binding: project.owns_binding,
            policy_detached: project.policy_detached,
            environment_verified: project.environment_verified,
            environment_attempt: project.environment_attempt.clone(),
            approved_plan: project.approved_plan.clone(),
            removal: project.removal.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectRecord {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    previous_guest_project_names: BTreeSet<String>,
    pub(super) device: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) volume_uuid: Option<[u8; 16]>,
    inode: u64,
    created: u128,
    pub updated_at: u64,
    pub last_opened_at: Option<u64>,
    options: Option<ProjectOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) preview: Option<super::project_preview::Approval>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) github_access: Option<super::project_github_access::Grant>,
    environment: SharedEnvironment,
}

impl From<Project> for ProjectRecord {
    fn from(project: Project) -> Self {
        let environment = SharedEnvironment::from(&project);
        Self {
            id: project.id,
            name: project.name,
            path: project.path,
            previous_guest_project_names: project.previous_guest_project_names,
            device: project.device,
            volume_uuid: project.volume_uuid,
            inode: project.inode,
            created: project.created,
            updated_at: project.updated_at,
            last_opened_at: project.last_opened_at,
            options: project.options,
            preview: project.preview,
            github_access: project.github_access,
            environment,
        }
    }
}

impl ProjectRecord {
    pub(super) fn reference(&self) -> String {
        format!("{}@{}", self.environment.profile, self.environment.label)
    }

    pub(super) fn project(&self, id: &EnvironmentId, agent: AgentKind) -> Result<Project, String> {
        if &self.environment.environment_id != id {
            return Err("This environment does not belong to the project.".into());
        }
        Ok(self.for_agent(agent))
    }

    pub(super) fn for_agent(&self, agent_kind: AgentKind) -> Project {
        let environment = &self.environment;
        Project {
            id: self.id.clone(),
            name: self.name.clone(),
            path: self.path.clone(),
            previous_guest_project_names: self.previous_guest_project_names.clone(),
            device: self.device,
            volume_uuid: self.volume_uuid,
            inode: self.inode,
            created: self.created,
            updated_at: self.updated_at,
            last_opened_at: self.last_opened_at,
            options: self.options.clone(),
            environment_id: environment.environment_id.clone(),
            agent_kind,
            network: environment.network.clone(),
            profile: environment.profile.clone(),
            label: environment.label.clone(),
            image_ref: environment.image_ref.clone(),
            image_update: environment.image_update.clone(),
            policy: environment.policy.clone(),
            stage: environment.stage,
            owns_session: environment.owns_session,
            adopted_managed_profile: false,
            owns_binding: environment.owns_binding,
            policy_detached: environment.policy_detached,
            environment_verified: environment.environment_verified,
            environment_attempt: environment.environment_attempt.clone(),
            approved_plan: environment.approved_plan.clone(),
            removal: environment.removal.clone(),
            preview: self.preview.clone(),
            github_access: self.github_access.clone(),
        }
    }

    pub(super) fn update(&mut self, mut project: Project) -> Result<(), String> {
        if self.environment.environment_id != project.environment_id
            || self.reference() != project.reference()
        {
            return Err("The shared environment identity changed.".into());
        }
        if let Some(options) = &project.options {
            project.network = options.network.clone();
        }
        let mut proposed = Self::from(project);
        proposed.preview = self.preview.clone();
        proposed.github_access = self.github_access.clone();
        proposed.validate()?;
        *self = proposed;
        Ok(())
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        if let Some(grant) = &self.github_access {
            grant.validate(&self.id)?;
        }
        if let Some(approval) = &self.preview {
            self.project(&approval.environment_id, AgentKind::Codex)?;
            approval.validate(&self.reference())?;
        }
        if !self
            .previous_guest_project_names
            .iter()
            .all(|name| super::guest_project_scope::valid_folder_name(name))
        {
            return Err("Invalid former guest project folder name.".into());
        }
        self.for_agent(AgentKind::Codex).validate()
    }

    pub(super) fn relink_verified_folder(&mut self, path: PathBuf) {
        let current = path.file_name().and_then(|name| name.to_str());
        let previous = self.path.file_name().and_then(|name| name.to_str());
        if let Some(previous) = previous.filter(|name| {
            Some(*name) != current && super::guest_project_scope::valid_folder_name(name)
        }) {
            self.previous_guest_project_names.insert(previous.into());
        }
        if let Some(current) = current {
            self.previous_guest_project_names.remove(current);
        }
        self.path = path;
    }

    pub(super) fn matches_folder(
        &self,
        metadata: &std::fs::Metadata,
        volume: Option<[u8; 16]>,
        recover: bool,
    ) -> bool {
        self.for_agent(AgentKind::Codex)
            .matches_folder(metadata, volume, recover)
    }
}
