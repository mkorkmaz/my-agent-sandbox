//! Activation is the only bridge from immutable JSON inputs to the SQL writer.
mod digest;
pub(super) mod guard;
mod inventory;
mod leaf;
mod stream;

use super::{codecs, error, Store};
use crate::storage::data_location::RecordsDirectory;
pub(super) use digest::revision;
use rusqlite::{params, TransactionBehavior};
use std::fs::{self, File};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(1);
const STAGING: &str = "usage.sqlite3.importing";
const DATABASE: &str = "usage.sqlite3";

#[derive(Eq, PartialEq)]
enum ActivationState { Active, Legacy }

#[derive(Eq, PartialEq)]
enum PublicationState { Missing, Staged, Published }

impl PublicationState {
    fn inspect(records: &RecordsDirectory) -> Result<Self, String> {
        if exists(&records.path().join(DATABASE))? { return Ok(Self::Published); }
        if exists(&records.path().join(STAGING))? { return Ok(Self::Staged); }
        Ok(Self::Missing)
    }
}

#[derive(Eq, PartialEq)]
enum StagingState { Empty, VerifiedImport }

fn sync_directory(records: &RecordsDirectory) -> Result<(), String> {
    records.verify()?;
    File::open(records.path())
        .and_then(|file| file.sync_all())
        .map_err(error)
}

fn exists(path: &Path) -> Result<bool, String> {
    match path.symlink_metadata() {
        Ok(_) => Ok(true),
        Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(failure) => Err(error(failure)),
    }
}

fn bootstrap(records: &RecordsDirectory) -> Result<(), String> {
    let scratch = records.path().join(format!(
        ".usage-import-{}-{}",
        std::process::id(),
        NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
    ));
    Store::create_staging(records, &scratch)?.close()?;
    checkpoint("bootstrap-committed");
    records.verify()?;
    fs::hard_link(&scratch, records.path().join(STAGING)).map_err(error)?;
    sync_directory(records)?;
    checkpoint("stage-published");
    fs::remove_file(scratch).map_err(error)?;
    sync_directory(records)
}

// An activated database can be copied under the staging name after a stale
// marker is restored. Identity alone does not prove that its facts are disposable.
fn staging_state(store: &Store, inputs: &inventory::Inventory) -> Result<StagingState, String> {
    store.verify()?;
    let connection = &store.connection;
    let prepared: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM usage_environments WHERE claude_pricing_version != 0)",
            [],
            |row| row.get(0),
        )
        .map_err(error)?;
    if prepared {
        return Err(
            "The staged usage database contains prepared prices. Both copies were preserved."
                .into(),
        );
    }
    let unreceipted:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM usage_environments e LEFT JOIN legacy_imports i USING(environment_key) WHERE i.environment_key IS NULL)",[],|row|row.get(0)).map_err(error)?;
    if unreceipted {
        return Err("The staged usage database contains usage without import receipts. Both copies were preserved.".into());
    }
    let receipt_count: i64 = connection
        .query_row("SELECT count(*) FROM legacy_imports", [], |row| row.get(0))
        .map_err(error)?;
    if receipt_count != 0 {
        // This checks all receipt counts and raw values before any staging write.
        // Exact candidates are reused instead of being erased and reimported.
        verify_candidate(store, inputs)?;
        return Ok(StagingState::VerifiedImport);
    }
    let facts:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM session_groups) OR EXISTS(SELECT 1 FROM session_records) OR EXISTS(SELECT 1 FROM usage_turns) OR EXISTS(SELECT 1 FROM usage_requests) OR EXISTS(SELECT 1 FROM source_cursors) OR EXISTS(SELECT 1 FROM session_rollups)",[],|row|row.get(0)).map_err(error)?;
    if facts {
        return Err("The staged usage database is not an empty import bootstrap. Both copies were preserved.".into());
    }
    store.integrity_check()?;
    Ok(StagingState::Empty)
}

