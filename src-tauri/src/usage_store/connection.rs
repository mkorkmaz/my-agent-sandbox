use super::{error, APPLICATION_ID, SCHEMA, SCHEMA_VERSION};
use crate::storage::data_location::RecordsDirectory;
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Copy, Eq, PartialEq)]
enum RecoveryState { Verified, Required }

pub(super) enum ReconnectDecision { Current, OriginalAvailable, TransactionPending, Replace }

#[derive(Clone)]
struct Binding {
    records: RecordsDirectory,
    path: PathBuf,
    device: u64,
    inode: u64,
    directory_device: u64,
    directory_inode: u64,
    prepared_codex: HashSet<i64>,
    recovery: RecoveryState,
}

pub(crate) struct Store {
    pub(crate) connection: Connection,
    binding: Binding,
}

impl Store {
    pub(crate) fn open(records: &RecordsDirectory) -> Result<Self, String> {
        Self::open_at(records, &records.path().join("usage.sqlite3"))
    }

    pub(crate) fn open_at(records: &RecordsDirectory, path: &Path) -> Result<Self, String> {
        let directory = verified_directory(records)?;
        if path.parent() != Some(records.path()) {
            return Err("The usage database is outside its verified records folder.".into());
        }
        let before = regular(path)?;
        verify_journal(path)?;
        let connection = open_existing(path)?;
        verify_database(&connection, records)?;
        let after = regular(path)?;
        if before.dev() != after.dev() || before.ino() != after.ino() {
            return Err("The usage database changed while opening it. Reconnect the original records folder.".into());
        }
        require_same_directory(records, &directory)?;
        configure(&connection)?;
        let store = Self {
            connection,
            binding: Binding {
                records: records.clone(),
                path: path.into(),
                device: after.dev(),
                inode: after.ino(),
                directory_device: directory.dev(),
                directory_inode: directory.ino(),
                prepared_codex: HashSet::new(),
                recovery: RecoveryState::Verified,
            },
        };
        store.verify()?;
        Ok(store)
    }

