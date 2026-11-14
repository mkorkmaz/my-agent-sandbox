use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};

const FILES: &[(&str, &str)] = &[
    ("projects", "projects.json"),
    ("settings", "settings.json"),
    ("interface", "interface.json"),
    ("chatgpt", "chatgpt/session.json"),
    ("claude", "claude/session.json"),
    ("usage", "usage.sqlite3"),
    ("usage", "usage.sqlite3-wal"),
];

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    device: u64,
    inode: u64,
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

#[derive(PartialEq, Eq)]
struct Snapshot {
    directory: PathBuf,
    identity: String,
    files: Vec<Option<Stamp>>,
}

impl Snapshot {
    fn read(directory: &Path, identity: &str) -> Result<Self, String> {
        let files = FILES
            .iter()
            .map(|(_, name)| stamp(&directory.join(name)))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            directory: directory.into(),
            identity: identity.into(),
            files,
        })
    }

    fn changes(&self, previous: Option<&Self>) -> Vec<&'static str> {
        let relocated = previous
            .is_none_or(|old| self.directory != old.directory || self.identity != old.identity);
        let mut changed = Vec::new();
        for (index, (name, _)) in FILES.iter().enumerate() {
            if (relocated || previous.is_some_and(|old| self.files[index] != old.files[index]))
                && !changed.contains(name)
            {
                changed.push(*name);
            }
        }
        changed
    }
}

fn stamp(path: &Path) -> Result<Option<Stamp>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    Ok(Some(Stamp {
        device: metadata.dev(),
        inode: metadata.ino(),
        length: metadata.len(),
        modified: (metadata.mtime(), metadata.mtime_nsec()),
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    }))
}

pub fn monitor(app: AppHandle) {
    let initial = crate::storage::data_location::records_directory(&app)
        .and_then(|records| Snapshot::read(records.path(), records.id()));
    std::thread::spawn(move || {
        let (mut previous, mut failure) = match initial {
            Ok(snapshot) => (Some(snapshot), None),
            Err(error) => {
                eprintln!("Could not monitor saved records: {error}");
                (None, Some(error))
            }
        };
        loop {
            if crate::desktop::lifecycle::is_closing() {
                std::thread::sleep(std::time::Duration::from_secs(2));
                continue;
            }
            let result = crate::storage::data_location::records_directory(&app)
                .and_then(|records| Snapshot::read(records.path(), records.id()));
            match result {
                Ok(snapshot) => {
                    let changes = snapshot.changes(previous.as_ref());
                    if !changes.is_empty() && app.emit("records-changed", &changes).is_err() {
                        eprintln!("Could not publish a saved-record change.");
                    } else {
                        previous = Some(snapshot);
                    }
                    failure = None;
                }
                Err(error) => {
                    if failure.as_ref() != Some(&error) {
                        eprintln!("Could not monitor saved records: {error}");
                    }
                    failure = Some(error);
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_atomic_replacements_deletions_and_database_wal_changes() {
        let root = std::env::temp_dir().join(format!(
            "tugboat-record-watch-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&root).unwrap();
        let before = Snapshot::read(&root, "fixture").unwrap();
        fs::write(root.join("staged"), b"one").unwrap();
        fs::rename(root.join("staged"), root.join("settings.json")).unwrap();
        fs::write(root.join("usage.sqlite3-wal"), b"transaction").unwrap();
        let after = Snapshot::read(&root, "fixture").unwrap();
        assert_eq!(after.changes(Some(&before)), vec!["settings", "usage"]);
        assert!(after.changes(Some(&after)).is_empty());
        assert_eq!(
            after.changes(None),
            vec![
                "projects",
                "settings",
                "interface",
                "chatgpt",
                "claude",
                "usage"
            ]
        );
        fs::remove_file(root.join("settings.json")).unwrap();
        let removed = Snapshot::read(&root, "fixture").unwrap();
        assert_eq!(removed.changes(Some(&after)), vec!["settings"]);
        fs::remove_dir_all(root).unwrap();
    }
}
