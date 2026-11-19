use super::Brig;
use std::path::Path;

impl Brig {
    pub(crate) fn enable_host_keychain(&mut self, directory: &Path) -> Result<(), String> {
        #[cfg(dev)]
        self.use_host_home_with_dev_tools(directory)?;
        #[cfg(not(dev))]
        let _ = directory;
        Ok(())
    }

    #[cfg(dev)]
    fn use_host_home_with_dev_tools(&mut self, directory: &Path) -> Result<(), String> {
        let host = std::env::home_dir().ok_or("The Mac user home is unavailable.")?;
        let dev = self
            .environment
            .get("HOME")
            .map(std::path::PathBuf::from)
            .ok_or("The Dev runtime home is unavailable.")?;
        if host == dev {
            return Ok(());
        }
        if !directory.is_absolute() || dev != host.join(".tugboat-dev") || !host.is_absolute() {
            return Err(
                "Could not verify the isolated Dev runtime home for Keychain access.".into(),
            );
        }
        self.isolate_runtime_home_for_keychain(directory, &host, &dev)
    }

    #[cfg(test)]
    pub(crate) fn enable_proof_keychain(&mut self, root: &Path) -> Result<(), String> {
        use std::os::unix::fs::MetadataExt;
        let host = std::env::home_dir().ok_or("The Mac user home is unavailable.")?;
        let home = root.join("home");
        let metadata = std::fs::symlink_metadata(root).map_err(|error| error.to_string())?;
        if !root.starts_with("/private/tmp")
            || root.parent() != Some(Path::new("/private/tmp"))
            || std::fs::canonicalize(root).ok().as_deref() != Some(root)
            || !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
            || self.environment.get("HOME").map(Path::new) != Some(home.as_path())
            || [
                "BRIG_STATE_DIR",
                "BRIG_PROFILE_DIR",
                "BRIG_POLICY_DIR",
                "XDG_CONFIG_HOME",
                "DOCKER_CONFIG",
            ]
            .iter()
            .any(|key| {
                !self
                    .environment
                    .get(*key)
                    .is_some_and(|value| Path::new(value).starts_with(root))
            })
        {
            return Err("Use a verified disposable Brig proof root.".into());
        }
        self.isolate_runtime_home_for_keychain(root, &host, &home)
    }

    #[cfg(any(dev, test))]
    fn isolate_runtime_home_for_keychain(
        &mut self,
        directory: &Path,
        host: &Path,
        dev: &Path,
    ) -> Result<(), String> {
        crate::platform::runtime::validate()?;
        for (tool, key) in [("hull", "BRIG_RUNTIME_BIN"), ("cosign", "BRIG_COSIGN_BIN")] {
            let tool = crate::platform::runtime::tool(tool)
                .ok_or("A bundled runtime tool is unavailable.")?;
            let name = tool
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("Invalid bundled runtime tool.")?;
            let wrapper = directory.join(format!("keychain-dev-{name}"));
            let path = quoted(&tool)?;
            let home = quoted(dev)?;
            install(
                &wrapper,
                format!("#!/bin/sh\nexport HOME={home}\nexec {path} \"$@\"\n").as_bytes(),
            )?;
            self.environment
                .insert(key.into(), wrapper.to_string_lossy().into_owned());
        }
        for (key, path) in [
            ("BRIG_STATE_DIR", dev.join(".brig")),
            ("BRIG_GATEWAY_DIR", dev.join(".brig")),
            ("XDG_CONFIG_HOME", dev.join(".config")),
            ("BRIG_POLICY_DIR", dev.join(".config/brig/policies")),
        ] {
            self.environment
                .entry(key.into())
                .or_insert_with(|| path.to_string_lossy().into_owned());
        }
        self.environment
            .insert("HOME".into(), host.to_string_lossy().into_owned());
        self.environment.insert(
            "TUGBOAT_HULL_HOME".into(),
            dev.to_string_lossy().into_owned(),
        );
        for (key, value) in [
            ("BRIG_GIT_CONFIG", "0"),
            ("BRIG_SKILLS", "0"),
            ("BRIG_FORWARD_ENV", ""),
            ("DO_NOT_TRACK", "1"),
        ] {
            self.environment.insert(key.into(), value.into());
        }
        Ok(())
    }
}

#[cfg(any(dev, test))]
fn quoted(path: &Path) -> Result<String, String> {
    let value = path
        .to_str()
        .ok_or("The runtime directory must have a UTF-8 path.")?;
    Ok(format!("'{}'", value.replace('\'', "'\"'\"'")))
}

#[cfg(any(dev, test))]
fn install(file: &Path, contents: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let _guard = crate::storage::lock(file)?;
    if let Some(existing) = crate::storage::read_regular_bytes(file)? {
        let metadata = std::fs::symlink_metadata(file)
            .map_err(|_| "The Dev runtime helper is unavailable.")?;
        if existing != contents
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err(
                "The Dev runtime helper changed. Review the Tugboat Dev records directory.".into(),
            );
        }
    } else {
        crate::storage::write_bytes(file, contents)?;
    }
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o700))
        .map_err(|_| "Could not enable the Dev runtime helper.".into())
}
