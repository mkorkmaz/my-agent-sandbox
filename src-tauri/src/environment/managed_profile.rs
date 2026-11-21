use crate::{
    projects::{EnvironmentId, Project, ENVIRONMENT_PROFILE, PREVIOUS_ENVIRONMENT_IMAGE},
    storage,
};
use serde_json::{json, Value};
use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

const DOCUMENT: &[u8] = include_bytes!("../../profiles/tugboat-project.yaml");

enum ProfileState {
    Absent,
    Verified,
    Changed,
}

impl ProfileState {
    fn inspect(file: &Path, expected: &[u8]) -> Result<Self, String> {
        match storage::read_regular_bytes(file)? {
            None => Ok(Self::Absent),
            Some(bytes) if bytes == expected => Ok(Self::Verified),
            Some(_) => Ok(Self::Changed),
        }
    }

    fn install(self, file: &Path, document: &[u8]) -> Result<(), String> {
        match self {
            Self::Absent => storage::write_bytes(file, document),
            Self::Verified => Ok(()),
            Self::Changed => Err("The managed Tugboat profile changed. Restore the bundled profile before preparing this environment.".into()),
        }
    }
}

pub fn shared(project: &Project) -> bool {
    project.profile == ENVIRONMENT_PROFILE
        && project.owns_session
        && !project.adopted_managed_profile
        && project.environment_id == EnvironmentId::for_project(&project.id)
}

pub(crate) fn expected_definition() -> Value {
    // The profile is part of approved plan fingerprints. The project pin overrides its image.
    json!({
        "name": ENVIRONMENT_PROFILE, "desc": "Shared Tugboat project guest",
        "kind": "agent", "binary": "bash", "image": PREVIOUS_ENVIRONMENT_IMAGE,
        "guestHome": "/root", "headless": true, "genericBoot": true,
        "hypervisor": "hvi", "network": "isolated", "mem": 4096, "cpus": 4,
        "env": [
            {"name":"IS_SANDBOX","value":"1"},
            {"name":"CLAUDE_CONFIG_DIR","value":"/brig/claude"},
            {"name":"DISABLE_UPDATES","value":"1"}
        ],
        "deny": ["OPENAI_API_KEY", "CODEX_API_KEY", "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"],
        "volumes": [
            {"kind":"tmpfs","path":".claude","size":"512m","at":"/brig/claude"},
            {"kind":"hostmount","path":".claude/.claude.json","file":true},
            {"kind":"hostmount","path":".claude/settings.json","file":true},
            {"kind":"hostmount","path":".claude/CLAUDE.md","file":true},
            {"kind":"hostmount","path":".claude/sessions"},
            {"kind":"hostmount","path":".claude/projects"},
            {"kind":"hostmount","path":".claude/plugins"},
            {"kind":"hostmount","path":".claude/skills"},
            {"kind":"hostmount","path":".claude/history.jsonl","file":true}
        ],
        "staleCredentialFiles": [".claude/.credentials.json"],
        "onboarding": {
            "file": ".claude/.claude.json",
            "seed": {"hasCompletedOnboarding": true, "hasTrustDialogAccepted": true},
            "trustKey": ["projects", "hasTrustDialogAccepted"]
        }
    })
}

pub fn validate(project: &Project, definition: &Value) -> Result<(), String> {
    if !shared(project) || definition != &expected_definition() {
        return Err("The shared Tugboat profile or guest layout could not be verified.".into());
    }
    Ok(())
}

pub fn install(directory: &Path) -> Result<(), String> {
    install_document(directory, ENVIRONMENT_PROFILE, DOCUMENT)
}

fn install_document(directory: &Path, name: &str, document: &[u8]) -> Result<(), String> {
    match DirBuilder::new().mode(0o700).create(directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.to_string()),
    }
    if !fs::symlink_metadata(directory)
        .map_err(|error| error.to_string())?
        .is_dir()
    {
        return Err("The managed profile directory is not a regular directory.".into());
    }
    let file = directory.join(format!("{name}.yaml"));
    let _lock = storage::lock(&file)?;
    ProfileState::inspect(&file, document)?.install(&file, document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::{
        ffi::OsStrExt,
        fs::{symlink, PermissionsExt},
    };

    #[test]
    fn managed_profile_is_private_and_changed_contents_are_preserved() {
        let root =
            std::env::temp_dir().join(format!("tugboat-managed-profile-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let directory = root.join("profiles");
        install(&directory).unwrap();
        let file = directory.join("tugboat-project.yaml");
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read(&file).unwrap(), DOCUMENT);
        install(&directory).unwrap();
        fs::write(&file, b"user changed profile").unwrap();
        assert!(install(&directory).unwrap_err().contains("changed"));
        assert_eq!(fs::read(&file).unwrap(), b"user changed profile");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_profile_rejects_symbolic_links_and_special_files_without_reading_them() {
        let root = std::env::temp_dir().join(format!(
            "tugboat-managed-profile-links-{}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let directory = root.join("profiles");
        install(&directory).unwrap();
        let file = directory.join("tugboat-project.yaml");
        fs::remove_file(&file).unwrap();
        fs::write(root.join("private"), b"synthetic private record").unwrap();
        symlink(root.join("private"), &file).unwrap();
        assert!(install(&directory).is_err());
        assert_eq!(
            fs::read(root.join("private")).unwrap(),
            b"synthetic private record"
        );
        fs::remove_file(&file).unwrap();
        let name = std::ffi::CString::new(file.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(install(&directory).unwrap_err().contains("regular file"));
        symlink(&directory, root.join("alias")).unwrap();
        assert!(install(&root.join("alias")).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
