use std::path::{Path, PathBuf};

pub fn executable() -> Option<PathBuf> {
    crate::platform::runtime::tool("hull")
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
            crate::platform::runtime::directory_for(Path::new(
                "/Applications/My Apps/Tugboat.app/Contents/MacOS/tugboat"
            )),
            Some(PathBuf::from(
                "/Applications/My Apps/Tugboat.app/Contents/MacOS"
            ))
        );
    }
}
