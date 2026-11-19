use super::{
    root,
    state::{FileIdentity, RootEvidence},
};
use crate::{platform::command, storage};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Duration,
};

const CHANGED: &str = "Temporary disk recovery files changed. Tugboat preserved the files and its original system disk backup.";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    source: RootEvidence,
    progress: Progress,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "kebab-case", deny_unknown_fields)]
enum Progress {
    Copying,
    Repairing { disk: FileIdentity },
    Ready { disk: RootEvidence },
}

struct Paths {
    receipt: PathBuf,
    work: PathBuf,
    published: Option<PathBuf>,
}

impl Paths {
    fn new(original: &Path, boot: bool) -> Self {
        let name = if boot {
            "restore-replay"
        } else {
            "inventory-replay"
        };
        Self {
            receipt: original.with_file_name(format!("{name}.json")),
            work: original.with_file_name(format!("{name}.ext4")),
            published: boot.then(|| original.with_file_name("root.ext4")),
        }
    }

    fn clear(&self, source: &RootEvidence) -> Result<(), String> {
        let mut existing = Vec::new();
        for path in std::iter::once(&self.work).chain(self.published.iter()) {
            match fs::symlink_metadata(path) {
                Ok(metadata) if metadata.is_file() => existing.push(path),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err(CHANGED.into()),
            }
        }
        if let Ok(metadata) = fs::symlink_metadata(&self.receipt) {
            if !metadata.is_file() || metadata.len() > 4096 {
                return Err(CHANGED.into());
            }
        }
        let Some(bytes) = storage::read_regular_bytes(&self.receipt)? else {
            if existing.is_empty() {
                return Ok(());
            }
            return Err(CHANGED.into());
        };
        let receipt: Receipt = serde_json::from_slice(&bytes).map_err(|_| CHANGED)?;
        if &receipt.source != source {
            return Err(CHANGED.into());
        }
        for path in &existing {
            let evidence = root::evidence(path)?;
            let owned = match &receipt.progress {
                Progress::Copying => {
                    *path == &self.work && evidence.fingerprint == source.fingerprint
                }
                Progress::Repairing { disk } => *path == &self.work && &evidence.disk == disk,
                Progress::Ready { disk } => &evidence == disk,
            };
            if !owned {
                return Err(CHANGED.into());
            }
        }
        for path in existing {
            fs::remove_file(path).map_err(root::disk_error)?;
        }
        fs::remove_file(&self.receipt).map_err(root::disk_error)?;
        Ok(())
    }
}

pub(super) struct Disk {
    paths: Paths,
    source: RootEvidence,
    _guard: storage::FileLock,
}

impl Disk {
    pub(super) fn prepare(
        original: &Path,
        repair: &Path,
        debugfs: &Path,
        environment: &BTreeMap<String, String>,
        boot: bool,
    ) -> Result<Self, String> {
        let source = root::evidence(original)?;
        let paths = Paths::new(original, boot);
        let guard = storage::lock(&paths.receipt)?;
        paths.clear(&source)?;
        let disk = Self {
            paths,
            source,
            _guard: guard,
        };
        disk.write(Progress::Copying)?;
        root::clone_disk(original, &disk.paths.work)?;
        let copied = root::evidence(&disk.paths.work)?;
        if copied.fingerprint != disk.source.fingerprint {
            return Err(CHANGED.into());
        }
        disk.write(Progress::Repairing { disk: copied.disk })?;
        repair_filesystem(repair, debugfs, &disk.paths.work, environment)?;
        disk.write(Progress::Ready {
            disk: root::evidence(&disk.paths.work)?,
        })?;
        if let Some(target) = &disk.paths.published {
            publish(&disk.paths.work, target)?;
        }
        Ok(disk)
    }

    fn write(&self, progress: Progress) -> Result<(), String> {
        storage::write(
            &self.paths.receipt,
            &Receipt {
                source: self.source.clone(),
                progress,
            },
        )
    }

    pub(super) fn path(&self) -> &Path {
        self.paths.published.as_deref().unwrap_or(&self.paths.work)
    }
}

impl Drop for Disk {
    fn drop(&mut self) {
        if self.paths.clear(&self.source).is_err() {
            eprintln!("Could not remove a verified temporary guest disk replay.");
        }
    }
}

pub(super) fn cleanup(original: &Path, source: &RootEvidence) -> Result<(), String> {
    for boot in [false, true] {
        let paths = Paths::new(original, boot);
        let _guard = storage::lock(&paths.receipt)?;
        paths.clear(source)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn publish(from: &Path, to: &Path) -> Result<(), String> {
    let from = std::ffi::CString::new(from.as_os_str().as_bytes()).map_err(|_| CHANGED)?;
    let to = std::ffi::CString::new(to.as_os_str().as_bytes()).map_err(|_| CHANGED)?;
    if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } != 0 {
        return Err(root::disk_error(std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn publish(_: &Path, _: &Path) -> Result<(), String> {
    Err("Guest image updates require macOS.".into())
}

fn repair_filesystem(
    repair: &Path,
    debugfs: &Path,
    path: &Path,
    environment: &BTreeMap<String, String>,
) -> Result<(), String> {
    let arguments = ["-p".into(), path.to_str().ok_or(CHANGED)?.into()];
    let result = command::run(
        repair,
        &arguments,
        None,
        environment,
        Duration::from_secs(90),
        &AtomicBool::new(false),
    )?;
    if result.timed_out
        || result.cancelled
        || result.output_exceeded
        || !matches!(result.exit_code, Some(0 | 1 | 2))
    {
        return Err(
            "The writable system disk needs filesystem recovery. The original disk was preserved."
                .into(),
        );
    }
    let stats = root::disk_query(debugfs, path, environment, "stats", Duration::from_secs(20))?;
    if !stats.lines().any(|line| {
        line.strip_prefix("Filesystem state:")
            .is_some_and(|value| value.trim() == "clean")
    }) || stats.lines().any(|line| {
        line.starts_with("Filesystem features:")
            && line.split_whitespace().any(|word| word == "needs_recovery")
    }) {
        return Err("The replayed system disk could not be verified as clean.".into());
    }
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

    #[test]
    fn interruption_cleans_the_recorded_replay_and_preserves_unexpected_restore_bytes() {
        let root = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("tugboat-replay-{}", std::process::id()));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let original = root.join("original.ext4");
        let file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&original)
            .unwrap();
        file.set_len(16 << 30).unwrap();
        drop(file);
        let source = root::evidence(&original).unwrap();
        let paths = Paths::new(&original, true);
        root::clone_disk(&original, &paths.work).unwrap();
        let copied = root::evidence(&paths.work).unwrap();
        storage::write(
            &paths.receipt,
            &Receipt {
                source: source.clone(),
                progress: Progress::Ready {
                    disk: copied.clone(),
                },
            },
        )
        .unwrap();
        let published = paths.published.as_ref().unwrap();
        publish(&paths.work, published).unwrap();
        cleanup(&original, &source).unwrap();
        assert!(!published.exists());
        assert!(!paths.receipt.exists());
        assert_eq!(root::evidence(&original).unwrap(), source);
        fs::write(published, "unrelated bytes").unwrap();
        storage::write(
            &paths.receipt,
            &Receipt {
                source: source.clone(),
                progress: Progress::Ready { disk: copied },
            },
        )
        .unwrap();
        assert!(cleanup(&original, &source).is_err());
        assert_eq!(fs::read_to_string(published).unwrap(), "unrelated bytes");
        assert_eq!(root::evidence(&original).unwrap(), source);
        fs::remove_dir_all(root).unwrap();
    }
}
