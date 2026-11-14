use crate::usage_store::Environment;
use crate::{agents::AgentKind, projects, storage, storage::data_location::RecordsDirectory};
use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::Deserializer;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Fingerprint {
    pub relative_path: String,
    pub length: u64,
    pub digest: Vec<u8>,
}

pub(super) struct Source {
    pub file: PathBuf,
    pub fingerprint: Fingerprint,
    pub environment: Environment,
    pub version: u32,
}

pub(super) struct Inventory {
    pub sources: Vec<Source>,
    pub backups: Vec<Fingerprint>,
}

pub(super) fn open_regular(path: &Path) -> Result<File, String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| error.to_string())?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("The legacy usage input is not a regular file.".into());
    }
    Ok(file)
}

fn fingerprint(root: &Path, path: &Path) -> Result<Fingerprint, String> {
    let relative_path = path
        .strip_prefix(root)
        .map_err(|_| "The legacy usage input is outside the records folder.")?
        .to_str()
        .ok_or("The legacy usage path is not valid UTF-8.")?
        .to_owned();
    let mut input = open_regular(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65_536];
    let mut length = 0u64;
    loop {
        let count = input.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        length = length
            .checked_add(count as u64)
            .ok_or("Legacy input length overflow.")?;
        digest.update(&buffer[..count]);
    }
    Ok(Fingerprint {
        relative_path,
        length,
        digest: digest.finalize().to_vec(),
    })
}

struct Version;
impl<'de> Visitor<'de> for Version {
    type Value = u32;
    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a versioned legacy usage object")
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<u32, M::Error> {
        let mut version = None;
        let mut fields = std::collections::BTreeSet::new();
        while let Some(field) = map.next_key::<String>()? {
            if !fields.insert(field.clone()) {
                return Err(serde::de::Error::custom("Duplicate legacy ledger field."));
            }
            if field == "schemaVersion" {
                version = Some(map.next_value::<u32>()?);
                continue;
            }
            if !matches!(field.as_str(), "sessions" | "variants" | "warnings") {
                return Err(serde::de::Error::custom("Unknown legacy ledger field."));
            }
            map.next_value::<IgnoredAny>()?;
        }
        match version {
            Some(version @ (1 | 2)) => Ok(version),
            _ => Err(serde::de::Error::custom(
                "Unsupported legacy ledger version.",
            )),
        }
    }
}

pub(super) fn ledger_version(path: &Path) -> Result<u32, String> {
    let mut input = serde_json::Deserializer::from_reader(BufReader::new(
        super::guard::GuardedRead::new(open_regular(path)?),
    ));
    let version = input
        .deserialize_map(Version)
        .map_err(|error| error.to_string())?;
    input.end().map_err(|error| error.to_string())?;
    Ok(version)
}

fn valid_project(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn source_for(root: &Path, file: &Path) -> Result<Source, String> {
    let relative = file
        .strip_prefix(root.join("conversation-ledgers"))
        .map_err(|_| "Unexpected legacy usage path.")?;
    let parts = relative.components().collect::<Vec<_>>();
    let name = file
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or("Invalid legacy ledger name.")?;
    let (project, agent) = match parts.as_slice() {
        [_] if valid_project(name) => (name.to_owned(), AgentKind::Codex),
        [project, _] => {
            let project = project
                .as_os_str()
                .to_str()
                .filter(|id| valid_project(id))
                .ok_or("Invalid legacy project directory.")?;
            let expected = projects::EnvironmentId::for_agent(project, AgentKind::ClaudeCode);
            if name != expected.to_string() {
                return Err(
                    "The nested usage ledger has an ambiguous environment identity.".into(),
                );
            }
            (project.to_owned(), AgentKind::ClaudeCode)
        }
        _ => return Err("Unexpected active JSON path in conversation-ledgers.".into()),
    };
    let environment = Environment::new(
        project.clone(),
        projects::EnvironmentId::for_agent(&project, agent),
        agent,
    )?;
    Ok(Source {
        file: file.to_owned(),
        fingerprint: fingerprint(root, file)?,
        environment,
        version: ledger_version(file)?,
    })
}

fn visit_directory(
    root: &Path,
    directory: &Path,
    depth: usize,
    inventory: &mut Inventory,
) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        let path = entry.path();
        if kind.is_symlink() {
            return Err(
                "A legacy usage input is a symbolic link. Original records were preserved.".into(),
            );
        }
        if kind.is_dir() {
            if depth != 0 || !entry.file_name().to_str().is_some_and(valid_project) {
                return Err("Unexpected folder beneath conversation-ledgers.".into());
            }
            visit_directory(root, &path, depth + 1, inventory)?;
            continue;
        }
        if !kind.is_file() {
            return Err("A legacy usage input is not a regular file.".into());
        }
        let name = entry.file_name();
        let name = name.to_str().ok_or("Invalid legacy usage path.")?;
        if name.ends_with(".lock") {
            continue;
        }
        if name.ends_with(".v1.backup") {
            inventory.backups.push(fingerprint(root, &path)?);
            continue;
        }
        if !name.ends_with(".json") {
            return Err("Unexpected active file beneath conversation-ledgers.".into());
        }
        inventory.sources.push(source_for(root, &path)?);
    }
    Ok(())
}

pub(super) fn inventory(records: &RecordsDirectory) -> Result<Inventory, String> {
    records.verify()?;
    let root = records.path();
    let ledgers = root.join("conversation-ledgers");
    let mut result = Inventory {
        sources: Vec::new(),
        backups: Vec::new(),
    };
    match fs::symlink_metadata(&ledgers) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(result),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        _ => {
            return Err("The legacy ledger directory is unavailable or is a symbolic link.".into())
        }
    }
    visit_directory(root, &ledgers, 0, &mut result)?;
    result.sources.sort_by(|first, second| {
        first
            .fingerprint
            .relative_path
            .cmp(&second.fingerprint.relative_path)
    });
    result
        .backups
        .sort_by(|first, second| first.relative_path.cmp(&second.relative_path));
    let registry = projects::Registry::load(&root.join("projects.json"))?;
    for source in &result.sources {
        if let Some(project) = registry
            .projects
            .iter()
            .find(|project| project.id == source.environment.project_id)
        {
            for known in project
                .environments
                .iter()
                .filter(|known| known.environment_id == source.environment.environment_id)
            {
                if known.agent_kind != source.environment.agent_kind {
                    return Err(
                        "A legacy ledger conflicts with the registered agent environment.".into(),
                    );
                }
            }
        }
    }
    Ok(result)
}

pub(super) fn backup_sources(
    records: &RecordsDirectory,
    inventory: &Inventory,
) -> Result<(), String> {
    for source in inventory
        .sources
        .iter()
        .filter(|source| source.version == 1)
    {
        records.verify()?;
        storage::immutable_v1_backup(&source.file)?;
    }
    Ok(())
}

pub(super) fn matches(first: &Inventory, second: &Inventory) -> bool {
    first.sources.len() == second.sources.len()
        && first
            .sources
            .iter()
            .zip(&second.sources)
            .all(|(first, second)| first.fingerprint == second.fingerprint)
        && first.backups == second.backups
}