    pub(crate) fn create_staging(records: &RecordsDirectory, path: &Path) -> Result<Self, String> {
        let directory = verified_directory(records)?;
        if path.parent() != Some(records.path()) {
            return Err("A usage database must stay in its verified records folder.".into());
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(error)?;
        let metadata = file.metadata().map_err(error)?;
        verify_journal(path)?;
        let mut connection = open_existing(path)?;
        let current = regular(path)?;
        if current.dev() != metadata.dev() || current.ino() != metadata.ino() {
            return Err(
                "The private usage staging file was replaced before initialization.".into(),
            );
        }
        require_same_directory(records, &directory)?;
        configure(&connection)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        transaction.execute_batch(SCHEMA).map_err(error)?;
        transaction
            .execute(
                "INSERT INTO store_identity(singleton,records_directory_id) VALUES(1,?1)",
                [records.id()],
            )
            .map_err(error)?;
        transaction.commit().map_err(error)?;
        file.sync_all().map_err(error)?;
        let store = Self {
            connection,
            binding: Binding {
                records: records.clone(),
                path: path.into(),
                device: metadata.dev(),
                inode: metadata.ino(),
                directory_device: directory.dev(),
                directory_inode: directory.ino(),
                prepared_codex: HashSet::new(),
                recovery: RecoveryState::Verified,
            },
        };
        store.verify()?;
        Ok(store)
    }

    pub(crate) fn verify(&self) -> Result<(), String> {
        self.verify_paths()?;
        verify_database(&self.connection, &self.binding.records)
    }

    fn verify_paths(&self) -> Result<(), String> {
        let directory = verified_directory(&self.binding.records)?;
        if directory.dev() != self.binding.directory_device
            || directory.ino() != self.binding.directory_inode
        {
            return Err("The records folder was replaced. Reconnect its original folder before using saved usage.".into());
        }
        let metadata = regular(&self.binding.path)?;
        verify_journal(&self.binding.path)?;
        if metadata.dev() != self.binding.device || metadata.ino() != self.binding.inode {
            return Err(
                "The usage database was replaced. Reconnect its original records folder.".into(),
            );
        }
        Ok(())
    }

    pub(super) fn prepare_access(&mut self) -> Result<(), String> {
        if self.binding.recovery == RecoveryState::Required {
            self.reopen_verified()?;
        }
        self.require_active()
    }

    pub(super) fn note_failure(&mut self, message: &str) {
        // SQLite can retain a successful statement's cache after a failed commit.
        // Do not read parser progress from that connection on the next poll.
        let code =
            unsafe { rusqlite::ffi::sqlite3_extended_errcode(self.connection.handle()) } & 0xff;
        let message = message.to_ascii_lowercase();
        if matches!(
            code,
            rusqlite::ffi::SQLITE_IOERR
                | rusqlite::ffi::SQLITE_FULL
                | rusqlite::ffi::SQLITE_CORRUPT
                | rusqlite::ffi::SQLITE_NOTADB
        ) || message.contains("disk i/o error")
            || message.contains("database disk image is malformed")
            || message.contains("file is not a database")
            || message.contains("database or disk is full")
            || message.contains("integrity check failed")
            || message.contains("contains broken references")
        {
            self.binding.recovery = RecoveryState::Required;
        }
    }

    fn reopen_verified(&mut self) -> Result<(), String> {
        self.verify_paths()?;
        if !self.connection.is_autocommit() {
            return Err("Usage recovery cannot continue with an unfinished transaction. Preserve the records folder and recover it before retrying.".into());
        }
        let connection = open_existing(&self.binding.path)?;
        self.verify_paths()?;
        verify_database(&connection, &self.binding.records)?;
        self.verify_paths()?;
        configure(&connection)?;
        let fresh = Self {
            connection,
            binding: self.binding.clone(),
        };
        fresh.integrity_check()?;
        fresh.require_active()?;
        self.verify_paths()?;
        let previous = std::mem::replace(&mut self.connection, fresh.connection);
        if let Err((previous, failure)) = previous.close() {
            // Keep the old handle if a statement still prevents its close.
            self.connection = previous;
            return Err(error(failure));
        }
        self.binding.recovery = RecoveryState::Verified;
        self.binding.prepared_codex.clear();
        Ok(())
    }

    pub(crate) fn require_active(&self) -> Result<(), String> {
        self.verify()?;
        if self.binding.records.schema_version()? != 2
            || self
                .binding
                .path
                .file_name()
                .is_none_or(|name| name != "usage.sqlite3")
        {
            return Err(
                "The usage database is not active. Finish the verified import first.".into(),
            );
        }
        Ok(())
    }

    pub(crate) fn is_at(&self, records: &RecordsDirectory) -> bool {
        self.binding.records.path() == records.path() && self.binding.records.id() == records.id()
    }

    pub(super) fn reconnect_decision(&self, records: &RecordsDirectory) -> ReconnectDecision {
        if self.require_active().is_ok() {
            if self.is_at(records) { return ReconnectDecision::Current; }
            return ReconnectDecision::OriginalAvailable;
        }
        if !self.connection.is_autocommit() { return ReconnectDecision::TransactionPending; }
        ReconnectDecision::Replace
    }

    pub(super) fn codex_prepared(&self, environment_key: i64) -> bool {
        self.binding.prepared_codex.contains(&environment_key)
    }
    pub(super) fn mark_codex_prepared(&mut self, environment_key: i64) {
        self.binding.prepared_codex.insert(environment_key);
    }

    pub(super) fn marker_version(&self) -> Result<u32, String> {
        self.binding.records.schema_version()
    }

    pub(crate) fn integrity_check(&self) -> Result<(), String> {
        self.verify()?;
        let mut statement = self
            .connection
            .prepare("PRAGMA integrity_check")
            .map_err(error)?;
        let results = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(error)?;
        if results != ["ok"] {
            return Err("The usage database integrity check failed. Preserve the records folder for recovery.".into());
        }
        let mut statement = self
            .connection
            .prepare("PRAGMA foreign_key_check")
            .map_err(error)?;
        if statement
            .query([])
            .map_err(error)?
            .next()
            .map_err(error)?
            .is_some()
        {
            return Err("The usage database contains broken references.".into());
        }
        Ok(())
    }

    pub(crate) fn close(self) -> Result<(), String> {
        self.try_close().map_err(|(_, failure)| failure)
    }

    /// Keep the bound connection available when shutdown cannot finish.
    pub(crate) fn try_close(self) -> Result<(), (Self, String)> {
        if let Err(failure) = self.prepare_close() {
            return Err((self, failure));
        }
        self.close_connection()
    }

    /// Close an unavailable binding after verifying its dataset at a new path.
    pub(crate) fn try_close_for_reconnect(self, target: &Store) -> Result<(), (Self, String)> {
        if let Err(failure) = self.verify_reconnect_target(target) {
            return Err((self, failure));
        }
        self.close_connection()
    }

    fn verify_reconnect_target(&self, target: &Store) -> Result<(), String> {
        if self.binding.records.id() != target.binding.records.id() {
            return Err("The reconnect target belongs to a different records folder. The current usage connection was preserved.".into());
        }
        if !self.connection.is_autocommit() {
            return Err("Usage storage still has an unfinished transaction. Finish or recover it before reconnecting.".into());
        }
        target.require_active()?;
        target.integrity_check()?;
        target.prepare_close()
    }

    fn close_connection(self) -> Result<(), (Self, String)> {
        let Self {
            connection,
            binding,
        } = self;
        match connection.close() {
            Ok(()) => Ok(()),
            Err((connection, failure)) => Err((
                Self {
                    connection,
                    binding,
                },
                error(failure),
            )),
        }
    }

    fn prepare_close(&self) -> Result<(), String> {
        self.verify()?;
        if !self.connection.is_autocommit() {
            return Err("Usage storage still has an unfinished transaction. Finish recording usage before closing it.".into());
        }
        let database = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&self.binding.path)
            .map_err(error)?;
        let metadata = database.metadata().map_err(error)?;
        if metadata.dev() != self.binding.device || metadata.ino() != self.binding.inode {
            return Err("The usage database changed before shutdown synchronization.".into());
        }
        database.sync_all().map_err(error)?;
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(self.binding.records.path())
            .map_err(error)?;
        let metadata = directory.metadata().map_err(error)?;
        if metadata.dev() != self.binding.directory_device
            || metadata.ino() != self.binding.directory_inode
        {
            return Err("The records folder changed before shutdown synchronization.".into());
        }
        directory.sync_all().map_err(error)?;
        self.verify()
    }
}

