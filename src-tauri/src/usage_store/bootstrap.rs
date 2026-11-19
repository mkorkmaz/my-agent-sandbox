use super::{error, Store};
use crate::storage::{self, data_location::RecordsDirectory};
use std::{
    ffi::CString,
    fs::{self, File},
    io,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

pub(crate) fn open_or_initialize(records: &RecordsDirectory) -> Result<Store, String> {
    let canonical = records.path().join("usage.sqlite3");
    records.verify()?;
    let _lock = storage::lock(&canonical)?;
    if records.schema_version()? == 2 {
        let store = Store::open(records)?;
        store.integrity_check()?;
        return Ok(store);
    }
    match fs::symlink_metadata(&canonical) {
        Ok(metadata) if metadata.nlink() == 1 => {}
        Ok(_) => {
            return Err(
                "The unopened usage database has another file link. Both names were preserved."
                    .into(),
            )
        }
        Err(failure) if failure.kind() == io::ErrorKind::NotFound => {
            publish_empty(records, &canonical)?
        }
        Err(failure) => return Err(error(failure)),
    }
    let store = Store::open(records)?;
    store.integrity_check()?;
    let empty: bool = store
        .connection
        .query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM usage_environments)",
            [],
            |row| row.get(0),
        )
        .map_err(error)?;
    if !empty {
        return Err(
            "The unopened usage database already contains sessions. The file was preserved.".into(),
        );
    }
    records.activate_sqlite()?;
    store.require_active()?;
    Ok(store)
}

fn publish_empty(records: &RecordsDirectory, canonical: &Path) -> Result<(), String> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let staging = records.path().join(format!(
        ".usage-init-{}-{time}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    Store::create_staging(records, &staging)?.close()?;
    records.verify()?;
    publish_file(&staging, canonical).map_err(error)?;
    File::open(records.path())
        .and_then(|file| file.sync_all())
        .map_err(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_finishes_a_published_empty_database_without_leaving_a_second_link() {
        let root =
            std::env::temp_dir().join(format!("tugboat-usage-bootstrap-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let records = RecordsDirectory::fixture(&root);
        let canonical = root.join("usage.sqlite3");
        publish_empty(&records, &canonical).unwrap();
        let identity = fs::metadata(&canonical).unwrap().ino();
        assert_eq!(records.schema_version().unwrap(), 1);
        assert_eq!(fs::metadata(&canonical).unwrap().nlink(), 1);
        assert!(fs::read_dir(&root).unwrap().all(|file| !file
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".usage-init-")));

        let unrelated = root.join(".usage-init-unrelated");
        fs::hard_link(&canonical, &unrelated).unwrap();
        assert!(open_or_initialize(&records)
            .err()
            .unwrap()
            .contains("another file link"));
        assert!(unrelated.exists());
        fs::remove_file(unrelated).unwrap();
        let reopened = open_or_initialize(&records).unwrap();
        assert_eq!(fs::metadata(&canonical).unwrap().ino(), identity);
        assert_eq!(records.schema_version().unwrap(), 2);
        reopened.close().unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}

fn publish_file(from: &Path, to: &Path) -> io::Result<()> {
    let from = CString::new(from.as_os_str().as_bytes())?;
    let to = CString::new(to.as_os_str().as_bytes())?;
    #[cfg(target_os = "macos")]
    let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
