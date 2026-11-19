use crate::{
    agents::AgentKind,
    desktop::settings,
    environment::{EnvironmentOperation, GuestHomes},
    platform::brig::Brig,
    projects::{
        transition::{self, RetiredRegistration, Transition},
        EnvironmentId, Project, Registry,
    },
    storage::{
        self,
        data_location::{self, RecordsDirectory},
    },
    usage_store,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

mod target;
use target::{EntryKind, Identity, Target};
#[cfg(test)]
mod tests;

const CHECKPOINT: &str = "project-reset.json";
const DATABASES: [&str; 2] = ["usage.sqlite3", "usage.sqlite3.importing"];

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    dataset: String,
    records_identity: Identity,
    source: String,
    replacement: String,
    backup: Option<Backup>,
    databases: [Option<Target>; 2],
    projects: Vec<RetiredHistory>,
    ready_to_publish: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Backup {
    file: Target,
    source: String,
    replacement: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RetiredHistory {
    project: String,
    environments: Vec<RetiredEnvironment>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RetiredEnvironment {
    id: EnvironmentId,
    reference: String,
    ledger: Option<Target>,
    backup: Option<Target>,
    home: Option<RetiredHome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_home: Option<PathBuf>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RetiredHome {
    sandbox: String,
    target: Target,
}

pub(crate) fn finish_startup(
    records: &RecordsDirectory,
    defaults: &settings::Settings,
) -> Result<(), String> {
    let mut defaults = defaults.clone();
    defaults.cli_path = None;
    activate(
        records,
        &defaults,
        || {
            let mut brig = Brig::new(&defaults)?;
            let profiles = records.path().join("profiles");
            if profiles.is_dir() {
                if fs::canonicalize(&profiles).ok().as_deref() != Some(&profiles) {
                    return Err("The saved Brig profile path changed. Review the records folder before resetting the projects.".into());
                }
                brig.environment.insert(
                    "BRIG_PROFILE_DIR".into(),
                    profiles.to_string_lossy().into_owned(),
                );
            }
            Ok(brig)
        },
        || {
            if crate::terminal::any_running() {
                return Err(
                    "End active project terminals before resetting the former environments.".into(),
                );
            }
            Ok(())
        },
    )
}

pub(crate) fn activate(
    records: &RecordsDirectory,
    defaults: &settings::Settings,
    brig: impl FnOnce() -> Result<Brig, String>,
    require_idle: impl Fn() -> Result<(), String>,
) -> Result<(), String> {
    records.verify()?;
    let registry_file = records.path().join("projects.json");
    let checkpoint_file = records.path().join(CHECKPOINT);
    let backup_file = storage::v1_backup_path(&registry_file);
    let _writer = storage::lock(&registry_file)?;
    let saved = storage::read_regular_bytes(&checkpoint_file)?
        .map(|bytes| {
            serde_json::from_slice::<Checkpoint>(&bytes)
                .map_err(|_| "Could not read the project reset checkpoint.")
        })
        .transpose()?;
    let Some(source) = storage::read_regular_bytes(&registry_file)? else {
        if saved.is_some() {
            return Err("The registry for this saved reset is unavailable.".into());
        }
        return Ok(());
    };
    let source_hash = digest(&source);
    let directory = Identity::folder(records.path())?;
    if let Some(checkpoint) = &saved {
        if checkpoint.dataset != records.id() || checkpoint.records_identity != directory {
            return Err("The project reset belongs to a different records folder.".into());
        }
        if checkpoint.replacement == source_hash
            || (checkpoint.ready_to_publish && Registry::load(&registry_file).is_ok())
        {
            Registry::load(&registry_file)?;
            return erase_checkpoint(&checkpoint_file);
        }
        if checkpoint.source != source_hash {
            return Err("The project registrations changed during reset. Review the original records before starting again.".into());
        }
    }
    let Some(transition) = transition::plan(&source, defaults)? else {
        return Ok(());
    };
    data_location::require_private_records(records.path(), &transition.registry)?;
    for group in &transition.retired {
        transition
            .registry
            .project(&group.project_id)?
            .check_path()?;
    }
    require_idle()?;
    let replacement = serde_json::to_vec_pretty(&transition.registry)
        .map_err(|_| "Could not write the shared project registrations.")?;
    let target_digest = digest(&replacement);
    let brig = brig()?;
    let mut checkpoint = if let Some(checkpoint) = saved {
        if checkpoint.replacement != target_digest {
            return Err("The fresh project registrations changed during reset. Review the same settings and retry.".into());
        }
        checkpoint
    } else {
        let homes = GuestHomes::read(&brig)?;
        let projects = transition
            .retired
            .iter()
            .map(|group| RetiredHistory::capture(records, &transition.registry, group, &homes))
            .collect::<Result<_, _>>()?;
        let checkpoint = Checkpoint {
            dataset: records.id().into(),
            records_identity: directory,
            source: source_hash,
            replacement: target_digest,
            backup: Backup::capture(&backup_file, &transition, defaults)?,
            databases: [
                Target::capture(&records.path().join(DATABASES[0]), EntryKind::Database)?,
                Target::capture(&records.path().join(DATABASES[1]), EntryKind::Database)?,
            ],
            projects,
            ready_to_publish: false,
        };
        storage::write(&checkpoint_file, &checkpoint)?;
        checkpoint
    };
    if checkpoint.projects.len() != transition.retired.len() {
        return Err("The project reset checkpoint is missing registrations.".into());
    }
    verify_databases(records, &checkpoint.databases)?;
    for (plan, group) in checkpoint.projects.iter().zip(&transition.retired) {
        plan.apply(records, &transition.registry, group, &brig, &require_idle)?;
    }
    let retired = transition
        .retired
        .into_iter()
        .flat_map(|group| group.environments)
        .collect::<Vec<_>>();
    verify_databases(records, &checkpoint.databases)?;
    usage_store::retire_databases(records, &retired)?;
    let provider_homes = checkpoint
        .projects
        .iter()
        .flat_map(|plan| {
            plan.environments.iter().filter_map(|environment| {
                environment
                    .provider_home
                    .as_deref()
                    .map(|path| (plan.project.as_str(), path))
            })
        })
        .collect::<Vec<_>>();
    crate::providers::shared_chatgpt::retire_bindings(records, &retired, &provider_homes)?;
    crate::providers::shared_claude::retire_bindings(records, &retired)?;
    match &checkpoint.backup {
        Some(backup) => backup.apply(&backup_file, &transition.registry, defaults)?,
        None => Target::verify_absence(&backup_file)?,
    }
    require_idle()?;
    records.verify()?;
    if Identity::folder(records.path())? != checkpoint.records_identity
        || storage::read_regular_bytes(&registry_file)?
            .map(|source| digest(&source))
            .as_deref()
            != Some(checkpoint.source.as_str())
    {
        return Err(
            "The records changed before the reset could publish the fresh registrations.".into(),
        );
    }
    checkpoint.ready_to_publish = true;
    storage::write(&checkpoint_file, &checkpoint)?;
    storage::write_bytes_existing(&registry_file, &replacement)?;
    erase_checkpoint(&checkpoint_file)
}

impl RetiredHistory {
    fn capture(
        records: &RecordsDirectory,
        registry: &Registry,
        group: &RetiredRegistration,
        homes: &GuestHomes,
    ) -> Result<Self, String> {
        let mut environments = Vec::new();
        for project in &group.environments {
            let ledger = ledger_path(project, records.path());
            check_worktree_paths(registry, &ledger)?;
            let home = homes
                .owned_sandbox(project)?
                .map(|sandbox| {
                    let path = homes.directory.join(&sandbox);
                    check_protected_paths(records, registry, &path)?;
                    homes.protect_shared_home(&project.reference(), &path)?;
                    Target::capture(&path, EntryKind::Folder)?
                        .map(|target| RetiredHome { sandbox, target })
                        .ok_or_else(|| "The owned Brig guest home is unavailable.".to_owned())
                })
                .transpose()?;
            environments.push(RetiredEnvironment {
                id: project.environment_id.clone(),
                reference: project.reference(),
                ledger: Target::capture(&ledger, EntryKind::File)?,
                backup: Target::capture(&storage::v1_backup_path(&ledger), EntryKind::File)?,
                home,
                provider_home: (project.agent_kind == AgentKind::Codex)
                    .then(|| homes.registered_home(project))
                    .flatten(),
            });
        }
        Ok(Self {
            project: group.project_id.clone(),
            environments,
        })
    }

    fn apply(
        &self,
        records: &RecordsDirectory,
        registry: &Registry,
        group: &RetiredRegistration,
        brig: &Brig,
        require_idle: impl Fn() -> Result<(), String>,
    ) -> Result<(), String> {
        if self.project != group.project_id
            || group.environment_id != EnvironmentId::for_project(&self.project)
            || !group
                .environments
                .iter()
                .map(|project| (&project.environment_id, project.reference()))
                .eq(self
                    .environments
                    .iter()
                    .map(|target| (&target.id, target.reference.clone())))
        {
            return Err("The project's retired environments changed after review.".into());
        }
        let _current = EnvironmentOperation::begin(&self.project, &group.environment_id)?;
        let _retired = group
            .environments
            .iter()
            .map(|project| EnvironmentOperation::begin(&project.id, &project.environment_id))
            .collect::<Result<Vec<_>, _>>()?;
        records.verify()?;
        require_idle()?;
        registry.project(&self.project)?.check_path()?;
        let homes = GuestHomes::read(brig)?;
        let mut files = Vec::new();
        let mut guests = Vec::new();
        let mut locks = Vec::new();
        for (project, environment) in group.environments.iter().zip(&self.environments) {
            let ledger = ledger_path(project, records.path());
            if let Some(target) = environment.ledger.as_ref().or(environment.backup.as_ref()) {
                target.verify_parent(&ledger)?;
                locks.push(storage::lock(&ledger)?);
            }
            let backup = storage::v1_backup_path(&ledger);
            for (path, target) in [(ledger, &environment.ledger), (backup, &environment.backup)] {
                check_worktree_paths(registry, &path)?;
                match target {
                    Some(target) => {
                        target.verify(&path, EntryKind::File)?;
                        files.push((target, path));
                    }
                    None => Target::verify_absence(&path)?,
                }
            }
            if let Some(home) = &environment.home {
                let path = checked_home_path(&homes, &home.sandbox)?;
                homes.verify_retired_home(project, &home.sandbox)?;
                check_protected_paths(records, registry, &path)?;
                home.target.verify(&path, EntryKind::Folder)?;
                guests.push((project, home, path));
            } else if homes.owned_sandbox(project)?.is_some() {
                return Err(
                    "A retired guest home appeared after review. Review the reset again.".into(),
                );
            }
        }
        for (project, home, path) in guests {
            require_idle()?;
            records.verify()?;
            remove_guest(brig, project, home, &path)?;
            home.target.erase(&path, EntryKind::Folder)?;
        }
        for (target, path) in files {
            require_idle()?;
            records.verify()?;
            target.erase(&path, EntryKind::File)?;
        }
        Ok(())
    }
}

impl Backup {
    fn capture(
        path: &Path,
        transition: &Transition,
        defaults: &settings::Settings,
    ) -> Result<Option<Self>, String> {
        let Some(file) = Target::capture(path, EntryKind::File)? else {
            return Ok(None);
        };
        let bytes = storage::read_regular_bytes(path)?
            .ok_or("The former registry backup changed during review.")?;
        let replacement = retained_backup(&bytes, &transition.registry, defaults)?;
        Ok(Some(Self {
            file,
            source: digest(&bytes),
            replacement: replacement.as_deref().map(digest),
        }))
    }

    fn apply(
        &self,
        path: &Path,
        registry: &Registry,
        defaults: &settings::Settings,
    ) -> Result<(), String> {
        let bytes = storage::read_regular_bytes(path)?;
        if bytes.as_deref().map(digest) == self.replacement {
            return Ok(());
        }
        let bytes = bytes.ok_or("The former registry backup is unavailable.")?;
        if digest(&bytes) != self.source || self.file.verify(path, EntryKind::File)?.is_none() {
            return Err(
                "The former registry backup changed after review. Its contents were preserved."
                    .into(),
            );
        }
        let remaining = retained_backup(&bytes, registry, defaults)?;
        if remaining.as_deref().map(digest) != self.replacement {
            return Err("The reviewed former registry backup scope changed.".into());
        }
        match remaining {
            Some(bytes) => storage::write_bytes_existing(path, &bytes),
            None => self.file.erase(path, EntryKind::File),
        }
    }
}

fn retained_backup(
    bytes: &[u8],
    registry: &Registry,
    defaults: &settings::Settings,
) -> Result<Option<Vec<u8>>, String> {
    if transition::plan(bytes, defaults)?.is_none() {
        return Err(
            "The former registry backup contains new model data. Its contents were preserved."
                .into(),
        );
    }
    let mut value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "Could not read the former registry backup.")?;
    let rows = value["projects"]
        .as_array_mut()
        .ok_or("The former registry backup is missing its projects.")?;
    let count = rows.len();
    rows.retain(|row| {
        !registry
            .projects
            .iter()
            .any(|project| row["id"] == project.id)
    });
    if rows.is_empty() {
        return Ok(None);
    }
    if rows.len() == count {
        return Ok(Some(bytes.to_vec()));
    }
    serde_json::to_vec_pretty(&value)
        .map(Some)
        .map_err(|_| "Could not retain unrelated former registration records.".into())
}

fn ledger_path(project: &Project, records: &Path) -> PathBuf {
    let directory = records.join("conversation-ledgers");
    match project.agent_kind {
        AgentKind::Codex => directory.join(format!("{}.json", project.id)),
        AgentKind::ClaudeCode => directory
            .join(&project.id)
            .join(format!("{}.json", project.environment_id)),
    }
}

fn verify_databases(
    records: &RecordsDirectory,
    targets: &[Option<Target>; 2],
) -> Result<(), String> {
    for (index, (name, target)) in DATABASES.into_iter().zip(targets).enumerate() {
        let path = records.path().join(name);
        match target {
            Some(target) => {
                if target.verify(&path, EntryKind::Database)?.is_none() && index == 0 {
                    return Err("The reviewed usage database is unavailable. Reconnect its original records folder.".into());
                }
            }
            None => Target::verify_absence(&path)?,
        }
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    crate::hash::hex(Sha256::digest(bytes))
}

fn erase_checkpoint(path: &Path) -> Result<(), String> {
    if let Some(target) = Target::capture(path, EntryKind::File)? {
        return target.erase(path, EntryKind::File);
    }
    Ok(())
}

fn checked_home_path(homes: &GuestHomes, sandbox: &str) -> Result<PathBuf, String> {
    if !sandbox.starts_with("brig-") || !settings::valid_identifier(sandbox) {
        return Err("The retired Brig sandbox name is invalid.".into());
    }
    Ok(homes.directory.join(sandbox))
}

fn check_protected_paths(
    records: &RecordsDirectory,
    registry: &Registry,
    home: &Path,
) -> Result<(), String> {
    if home.starts_with(records.path()) || records.path().starts_with(home) {
        return Err("The retired guest home overlaps an app records or working folder.".into());
    }
    check_worktree_paths(registry, home)
}

fn check_worktree_paths(registry: &Registry, target: &Path) -> Result<(), String> {
    let overlaps = |path: &Path| target.starts_with(path) || path.starts_with(target);
    if registry.projects.iter().any(|project| {
        overlaps(&project.path) || fs::canonicalize(&project.path).is_ok_and(|path| overlaps(&path))
    }) {
        return Err("The retired data overlaps a registered working folder.".into());
    }
    Ok(())
}

fn guest_row(
    brig: &Brig,
    project: &Project,
    sandbox: &str,
) -> Result<Option<serde_json::Value>, String> {
    let inventory = brig
        .json(&["--json".into(), "ls".into()], "SandboxList", None)
        .map_err(|_| "Could not read the retired Brig inventory.")?;
    let data = &inventory["data"];
    if data["runtime"]["available"] != true || data["runtime"]["kind"] != "hull" {
        return Err(
            "Brig could not verify the retired guest. Check the Mac runtime and retry.".into(),
        );
    }
    let rows = data["sandboxes"]
        .as_array()
        .ok_or("The Brig sandbox list is missing.")?;
    let reference = project.reference();
    let mut matches = rows
        .iter()
        .filter(|row| row["sandbox"] == sandbox || row["ref"] == reference);
    let Some(row) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() || row["sandbox"] != sandbox || row["ref"] != reference {
        return Err("The retired Brig guest changed after review. The guest was preserved.".into());
    }
    Ok(Some(row.clone()))
}

fn remove_guest(
    brig: &Brig,
    project: &Project,
    home: &RetiredHome,
    path: &Path,
) -> Result<(), String> {
    let Some(row) = guest_row(brig, project, &home.sandbox)? else {
        return Ok(());
    };
    if row["state"] != "stopped" {
        return Err("Stop the retired Brig guest before resetting this project. Active guest work was preserved.".into());
    }
    if !project.owns_session
        || project.adopted_managed_profile
        || row["workspace"].as_str() != path.to_str()
        || home.target.verify(path, EntryKind::Folder)?.is_none()
    {
        return Err(
            "The retired Brig guest home changed after review. The guest was preserved.".into(),
        );
    }
    let info = super::verify_registered_reference(brig, project)?;
    if info["data"]["workspace"].as_str() != path.to_str() {
        return Err(
            "Brig reported a different retired guest home. The guest was preserved.".into(),
        );
    }
    let reference = project.reference();
    let preview = brig
        .checked(&["rm".into(), reference.clone(), "--dry-run".into()], None)
        .map_err(|_| "Could not review the retired Brig guest removal.")?;
    let expected = format!(
        "would remove {reference} (sandbox {}) and its guest home {}",
        home.sandbox,
        path.display()
    );
    if preview.stdout.trim() != expected {
        return Err(
            "Brig reported a different retired guest removal. The guest was preserved.".into(),
        );
    }
    home.target
        .verify(path, EntryKind::Folder)?
        .ok_or("The retired guest home is unavailable.")?;
    brig.checked(&["rm".into(), reference], None).map_err(|_| {
        "Brig did not confirm retired guest removal. Restart Tugboat to retry the reset."
    })?;
    if guest_row(brig, project, &home.sandbox)?.is_some() {
        return Err("Brig still reports the retired guest. Its home was preserved.".into());
    }
    Ok(())
}
