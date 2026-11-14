//! Closed-file validation for the existing restart-based records move.
use super::{error, Store, APPLICATION_ID, SCHEMA_VERSION};
use crate::storage::data_location::RecordsDirectory;
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

pub(crate) struct Snapshot([u8; 32]);

fn fingerprint(path: &Path) -> Result<[u8; 32], String> {
    let mut input = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(error)?;
    let metadata = input.metadata().map_err(error)?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 {
        return Err("The usage database must be a private regular file.".into());
    }
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65_536];
    loop {
        let length = input.read(&mut buffer).map_err(error)?;
        if length == 0 {
            return Ok(digest.finalize().into());
        }
        digest.update(&buffer[..length]);
    }
}

pub(crate) fn recover(records: &RecordsDirectory) -> Result<Option<Snapshot>, String> {
    records.verify()?;
    let path = records.path().join("usage.sqlite3");
    if records.schema_version()? == 1 && !path.try_exists().map_err(error)? {
        return Ok(None);
    }
    let store = Store::open(records)?;
    store.integrity_check()?;
    store.close()?;
    records.verify()?;
    if path
        .with_extension("sqlite3-journal")
        .try_exists()
        .map_err(error)?
    {
        return Err("The usage database journal is still present. Preserve it and recover the records before moving them.".into());
    }
    Ok(Some(Snapshot(fingerprint(&path)?)))
}

pub(crate) fn verify_copy(
    records: &RecordsDirectory,
    target: &Path,
    snapshot: Option<&Snapshot>,
) -> Result<(), String> {
    records.verify()?;
    let directory = fs::symlink_metadata(target).map_err(error)?;
    if !directory.is_dir()
        || directory.file_type().is_symlink()
        || directory.permissions().mode() & 0o077 != 0
        || directory.uid() != unsafe { libc::geteuid() }
    {
        return Err(
            "The destination records folder must be private before activating its database.".into(),
        );
    }
    let Some(snapshot) = snapshot else {
        if target.join("usage.sqlite3").symlink_metadata().is_ok() {
            return Err(
                "The destination has an unexpected usage database. Both folders were preserved."
                    .into(),
            );
        }
        return Ok(());
    };
    let source = records.path().join("usage.sqlite3");
    let path = target.join("usage.sqlite3");
    if fingerprint(&source)? != snapshot.0 || fingerprint(&path)? != snapshot.0 {
        return Err(
            "The copied usage database changed or does not match. Both folders were preserved."
                .into(),
        );
    }
    let database = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(error)?;
    verify_identity(&database, records.id())?;
    let mut statement = database.prepare("PRAGMA integrity_check").map_err(error)?;
    let results = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(error)?;
    if results != ["ok"] {
        return Err("The copied usage database failed its integrity check.".into());
    }
    if database
        .prepare("PRAGMA foreign_key_check")
        .map_err(error)?
        .query([])
        .map_err(error)?
        .next()
        .map_err(error)?
        .is_some()
    {
        return Err("The copied usage database contains broken references.".into());
    }
    drop(statement);
    database.close().map_err(|(_, failure)| error(failure))?;
    File::open(&path)
        .and_then(|file| file.sync_all())
        .map_err(error)?;
    File::open(target)
        .and_then(|file| file.sync_all())
        .map_err(error)?;
    Ok(())
}

pub(crate) fn require_file(records: &RecordsDirectory) -> Result<(), String> {
    records.verify()?;
    if records.schema_version()? == 2 {
        fingerprint(&records.path().join("usage.sqlite3"))?;
    }
    Ok(())
}

pub(crate) fn copy_closed_file(source: &Path, target: &Path) -> Result<(), String> {
    let expected = fingerprint(source)?;
    let mut input = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(source)
        .map_err(error)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(target)
        .map_err(error)?;
    std::io::copy(&mut input, &mut output).map_err(error)?;
    output
        .flush()
        .and_then(|_| output.sync_all())
        .map_err(error)?;
    if fingerprint(target)? != expected || fingerprint(source)? != expected {
        return Err(
            "The closed usage database changed while copying. Both folders were preserved.".into(),
        );
    }
    File::open(target.parent().ok_or("Invalid database destination.")?)
        .and_then(|directory| directory.sync_all())
        .map_err(error)
}

fn verify_identity(database: &Connection, expected_identity: &str) -> Result<(), String> {
    let application: i64 = database
        .query_row("PRAGMA application_id", [], |row| row.get(0))
        .map_err(error)?;
    let version: i64 = database
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(error)?;
    if application != APPLICATION_ID || version != SCHEMA_VERSION {
        return Err("The copied usage database has a different application or schema.".into());
    }
    let identity: String = database
        .query_row(
            "SELECT records_directory_id FROM store_identity WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .map_err(error)?;
    if identity != expected_identity {
        return Err("The copied usage database belongs to a different records folder.".into());
    }
    Ok(())
}

pub(crate) fn verify_partial_target(
    target: &Path,
    records: &RecordsDirectory,
) -> Result<(), String> {
    let path = target.join("usage.sqlite3");
    match path.symlink_metadata() {
        Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(failure) => return Err(error(failure)),
        Ok(_) => {}
    }
    fingerprint(&path)?;
    let database = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(error)?;
    verify_identity(&database, records.id())
}

pub(crate) fn verify_closed_match(source: &Path, target: &Path) -> Result<(), String> {
    if fingerprint(source)? != fingerprint(target)? {
        return Err("An original usage database changed after copying. It was preserved.".into());
    }
    Ok(())
}
