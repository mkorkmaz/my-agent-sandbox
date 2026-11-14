use super::{decode, decode_optional, error, json, optional_json};
use crate::conversations::{ProjectExclusion, SessionRecord};
use crate::usage_store::{integer, Rollup};
use rusqlite::{named_params, params, Connection, Row};
use std::collections::BTreeMap;

const METADATA:&str="guest_namespace=:guest_namespace,agent_kind=:agent_kind,title=:title,updated_at=:updated_at,state=:state,available=:available,parser_version=:parser_version,warning=:warning,lineage_json=:lineage_json,lineage_problem=:lineage_problem,source_root_id=:source_root_id,exclusion_root_id=:exclusion_root_id,exclusion_root_cwd=:exclusion_root_cwd,claude_source_json=:claude_source_json";

pub(crate) fn insert_record(
    connection: &Connection,
    session_key: i64,
    variant_key: Option<&str>,
    record: &SessionRecord,
) -> Result<i64, String> {
    connection.execute("INSERT INTO session_records(session_key,variant_key,title,updated_at,state,available,parser_version,lineage_json) VALUES(?1,?2,'',0,'active',0,0,'{}')",params![session_key,variant_key]).map_err(error)?;
    let record_id = connection.last_insert_rowid();
    update_record_metadata(connection, record_id, record)?;
    for (bucket, turn) in &record.turns {
        super::insert_turn(connection, record_id, bucket, turn)?;
    }
    for (path, cursor) in &record.cursors {
        super::insert_cursor(connection, record_id, path, cursor)?;
    }
    Ok(record_id)
}

pub(crate) fn update_record_metadata(
    connection: &Connection,
    record_id: i64,
    record: &SessionRecord,
) -> Result<(), String> {
    let state = json(&record.state)?;
    let problem = optional_json(&record.lineage_problem)?;
    let cwd = record
        .project_exclusion
        .as_ref()
        .map(|exclusion| {
            exclusion
                .root_cwd
                .to_str()
                .ok_or("A saved exclusion path is not valid Unicode.")
        })
        .transpose()?;
    connection.execute(&format!("UPDATE session_records SET {METADATA} WHERE record_id=:record_id"),named_params!{
        ":record_id":record_id,":guest_namespace":record.guest_namespace,":agent_kind":record.agent_kind.map(super::kind),
        ":title":record.title,":updated_at":integer(record.updated_at)?,":state":state.trim_matches('"'),
        ":available":record.available,":parser_version":i64::from(record.parser_version),":warning":record.warning,
        ":lineage_json":json(&record.lineage)?,":lineage_problem":problem.as_deref().map(|value|value.trim_matches('"')),
        ":source_root_id":record.source_root_id,":exclusion_root_id":record.project_exclusion.as_ref().map(|value|value.root_session_id.as_str()),
        ":exclusion_root_cwd":cwd,":claude_source_json":optional_json(&record.claude_source)?
    }).map_err(error)?;
    Ok(())
}

pub(crate) fn read_record_metadata(row: &Row<'_>) -> rusqlite::Result<SessionRecord> {
    #[cfg(test)]
    let _benchmark_detail =
        crate::conversations::sqlite_benchmark::Detail::new("read_record_metadata");
    let agent: Option<String> = row.get("agent_kind")?;
    let state: String = row.get("state")?;
    let problem: Option<String> = row.get("lineage_problem")?;
    let exclusion: Option<String> = row.get("exclusion_root_id")?;
    let parse_enum = |source: &str| {
        serde_json::from_value(serde_json::Value::String(source.into())).map_err(|failure| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(failure),
            )
        })
    };
    Ok(SessionRecord {
        agent_kind: agent
            .as_deref()
            .map(|source| {
                serde_json::from_value(serde_json::Value::String(source.into())).map_err(
                    |failure| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(failure),
                        )
                    },
                )
            })
            .transpose()?,
        claude_source: decode_optional(row, "claude_source_json")?,
        guest_namespace: row.get("guest_namespace")?,
        title: row.get("title")?,
        updated_at: super::unsigned(row, "updated_at")?,
        state: parse_enum(&state)?,
        turns: BTreeMap::new(),
        cursors: BTreeMap::new(),
        warning: row.get("warning")?,
        parser_version: row.get("parser_version")?,
        available: row.get("available")?,
        lineage: decode(row, "lineage_json")?,
        lineage_problem: problem
            .map(|source| {
                serde_json::from_value(serde_json::Value::String(source)).map_err(|failure| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(failure),
                    )
                })
            })
            .transpose()?,
        source_root_id: row.get("source_root_id")?,
        project_exclusion: exclusion
            .map(|root_session_id| {
                Ok::<_, rusqlite::Error>(ProjectExclusion {
                    root_session_id,
                    root_cwd: row.get::<_, String>("exclusion_root_cwd")?.into(),
                })
            })
            .transpose()?,
    })
}

