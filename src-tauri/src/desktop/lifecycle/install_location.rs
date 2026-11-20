use std::ffi::{CString, OsStr};
use std::fs;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Child;

const MANUAL_INSTALL: &str = "Download the signed installer from https://github.com/reformo/tugboat/releases/latest and move Tugboat to Applications.";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct InstallLocation {
    bundle: PathBuf,
    executable: PathBuf,
}

impl InstallLocation {
    pub(super) fn current() -> Result<Self, String> {
        // Use the same cached, symlink-checked executable path as Tauri's updater.
        let executable = tauri::utils::platform::current_exe()
            .map_err(|error| format!("Could not locate the Tugboat app. {error}"))?;
        Self::for_executable(executable)
    }

    pub(super) fn for_executable(executable: PathBuf) -> Result<Self, String> {
        let bundle = bundle_directory(&executable)
            .ok_or_else(|| {
                format!("This copy of Tugboat is outside an app bundle. {MANUAL_INSTALL}")
            })?
            .to_owned();
        Ok(Self { bundle, executable })
    }

    pub(super) fn require_writable(&self) -> Result<(), String> {
        let path = CString::new(self.bundle.as_os_str().as_bytes())
            .map_err(|_| "The Tugboat app path is invalid.")?;
        let mut status = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // statvfs writes the volume flags into status when it succeeds.
        if unsafe { libc::statvfs(path.as_ptr(), status.as_mut_ptr()) } != 0 {
            return Err(format!(
                "Could not check the Tugboat install volume. {}",
                std::io::Error::last_os_error()
            ));
        }
        let status = unsafe { status.assume_init() };
        require_writable_volume(status.f_flag & libc::ST_RDONLY != 0)
    }

    pub(super) fn preserved_at(&self, path: &Path) -> Result<Self, String> {
        let metadata = path.symlink_metadata().map_err(|error| error.to_string())?;
        if !metadata.is_dir() {
            return Err("The previous Tugboat bundle is unavailable.".into());
        }
        // The updater can preserve the previous bundle in a directory named current_app.
        let bundle = path.canonicalize().map_err(|error| error.to_string())?;
        let relative_executable = self
            .executable
            .strip_prefix(&self.bundle)
            .map_err(|_| "The Tugboat executable is outside the app bundle.")?;
        let executable = bundle.join(relative_executable);
        Ok(Self { bundle, executable })
    }

    pub(super) fn installed_version(&self, identifier: &str) -> Result<String, String> {
        let executable = fs::canonicalize(&self.executable)
            .map_err(|error| format!("Could not find the installed Tugboat executable. {error}"))?;
        let metadata = executable.metadata().map_err(|error| error.to_string())?;
        if executable != self.executable
            || !metadata.is_file()
            || metadata.permissions().mode() & 0o111 == 0
        {
            return Err("The installed Tugboat executable is unavailable.".into());
        }
        let plist = self.bundle.join("Contents/Info.plist");
        let mut bytes = Vec::new();
        fs::File::open(plist)
            .map_err(|error| format!("Could not read the installed app information. {error}"))?
            .take(256 * 1024)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("Could not read the installed app information. {error}"))?;
        let executable = self
            .executable
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or("The installed Tugboat executable name is invalid.")?;
        read_version(&bytes, identifier, executable)
    }

    pub(super) fn relaunch(
        &self,
        identifier: &str,
        expected_version: Option<&str>,
        records_id: &str,
    ) -> Result<Child, String> {
        let version = self.installed_version(identifier)?;
        if expected_version.is_some_and(|expected| expected != version) {
            return Err(format!(
                "Found installed Tugboat {version} instead of the approved update."
            ));
        }
        super::relaunch::launch(&self.executable, &version, records_id)
    }
}

fn bundle_directory(executable: &Path) -> Option<&Path> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let application = contents.parent()?;
    (macos.ends_with("MacOS")
        && contents.ends_with("Contents")
        && application.extension() == Some(OsStr::new("app")))
    .then_some(application)
}

fn require_writable_volume(read_only: bool) -> Result<(), String> {
    if read_only {
        return Err(format!(
            "This copy of Tugboat is on a read-only volume. {MANUAL_INSTALL}"
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn read_version(bytes: &[u8], identifier: &str, executable: &str) -> Result<String, String> {
    let fields = plist::Value::from_reader(std::io::Cursor::new(bytes))
        .map_err(|_| "Could not read the installed app information.")?
        .into_dictionary()
        .ok_or("The installed app information is invalid.")?;
    let field = |key: &str| fields.get(key).and_then(plist::Value::as_string);
    if field("CFBundleIdentifier") != Some(identifier)
        || field("CFBundleExecutable") != Some(executable)
    {
        return Err("The installed app identity has changed.".into());
    }
    field("CFBundleShortVersionString")
        .filter(|version| !version.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "The installed app version is unavailable.".into())
}

#[cfg(not(target_os = "macos"))]
fn read_version(_: &[u8], _: &str, _: &str) -> Result<String, String> {
    Err("Tugboat app updates require macOS.".into())
}

#[cfg(test)]
mod tests;
