use super::{error, Environment};
use crate::conversations::{Ledger, SessionRecord};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{de::DeserializeOwned, Serialize};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

mod record;
mod request;
pub(crate) use record::{
    insert_record, read_cursors, read_record, read_record_metadata, update_record_metadata,
    write_rollup,
};
pub(crate) use request::{insert_request, read_request, request_upsert};

pub(crate) fn json(value: &impl Serialize) -> Result<String, String> {
    serde_json::to_string(value).map_err(error)
}

pub(crate) fn optional_json<T: Serialize>(value: &Option<T>) -> Result<Option<String>, String> {
    value.as_ref().map(json).transpose()
}

pub(crate) fn decode<T: DeserializeOwned>(row: &Row<'_>, column: &str) -> rusqlite::Result<T> {
    let source: String = row.get(column)?;
    super::exact_json::decode(&source).map_err(|failure| {
        rusqlite::Error::FromSqlConversionFailure(
            row.as_ref().column_index(column).unwrap_or(0),
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                failure,
            )),
        )
    })
}

pub(crate) fn decode_cursor(row: &Row<'_>, column: &str) -> rusqlite::Result<crate::usage::Cursor> {
    // Cursor, Exact and Counter contain integers, strings and flags, without floats.
    // Keep prices and correction evidence on the exact numeric decoder above.
    let invalid = |message| {
        rusqlite::Error::FromSqlConversionFailure(
            row.as_ref().column_index(column).unwrap_or(0),
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                message,
            )),
        )
    };
    let source = row
        .get_ref(column)?
        .as_str()
        .map_err(|_| invalid("The stored usage cursor does not match its type."))?;
    std::io::copy(
        &mut super::importer::guard::GuardedRead::new(source.as_bytes()),
        &mut std::io::sink(),
    )
    .map_err(|_| invalid("The stored usage cursor exceeds its decoding limits."))?;
    serde_json::from_str(source)
        .map_err(|_| invalid("The stored usage cursor does not match its type."))
}

pub(crate) fn decode_optional<T: DeserializeOwned>(
    row: &Row<'_>,
    column: &str,
) -> rusqlite::Result<Option<T>> {
    let source: Option<String> = row.get(column)?;
    source
        .map(|source| {
            super::exact_json::decode(&source).map_err(|failure| {
                rusqlite::Error::FromSqlConversionFailure(
                    row.as_ref().column_index(column).unwrap_or(0),
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        failure,
                    )),
                )
            })
        })
        .transpose()
}

pub(crate) fn environment_key(
    connection: &Connection,
    environment: &Environment,
) -> Result<Option<i64>, String> {
    connection.query_row("SELECT environment_key FROM usage_environments WHERE project_id=?1 AND environment_id=?2 AND agent_kind=?3",params![environment.project_id,environment.environment_id.to_string(),kind(environment.agent_kind)],|row|row.get(0)).optional().map_err(error)
}

pub(crate) fn ensure_environment(
    connection: &Connection,
    environment: &Environment,
    pricing_version: i64,
    warnings: &[String],
) -> Result<i64, String> {
    connection.execute("INSERT INTO usage_environments(project_id,environment_id,agent_kind,claude_pricing_version,warnings_json) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(project_id,environment_id) DO NOTHING",params![environment.project_id,environment.environment_id.to_string(),kind(environment.agent_kind),pricing_version,json(&warnings)?]).map_err(error)?;
    environment_key(connection, environment)?
        .ok_or_else(|| "The saved usage environment has a conflicting identity.".into())
}

pub(crate) fn kind(kind: crate::agents::AgentKind) -> &'static str {
    match kind {
        crate::agents::AgentKind::Codex => "codex",
        crate::agents::AgentKind::ClaudeCode => "claude-code",
    }
}

pub(crate) fn ensure_group(
    connection: &Connection,
    environment_key: i64,
    id: &str,
    has_variants_bucket: bool,
) -> Result<i64, String> {
    if !crate::conversations::valid_id(id) {
        return Err("A saved usage session ID is invalid.".into());
    }
    connection.execute("INSERT INTO session_groups(environment_key,native_session_id,has_variants_bucket) VALUES(?1,?2,?3) ON CONFLICT(environment_key,native_session_id) DO NOTHING",params![environment_key,id,has_variants_bucket]).map_err(error)?;
    if has_variants_bucket {
        connection.execute("UPDATE session_groups SET has_variants_bucket=1 WHERE environment_key=?1 AND native_session_id=?2 AND has_variants_bucket=0",params![environment_key,id]).map_err(error)?;
    }
    connection.query_row("SELECT session_key FROM session_groups WHERE environment_key=?1 AND native_session_id=?2",params![environment_key,id],|row|row.get(0)).map_err(error)
}

pub(crate) fn insert_turn(
    connection: &Connection,
    record_id: i64,
    bucket_key: &str,
    turn: &crate::usage::TurnUsage,
) -> Result<i64, String> {
    connection.execute("INSERT INTO usage_turns(record_id,bucket_key,native_turn_id,status,origin_thread_id,root_turn_id,parent_turn_id) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![record_id,bucket_key,turn.id,turn.status,turn.origin_thread_id,turn.root_turn_id,turn.parent_turn_id]).map_err(error)?;
    let turn_key = connection.last_insert_rowid();
    for (ordinal, request) in turn.requests.iter().enumerate() {
        insert_request(connection, turn_key, ordinal, request)?;
    }
    Ok(turn_key)
}

pub(crate) fn insert_cursor(
    connection: &Connection,
    record_id: i64,
    path: &Path,
    cursor: &crate::usage::Cursor,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO source_cursors(record_id,source_path,cursor_json) VALUES(?1,?2,?3)",
            params![record_id, path.as_os_str().as_bytes(), json(cursor)?],
        )
        .map_err(error)?;
    Ok(())
}

pub(crate) fn add_record(
    ledger: &mut Ledger,
    id: String,
    variant: Option<String>,
    record: SessionRecord,
) {
    if let Some(variant) = variant {
        ledger
            .variants
            .entry(id)
            .or_default()
            .insert(variant, record);
        return;
    }
    ledger.sessions.insert(id, record);
}

pub(crate) fn unsigned(row: &Row<'_>, column: &str) -> rusqlite::Result<u64> {
    let value: i64 = row.get(column)?;
    u64::try_from(value).map_err(|_| {
        rusqlite::Error::IntegralValueOutOfRange(
            row.as_ref().column_index(column).unwrap_or(0),
            value,
        )
    })
}

pub(crate) fn optional_unsigned(row: &Row<'_>, column: &str) -> rusqlite::Result<Option<u64>> {
    let value: Option<i64> = row.get(column)?;
    value
        .map(|value| {
            u64::try_from(value).map_err(|_| {
                rusqlite::Error::IntegralValueOutOfRange(
                    row.as_ref().column_index(column).unwrap_or(0),
                    value,
                )
            })
        })
        .transpose()
}