pub(crate) fn read_record(
    connection: &Connection,
    record_id: i64,
) -> Result<SessionRecord, String> {
    #[cfg(test)]
    let _benchmark_detail = crate::conversations::sqlite_benchmark::Detail::new("read_record");
    #[cfg(test)]
    super::super::record_reads::record(record_id);
    let mut record = connection
        .prepare_cached("SELECT * FROM session_records WHERE record_id=?1")
        .map_err(error)?
        .query_row([record_id], read_record_metadata)
        .map_err(error)?;
    let mut statement=connection.prepare_cached("SELECT t.bucket_key,t.native_turn_id,t.status,t.origin_thread_id AS turn_origin_thread_id,t.root_turn_id AS turn_root_turn_id,t.parent_turn_id,q.* FROM usage_turns t LEFT JOIN usage_requests q ON q.turn_key=t.turn_key WHERE t.record_id=?1 ORDER BY t.bucket_key,q.ordinal").map_err(error)?;
    let mut rows = statement.query([record_id]).map_err(error)?;
    while let Some(row) = rows.next().map_err(error)? {
        let bucket: String = row.get("bucket_key").map_err(error)?;
        if !record.turns.contains_key(&bucket) {
            record.turns.insert(
                bucket.clone(),
                crate::usage::TurnUsage {
                    id: row.get("native_turn_id").map_err(error)?,
                    status: row.get("status").map_err(error)?,
                    origin_thread_id: row.get("turn_origin_thread_id").map_err(error)?,
                    root_turn_id: row.get("turn_root_turn_id").map_err(error)?,
                    parent_turn_id: row.get("parent_turn_id").map_err(error)?,
                    requests: Vec::new(),
                },
            );
        }
        if row
            .get::<_, Option<i64>>("ordinal")
            .map_err(error)?
            .is_some()
        {
            record
                .turns
                .get_mut(&bucket)
                .expect("stored turn bucket")
                .requests
                .push(super::read_request(row).map_err(error)?);
        }
    }
    record.cursors = read_cursors(connection, record_id)?;
    Ok(record)
}

pub(crate) fn read_cursors(
    connection: &Connection,
    record_id: i64,
) -> Result<BTreeMap<std::path::PathBuf, crate::usage::Cursor>, String> {
    #[cfg(test)]
    let _benchmark_detail = crate::conversations::sqlite_benchmark::Detail::new("read_cursors");
    let mut result = BTreeMap::new();
    let mut statement=connection.prepare_cached("SELECT source_path,cursor_json FROM source_cursors WHERE record_id=?1 ORDER BY source_path").map_err(error)?;
    let cursors = statement
        .query_map([record_id], |row| {
            use std::os::unix::ffi::OsStringExt;
            let path: Vec<u8> = row.get(0)?;
            Ok((
                std::path::PathBuf::from(std::ffi::OsString::from_vec(path)),
                super::decode_cursor(row, "cursor_json")?,
            ))
        })
        .map_err(error)?;
    for cursor in cursors {
        let (path, cursor) = cursor.map_err(error)?;
        result.insert(path, cursor);
    }
    Ok(result)
}

pub(crate) fn write_rollup(
    connection: &Connection,
    record_id: i64,
    rollup: &Rollup,
) -> Result<(), String> {
    connection.execute("INSERT INTO session_rollups(record_id,own_summary_json,resolved_guest_namespace,has_stored_turns,sourced_request_count,missing_turn_links,has_unread_bytes,has_ownership_problem,usage_revision) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(record_id) DO UPDATE SET own_summary_json=excluded.own_summary_json,resolved_guest_namespace=excluded.resolved_guest_namespace,has_stored_turns=excluded.has_stored_turns,sourced_request_count=excluded.sourced_request_count,missing_turn_links=excluded.missing_turn_links,has_unread_bytes=excluded.has_unread_bytes,has_ownership_problem=excluded.has_ownership_problem,usage_revision=excluded.usage_revision WHERE own_summary_json IS NOT excluded.own_summary_json OR resolved_guest_namespace IS NOT excluded.resolved_guest_namespace OR has_stored_turns IS NOT excluded.has_stored_turns OR sourced_request_count IS NOT excluded.sourced_request_count OR missing_turn_links IS NOT excluded.missing_turn_links OR has_unread_bytes IS NOT excluded.has_unread_bytes OR has_ownership_problem IS NOT excluded.has_ownership_problem OR usage_revision IS NOT excluded.usage_revision",params![record_id,json(&crate::usage_store::StoredSummary{summary:rollup.own_summary.clone(),offline_usage_revision:rollup.offline_usage_revision.clone()})?,rollup.resolved_guest_namespace,rollup.has_stored_turns,integer(rollup.sourced_request_count)?,rollup.missing_turn_links,rollup.has_unread_bytes,rollup.has_ownership_problem,rollup.usage_revision]).map_err(error)?;
    Ok(())
}
