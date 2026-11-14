use super::{
    binding_state::BindingPhase,
    credentials::Credentials,
    directory::Directory,
    store::{Binding, PendingLogin, Session, Store},
};
use crate::{
    environment::{self, CheckedProject},
    terminal,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

pub fn hash(bytes: &[u8]) -> String {
    crate::hash::hex(Sha256::digest(bytes))
}
pub fn current_hash(directory: &Path) -> Result<Option<String>, String> {
    Ok(Directory::open(directory, None)?
        .read()?
        .as_deref()
        .map(hash))
}

pub fn directory(checked: &CheckedProject) -> Result<PathBuf, String> {
    checked.project.require_codex()?;
    let home = fs::canonicalize(&checked.paths.host_home)
        .map_err(|_| "The project guest home is unavailable.")?;
    if home.starts_with(&checked.project.path)
        || checked.project.path.starts_with(&home)
        || std::env::var_os("HOME").is_some_and(|host| home == PathBuf::from(host))
    {
        return Err("Shared ChatGPT sign-in requires a separate Brig guest home.".into());
    }
    // A custom CODEX_HOME or memory-only auth path must not be guessed.
    let output = checked.brig.checked(
        &[
            "agent".into(),
            "show".into(),
            checked.project.profile.clone(),
            "--json".into(),
        ],
        None,
    )?;
    let profile: Value =
        serde_json::from_str(&output.stdout).map_err(|_| "Could not verify the Codex profile.")?;
    if profile["env"]
        .as_array()
        .is_some_and(|env| env.iter().any(|item| item["name"] == "CODEX_HOME"))
        || profile["volumes"]
            .as_array()
            .is_some_and(|volumes| !volumes.is_empty())
        || profile["files"]
            .as_array()
            .is_some_and(|files| !files.is_empty())
    {
        return Err("This Codex profile uses a custom credential location. Shared sign-in supports the standard guest .codex directory.".into());
    }
    let directory = home.join(".codex");
    match DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err("Could not create the guest Codex directory.".into()),
    }
    identity(&directory)?;
    Ok(directory)
}

pub fn identity(directory: &Path) -> Result<(u64, u64), String> {
    Directory::open(directory, None)?.identity()
}

pub fn read(directory: &Path) -> Result<Option<Credentials>, String> {
    let Some(bytes) = Directory::open(directory, None)?.read()? else {
        return Ok(None);
    };
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "Could not read the project's ChatGPT sign-in.")?;
    Credentials::parse(value, true).map(Some)
}

pub fn require_idle(checked: &CheckedProject, skip_login: bool) -> Result<(), String> {
    checked.project.require_codex()?;
    if terminal::terminal_sessions(checked.project.id.clone())
        .iter()
        .any(|process| {
            process.running
                && process.belongs_to(&checked.project)
                && ["codex", "resume", "shell", "login"].contains(&process.purpose)
                && !(skip_login && process.purpose == "login" && process.exit_code.is_some())
        })
    {
        return Err(
            "End this project's chats and guest terminal before changing its ChatGPT sign-in."
                .into(),
        );
    }
    if !environment::is_running(&checked.brig, &checked.project, &checked.paths)? {
        return Ok(());
    }
    let output = checked.brig.checked(&["sh".into(), checked.project.reference(), "sh".into(), "-c".into(),
        "for file in /proc/[0-9]*/comm; do IFS= read -r name < \"$file\" 2>/dev/null || continue; case \"$name\" in codex|codex-*) printf busy; exit 0;; esac; done; printf idle".into()], None)?;
    if output.stdout.trim() != "idle" {
        return Err("A Codex process still uses this project's sign-in. End it, or stop and reopen its Brig environment, before connecting the shared account.".into());
    }
    Ok(())
}

