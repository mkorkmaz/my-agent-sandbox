use super::{
    configure, error, open_existing, regular, require_same_directory, verified_directory,
    verify_journal, APPLICATION_ID, SCHEMA_VERSION,
};
use crate::{
    agents::AgentKind,
    projects::{EnvironmentId, Project},
    storage::{self, data_location::RecordsDirectory},
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::{collections::HashSet, fs, os::unix::fs::MetadataExt, path::Path};

const CHANGE_IDENTITY: &str = include_str!("../migrations/002_project_identity.sql");

pub(crate) fn retire_databases(
    records: &RecordsDirectory,
    retired: &[Project],
) -> Result<(), String> {
    let mut identities = HashSet::new();
    for project in retired {
        if project.id.len() != 32
            || !project.id.bytes().all(|byte| byte.is_ascii_hexdigit())
            || project.environment_id != EnvironmentId::for_agent(&project.id, project.agent_kind)
            || !identities.insert((&project.id, &project.environment_id))
        {
            return Err("A retired usage identity does not match its project and agent.".into());
        }
    }
    let canonical = records.path().join("usage.sqlite3");
    let staging = records.path().join("usage.sqlite3.importing");
    let _writer = storage::lock(&canonical)?;
    retire_database(records, &staging, retired, true)?;
    retire_database(records, &canonical, retired, false)?;
    if canonical.is_file() && records.schema_version()? != 2 {
        records.activate_sqlite()?;
    }
    Ok(())
}

fn retire_database(
    records: &RecordsDirectory,
    path: &Path,
    retired: &[Project],
    staging: bool,
) -> Result<(), String> {
    let directory = verified_directory(records)?;
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(failure) => return Err(error(failure)),
        Ok(_) => {}
    }
    let before = regular(path)?;
    verify_journal(path)?;
    if before.nlink() != 1 {
        return Err(
            "The retired usage database has an additional link. Preserve it for review.".into(),
        );
    }
    let mut database = open_existing(path)?;
    verify_scope(&database, records)?;
    require_same_file(path, &before)?;
    require_same_directory(records, &directory)?;
    configure(&database)?;
    check_references(&database)?;
    let transaction = database
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(error)?;
    let has_imports: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='legacy_imports')", [], |row| row.get(0)).map_err(error)?;
    for project in retired {
        erase_usage(&transaction, project, has_imports)?;
    }
    transaction.commit().map_err(error)?;
    require_same_file(path, &before)?;
    require_same_directory(records, &directory)?;
    if staging {
        let empty: bool = database
            .query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM usage_environments)",
                [],
                |row| row.get(0),
            )
            .map_err(error)?;
        database.close().map_err(|(_, failure)| error(failure))?;
        if empty {
            fs::remove_file(path).map_err(error)?;
            fs::File::open(records.path())
                .and_then(|directory| directory.sync_all())
                .map_err(error)?;
        }
        return Ok(());
    }
    if verify_scope(&database, records)? == 1 {
        upgrade_identity(&mut database, has_imports)?;
    }
    check_references(&database)?;
    verify_scope(&database, records)?;
    database.close().map_err(|(_, failure)| error(failure))?;
    require_same_file(path, &before)?;
    require_same_directory(records, &directory)
}

fn require_same_file(path: &Path, expected: &fs::Metadata) -> Result<(), String> {
    let observed = regular(path)?;
    if (observed.dev(), observed.ino(), observed.nlink()) != (expected.dev(), expected.ino(), 1) {
        return Err("The retired usage database changed after review.".into());
    }
    Ok(())
}

fn verify_scope(database: &Connection, records: &RecordsDirectory) -> Result<i64, String> {
    let application: i64 = database
        .query_row("PRAGMA application_id", [], |row| row.get(0))
        .map_err(error)?;
    let version: i64 = database
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(error)?;
    if application != APPLICATION_ID || !matches!(version, 1 | SCHEMA_VERSION) {
        return Err(
            "The usage reset found an unrecognized database. Its contents were preserved.".into(),
        );
    }
    let identity: String = database
        .query_row(
            "SELECT records_directory_id FROM store_identity WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .map_err(error)?;
    if identity != records.id() {
        return Err(
            "The usage reset found a different records store. Its contents were preserved.".into(),
        );
    }
    Ok(version)
}

fn check_references(database: &Connection) -> Result<(), String> {
    let healthy: String = database
        .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
        .map_err(error)?;
    let orphan: bool = database
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
            [],
            |row| row.get(0),
        )
        .map_err(error)?;
    if healthy != "ok" || orphan {
        return Err("The usage reset found an invalid database. Review the records folder.".into());
    }
    Ok(())
}

fn erase_usage(database: &Connection, project: &Project, has_imports: bool) -> Result<(), String> {
    let agent = match project.agent_kind {
        AgentKind::Codex => "codex",
        AgentKind::ClaudeCode => "claude-code",
    };
    let key: Option<i64> = database.query_row("SELECT environment_key FROM usage_environments WHERE project_id=?1 AND environment_id=?2 AND agent_kind=?3", params![project.id, project.environment_id.to_string(), agent], |row| row.get(0)).optional().map_err(error)?;
    let Some(key) = key else {
        return Ok(());
    };
    database.execute("DELETE FROM session_records WHERE session_key IN (SELECT session_key FROM session_groups WHERE environment_key=?1)", [key]).map_err(error)?;
    database
        .execute("DELETE FROM session_groups WHERE environment_key=?1", [key])
        .map_err(error)?;
    if has_imports {
        database
            .execute("DELETE FROM legacy_imports WHERE environment_key=?1", [key])
            .map_err(error)?;
    }
    database
        .execute(
            "DELETE FROM usage_environments WHERE environment_key=?1",
            [key],
        )
        .map_err(error)?;
    Ok(())
}

fn upgrade_identity(database: &mut Connection, has_imports: bool) -> Result<(), String> {
    database
        .execute_batch("PRAGMA foreign_keys=OFF;")
        .map_err(error)?;
    let transaction = database
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(error)?;
    transaction.execute_batch(CHANGE_IDENTITY).map_err(error)?;
    if has_imports {
        let empty: bool = transaction
            .query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM legacy_imports)",
                [],
                |row| row.get(0),
            )
            .map_err(error)?;
        if empty {
            transaction
                .execute_batch("DROP TABLE legacy_imports;")
                .map_err(error)?;
        }
    }
    check_references(&transaction)?;
    transaction.commit().map_err(error)?;
    database
        .execute_batch("PRAGMA foreign_keys=ON;")
        .map_err(error)
}
