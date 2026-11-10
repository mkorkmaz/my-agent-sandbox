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

#[cfg(test)]
mod tests {
    use super::*;

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