fn import_sources(store: &mut Store, sources: &inventory::Inventory) -> Result<(), String> {
    store.verify()?;
    let transaction = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(error)?;
    for source in &sources.sources {
        let environment_key =
            codecs::ensure_environment(&transaction, &source.environment, 0, &[])?;
        let (version, warnings, counts) =
            stream::stream(&transaction, environment_key, &source.file, false).map_err(
                |failure| {
                    format!(
                        "Legacy usage import, {}: {failure}",
                        source.fingerprint.relative_path
                    )
                },
            )?;
        if version != source.version {
            return Err(
                "A legacy ledger changed version during import. Original records were preserved."
                    .into(),
            );
        }
        transaction
            .execute(
                "UPDATE usage_environments SET warnings_json=?1 WHERE environment_key=?2",
                params![codecs::json(&warnings)?, environment_key],
            )
            .map_err(error)?;
        transaction.execute("INSERT INTO legacy_imports(relative_path,environment_key,ledger_version,source_sha256,source_bytes,record_count,turn_count,request_count) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", params![source.fingerprint.relative_path,environment_key,version,source.fingerprint.digest,super::integer(source.fingerprint.length)?,super::integer(counts.records)?,super::integer(counts.turns)?,super::integer(counts.requests)?]).map_err(error)?;
    }
    checkpoint("facts-written-before-commit");
    transaction.commit().map_err(error)?;
    checkpoint("facts-committed");
    Ok(())
}

