use crate::{command, host, settings, storage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, State};

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
    inode: u64,
    created: u128,
    pub profile: String,
    pub label: String,
    pub policy: Option<Policy>,
    pub stage: String,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentAttempt {
    pub token: String,
    pub terminal_identifier: u64,
    pub completed: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    pub fn removal_pending(&self) -> bool {
        self.stage == "removing" || self.removal.is_some()
    }
    pub fn require_active(&self) -> Result<(), String> {
        if self.removal_pending() {
            return Err("Removal is incomplete. Review the removal preview to continue before using this project.".into());
        }
        Ok(())
    }
    pub fn settings(&self, mut defaults: settings::Settings) -> Result<settings::Settings, String> {
        if let Some(options) = &self.options {
            options.apply(&mut defaults);
        }
        defaults.validate()?;
        Ok(defaults)
    }
    pub fn reference(&self) -> String {
        format!("{}@{}", self.profile, self.label)
    }
    pub fn validate(&self) -> Result<(), String> {
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
        let metadata = self
            .path
            .metadata()
            .map_err(|_| "The project moved or is unavailable. Select the folder again.")?;
        if metadata.dev() != self.device
            || metadata.ino() != self.inode
            || creation_time(&metadata) != self.created
        {
            return Err(
                "The folder at this path changed. Select it again to create a new record.".into(),
            );
        }
        Ok(())
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

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Registry {
    schema_version: u32,
    pub projects: Vec<Project>,
}

impl Registry {
    pub fn load(file: &Path) -> Result<Self, String> {
        let mut registry = storage::read::<Self>(file)?.unwrap_or(Self {
            schema_version: 1,
            projects: Vec::new(),
        });
        if registry.schema_version != 1 {
            return Err("This project registry version is unsupported.".into());
        }
        for project in &mut registry.projects {
            project.validate()?;
            project.policy_detached |= project.stage == "policy-detached";
            if project.removal.is_some() {
                project.stage = "removing".into();
            }
            project
                .environment_verified
                .get_or_insert(project.stage == "environment-ready");
            project
                .last_opened_at
                .get_or_insert(project.updated_at.saturating_mul(1000));
        }
        let mut identities = std::collections::HashSet::new();
        let mut references = std::collections::HashSet::new();
        for project in &registry.projects {
            if !identities.insert(&project.id) || !references.insert(project.reference()) {
                return Err("Duplicate project record found.".into());
            }
        }
        Ok(registry)
    }

    pub fn register(&mut self, selected: &Path, profile: &str) -> Result<Project, String> {
        let path = check_folder(selected)?;
        let metadata = path.metadata().map_err(|error| error.to_string())?;
        let created = creation_time(&metadata);
        let opened_at = self.next_opened_at();
        let previous = self.projects.iter_mut().find(|project| {
            project.device == metadata.dev()
                && project.inode == metadata.ino()
                && project.created == created
                && (project.path == path || !project.path.exists())
        });
        if let Some(project) = previous {
            project.path = path;
            project.updated_at = now();
            project.last_opened_at = Some(opened_at);
            return Ok(project.clone());
        }
        let mut identity = Sha256::new();
        identity.update(path.as_os_str().as_encoded_bytes());
        identity.update(metadata.dev().to_le_bytes());
        identity.update(metadata.ino().to_le_bytes());
        identity.update(created.to_le_bytes());
        let id = format!("{:x}", identity.finalize())[..32].to_owned();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let project = Project {
            // hull places QMP sockets below the instance name; Darwin limits their path to 103 bytes.
            label: format!("bd-{}", &id[..16]),
            id,
            name,
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
            created,
            profile: profile.into(),
            policy: None,
            stage: "selected".into(),
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
        self.projects.push(project.clone());
        Ok(project)
    }

    pub fn project(&self, id: &str) -> Result<Project, String> {
        self.projects
            .iter()
            .find(|project| project.id == id)
            .cloned()
            .ok_or("Project record not found. Select the folder again.".into())
    }
    fn forget_unavailable(&mut self, id: &str) -> Result<(), String> {
        let project = self.project(id)?;
        if project.check_path().is_ok() {
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
        let previous = self.projects.iter().map(opened_at).max().unwrap_or(0);
        current.max(previous.saturating_add(1))
    }
    pub fn update(&mut self, project: Project) -> Result<(), String> {
        project.validate()?;
        if self
            .projects
            .iter()
            .any(|other| other.id != project.id && other.reference() == project.reference())
        {
            return Err("This Brig session belongs to another project.".into());
        }
        let current = self
            .projects
            .iter_mut()
            .find(|current| current.id == project.id)
            .ok_or("Project not found.")?;
        *current = project;
        Ok(())
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
pub fn file(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_config_dir()
        .map_err(|error| error.to_string())?
        .join("projects.json"))
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
) -> Result<Project, String> {
    let _guard = state
        .0
        .try_lock()
        .map_err(|_| "Project setup is running. Try again shortly.")?;
    let settings = settings::load(&settings::file(&app)?)?;
    let file = file(&app)?;
    let _file_guard = storage::lock(&file)?;
    let mut registry = Registry::load(&file)?;
    let project = registry.register(Path::new(&folder), &settings.profile)?;
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
    let git = host::discover("git", None)?;
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

#[tauri::command]
pub async fn recent_projects(app: AppHandle) -> Result<Vec<ProjectSummary>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut projects = Registry::load(&file(&app)?)?.projects;
        projects.sort_by_key(|project| std::cmp::Reverse(opened_at(project)));
        Ok(projects
            .into_iter()
            .map(|project| ProjectSummary {
                available: project.check_path().is_ok(),
                git_branch: git_branch(&project),
                project,
            })
            .collect())
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
) -> Result<ProjectPreferences, String> {
    let project = Registry::load(&file(&app)?)?.project(&identifier)?;
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
        let mut defaults = settings::load(&settings::file(&app)?)?;
        options.apply(&mut defaults);
        defaults.validate()?;
        let file = file(&app)?;
        let _file_guard = storage::lock(&file)?;
        let mut registry = Registry::load(&file)?;
        let mut project = registry.project(&identifier)?;
        project.require_active()?;
        if crate::terminal::status(&identifier).is_some_and(|status| status.running) {
            return Err("End this project's terminal process before changing its settings.".into());
        }
        let brig = crate::brig::Brig::new(&defaults)?;
        let sessions = brig.json(&["--json".into(), "ls".into()], "SandboxList", None)?;
        if sessions["data"]["sandboxes"]
            .as_array()
            .ok_or("Could not read Brig environment state.")?
            .iter()
            .any(|session| session["ref"] == project.reference() && session["state"] == "running")
        {
            return Err(
                "Stop this project's Brig environment before changing its settings.".into(),
            );
        }
        project.name = name.into();
        project.options = Some(options);
        project.updated_at = now();
        if project.stage == "environment-ready" {
            project.stage = "ready".into();
        }
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
        let mut legacy = serde_json::to_value(&registry).unwrap();
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
        first.stage = "environment-ready".into();
        registry.update(first.clone()).unwrap();
        assert!(opened_at(&second) > opened_at(&registry.project(&first.id).unwrap()));
        let reopened = registry.register(&root.join("first"), "codex").unwrap();
        assert!(opened_at(&reopened) > opened_at(&second));
        let mut legacy = serde_json::to_value(&registry).unwrap();
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
        let status = std::process::Command::new(host::discover("git", None).unwrap())
            .args(["init", "--template=", "--initial-branch=feature/sidebar"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(status.status.success());
        assert_eq!(git_branch(&project).as_deref(), Some("feature/sidebar"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn spaced_project_names_keep_the_original_folder_and_identity() {
        let root =
            std::env::temp_dir().join(format!("brig-spaced-projects-{}", std::process::id()));
        let folder = root.join("parent folder/sample project");
        fs::create_dir_all(&folder).unwrap();
        let mut registry = Registry {
            schema_version: 1,
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
            schema_version: 1,
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
            schema_version: 1,
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