fn verified_directory(records: &RecordsDirectory) -> Result<fs::Metadata, String> {
    let metadata = fs::symlink_metadata(records.path()).map_err(|_| {
        "The records folder is unavailable. Reconnect its drive or locate the original folder."
            .to_owned()
    })?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err(
            "The usage records folder must be a private folder owned by the current user.".into(),
        );
    }
    if fs::canonicalize(records.path()).map_err(error)? != records.path() {
        return Err(
            "The usage records folder path contains a symbolic link. Locate the original folder."
                .into(),
        );
    }
    records.verify()?;
    Ok(metadata)
}

fn require_same_directory(
    records: &RecordsDirectory,
    expected: &fs::Metadata,
) -> Result<(), String> {
    let current = verified_directory(records)?;
    if current.dev() != expected.dev() || current.ino() != expected.ino() {
        return Err("The records folder changed during usage database open.".into());
    }
    Ok(())
}

fn verify_journal(path: &Path) -> Result<(), String> {
    let mut name = path.as_os_str().to_os_string();
    name.push("-journal");
    match fs::symlink_metadata(Path::new(&name)) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() && metadata.permissions().mode() & 0o077 == 0 && metadata.uid()==unsafe{libc::geteuid()} && metadata.nlink()==1=>Ok(()),
        Err(failure) if failure.kind()==std::io::ErrorKind::NotFound=>Ok(()),
        _=>Err("The usage rollback journal must be a private regular file. Preserve the records folder for recovery.".into()),
    }
}

fn regular(path: &Path) -> Result<fs::Metadata, String> {
    let metadata = path.symlink_metadata().map_err(|_| {
        "The usage database is unavailable. Reconnect or recover its records folder.".to_owned()
    })?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err("The usage database must be a private regular file.".into());
    }
    Ok(metadata)
}

fn open_existing(path: &Path) -> Result<Connection, String> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(error)
}

fn verify_database(connection: &Connection, records: &RecordsDirectory) -> Result<(), String> {
    let application: i64 = connection
        .query_row("PRAGMA application_id", [], |row| row.get(0))
        .map_err(error)?;
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(error)?;
    if application != APPLICATION_ID || version != SCHEMA_VERSION {
        return Err("This usage database belongs to another application or schema version.".into());
    }
    let identity: String = connection
        .query_row(
            "SELECT records_directory_id FROM store_identity WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .map_err(error)?;
    if identity != records.id() {
        return Err("This usage database belongs to a different records folder.".into());
    }
    Ok(())
}

fn configure(connection: &Connection) -> Result<(), String> {
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(error)?;
    connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA journal_mode=DELETE; PRAGMA synchronous=EXTRA;").map_err(error)?;
    #[cfg(target_os = "macos")]
    connection
        .execute_batch("PRAGMA fullfsync=ON;")
        .map_err(error)?;
    for (pragma, expected) in [
        ("foreign_keys", 1),
        ("trusted_schema", 0),
        ("synchronous", 3),
        ("busy_timeout", 5000),
    ] {
        let actual: i64 = connection
            .query_row(&format!("PRAGMA {pragma}"), [], |row| row.get(0))
            .map_err(error)?;
        if actual != expected {
            return Err(format!("The usage database could not set {pragma}."));
        }
    }
    let journal: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .map_err(error)?;
    if journal != "delete" {
        return Err("The usage database journal mode is unsupported.".into());
    }
    #[cfg(target_os = "macos")]
    if connection
        .query_row("PRAGMA fullfsync", [], |row| row.get::<_, i64>(0))
        .map_err(error)?
        != 1
    {
        return Err("The usage database could not enable fullfsync.".into());
    }
    Ok(())
}
