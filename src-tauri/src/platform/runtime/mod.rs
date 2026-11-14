use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

pub const TOOLS: [&str; 10] = [
    "brig", "brigd", "brig-broker", "hull", "hvi", "vz-runner", "cosign",
    "mke2fs", "debugfs", "e2fsck",
];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    schema_version: u8,
    brig_version: String,
    tools: BTreeMap<String, FileRecord>,
    libraries: Vec<Library>,
    configuration: BTreeMap<String, FileRecord>,
}

#[derive(Deserialize)]
struct FileRecord { sha256: String }

#[derive(Deserialize)]
struct Library { name: String, sha256: String }

pub fn directory_for(executable: &Path) -> Option<PathBuf> {
    let directory = executable.parent()?;
    if directory.file_name()? == "MacOS" { return Some(directory.to_owned()); }
    cfg!(debug_assertions)
        .then(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../.build/brig-runtime/bin"))
}

pub fn directory() -> Option<PathBuf> {
    directory_for(&std::env::current_exe().ok()?)
}

pub fn data_directory(directory: &Path) -> Option<PathBuf> {
    if directory.file_name()? == "MacOS" {
        return Some(directory.parent()?.join("Resources/runtime"));
    }
    Some(directory.parent()?.to_owned())
}

pub fn configuration() -> Option<PathBuf> {
    Some(data_directory(&directory()?)?.join("mke2fs.conf"))
}

pub fn registry_directory() -> Option<PathBuf> {
    Some(data_directory(&directory()?)?.join("registry"))
}

pub fn executable(path: &Path) -> bool {
    path.metadata().is_ok_and(|metadata| {
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    })
}

pub fn tool(name: &str) -> Option<PathBuf> {
    let path = directory()?.join(name);
    executable(&path).then_some(path)
}

pub fn validate() -> Result<(), String> {
    let directory = directory().ok_or("The bundled runtime directory is unavailable. Reinstall Tugboat.")?;
    validate_at(&directory).map_err(|error| format!("{error} Reinstall Tugboat or prepare the development runtime again."))
}

fn validate_at(directory: &Path) -> Result<(), String> {
    let data = data_directory(directory).ok_or("The runtime data directory is invalid.")?;
    let manifest_path = data.join("build.json");
    let metadata = fs::metadata(&manifest_path)
        .map_err(|error| format!("The runtime manifest is unavailable: {error}"))?;
    if metadata.len() > 262_144 {
        return Err("The runtime manifest exceeds the size limit.".into());
    }
    let bytes = fs::read(&manifest_path)
        .map_err(|error| format!("The runtime manifest is unavailable: {error}"))?;
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("The runtime manifest is invalid: {error}"))?;
    if manifest.schema_version != 1
        || manifest.brig_version != crate::platform::host::SUPPORTED_BRIG_VERSION
    {
        return Err("The bundled runtime version is unsupported.".into());
    }
    if manifest.tools.len() != TOOLS.len() {
        return Err("The runtime tool inventory is incomplete.".into());
    }
    for name in TOOLS {
        let record = manifest
            .tools
            .get(name)
            .ok_or_else(|| format!("The runtime manifest is missing {name}."))?;
        let file = directory.join(name);
        if !executable(&file) {
            return Err(format!("The bundled {name} executable is unavailable."));
        }
        verify(&file, &record.sha256)?;
    }
    for name in ["mke2fs.conf", "registry/config.json"] {
        let configuration = manifest
            .configuration
            .get(name)
            .ok_or_else(|| format!("The runtime manifest is missing {name}."))?;
        verify(&data.join(name), &configuration.sha256)?;
    }
    let libraries = directory
        .parent()
        .ok_or("The runtime library directory is invalid.")?
        .join(
            if directory.file_name().is_some_and(|name| name == "MacOS") {
                "Frameworks"
            } else {
                "lib"
            },
        );
    for library in manifest.libraries {
        if Path::new(&library.name).components().count() != 1
            || !matches!(
                Path::new(&library.name).components().next(),
                Some(Component::Normal(_))
            )
        {
            return Err("The runtime manifest contains an invalid library name.".into());
        }
        verify(&libraries.join(library.name), &library.sha256)?;
    }
    Ok(())
}

fn verify(path: &Path, expected: &str) -> Result<(), String> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    if !fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file()) {
        return Err(format!("The bundled {name} file must be a regular file."));
    }
    let mut file = fs::File::open(path).map_err(|error| format!("The bundled {name} file is unavailable: {error}"))?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 65_536];
    loop {
        let size = file.read(&mut buffer).map_err(|error| format!("Could not verify bundled {name}: {error}"))?;
        if size == 0 { break; }
        digest.update(&buffer[..size]);
    }
    if crate::hash::hex(digest.finalize()) != expected {
        return Err(format!("The bundled {name} SHA-256 does not match its manifest."));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
