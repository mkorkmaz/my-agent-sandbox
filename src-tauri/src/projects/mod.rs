use crate::agents::AgentKind;
use crate::{desktop::settings, platform::command, platform::host, storage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, State};

mod agent_environment;
mod attempt;
mod environment_id;
mod folder;
mod migration;
#[cfg(test)]
pub(crate) mod migration_tests;
pub(crate) mod operations;
pub(crate) mod project_policy;
mod record;
mod removal;
mod stage;
pub use agent_environment::{AgentEnvironment, Environments};
use agent_environment::{Membership, RemovalEffect};
pub use attempt::{AttemptCompletion, EnvironmentAttempt};
pub use environment_id::EnvironmentId;
pub use folder::FolderState;
pub use stage::ProjectStage;
pub use removal::RemovalStep;
use record::ProjectRecord;

static NEXT_PROBE: AtomicU64 = AtomicU64::new(1);
#[derive(Default)]
pub struct Projects(pub Arc<Mutex<()>>);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policy {
    pub name: String,
    pub owned: bool,
    pub fingerprint: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    device: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    volume_uuid: Option<[u8; 16]>,
    inode: u64,
    created: u128,
    pub environment_id: EnvironmentId,
    pub agent_kind: AgentKind,
    pub network: settings::Network,
    pub profile: String,
    pub label: String,
    pub policy: Option<Policy>,
    pub stage: ProjectStage,
    #[serde(default)]
    pub owns_session: bool,
    #[serde(default)]
    pub owns_binding: bool,
    #[serde(default)]
    pub policy_detached: bool,
    #[serde(default)]
    pub environment_verified: Option<bool>,
    #[serde(default)]
    pub environment_attempt: Option<EnvironmentAttempt>,
    #[serde(default)]
    pub removal: Option<Removal>,
    pub updated_at: u64,
    #[serde(default)]
    pub last_opened_at: Option<u64>,
    #[serde(default)]
    pub options: Option<ProjectOptions>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Removal {
    pub reference: String,
    pub workspace: String,
    pub logout: bool,
    pub had_environment: bool,
    pub logged_out: bool,
    pub stopped: bool,
    pub environment_removed: bool,
    pub binding_detached: bool,
    pub policy_handled: bool,
    pub preserved_policy: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectOptions {
    pub cpus: u16,
    pub memory_mb: u32,
    pub verification: settings::Verification,
    pub network: settings::Network,
}
impl ProjectOptions {
    pub fn apply(&self, settings: &mut settings::Settings) {
        settings.cpus = self.cpus;
        settings.memory_mb = self.memory_mb;
        settings.verification = self.verification.clone();
        settings.network = self.network.clone();
    }
}

impl Project {
    pub fn require_codex(&self) -> Result<(), String> {
        if self.agent_kind != AgentKind::Codex {
            return Err("Shared ChatGPT sign-in is available only for Codex environments.".into());
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn ledger_file(&self, records: &Path) -> Result<PathBuf, String> {
        self.validate()?;
        let ledgers = records.join("conversation-ledgers");
        if self.agent_kind == AgentKind::Codex {
            return Ok(ledgers.join(format!("{}.json", self.id)));
        }
        Ok(ledgers.join(&self.id).join(self.environment_id.filename()))
    }

    pub fn removal_pending(&self) -> bool {
        self.stage == crate::projects::ProjectStage::Removing || self.removal.is_some()
    }
    pub fn require_active(&self) -> Result<(), String> {
        if self.removal_pending() {
            return Err("Removal is incomplete. Review the removal preview to continue before using this project.".into());
        }
        Ok(())
    }
    pub fn settings(&self, mut defaults: settings::Settings) -> Result<settings::Settings, String> {
        defaults.network = self.network.clone();
        if let Some(options) = &self.options {
            options.apply(&mut defaults);
        }
        defaults.validate()?;
        Ok(defaults)
    }
    pub fn environment_stage(&self, running: bool, fingerprint: &str) -> ProjectStage {
        if running
            && self.environment_verified == Some(true)
            && self.environment_attempt.as_ref().is_some_and(|attempt| {
                attempt.verifies(fingerprint)
            })
        {
            return ProjectStage::EnvironmentReady;
        }
        ProjectStage::Ready
    }
    pub fn begin_environment(&mut self, attempt: EnvironmentAttempt) -> Result<(), String> {
        self.require_active()?;
        if self.policy_detached || !matches!(self.stage, ProjectStage::Ready | ProjectStage::EnvironmentReady) {
            return Err("Complete project setup before starting the environment.".into());
        }
        self.stage = ProjectStage::EnvironmentStarting;
        self.environment_verified = Some(false);
        self.environment_attempt = Some(attempt);
        Ok(())
    }

    pub(crate) fn recover_preparation(&mut self, running: bool) -> Result<bool, String> {
        if self.stage != ProjectStage::EnvironmentStarting || running {
            return Ok(false);
        }
        if let Some(attempt) = self.environment_attempt.as_mut() {
            if attempt.state() == attempt::AttemptState::Pending {
                attempt.finish(Err("The preparation process ended before verification. Prepare again.".into()))?;
            }
        }
        self.environment_verified = Some(false);
        self.stage = ProjectStage::Ready;
        Ok(true)
    }

    pub fn stop_environment(&mut self) -> Result<(), String> {
        self.require_active()?;
        self.stage = ProjectStage::Stopping;
        self.environment_verified = Some(false);
        self.environment_attempt = None;
        Ok(())
    }

    pub fn finish_stop(&mut self) -> Result<(), String> {
        if self.stage != ProjectStage::Stopping {
            return Err("This environment is not stopping.".into());
        }
        self.stage = ProjectStage::Ready;
        Ok(())
    }
    fn set_preferences(
        &mut self,
        name: &str,
        options: ProjectOptions,
        previous: &settings::Settings,
    ) {
        if options.cpus != previous.cpus
            || options.memory_mb != previous.memory_mb
            || options.verification != previous.verification
            || options.network != previous.network
        {
            self.environment_verified = Some(false);
            self.environment_attempt = None;
            self.stage = self.stage.invalidate_preparation();
        }
        self.network = options.network.clone();
        self.name = name.into();
        self.options = Some(options);
        self.updated_at = now();
    }
    pub fn reference(&self) -> String {
        format!("{}@{}", self.profile, self.label)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.environment_id != EnvironmentId::for_agent(&self.id, self.agent_kind) {
            return Err("The environment identity does not match this project.".into());
        }
        if self.id.len() != 32
            || !self.id.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !self.path.is_absolute()
            || !settings::valid_identifier(&self.label)
            || !settings::valid_identifier(&self.profile)
        {
            return Err("Invalid project record. Select the folder again.".into());
        }
        if let Some(policy) = &self.policy {
            if !settings::valid_identifier(&policy.name) {
                return Err("Invalid project policy record.".into());
            }
        }
        if let Some(removal) = &self.removal {
            removal.validate()?;
            if removal.reference != self.reference() || !Path::new(&removal.workspace).is_absolute()
            {
                return Err("Invalid removal recovery record. Review the project record.".into());
            }
        }
        if let Some(options) = &self.options {
            let mut defaults = settings::Settings::default();
            options.apply(&mut defaults);
            defaults.validate()?;
        }
        Ok(())
    }
    pub fn check_path(&self) -> Result<(), String> {
        match self.folder_state() {
            FolderState::Available => Ok(()),
            FolderState::Unavailable => Err("The project moved or is unavailable. Select the folder again.".into()),
            FolderState::Replaced => Err(
                "The folder at this path changed. Select it again to create a new record.".into(),
            ),
        }
    }
    fn matches_folder(
        &self,
        metadata: &fs::Metadata,
        volume: Option<[u8; 16]>,
        recover_legacy_remount: bool,
    ) -> bool {
        let same_volume = match self.volume_uuid {
            Some(expected) => volume == Some(expected),
            None => metadata.dev() == self.device || (recover_legacy_remount && self.created != 0),
        };
        metadata.is_dir()
            && metadata.ino() == self.inode
            && creation_time(metadata) == self.created
            && same_volume
    }
    pub fn check_guest_path(&self) -> Result<(), String> {
        let name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("Select a project subfolder.")?;
        validate_guest_path(name)
    }
}

pub fn validate_guest_path(path: &str) -> Result<(), String> {
    if path
        .chars()
        .any(|character| matches!(character, '\0' | '\t' | '\r' | '\n' | '\'' | '"'))
    {
        return Err(
            "This folder name contains tabs, line breaks or quotes that hull cannot mount.".into(),
        );
    }
    Ok(())
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Registry {
    schema_version: u32,
    pub projects: Vec<ProjectRecord>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            schema_version: 2,
            projects: Vec::new(),
        }
    }
}

impl Registry {
    pub fn load(file: &Path) -> Result<Self, String> {
        let Some(bytes) = storage::read_regular_bytes(file)? else {
            return Ok(Self::default());
        };
        Ok(migration::decode(file, &bytes)?.into_record())
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema_version != 2 {
            return Err("This project registry version is unsupported.".into());
        }
        let mut identities = std::collections::HashSet::new();
        let mut references = std::collections::HashSet::new();
        for project in &self.projects {
            project.validate()?;
            if !identities.insert(&project.id) {
                return Err("Duplicate project record found.".into());
            }
            for environment in project.environments.iter() {
                if !references.insert(environment.reference()) {
                    return Err("Duplicate Brig environment reference found.".into());
                }
            }
        }
        Ok(())
    }

    pub fn codex_projects(&self) -> Result<Vec<Project>, String> {
        self.projects
            .iter()
            .filter(|record| record.environments.by_kind(AgentKind::Codex).is_ok())
            .map(ProjectRecord::codex_project)
            .collect()
    }

    pub(crate) fn project_views(&self) -> Result<Vec<Project>, String> {
        self.projects
            .iter()
            .map(ProjectRecord::default_project)
            .collect()
    }

    pub fn environment(
        &self,
        project: &str,
        environment: &EnvironmentId,
    ) -> Result<Project, String> {
        self.record(project)?.project(environment)
    }

    pub fn resolve_environment(
        &self,
        project: &str,
        environment: Option<&EnvironmentId>,
    ) -> Result<Project, String> {
        if let Some(environment) = environment {
            return self.environment(project, environment);
        }
        self.project(project)
    }

    fn record(&self, id: &str) -> Result<&ProjectRecord, String> {
        self.projects
            .iter()
            .find(|project| project.id == id)
            .ok_or("Project record not found. Select the folder again.".into())
    }

    pub fn environments(&self, id: &str) -> Result<Environments, String> {
        Ok(self.record(id)?.environments.clone())
    }

    pub fn register_with_defaults(
        &mut self,
        selected: &Path,
        defaults: &settings::Settings,
    ) -> Result<Project, String> {
        let previous_count = self.projects.len();
        let mut project = self.register(selected, &defaults.codex.profile)?;
        if self.projects.len() > previous_count {
            project.network = defaults.network.clone();
            self.update(project.clone())?;
        }
        Ok(project)
    }

    fn register_for_agent(
        &mut self,
        selected: &Path,
        defaults: &settings::Settings,
        kind: Option<AgentKind>,
    ) -> Result<Project, String> {
        let mut proposed = self.clone();
        let previous_count = proposed.projects.len();
        let project = proposed.register_with_defaults(selected, defaults)?;
        let kind = kind.unwrap_or(project.agent_kind);
        let environment = proposed.ensure_environment(&project.id, kind, defaults)?;
        if proposed.projects.len() > previous_count && kind == AgentKind::ClaudeCode {
            proposed.remove_environment(
                &project.id,
                &EnvironmentId::for_agent(&project.id, AgentKind::Codex),
            )?;
        }
        let selected = proposed.environment(&project.id, &environment.environment_id)?;
        *self = proposed;
        Ok(selected)
    }

    pub fn register(&mut self, selected: &Path, profile: &str) -> Result<Project, String> {
        let path = check_folder(selected)?;
        let metadata = path.metadata().map_err(|error| error.to_string())?;
        let created = creation_time(&metadata);
        let volume = volume_uuid(&path);
        let opened_at = self.next_opened_at();
        let previous = self.projects.iter_mut().find(|project| {
            project.matches_folder(&metadata, volume, true)
                && (project.path == path || !project.path.exists())
        });
        if let Some(project) = previous {
            project.path = path;
            project.device = metadata.dev();
            project.volume_uuid = volume;
            project.updated_at = now();
            project.last_opened_at = Some(opened_at);
            return project.default_project();
        }
        let mut identity = Sha256::new();
        identity.update(path.as_os_str().as_encoded_bytes());
        identity.update(metadata.dev().to_le_bytes());
        identity.update(metadata.ino().to_le_bytes());
        identity.update(created.to_le_bytes());
        let id = crate::hash::hex(identity.finalize())[..32].to_owned();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let project = Project {
            // hull places QMP sockets below the instance name; Darwin limits their path to 103 bytes.
            label: format!("bd-{}", &id[..16]),
            environment_id: EnvironmentId::for_agent(&id, AgentKind::Codex),
            agent_kind: AgentKind::Codex,
            network: settings::Settings::default().network,
            id,
            name,
            path,
            device: metadata.dev(),
            volume_uuid: volume,
            inode: metadata.ino(),
            created,
            profile: profile.into(),
            policy: None,
            stage: crate::projects::ProjectStage::Selected,
            owns_session: true,
            owns_binding: false,
            policy_detached: false,
            environment_verified: Some(false),
            environment_attempt: None,
            removal: None,
            updated_at: now(),
            last_opened_at: Some(opened_at),
            options: None,
        };
        project.validate()?;
        self.projects.push(project.clone().into());
        Ok(project)
    }

    pub fn project(&self, id: &str) -> Result<Project, String> {
        self.record(id)?.default_project()
    }

    pub fn remove_environment(
        &mut self,
        id: &str,
        environment: &EnvironmentId,
    ) -> Result<(), String> {
        self.environment(id, environment)?;
        let mut proposed = self.clone();
        if proposed.record(id)?.environments.removal_effect(environment)? == RemovalEffect::RemoveProject {
            proposed.projects.retain(|record| record.id != id);
            proposed.validate()?;
            *self = proposed;
            return Ok(());
        }
        proposed
            .projects
            .iter_mut()
            .find(|record| record.id == id)
            .ok_or("Project not found.")?
            .remove_environment(environment)?;
        proposed.validate()?;
        *self = proposed;
        Ok(())
    }
    fn rescan_paths(&mut self) -> bool {
        let mut changed = false;
        for project in &mut self.projects {
            let Ok(metadata) = project.path.metadata() else {
                continue;
            };
            let volume = volume_uuid(&project.path);
            if !project.matches_folder(&metadata, volume, true) {
                continue;
            }
            if project.device != metadata.dev() || project.volume_uuid != volume {
                project.device = metadata.dev();
                project.volume_uuid = volume;
                changed = true;
            }
        }
        changed
    }
    fn forget_unavailable(&mut self, id: &str) -> Result<(), String> {
        let project = self.project(id)?;
        if project.folder_state() == FolderState::Available {
            return Err(
                "The folder is available again. Open the project to manage its environment.".into(),
            );
        }
        self.projects.retain(|project| project.id != id);
        Ok(())
    }
    fn next_opened_at(&self) -> u64 {
        let current = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let previous = self
            .projects
            .iter()
            .map(|project| {
                project
                    .last_opened_at
                    .unwrap_or(project.updated_at.saturating_mul(1000))
            })
            .max()
            .unwrap_or(0);
        current.max(previous.saturating_add(1))
    }
    pub fn update(&mut self, project: Project) -> Result<(), String> {
        project.validate()?;
        let mut proposed = self.clone();
        let current = proposed
            .projects
            .iter_mut()
            .find(|current| current.id == project.id)
            .ok_or("Project not found.")?;
        current.update(project)?;
        proposed.validate()?;
        *self = proposed;
        Ok(())
    }

    pub fn ensure_environment(
        &mut self,
        project_id: &str,
        kind: AgentKind,
        defaults: &settings::Settings,
    ) -> Result<AgentEnvironment, String> {
        defaults.validate()?;
        if let Membership::Present(existing) = self.record(project_id)?.environments.membership(kind) {
            return Ok(existing.clone());
        }
        let mut project = self.project(project_id)?;
        project.require_active()?;
        let configuration = match kind {
            AgentKind::Codex => &defaults.codex,
            AgentKind::ClaudeCode => &defaults.claude_code,
        };
        project.environment_id = EnvironmentId::for_agent(project_id, kind);
        project.agent_kind = kind;
        project.profile = configuration.profile.clone();
        project.label = project_id[..16].into();
        project.network = defaults.network.clone();
        project.policy = None;
        project.stage = crate::projects::ProjectStage::Selected;
        project.owns_session = true;
        project.owns_binding = false;
        project.policy_detached = false;
        project.environment_verified = Some(false);
        project.environment_attempt = None;
        project.removal = None;
        project.validate()?;
        let environment = AgentEnvironment::from_project(&project);
        let mut proposed = self.clone();
        proposed
            .projects
            .iter_mut()
            .find(|record| record.id == project_id)
            .ok_or("Project not found.")?
            .environments
            .add(environment.clone())?;
        proposed.validate()?;
        *self = proposed;
        Ok(environment)
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn creation_time(metadata: &fs::Metadata) -> u128 {
    metadata
        .created()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |time| time.as_nanos())
}

// Device numbers can change after a remount. The volume UUID identifies the disk across boots.
#[cfg(target_os = "macos")]
fn volume_uuid(path: &Path) -> Option<[u8; 16]> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: statfs initializes the supplied buffer on success.
    if unsafe { libc::statfs(path.as_ptr(), filesystem.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: statfs returned success and initialized the entire buffer.
    let filesystem = unsafe { filesystem.assume_init() };
    let mut attributes = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: 0,
        volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_UUID,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    #[repr(C)]
    struct VolumeAttributes {
        length: u32,
        uuid: [u8; 16],
    }
    let mut result = VolumeAttributes {
        length: 0,
        uuid: [0; 16],
    };
    // SAFETY: Both pointers refer to initialized buffers with the requested layout and size.
    let status = unsafe {
        libc::getattrlist(
            filesystem.f_mntonname.as_ptr(),
            &mut attributes as *mut libc::attrlist as *mut libc::c_void,
            &mut result as *mut VolumeAttributes as *mut libc::c_void,
            std::mem::size_of::<VolumeAttributes>(),
            0,
        )
    };
    // Some file systems do not provide a persistent UUID. Retain device checks for those volumes.
    (status == 0
        && result.length as usize == std::mem::size_of::<VolumeAttributes>()
        && result.uuid != [0; 16])
        .then_some(result.uuid)
}

#[cfg(not(target_os = "macos"))]
fn volume_uuid(_path: &Path) -> Option<[u8; 16]> {
    None
}
pub fn file(app: &AppHandle) -> Result<PathBuf, String> {
    let records = crate::storage::data_location::records_directory(app)?;
    let file = crate::storage::data_location::directory(app)?.join("projects.json");
    migrate(&file, &records)?;
    Ok(file)
}

pub fn migrate(
    file: &Path,
    records: &crate::storage::data_location::RecordsDirectory,
) -> Result<bool, String> {
    storage::upgrade_record(file, records, |bytes| {
        let decoded = migration::decode(file, bytes)?;
        if let storage::RecordSchema::Upgrade(registry) = &decoded {
            crate::storage::data_location::require_private_records(
                file.parent().ok_or("Invalid project records location.")?,
                registry,
            )?;
        }
        Ok(decoded)
    })
}

pub fn check_folder(selected: &Path) -> Result<PathBuf, String> {
    let path = fs::canonicalize(selected)
        .map_err(|_| "The folder is unavailable. Check the path and select it again.")?;
    if !path.is_dir() || fs::read_dir(&path).is_err() {
        return Err("Select a readable project folder.".into());
    }
    let probe = path.join(format!(
        ".brig-desktop-access-{}-{}",
        std::process::id(),
        NEXT_PROBE.fetch_add(1, Ordering::Relaxed)
    ));
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&probe)
        .map_err(|_| "This folder is not writable. Select a folder with write access.")?;
    let identity = output.metadata().map_err(|error| error.to_string())?;
    drop(output);
    let current = fs::symlink_metadata(&probe).map_err(|error| error.to_string())?;
    if current.ino() != identity.ino() || current.dev() != identity.dev() || !current.is_file() {
        return Err("The probe file changed during the folder access check.".into());
    }
    fs::remove_file(probe).map_err(|error| error.to_string())?;
    Ok(path)
}

#[tauri::command]
pub fn register_project(
    app: AppHandle,
    state: State<'_, Projects>,
    folder: String,
    agent_kind: Option<AgentKind>,
) -> Result<Project, String> {
    let _guard = state
        .0
        .try_lock()
        .map_err(|_| "Project setup is running. Try again shortly.")?;
    let settings = settings::load(&settings::file(&app)?)?;
    let file = file(&app)?;
    let _file_guard = storage::lock(&file)?;
    let mut registry = Registry::load(&file)?;
    let project = registry.register_for_agent(Path::new(&folder), &settings, agent_kind)?;
    crate::storage::data_location::validate_project_records(&app, &registry)?;
    storage::write(&file, &registry)?;
    Ok(project)
}

#[tauri::command]
pub fn project_environments(app: AppHandle, identifier: String) -> Result<Environments, String> {
    Registry::load(&file(&app)?)?.environments(&identifier)
}

#[tauri::command]
pub fn ensure_project_environment(
    app: AppHandle,
    state: State<'_, Projects>,
    identifier: String,
    agent_kind: AgentKind,
) -> Result<Project, String> {
    let _guard = state
        .0
        .try_lock()
        .map_err(|_| "Project setup is running. Try again shortly.")?;
    let records = crate::storage::data_location::records_directory(&app)?;
    let file = file(&app)?;
    let _file_guard = storage::lock(&file)?;
    let defaults = settings::load(&settings::file(&app)?)?;
    let mut registry = Registry::load(&file)?;
    let environment = registry.ensure_environment(&identifier, agent_kind, &defaults)?;
    let project = registry.environment(&identifier, &environment.environment_id)?;
    crate::storage::data_location::validate_project_records(&app, &registry)?;
    records.verify_at(file.parent().ok_or("Invalid project storage location.")?)?;
    storage::write(&file, &registry)?;
    Ok(project)
}

fn opened_at(project: &Project) -> u64 {
    project
        .last_opened_at
        .unwrap_or(project.updated_at.saturating_mul(1000))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    #[serde(flatten)]
    project: Project,
    git_branch: Option<String>,
    available: bool,
}

fn git_branch(project: &Project) -> Option<String> {
    project.check_path().ok()?;
    let git = git_for_metadata(host::discover("git", None)?, selected_developer_tools)?;
    let read = |arguments: &[&str]| {
        command::run(
            &git,
            &arguments
                .iter()
                .map(|argument| (*argument).into())
                .collect::<Vec<_>>(),
            Some(&project.path),
            &command::environment(),
            Duration::from_secs(3),
            &AtomicBool::new(false),
        )
        .ok()
        .filter(|output| {
            output.exit_code == Some(0)
                && !output.timed_out
                && !output.cancelled
                && !output.output_exceeded
        })
        .map(|output| output.stdout.trim().to_owned())
        .filter(|branch| !branch.is_empty())
    };
    if let Some(branch) = read(&[
        "--no-optional-locks",
        "symbolic-ref",
        "--quiet",
        "--short",
        "HEAD",
    ]) {
        return Some(branch);
    }
    read(&["--no-optional-locks", "rev-parse", "--short", "HEAD"])
        .map(|commit| format!("Detached at {commit}"))
}

fn git_for_metadata(
    git: PathBuf,
    developer_tools: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    if git == Path::new("/usr/bin/git")
        && !developer_tools().is_some_and(|directory| directory.is_absolute() && directory.is_dir()) {
        return None;
    }
    Some(git)
}

fn selected_developer_tools() -> Option<PathBuf> {
    let output = command::run(
        Path::new("/usr/bin/xcode-select"),
        &["-p".into()],
        None,
        &command::environment(),
        Duration::from_secs(2),
        &AtomicBool::new(false),
    ).ok()?;
    if output.exit_code != Some(0) || output.cancelled || output.timed_out || output.output_exceeded {
        return None;
    }
    Some(PathBuf::from(output.stdout.trim()))
}

#[tauri::command]
pub async fn recent_projects(app: AppHandle) -> Result<Vec<ProjectSummary>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        Ok(project_summaries(
            Registry::load(&file(&app)?)?.project_views()?,
        ))
    })
    .await
    .map_err(|error| error.to_string())?
}

fn project_summaries(mut projects: Vec<Project>) -> Vec<ProjectSummary> {
    projects.sort_by_key(|project| std::cmp::Reverse(opened_at(project)));
    projects
        .into_iter()
        .map(|project| ProjectSummary {
            available: project.check_path().is_ok(),
            git_branch: git_branch(&project),
            project,
        })
        .collect()
}

#[tauri::command]
pub async fn rescan_projects(
    app: AppHandle,
    state: State<'_, Projects>,
) -> Result<Vec<ProjectSummary>, String> {
    let lock = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let projects = {
            let _guard = lock.lock().map_err(|error| error.to_string())?;
            let file = file(&app)?;
            let _file_guard = storage::lock(&file)?;
            let mut registry = Registry::load(&file)?;
            if registry.rescan_paths() {
                storage::write(&file, &registry)?;
            }
            registry.project_views()?
        };
        Ok(project_summaries(projects))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn forget_project(
    app: AppHandle,
    state: State<'_, Projects>,
    identifier: String,
) -> Result<(), String> {
    let lock = state.0.clone();
    let mut account = app
        .state::<crate::providers::shared_chatgpt::SharedChatgpt>()
        .0
        .clone()
        .lock_owned()
        .await;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = lock.lock().map_err(|error| error.to_string())?;
        if crate::terminal::terminal_sessions(identifier.clone())
            .iter()
            .any(|terminal| terminal.running)
        {
            return Err(
                "End this project's terminal processes before forgetting its record.".into(),
            );
        }
        let file = file(&app)?;
        let _file_guard = storage::lock(&file)?;
        let mut registry = Registry::load(&file)?;
        registry.forget_unavailable(&identifier)?;
        crate::providers::shared_chatgpt::detach_project(&app, &mut account, &identifier, false)?;
        crate::providers::shared_claude::forget_project(&app, &identifier)?;
        storage::write(&file, &registry)?;
        crate::conversations::unobserve(&app, &identifier)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn open_project(
    app: AppHandle,
    state: State<'_, Projects>,
    identifier: String,
) -> Result<Project, String> {
    let _guard = state
        .0
        .try_lock()
        .map_err(|_| "Project setup is running. Try again shortly.")?;
    let file = file(&app)?;
    let _file_guard = storage::lock(&file)?;
    let mut registry = Registry::load(&file)?;
    let opened_at = registry.next_opened_at();
    let mut project = registry.project(&identifier)?;
    project.check_path()?;
    check_folder(&project.path)?;
    project.updated_at = now();
    project.last_opened_at = Some(opened_at);
    registry.update(project.clone())?;
    storage::write(&file, &registry)?;
    Ok(project)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPreferences {
    project: Project,
    options: ProjectOptions,
}
#[tauri::command]
pub fn read_project_preferences(
    app: AppHandle,
    identifier: String,
    environment_id: Option<EnvironmentId>,
) -> Result<ProjectPreferences, String> {
    let project =
        Registry::load(&file(&app)?)?.resolve_environment(&identifier, environment_id.as_ref())?;
    let settings = project.settings(settings::load(&settings::file(&app)?)?)?;
    let options = ProjectOptions {
        cpus: settings.cpus,
        memory_mb: settings.memory_mb,
        verification: settings.verification,
        network: settings.network,
    };
    Ok(ProjectPreferences { project, options })
}
#[tauri::command]
pub async fn save_project_preferences(
    app: AppHandle,
    state: State<'_, Projects>,
    identifier: String,
    environment_id: Option<EnvironmentId>,
    name: String,
    options: ProjectOptions,
) -> Result<Project, String> {
    let lock = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = lock.lock().map_err(|error| error.to_string())?;
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
            return Err("Use a project name with 1 to 120 characters.".into());
        }
        let defaults = settings::load(&settings::file(&app)?)?;
        let mut proposed = defaults.clone();
        options.apply(&mut proposed);
        proposed.validate()?;
        let file = file(&app)?;
        let _file_guard = storage::lock(&file)?;
        let mut registry = Registry::load(&file)?;
        crate::storage::data_location::validate_project_records(&app, &registry)?;
        let mut project = registry.resolve_environment(&identifier, environment_id.as_ref())?;
        project.require_active()?;
        let previous = project.settings(defaults)?;
        let shared_resources = options.cpus != previous.cpus
            || options.memory_mb != previous.memory_mb
            || options.verification != previous.verification;
        if crate::terminal::terminal_sessions(identifier.clone())
            .iter()
            .any(|process| {
                process.running
                    && (shared_resources
                        || process.environment_id.is_none()
                        || process.belongs_to(&project))
            })
        {
            return Err(
                "End the affected environment terminal processes before changing these settings."
                    .into(),
            );
        }
        let brig = crate::environment::brig_for_project(&app, &project, &proposed)?;
        let targets = registry.environments(&identifier)?;
        let sessions = brig.json(&["--json".into(), "ls".into()], "SandboxList", None)?;
        if sessions["data"]["sandboxes"]
            .as_array()
            .ok_or("Could not read Brig environment state.")?
            .iter()
            .any(|session| {
                session["state"] != "stopped"
                    && targets.iter().any(|environment| {
                        session["ref"] == environment.reference()
                            && (shared_resources
                                || environment.environment_id == project.environment_id)
                    })
            })
        {
            return Err(
                "Stop the affected Brig environments before changing these settings.".into(),
            );
        }
        project.set_preferences(name, options, &previous);
        registry.update(project.clone())?;
        storage::write(&file, &registry)?;
        Ok(project)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    #[cfg(target_os = "macos")]
    fn persistent_volume_identity_accepts_a_new_device_number_but_rejects_another_volume() {
        let root = std::env::temp_dir().join(format!("tugboat-volume-{}", std::process::id()));
        fs::create_dir_all(root.join("project")).unwrap();
        let mut registry = Registry::default();
        let mut project = registry.register(&root.join("project"), "codex").unwrap();
        assert!(
            project.volume_uuid.is_some(),
            "the test volume must provide a UUID"
        );
        assert_eq!(project.volume_uuid, volume_uuid(&root));
        project.device = project.device.wrapping_add(1);
        project.check_path().unwrap();
        let metadata = project.path.metadata().unwrap();
        assert!(!project.matches_folder(&metadata, Some([0; 16]), true));
        assert!(!project.matches_folder(&metadata, None, true));
        project.inode = project.inode.wrapping_add(1);
        assert!(
            project.check_path().is_err(),
            "the same disk must not accept a replacement folder"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rescanning_a_legacy_remount_preserves_the_project_record_and_opening_order() {
        let root =
            std::env::temp_dir().join(format!("tugboat-rescan-legacy-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let file = root.join("projects.json");
        let mut registry = Registry::load(&file).unwrap();
        let mut project = registry.register(&root, "codex").unwrap();
        project.name = "Saved project name".into();
        project.stage = crate::projects::ProjectStage::EnvironmentReady;
        project.policy = Some(Policy {
            name: "saved-policy".into(),
            owned: true,
            fingerprint: Some("saved-fingerprint".into()),
        });
        project.volume_uuid = None;
        project.device = project.device.wrapping_add(1);
        registry.update(project.clone()).unwrap();
        storage::write(&file, &registry).unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert!(saved["projects"][0].get("volumeUuid").is_none());
        let mut registry = Registry::load(&file).unwrap();
        assert!(registry.project(&project.id).unwrap().check_path().is_err());
        assert!(registry.rescan_paths());
        let recovered = registry.project(&project.id).unwrap();
        recovered.check_path().unwrap();
        assert_eq!(recovered.device, recovered.path.metadata().unwrap().dev());
        let mut expected = serde_json::to_value(&project).unwrap();
        expected["device"] = serde_json::to_value(recovered.device).unwrap();
        if let Some(volume) = recovered.volume_uuid {
            expected["volumeUuid"] = serde_json::to_value(volume).unwrap();
        }
        assert_eq!(serde_json::to_value(&recovered).unwrap(), expected);
        assert_eq!(registry.projects.len(), 1);
        assert!(
            !registry.rescan_paths(),
            "a second scan must not change the record"
        );
        storage::write(&file, &registry).unwrap();
        Registry::load(&file)
            .unwrap()
            .project(&project.id)
            .unwrap()
            .check_path()
            .unwrap();
        assert_eq!(registry.register(&root, "codex").unwrap().id, project.id);
        assert_eq!(registry.projects.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rescanning_keeps_missing_or_replaced_folders_unavailable_until_the_original_returns() {
        let root =
            std::env::temp_dir().join(format!("tugboat-rescan-missing-{}", std::process::id()));
        let folder = root.join("project");
        fs::create_dir_all(&folder).unwrap();
        let mut registry = Registry::default();
        let mut project = registry.register(&folder, "codex").unwrap();
        project.volume_uuid = None;
        project.device = project.device.wrapping_add(1);
        registry.update(project.clone()).unwrap();
        fs::rename(&folder, root.join("disconnected")).unwrap();
        assert!(!registry.rescan_paths());
        assert!(registry.project(&project.id).unwrap().check_path().is_err());
        fs::create_dir_all(&folder).unwrap();
        let before = serde_json::to_value(&registry).unwrap();
        assert!(!registry.rescan_paths());
        assert_eq!(serde_json::to_value(&registry).unwrap(), before);
        assert!(registry.project(&project.id).unwrap().check_path().is_err());
        fs::remove_dir(&folder).unwrap();
        fs::rename(root.join("disconnected"), &folder).unwrap();
        assert!(registry.rescan_paths());
        registry.project(&project.id).unwrap().check_path().unwrap();
        assert_eq!(registry.projects.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reselecting_a_legacy_remount_reuses_the_record_but_needs_a_known_creation_time() {
        let root =
            std::env::temp_dir().join(format!("tugboat-reselect-remount-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let mut registry = Registry::default();
        let mut project = registry.register(&root, "codex").unwrap();
        let metadata = root.metadata().unwrap();
        project.volume_uuid = None;
        project.device = project.device.wrapping_add(1);
        let mut unknown_creation = project.clone();
        unknown_creation.created = 0;
        assert!(!unknown_creation.matches_folder(&metadata, volume_uuid(&root), true));
        registry.update(project.clone()).unwrap();
        let reopened = registry.register(&root, "codex").unwrap();
        assert_eq!(reopened.id, project.id);
        assert_eq!(reopened.reference(), project.reference());
        assert!(reopened.last_opened_at > project.last_opened_at);
        reopened.check_path().unwrap();
        assert_eq!(registry.projects.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn forgetting_an_unavailable_record_preserves_other_records_and_all_external_files() {
        let root = std::env::temp_dir().join(format!("tugboat-forget-{}", std::process::id()));
        fs::create_dir_all(root.join("project")).unwrap();
        fs::create_dir_all(root.join("other")).unwrap();
        fs::write(root.join("project/source.txt"), "kept").unwrap();
        fs::write(root.join("guest-marker"), "kept").unwrap();
        let mut registry = Registry::load(&root.join("projects.json")).unwrap();
        let first = registry.register(&root.join("project"), "codex").unwrap();
        let other = registry.register(&root.join("other"), "codex").unwrap();
        assert!(registry.forget_unavailable(&first.id).is_err());
        fs::rename(root.join("project"), root.join("moved")).unwrap();
        assert_eq!(
            registry.projects.len(),
            2,
            "unavailability alone must not remove a record"
        );
        registry.forget_unavailable(&first.id).unwrap();
        assert_eq!(registry.projects.len(), 1);
        assert_eq!(registry.projects[0].id, other.id);
        assert_eq!(
            fs::read_to_string(root.join("moved/source.txt")).unwrap(),
            "kept"
        );
        assert_eq!(
            fs::read_to_string(root.join("guest-marker")).unwrap(),
            "kept"
        );
        assert!(registry.forget_unavailable(&first.id).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn registry_writer_process() {
        let Some(root) = std::env::var_os("TUGBOAT_REGISTRY_TEST_ROOT") else {
            return;
        };
        let root = PathBuf::from(root);
        let selected = root.join(std::env::var("TUGBOAT_REGISTRY_TEST_FOLDER").unwrap());
        let file = root.join("projects.json");
        let _guard = storage::lock(&file).unwrap();
        let mut registry = Registry::load(&file).unwrap();
        // Both writers would read the same snapshot without the interprocess lock.
        std::thread::sleep(Duration::from_millis(100));
        registry.register(&selected, "codex").unwrap();
        storage::write(&file, &registry).unwrap();
    }

    #[test]
    fn independent_processes_preserve_both_project_registrations() {
        let root =
            std::env::temp_dir().join(format!("tugboat-registry-processes-{}", std::process::id()));
        fs::create_dir_all(root.join("first")).unwrap();
        fs::create_dir_all(root.join("second")).unwrap();
        let mut children = Vec::new();
        for folder in ["first", "second"] {
            children.push(
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "projects::tests::registry_writer_process",
                        "--nocapture",
                    ])
                    .env("TUGBOAT_REGISTRY_TEST_ROOT", &root)
                    .env("TUGBOAT_REGISTRY_TEST_FOLDER", folder)
                    .spawn()
                    .unwrap(),
            );
        }
        for child in &mut children {
            assert!(child.wait().unwrap().success());
        }
        let registry = Registry::load(&root.join("projects.json")).unwrap();
        assert_eq!(registry.projects.len(), 2);
        assert!(registry
            .projects
            .iter()
            .any(|project| project.name == "first"));
        assert!(registry
            .projects
            .iter()
            .any(|project| project.name == "second"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reselecting_or_moving_a_folder_preserves_its_saved_name_and_refreshes_recency() {
        let root =
            std::env::temp_dir().join(format!("tugboat-project-name-{}", std::process::id()));
        fs::create_dir_all(root.join("original")).unwrap();
        let file = root.join("registry.json");
        let mut registry = Registry::load(&file).unwrap();
        let mut project = registry.register(&root.join("original"), "codex").unwrap();
        let first_opened = project.last_opened_at;
        project.name = "Customer billing".into();
        registry.update(project.clone()).unwrap();
        let mut legacy = migration_tests::legacy_registry(&registry);
        legacy["projects"][0]
            .as_object_mut()
            .unwrap()
            .remove("lastOpenedAt");
        storage::write(&file, &legacy).unwrap();
        let mut registry = Registry::load(&file).unwrap();
        let reopened = registry.register(&root.join("original"), "codex").unwrap();
        assert_eq!(reopened.name, "Customer billing");
        assert_eq!(reopened.id, project.id);
        assert!(reopened.last_opened_at > first_opened);
        fs::rename(root.join("original"), root.join("moved")).unwrap();
        let moved = registry.register(&root.join("moved"), "codex").unwrap();
        assert_eq!(moved.name, "Customer billing");
        assert_eq!(moved.id, project.id);
        assert_eq!(moved.path, fs::canonicalize(root.join("moved")).unwrap());
        assert!(moved.last_opened_at > reopened.last_opened_at);
        assert_eq!(registry.projects.len(), 1);
        storage::write(&file, &registry).unwrap();
        assert_eq!(
            Registry::load(&file)
                .unwrap()
                .project(&project.id)
                .unwrap()
                .name,
            "Customer billing"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_resource_options_are_validated_and_do_not_replace_global_preferences() {
        let root =
            std::env::temp_dir().join(format!("tugboat-project-options-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let mut registry = Registry::default();
        let mut project = registry.register(&root, "codex").unwrap();
        let defaults = settings::Settings::default();
        project.options = Some(ProjectOptions {
            cpus: 2,
            memory_mb: 2048,
            verification: settings::Verification::Warn,
            network: settings::Network::Offline,
        });
        let effective = project.settings(defaults.clone()).unwrap();
        assert_eq!(effective.cpus, 2);
        assert_eq!(effective.memory_mb, 2048);
        assert_eq!(effective.network, settings::Network::Offline);
        assert_eq!(defaults.cpus, 4);
        assert_eq!(defaults.network, settings::Network::Isolated);
        project.options.as_mut().unwrap().cpus = 0;
        assert!(project.validate().is_err());
        assert!(project.settings(defaults).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn environment_proof_survives_rename_and_expires_when_resources_or_plan_change() {
        let mut registry = Registry::default();
        let mut project = registry.register(&std::env::temp_dir(), "codex").unwrap();
        project.stage = crate::projects::ProjectStage::EnvironmentReady;
        project.environment_verified = Some(true);
        project.environment_attempt = Some(EnvironmentAttempt {
            plan_fingerprint: Some("p1".into()),
            token: "attempt".into(),
            terminal_identifier: 1,
            completed: true,
            error: None,
        });
        let defaults = settings::Settings::default();
        let mut options = ProjectOptions {
            cpus: defaults.cpus,
            memory_mb: defaults.memory_mb,
            network: defaults.network.clone(),
            verification: defaults.verification.clone(),
        };
        project.set_preferences("Renamed", options.clone(), &defaults);
        assert_eq!(project.environment_stage(true, "p1"), "environment-ready");
        assert_eq!(project.environment_stage(true, "p2"), "ready");
        options.cpus += 1;
        project.set_preferences("Renamed", options, &defaults);
        assert_eq!(project.environment_stage(true, "p1"), "ready");
        assert!(project.environment_attempt.is_none());
    }

    #[test]
    fn opening_order_is_independent_of_setup_updates_and_migrates_older_records() {
        let root =
            std::env::temp_dir().join(format!("tugboat-project-recency-{}", std::process::id()));
        fs::create_dir_all(root.join("first")).unwrap();
        fs::create_dir_all(root.join("second")).unwrap();
        let mut registry = Registry::load(&root.join("registry.json")).unwrap();
        let mut first = registry.register(&root.join("first"), "codex").unwrap();
        let second = registry.register(&root.join("second"), "codex").unwrap();
        assert!(opened_at(&second) > opened_at(&first));
        first.updated_at = now() + 86_400;
        first.stage = crate::projects::ProjectStage::EnvironmentReady;
        registry.update(first.clone()).unwrap();
        assert!(opened_at(&second) > opened_at(&registry.project(&first.id).unwrap()));
        let reopened = registry.register(&root.join("first"), "codex").unwrap();
        assert!(opened_at(&reopened) > opened_at(&second));
        let mut legacy = migration_tests::legacy_registry(&registry);
        for project in legacy["projects"].as_array_mut().unwrap() {
            project.as_object_mut().unwrap().remove("lastOpenedAt");
        }
        storage::write(&root.join("registry.json"), &legacy).unwrap();
        let migrated = Registry::load(&root.join("registry.json")).unwrap();
        assert!(migrated
            .projects
            .iter()
            .all(|project| project.last_opened_at == Some(project.updated_at * 1000)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn branch_metadata_reads_the_actual_repository_and_handles_non_git_projects() {
        let root =
            std::env::temp_dir().join(format!("tugboat-project-branch-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let mut registry = Registry::load(&root.join("registry.json")).unwrap();
        let project = registry.register(&root, "codex").unwrap();
        assert_eq!(git_branch(&project), None);
        let git = host::discover("git", None).and_then(|git| git_for_metadata(git, selected_developer_tools));
        let Some(git) = git else {
            fs::remove_dir_all(root).unwrap();
            return;
        };
        let status = std::process::Command::new(git)
            .args(["init", "--template=", "--initial-branch=feature/sidebar"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(status.status.success());
        assert_eq!(git_branch(&project).as_deref(), Some("feature/sidebar"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn branch_metadata_skips_the_system_git_shim_without_selected_developer_tools() {
        let system_git = PathBuf::from("/usr/bin/git");
        assert!(git_for_metadata(system_git.clone(), || None).is_none());
        assert!(git_for_metadata(system_git.clone(), || Some("relative/Developer".into())).is_none());
        assert!(git_for_metadata(system_git.clone(), || Some("/missing/tugboat/Developer".into())).is_none());
        let root = std::env::temp_dir().join(format!("tugboat-developer-tools-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        assert_eq!(git_for_metadata(system_git.clone(), || Some(root.clone())), Some(system_git));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn branch_metadata_keeps_custom_git_without_checking_apple_developer_tools() {
        for executable in ["/opt/homebrew/bin/git", "/usr/local/bin/git", "/fixture/custom/git"] {
            let git = PathBuf::from(executable);
            assert_eq!(git_for_metadata(git.clone(), || panic!("Custom Git must not query xcode-select")), Some(git));
        }
    }

    #[test]
    fn spaced_project_names_keep_the_original_folder_and_identity() {
        let root =
            std::env::temp_dir().join(format!("brig-spaced-projects-{}", std::process::id()));
        let folder = root.join("parent folder/sample project");
        fs::create_dir_all(&folder).unwrap();
        let mut registry = Registry {
            schema_version: 2,
            projects: Vec::new(),
        };
        let project = registry.register(&folder, "codex").unwrap();
        assert_eq!(project.path, fs::canonicalize(&folder).unwrap());
        assert_eq!(project.name, "sample project");
        project.check_guest_path().unwrap();
        assert_eq!(registry.register(&folder, "codex").unwrap().id, project.id);
        for name in ["tab\tname", "line\nname", "quote'name"] {
            assert!(validate_guest_path(name).is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn symlinks_and_same_names_have_stable_distinct_project_records() {
        let root = std::env::temp_dir().join(format!("brig-projects-{}", std::process::id()));
        fs::create_dir_all(root.join("one/project")).unwrap();
        fs::create_dir_all(root.join("two/project")).unwrap();
        symlink(root.join("one/project"), root.join("alias")).unwrap();
        let mut registry = Registry {
            schema_version: 2,
            projects: Vec::new(),
        };
        let first = registry
            .register(&root.join("one/project"), "codex")
            .unwrap();
        let alias = registry.register(&root.join("alias"), "codex").unwrap();
        let second = registry
            .register(&root.join("two/project"), "codex")
            .unwrap();
        assert_eq!(first.id, alias.id);
        assert_ne!(first.label, second.label);
        assert_eq!(registry.projects.len(), 2);
        fs::rename(root.join("one/project"), root.join("moved")).unwrap();
        assert_eq!(
            registry.register(&root.join("moved"), "codex").unwrap().id,
            first.id
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_and_read_only_folders_are_rejected() {
        let root = std::env::temp_dir().join(format!("brig-readonly-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        assert!(check_folder(&root.join("missing")).is_err());
        fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).unwrap();
        let readonly = check_folder(&root);
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_dir_all(root).unwrap();
        assert!(readonly.is_err());
    }

    #[test]
    fn long_project_names_keep_hvi_socket_names_short() {
        let root = std::env::temp_dir().join(format!("brig-long-project-{}", std::process::id()));
        let folder = root.join("a-very-long-project-name-that-must-not-lengthen-the-hvi-socket");
        fs::create_dir_all(&folder).unwrap();
        let mut registry = Registry {
            schema_version: 2,
            projects: Vec::new(),
        };
        let project = registry.register(&folder, "codex").unwrap();
        assert_eq!(project.name, folder.file_name().unwrap().to_str().unwrap());
        assert_eq!(project.label.len(), 19);
        let socket = format!(
            "/private/tmp/brig-bd15-mCHq6N/home/.hull/store/instances/brig-codex-{}/qmp.sock",
            project.label
        );
        assert!(socket.len() <= 103);
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
pub(crate) fn forget_fixture_project(registry: &mut Registry, id: &str) -> Result<(), String> {
    registry.forget_unavailable(id)
}


#[cfg(test)]
mod transition_review {
    use super::*;
    #[test]
    fn recheck_reloaded_removal_cannot_skip_environment_and_binding_checkpoints() {
        let root = std::env::temp_dir().join(format!("tugboat-removal-review-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("projects.json");
        let mut registry = Registry::default();
        let mut project = registry.register(&root, "codex").unwrap();
        project.stage = ProjectStage::Removing;
        project.removal = Some(Removal { reference:project.reference(), workspace:root.to_string_lossy().into(),
            had_environment:true,logout:false,logged_out:false,stopped:true,environment_removed:true,
            binding_detached:false, policy_handled:false,preserved_policy:false });
        registry.update(project).unwrap();
        let mut saved = serde_json::to_value(&registry).unwrap();
        assert_eq!(saved["projects"][0]["environments"][0]["removal"]["bindingDetached"], false);
        saved["projects"][0]["environments"][0]["removal"]["policyHandled"] = true.into();
        crate::storage::write(&file, &saved).unwrap();
        let loaded = Registry::load(&file);
        std::fs::remove_dir_all(root).unwrap();
        if let Ok(registry) = loaded {
            let mut removal = registry.projects[0].codex_project().unwrap().removal.unwrap();
            removal.complete(RemovalStep::Binding).unwrap();
            assert_ne!(removal.next_step(), RemovalStep::Record, "policy cleanup was skipped");
        }
    }
}
