use crate::{agents::AgentKind, projects::Project, storage};
use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

pub const CLAUDE_PROFILE: &str = "tugboat-claude";
const DOCUMENT: &[u8] = include_bytes!("../../profiles/tugboat-claude.yaml");

enum ProfileState {
    Absent,
    Verified,
    Changed,
}

impl ProfileState {
    fn inspect(file: &Path) -> Result<Self, String> {
        match storage::read_regular_bytes(file)? {
            None => Ok(Self::Absent),
            Some(bytes) if bytes == DOCUMENT => Ok(Self::Verified),
            Some(_) => Ok(Self::Changed),
        }
    }

    fn install(self, file: &Path) -> Result<(), String> {
        match self {
            Self::Absent => storage::write_bytes(file, DOCUMENT),
            Self::Verified => Ok(()),
            Self::Changed => Err("The managed Claude profile changed. Its contents were preserved. Restore the Tugboat profile before preparing this environment.".into()),
        }
    }
}

pub fn owns(project: &Project) -> bool {
    project.agent_kind == AgentKind::ClaudeCode
        && project.owns_session
        && project.profile == CLAUDE_PROFILE
}

pub fn install(directory: &Path) -> Result<(), String> {
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
    let file = directory.join(format!("{CLAUDE_PROFILE}.yaml"));
    let _lock = storage::lock(&file)?;
    ProfileState::inspect(&file)?.install(&file)
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
        let file = directory.join("tugboat-claude.yaml");
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
        let file = directory.join("tugboat-claude.yaml");
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
