use super::{
    EnvironmentAttempt, EnvironmentId, Policy, Project, ProjectOptions, ProjectStage, Registry,
    Removal, DEFAULT_ENVIRONMENT_IMAGE, ENVIRONMENT_PROFILE, PREVIOUS_ENVIRONMENT_IMAGE,
};
use crate::{
    agents::AgentKind,
    desktop::settings::{Network, Settings, Verification},
};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashSet},
    path::PathBuf,
};

pub(crate) struct Transition {
    pub(crate) registry: Registry,
    pub(crate) retired: Vec<RetiredRegistration>,
}

pub(crate) struct RetiredRegistration {
    pub(crate) project_id: String,
    pub(crate) environment_id: EnvironmentId,
    pub(crate) environments: Vec<Project>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Source {
    schema_version: u32,
    projects: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FormerProject {
    id: String,
    name: String,
    path: PathBuf,
    #[serde(default)]
    previous_guest_project_names: BTreeSet<String>,
    device: u64,
    volume_uuid: Option<[u8; 16]>,
    inode: u64,
    created: u128,
    updated_at: u64,
    last_opened_at: Option<u64>,
    options: Option<FormerResources>,
    #[serde(rename = "preview")]
    _preview: Option<super::project_preview::Approval>,
    environments: Vec<FormerEnvironment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FormerResources {
    cpus: u16,
    memory_mb: u32,
    verification: Verification,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FormerEnvironment {
    environment_id: EnvironmentId,
    agent_kind: AgentKind,
    profile: String,
    label: String,
    network: Network,
    policy: Option<Policy>,
    stage: ProjectStage,
    owns_session: bool,
    #[serde(default)]
    adopted_managed_profile: bool,
    owns_binding: bool,
    policy_detached: bool,
    environment_verified: Option<bool>,
    environment_attempt: Option<EnvironmentAttempt>,
    removal: Option<Value>,
}

impl FormerProject {
    fn project(&self, defaults: &Settings) -> Project {
        let network = self
            .environments
            .iter()
            .map(|environment| &environment.network)
            .find(|network| **network == Network::Offline)
            .or_else(|| {
                self.environments
                    .first()
                    .map(|environment| &environment.network)
            })
            .unwrap_or(&defaults.network)
            .clone();
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
            environment_id: EnvironmentId::for_project(&self.id),
            agent_kind: AgentKind::Codex,
            network: network.clone(),
            profile: ENVIRONMENT_PROFILE.into(),
            label: String::new(),
            image_ref: DEFAULT_ENVIRONMENT_IMAGE.into(),
            image_update: None,
            policy: None,
            stage: ProjectStage::Selected,
            owns_session: true,
            adopted_managed_profile: false,
            owns_binding: false,
            policy_detached: false,
            environment_verified: Some(false),
            environment_attempt: None,
            approved_plan: None,
            removal: None,
            preview: None,
            github_access: None,
            options: self.options.as_ref().map(|options| ProjectOptions {
                cpus: options.cpus,
                memory_mb: options.memory_mb,
                verification: options.verification.clone(),
                network,
            }),
        }
    }

    fn into_views(self, defaults: &Settings) -> Result<(Project, Vec<Project>), String> {
        let mut shared = self.project(defaults);
        if shared.id.len() != 32 || !shared.id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("The saved project identity is invalid.".into());
        }
        shared.label = super::project_label(&shared.id);
        let mut agents = HashSet::new();
        let mut retired = Vec::new();
        for former in self.environments {
            if !agents.insert(former.agent_kind) {
                return Err("The saved project repeats an agent environment.".into());
            }
            let mut project = shared.clone();
            project.image_ref = PREVIOUS_ENVIRONMENT_IMAGE.into();
            project.environment_id = former.environment_id;
            project.agent_kind = former.agent_kind;
            project.profile = former.profile;
            project.label = former.label;
            project.network = former.network;
            if let Some(options) = &mut project.options {
                options.network = project.network.clone();
            }
            project.policy = former.policy;
            project.stage = former.stage;
            project.owns_session = former.owns_session;
            project.adopted_managed_profile = former.adopted_managed_profile;
            project.owns_binding = former.owns_binding;
            project.policy_detached = former.policy_detached;
            project.environment_verified = former.environment_verified;
            project.environment_attempt = former.environment_attempt;
            project.removal = former.removal.map(decode_removal).transpose()?;
            retired.push(project);
        }
        Ok((shared, retired))
    }
}

fn v1(mut value: Value, defaults: &Settings) -> Result<(Project, Vec<Project>), String> {
    let fields = value
        .as_object_mut()
        .ok_or("Invalid saved project record.")?;
    let id = fields
        .get("id")
        .and_then(Value::as_str)
        .ok_or("Missing saved project identity.")?
        .to_owned();
    if [
        "environmentId",
        "agentKind",
        "network",
        "environments",
        "imageRef",
        "approvedPlan",
    ]
    .iter()
    .any(|key| fields.contains_key(*key))
    {
        return Err("Invalid version 1 project environment fields.".into());
    }
    let network = fields
        .get("options")
        .and_then(|value| value.get("network"))
        .cloned()
        .unwrap_or(
            serde_json::to_value(&defaults.network).map_err(|_| "Invalid default network mode.")?,
        );
    fields.insert(
        "environmentId".into(),
        serde_json::json!(EnvironmentId::for_agent(&id, AgentKind::Codex)),
    );
    fields.insert("agentKind".into(), serde_json::json!(AgentKind::Codex));
    fields.insert("network".into(), network);
    fields.insert(
        "imageRef".into(),
        serde_json::json!(PREVIOUS_ENVIRONMENT_IMAGE),
    );
    if let Some(removal) = fields
        .get_mut("removal")
        .filter(|removal| !removal.is_null())
    {
        *removal = serde_json::json!(decode_removal(removal.take())?);
    }
    let old: Project = serde_json::from_value(value)
        .map_err(|_| "Could not read the version 1 project record.")?;
    old.validate_fields()?;
    let mut shared = old.clone();
    shared.environment_id = EnvironmentId::for_project(&id);
    shared.profile = ENVIRONMENT_PROFILE.into();
    shared.image_ref = DEFAULT_ENVIRONMENT_IMAGE.into();
    shared.label = super::project_label(&id);
    shared.policy = None;
    shared.stage = ProjectStage::Selected;
    shared.owns_session = true;
    shared.adopted_managed_profile = false;
    shared.owns_binding = false;
    shared.policy_detached = false;
    shared.environment_verified = Some(false);
    shared.environment_attempt = None;
    shared.approved_plan = None;
    shared.removal = None;
    shared.preview = None;
    Ok((shared, vec![old]))
}

fn decode_removal(mut value: Value) -> Result<Removal, String> {
    let fields = value
        .as_object_mut()
        .ok_or("Invalid former removal record.")?;
    for field in ["logout", "loggedOut"] {
        if fields
            .remove(field)
            .is_some_and(|value| !value.is_boolean())
        {
            return Err("Invalid former removal checkpoint.".into());
        }
    }
    serde_json::from_value(value).map_err(|_| "Invalid former removal record.".into())
}

pub(crate) fn plan(bytes: &[u8], defaults: &Settings) -> Result<Option<Transition>, String> {
    let source: Source =
        serde_json::from_slice(bytes).map_err(|_| "Could not read the project registrations.")?;
    if source.schema_version == 3 {
        return Ok(None);
    }
    if !matches!(source.schema_version, 1 | 2) {
        return Err("This project registry version is unsupported.".into());
    }
    let mut transition = Transition {
        registry: Registry::default(),
        retired: Vec::new(),
    };
    let mut references = HashSet::new();
    for record in source.projects {
        let (mut project, environments) = if source.schema_version == 1 {
            v1(record, defaults)?
        } else {
            let recorded: FormerProject = serde_json::from_value(record)
                .map_err(|_| "Could not read the version 2 project registration.")?;
            recorded.into_views(defaults)?
        };
        for environment in &environments {
            environment.validate_fields()?;
            if environment.environment_id
                != EnvironmentId::for_agent(&project.id, environment.agent_kind)
                || !references.insert(environment.reference())
            {
                return Err("A retired Brig identity does not match its saved project.".into());
            }
        }
        if let Some(options) = &mut project.options {
            options.verification = Verification::Require;
        }
        project.previous_guest_project_names.clear();
        transition.retired.push(RetiredRegistration {
            project_id: project.id.clone(),
            environment_id: project.environment_id.clone(),
            environments,
        });
        transition.registry.projects.push(project.into());
    }
    transition.registry.validate()?;
    Ok(Some(transition))
}