fn verify_rollups(connection: &rusqlite::Connection, environment_key: i64) -> Result<(), String> {
    let mut statement=connection.prepare("SELECT r.record_id,g.native_session_id FROM session_groups g JOIN session_records r USING(session_key) WHERE g.environment_key=?1 ORDER BY g.native_session_id,r.variant_key").map_err(error)?;
    let rows = statement
        .query_map([environment_key], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(error)?;
    for row in rows {
        let (record_id, owner) = row.map_err(error)?;
        let expected = super::import_rollup::build(connection, &owner, record_id)?;
        let actual = connection
            .query_row(
                "SELECT * FROM session_rollups WHERE record_id=?1",
                [record_id],
                super::queries::read_rollup,
            )
            .map_err(error)?;
        if actual != expected {
            return Err("The staged usage projection differs from its raw facts. Both copies were preserved.".into());
        }
    }
    Ok(())
}

fn verify_candidate(store: &Store, inputs: &inventory::Inventory) -> Result<(), String> {
    store.verify()?;
    let connection = &store.connection;
    connection
        .execute_batch("PRAGMA temp_store=FILE;")
        .map_err(error)?;
    let receipt_count: u64 = connection
        .query_row("SELECT count(*) FROM legacy_imports", [], |row| {
            stream::count_row(row, 0)
        })
        .map_err(error)?;
    let environment_count: u64 = connection
        .query_row("SELECT count(*) FROM usage_environments", [], |row| {
            stream::count_row(row, 0)
        })
        .map_err(error)?;
    if receipt_count != inputs.sources.len() as u64 || environment_count != receipt_count {
        return Err("The usage database receipt set does not match the legacy source set.".into());
    }
    for source in &inputs.sources {
        let environment_key = codecs::environment_key(connection, &source.environment)?
            .ok_or("A legacy usage environment is missing from the candidate.")?;
        let receipt = connection.query_row("SELECT environment_key,ledger_version,source_sha256,source_bytes,record_count,turn_count,request_count FROM legacy_imports WHERE relative_path=?1", [&source.fingerprint.relative_path], |row| Ok((row.get::<_,i64>(0)?,row.get::<_,u32>(1)?,row.get::<_,Vec<u8>>(2)?,stream::count_row(row,3)?,stream::Counts{ records:stream::count_row(row,4)?,turns:stream::count_row(row,5)?,requests:stream::count_row(row,6)? }))).map_err(error)?;
        if receipt.0 != environment_key
            || receipt.1 != source.version
            || receipt.2 != source.fingerprint.digest
            || receipt.3 != source.fingerprint.length
        {
            return Err("A usage import receipt differs from its original source. Both copies were preserved.".into());
        }
        let (version, warnings, counts) =
            stream::stream(connection, environment_key, &source.file, true).map_err(|failure| {
                format!(
                    "Legacy usage parity, {}: {failure}",
                    source.fingerprint.relative_path
                )
            })?;
        let stored_warnings: String = connection
            .query_row(
                "SELECT warnings_json FROM usage_environments WHERE environment_key=?1",
                [environment_key],
                |row| row.get(0),
            )
            .map_err(error)?;
        let stored_warnings: Vec<String> = serde_json::from_str(&stored_warnings).map_err(error)?;
        if version != receipt.1 || counts != receipt.4 || stored_warnings != warnings {
            return Err("Legacy usage counts or warnings did not round-trip.".into());
        }
        let pricing_version: i64 = connection
            .query_row(
                "SELECT claude_pricing_version FROM usage_environments WHERE environment_key=?1",
                [environment_key],
                |row| row.get(0),
            )
            .map_err(error)?;
        if pricing_version != 0 {
            return Err("An unactivated legacy usage database contains prepared prices. Both copies were preserved.".into());
        }
        verify_rollups(connection, environment_key)?;
    }
    store.integrity_check()
}

fn recheck(records: &RecordsDirectory, original: &inventory::Inventory) -> Result<(), String> {
    let current = inventory::inventory(records)?;
    if !inventory::matches(original, &current) {
        return Err("Legacy usage sources changed during import. Original records were preserved; restart to retry.".into());
    }
    Ok(())
}

fn remove_matching_staging(records: &RecordsDirectory) -> Result<(), String> {
    let staging = records.path().join(STAGING);
    if !exists(&staging)? {
        return Ok(());
    }
    let stage = fs::symlink_metadata(&staging).map_err(error)?;
    let active = fs::symlink_metadata(records.path().join(DATABASE)).map_err(error)?;
    if !stage.is_file()
        || stage.file_type().is_symlink()
        || stage.dev() != active.dev()
        || stage.ino() != active.ino()
    {
        return Ok(());
    }
    fs::remove_file(staging).map_err(error)?;
    sync_directory(records)
}

pub(crate) fn activate(records: &RecordsDirectory) -> Result<(), String> {
    records.verify()?;
    let activation = match records.schema_version()? {
        2 => ActivationState::Active,
        _ => ActivationState::Legacy,
    };
    if activation == ActivationState::Active {
        let store = Store::open(records)?;
        store.integrity_check()?;
        return store.close();
    }
    let before_backup = inventory::inventory(records)?;
    inventory::backup_sources(records, &before_backup)?;
    let inputs = inventory::inventory(records)?;
    if before_backup
        .sources
        .iter()
        .map(|source| &source.fingerprint)
        .ne(inputs.sources.iter().map(|source| &source.fingerprint))
    {
        return Err("A legacy source changed while creating its immutable backup.".into());
    }
    let canonical = records.path().join(DATABASE);
    let publication = PublicationState::inspect(records)?;
    if publication == PublicationState::Published {
        let published = Store::open_at(records, &canonical)?;
        verify_candidate(&published, &inputs)?;
        recheck(records, &inputs)?;
        published.close()?;
        remove_matching_staging(records)?;
        sync_directory(records)?;
        checkpoint("before-marker");
        records.activate_sqlite()?;
        checkpoint("marker-activated");
        return Ok(());
    }
    let staging = records.path().join(STAGING);
    if publication == PublicationState::Missing {
        bootstrap(records)?;
    }
    let mut candidate = Store::open_at(records, &staging)?;
    candidate
        .connection
        .execute_batch("PRAGMA temp_store=FILE;")
        .map_err(error)?;
    if staging_state(&candidate, &inputs)? == StagingState::Empty {
        import_sources(&mut candidate, &inputs)?;
        verify_candidate(&candidate, &inputs)?;
    }
    recheck(records, &inputs)?;
    checkpoint("parity-complete");
    candidate.close()?;
    sync_directory(records)?;
    records.verify()?;
    fs::hard_link(&staging, &canonical).map_err(error)?;
    sync_directory(records)?;
    checkpoint("canonical-published");
    remove_matching_staging(records)?;
    recheck(records, &inputs)?;
    sync_directory(records)?;
    checkpoint("before-marker");
    records.activate_sqlite()?;
    checkpoint("marker-activated");
    Ok(())
}

fn checkpoint(_phase: &str) {
    #[cfg(test)]
    if std::env::var("TUGBOAT_IMPORT_FAILPOINT").as_deref() == Ok(_phase) {
        std::process::exit(91);
    }
}

#[cfg(test)]
mod tests;
