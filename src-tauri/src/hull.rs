use std::path::{Path, PathBuf};

fn bundled(executable: &Path) -> Option<PathBuf> {
    let directory = executable.parent()?;
    if directory.file_name()? != "MacOS" {
        return None;
    }
    Some(directory.parent()?.join("Resources/runtime/hull"))
}

pub fn executable() -> Option<PathBuf> {
    let bundled = std::env::current_exe().ok().and_then(|path| bundled(&path));
    let path = bundled.or_else(|| {
        cfg!(debug_assertions)
            .then(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../.build/hull-compat/bin/hull"))
    })?;
    path.is_file().then_some(path)
}

pub fn check_instance_socket_paths(home: &Path, sandbox: &str) -> Result<(), String> {
    // Tugboat invokes the bundled hull with its default store. hull checks both
    // addresses in run.go; Brig can already have started its network helper by then.
    let instance = home.join(".hull/store/instances").join(sandbox);
    const MAX_SOCKET_BYTES: usize = 103; // Darwin sun_path includes a trailing NUL.
    for socket in ["qmp.sock", "agent.sock"] {
        let path = instance.join(socket);
        let length = path.as_os_str().as_encoded_bytes().len();
        if length > MAX_SOCKET_BYTES {
            return Err(format!("The hull socket path is {length} bytes (macOS limit {MAX_SOCKET_BYTES}): {}. Use a shorter home directory or Brig session name before preparing the environment.", path.display()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_preflight_accepts_the_limit_and_counts_multibyte_names_in_bytes() {
        let home = Path::new("/tmp");
        let fixed = home
            .join(".hull/store/instances/x/agent.sock")
            .as_os_str()
            .as_encoded_bytes()
            .len()
            - 1;
        let name = "x".repeat(103 - fixed);
        assert!(check_instance_socket_paths(home, &name).is_ok());
        let longer = format!("{}é", &name[1..]);
        let error = check_instance_socket_paths(home, &longer).unwrap_err();
        assert!(error.contains("104 bytes") && error.contains("shorter"));
    }
    #[test]
    fn runtime_location_preserves_spaces_in_the_application_path() {
        assert_eq!(
            bundled(Path::new(
                "/Applications/My Apps/Tugboat.app/Contents/MacOS/tugboat"
            )),
            Some(PathBuf::from(
                "/Applications/My Apps/Tugboat.app/Contents/Resources/runtime/hull"
            ))
        );
        assert_eq!(bundled(Path::new("/tmp/tugboat")), None);
    }
}