pub fn bind(
    session: &mut Session,
    checked: &CheckedProject,
    directory: PathBuf,
) -> Result<(), String> {
    checked.project.require_codex()?;
    let replacement = Binding {
        project_id: checked.project.id.clone(),
        auth_hash: current_hash(&directory)?,
        access_hash: None,
        identity: identity(&directory)?,
        directory,
        pending: true,
    };
    if let Some(binding) = session
        .bindings
        .iter_mut()
        .find(|binding| binding.belongs_to(&checked.project))
    {
        *binding = replacement;
        return Ok(());
    }
    session.bindings.push(replacement);
    Ok(())
}

fn same_access(bytes: &[u8], guest: &Value) -> bool {
    serde_json::from_slice::<Value>(bytes).is_ok_and(|value| {
        value["tokens"]["refresh_token"] == ""
            && value["tokens"]["access_token"] == guest["tokens"]["access_token"]
            && value["tokens"]["account_id"] == guest["tokens"]["account_id"]
    })
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ProjectionState { Current, NeedsReceipt, NeedsWrite, Foreign }

fn projection_state(binding: &Binding, current: Option<&[u8]>, guest: &Value) -> ProjectionState {
    let matches = current.is_some_and(|bytes| same_access(bytes, guest));
    let unchanged = current.map(hash) == binding.auth_hash;
    if !matches && !unchanged { return ProjectionState::Foreign; }
    if !matches { return ProjectionState::NeedsWrite; }
    if unchanged && binding.phase() == BindingPhase::Published { return ProjectionState::Current; }
    ProjectionState::NeedsReceipt
}

pub fn publish(store: &Store, session: &mut Session) -> Result<(), String> {
    let Some(credentials) = &session.credentials else {
        return Ok(());
    };
    let guest = credentials.guest();
    let bytes = serde_json::to_vec_pretty(&guest)
        .map_err(|_| "Could not prepare the shared ChatGPT sign-in.")?;
    for index in 0..session.bindings.len() {
        let binding = &session.bindings[index];
        let directory = Directory::open(&binding.directory, Some(binding.identity));
        if directory.is_err() {
            if binding.phase() == BindingPhase::Pending {
                return Err(
                    "The source guest home is unavailable. Reconnect it to finish sharing sign-in."
                        .into(),
                );
            }
            continue;
        }
        let directory = directory?;
        let current = directory.read()?;
        let projection = projection_state(binding, current.as_deref(), &guest);
        if projection == ProjectionState::Foreign {
            if binding.phase() == BindingPhase::Pending {
                return Err(
                    "The project's sign-in changed during transfer. Reopen the project and retry."
                        .into(),
                );
            }
            continue;
        }
        if projection == ProjectionState::Current {
            continue;
        }
        if projection == ProjectionState::NeedsWrite {
            directory.write(&bytes)?;
        }
        let binding = &mut session.bindings[index];
        binding.publish_receipt(if projection == ProjectionState::NeedsReceipt {
            current.as_deref().unwrap()
        } else {
            &bytes
        }, guest["tokens"]["access_token"].as_str().unwrap());
        store.save(session)?;
    }
    Ok(())
}

pub fn connected(
    session: &Session,
    checked: &CheckedProject,
    directory: &Path,
) -> Result<bool, String> {
    let Some(binding) = session
        .bindings
        .iter()
        .find(|binding| binding.belongs_to(&checked.project))
    else {
        return Ok(false);
    };
    let Some(credentials) = &session.credentials else {
        return Ok(false);
    };
    let active = Directory::open(directory, None)?;
    if binding.directory != directory || binding.identity != active.identity()? || binding.phase() == BindingPhase::Pending {
        return Ok(false);
    }
    let current = active.read()?;
    Ok(current
        .as_deref()
        .is_some_and(|bytes| same_access(bytes, &credentials.guest())))
}

pub fn cleanup(session: &mut Session) -> Result<usize, String> {
    let mut remaining = Vec::new();
    for binding in &session.bindings {
        if cleanup_binding(binding)? == CleanupState::Deferred {
            remaining.push(binding.clone());
        }
    }
    session.bindings = remaining;
    Ok(session.bindings.len())
}

#[derive(Eq, PartialEq)]
enum CleanupState { Deferred, Preserved, Removed }

fn cleanup_binding(binding: &Binding) -> Result<CleanupState, String> {
    let Ok(directory) = Directory::open(&binding.directory, Some(binding.identity)) else {
        return Ok(CleanupState::Deferred);
    };
    let current = directory.read()?;
    if !current.as_deref().is_some_and(|bytes| managed_copy(bytes, binding)) {
        return Ok(CleanupState::Preserved);
    }
    if directory.remove().is_err() { return Ok(CleanupState::Deferred); }
    Ok(CleanupState::Removed)
}

fn managed_copy(bytes: &[u8], binding: &Binding) -> bool {
    if Some(hash(bytes)) == binding.auth_hash {
        return true;
    }
    serde_json::from_slice::<Value>(bytes).is_ok_and(|value| {
        value["tokens"]["refresh_token"] == ""
            && value["tokens"]["access_token"]
                .as_str()
                .is_some_and(|access| Some(hash(access.as_bytes())) == binding.access_hash)
    })
}

pub fn detach(
    store: &Store,
    session: &mut Session,
    project_id: &str,
    remove_auth: bool,
) -> Result<bool, String> {
    let Some(index) = session
        .bindings
        .iter()
        .position(|binding| binding.project_id == project_id)
    else {
        return Ok(false);
    };
    let binding = &session.bindings[index];
    if !remove_auth && binding.phase() == BindingPhase::Pending {
        return Err("Reconnect this guest home and finish sharing its sign-in before removing its project record.".into());
    }
    if remove_auth {
        let directory = Directory::open(&binding.directory, Some(binding.identity))?;
        if let Some(bytes) = directory.read()? {
            if !managed_copy(&bytes, binding) {
                return Err(
                    "The guest sign-in changed outside Tugboat. Its contents were preserved."
                        .into(),
                );
            }
            directory.remove()?;
        }
    }
    session.bindings.remove(index);
    store.save(session)?;
    Ok(true)
}

pub fn login_directory(checked: &CheckedProject, nonce: &str) -> Result<(PathBuf, String), String> {
    if nonce.len() != 32 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("The ChatGPT sign-in transaction is invalid.".into());
    }
    let guest = checked.plan["data"]["home"]["guest"]
        .as_str()
        .ok_or("The guest home path is unavailable.")?;
    let guest = Path::new(guest);
    if !guest.is_absolute()
        || guest
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err("The guest home path is unsupported.".into());
    }
    let codex = directory(checked)?;
    let parent = codex.parent().unwrap().join(".tugboat-chatgpt-login");
    match DirBuilder::new().mode(0o700).create(&parent) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err("Could not create the temporary ChatGPT sign-in directory.".into()),
    }
    identity(&parent)?;
    let staging = parent.join(nonce);
    DirBuilder::new()
        .mode(0o700)
        .create(&staging)
        .map_err(|_| "Could not create the ChatGPT sign-in transaction.")?;
    Ok((
        staging,
        guest
            .join(".tugboat-chatgpt-login")
            .join(nonce)
            .to_string_lossy()
            .into_owned(),
    ))
}

pub fn remove_login_directory(pending: &PendingLogin) -> Result<(), String> {
    match fs::symlink_metadata(&pending.directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("Could not inspect the temporary ChatGPT sign-in directory.".into()),
        Ok(_) => {}
    }
    if identity(&pending.directory)? != pending.identity {
        return Err(
            "The temporary ChatGPT sign-in directory changed. Its contents were preserved.".into(),
        );
    }
    fs::remove_dir_all(&pending.directory)
        .map_err(|_| "Could not remove the temporary ChatGPT sign-in files.".into())
}
