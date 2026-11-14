//! Keep projection memory bounded by one typed request or cursor. Temporary SQL
//! keys reproduce own_turns ordering and canonical source preference in Rust.
use super::{codecs, error, importer, Rollup};
use crate::usage::{self, Cursor, RequestUsage, Summary, TurnUsage};
use rusqlite::{params, Connection};
use std::collections::BTreeMap;

fn temporary_tables(connection: &Connection) -> Result<(), String> {
    connection.execute_batch("CREATE TEMP TABLE IF NOT EXISTS projection_turns(native_id TEXT PRIMARY KEY,status TEXT NOT NULL) WITHOUT ROWID;
CREATE TEMP TABLE IF NOT EXISTS projection_unsourced(target_turn TEXT NOT NULL,ordinal INTEGER NOT NULL,turn_key INTEGER NOT NULL,PRIMARY KEY(target_turn,ordinal)) WITHOUT ROWID;
CREATE TEMP TABLE IF NOT EXISTS projection_exact(source_key TEXT PRIMARY KEY,target_turn TEXT NOT NULL,turn_key INTEGER NOT NULL,ordinal INTEGER NOT NULL,entry_key TEXT NOT NULL,missing_links INTEGER NOT NULL) WITHOUT ROWID;
DELETE FROM temp.projection_turns; DELETE FROM temp.projection_unsourced; DELETE FROM temp.projection_exact;").map_err(error)
}

fn namespace_observation(candidate: &mut Option<String>, ambiguous: &mut bool, next: Option<&str>) {
    let Some(next) = next.filter(|next| !next.is_empty()) else {
        return;
    };
    if candidate
        .as_deref()
        .is_some_and(|candidate| candidate != next)
    {
        *ambiguous = true;
    }
    if candidate.is_none() {
        *candidate = Some(next.into());
    }
}

fn canonical_request(
    connection: &Connection,
    owner: &str,
    turn_key: i64,
    ordinal: i64,
    native_turn: &str,
    request: &RequestUsage,
) -> Result<(), String> {
    let source = request.source.as_ref().expect("sourced request");
    if source.origin_thread_id != owner {
        return Ok(());
    }
    connection.execute("INSERT INTO temp.projection_exact(source_key,target_turn,turn_key,ordinal,entry_key,missing_links) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(source_key) DO UPDATE SET target_turn=excluded.target_turn,turn_key=excluded.turn_key,ordinal=excluded.ordinal,entry_key=excluded.entry_key,missing_links=excluded.missing_links WHERE substr(projection_exact.entry_key,1,7)='source:' AND substr(excluded.entry_key,1,7)!='source:'", params![source.physical_key(),source.origin_turn_id.as_deref().unwrap_or(native_turn),turn_key,ordinal,request.key,source.origin_turn_id.is_none() || source.root_turn_id.is_none()]).map_err(error)?;
    Ok(())
}

fn own_turns(
    connection: &Connection,
    record_id: i64,
    owner: &str,
    namespace: &mut Option<String>,
    ambiguous: &mut bool,
) -> Result<u64, String> {
    let mut statement = connection.prepare("SELECT turn_key,native_turn_id,status,origin_thread_id FROM usage_turns WHERE record_id=?1 ORDER BY bucket_key").map_err(error)?;
    let turns = statement
        .query_map([record_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(error)?;
    let mut count = 0;
    for turn in turns {
        let (turn_key, id, status, origin) = turn.map_err(error)?;
        count += 1;
        let own = origin.as_deref().is_none_or(|origin| origin == owner);
        if own {
            connection.execute("INSERT INTO temp.projection_turns(native_id,status) VALUES(?1,?2) ON CONFLICT(native_id) DO UPDATE SET status=excluded.status",params![id,status]).map_err(error)?;
            connection
                .execute(
                    "DELETE FROM temp.projection_unsourced WHERE target_turn=?1",
                    [&id],
                )
                .map_err(error)?;
        }
        let mut requests = connection
            .prepare("SELECT * FROM usage_requests WHERE turn_key=?1 ORDER BY ordinal")
            .map_err(error)?;
        let rows = requests
            .query_map([turn_key], |row| {
                Ok((row.get::<_, i64>("ordinal")?, codecs::read_request(row)?))
            })
            .map_err(error)?;
        for row in rows {
            let (ordinal, request) = row.map_err(error)?;
            if let Some(source) = &request.source {
                if source.origin_thread_id == owner {
                    namespace_observation(namespace, ambiguous, Some(&source.guest_namespace));
                }
                canonical_request(connection, owner, turn_key, ordinal, &id, &request)?;
                continue;
            }
            if own {
                connection.execute("INSERT INTO temp.projection_unsourced(target_turn,ordinal,turn_key) VALUES(?1,?2,?3)",params![id,ordinal,turn_key]).map_err(error)?;
            }
        }
    }
    connection.execute("INSERT INTO temp.projection_turns(native_id,status) SELECT DISTINCT target_turn,'in-progress' FROM temp.projection_exact WHERE true ON CONFLICT(native_id) DO NOTHING",[]).map_err(error)?;
    // Retain an empty own turn only under the same original map-key test.
    connection.execute("DELETE FROM temp.projection_turns AS p WHERE NOT EXISTS(SELECT 1 FROM temp.projection_unsourced u WHERE u.target_turn=p.native_id) AND NOT EXISTS(SELECT 1 FROM temp.projection_exact x WHERE x.target_turn=p.native_id) AND NOT EXISTS(SELECT 1 FROM usage_turns t WHERE t.record_id=?1 AND t.bucket_key=p.native_id AND NOT EXISTS(SELECT 1 FROM usage_requests q WHERE q.turn_key=t.turn_key))",[record_id]).map_err(error)?;
    Ok(count)
}

fn add_summary(total: &mut Summary, next: &Summary) -> Result<(), String> {
    macro_rules! add_counts { ($($field:ident),+) => { $(total.$field = total.$field.checked_add(next.$field).ok_or("Usage summary exceeds its supported integer range.")?;)+ }; }
    add_counts!(
        input,
        cached,
        cache_write,
        output,
        reasoning,
        total,
        unpriced_requests,
        requests,
        completed_turns,
        assumed_tier_requests,
        missing_cache_requests,
        missing_cached_requests,
        missing_cache_write_requests,
        unmeasured_turns
    );
    total.cost_usd += next.cost_usd;
    if !total.cost_usd.is_finite() {
        return Err("Usage summary cost exceeds its finite range.".into());
    }
    Ok(())
}

fn summarize_request(total: &mut Summary, request: RequestUsage) -> Result<(), String> {
    // Use the existing domain summary for all price and token-knownness rules.
    let turn = TurnUsage {
        id: String::new(),
        status: crate::usage::TurnStatus::InProgress,
        requests: vec![request],
        ..TurnUsage::default()
    };
    add_summary(
        total,
        &usage::summarize(&BTreeMap::from([(String::new(), turn)])),
    )
}

fn summary(connection: &Connection) -> Result<Summary, String> {
    let mut total = Summary::default();
    let mut statement = connection
        .prepare("SELECT native_id,status FROM temp.projection_turns ORDER BY native_id")
        .map_err(error)?;
    let turns = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(error)?;
    for turn in turns {
        let (id, status) = turn.map_err(error)?;
        let before = total.requests;
        let mut unsourced = connection.prepare("SELECT q.* FROM temp.projection_unsourced u JOIN usage_requests q ON q.turn_key=u.turn_key AND q.ordinal=u.ordinal WHERE u.target_turn=?1 ORDER BY u.ordinal").map_err(error)?;
        for request in unsourced
            .query_map([&id], codecs::read_request)
            .map_err(error)?
        {
            summarize_request(&mut total, request.map_err(error)?)?;
        }
        let mut exact = connection.prepare("SELECT q.* FROM temp.projection_exact x JOIN usage_requests q ON q.turn_key=x.turn_key AND q.ordinal=x.ordinal WHERE x.target_turn=?1 ORDER BY x.source_key").map_err(error)?;
        for request in exact
            .query_map([&id], codecs::read_request)
            .map_err(error)?
        {
            summarize_request(&mut total, request.map_err(error)?)?;
        }
        if status == "complete" {
            total.completed_turns += 1;
        }
        if total.requests == before {
            total.unmeasured_turns += 1;
        }
    }
    Ok(total)
}

pub(crate) fn build(
    connection: &Connection,
    owner: &str,
    record_id: i64,
) -> Result<Rollup, String> {
    temporary_tables(connection)?;
    let metadata = connection
        .query_row(
            "SELECT * FROM session_records WHERE record_id=?1",
            [record_id],
            codecs::read_record_metadata,
        )
        .map_err(error)?;
    let mut namespace = None;
    let mut ambiguous = false;
    let mut unread = false;
    let mut ownership_problem = false;
    let mut cursors = connection
        .prepare("SELECT cursor_json FROM source_cursors WHERE record_id=?1 ORDER BY source_path")
        .map_err(error)?;
    for cursor in cursors
        .query_map([record_id], |row| {
            codecs::decode::<Cursor>(row, "cursor_json")
        })
        .map_err(error)?
    {
        let cursor = cursor.map_err(error)?;
        namespace_observation(&mut namespace, &mut ambiguous, cursor.guest_namespace());
        unread |= cursor.has_unread_bytes();
        ownership_problem |= cursor.has_ownership_problem();
    }
    let turns = own_turns(connection, record_id, owner, &mut namespace, &mut ambiguous)?;
    let sourced: i64 = connection
        .query_row("SELECT count(*) FROM temp.projection_exact", [], |row| {
            row.get(0)
        })
        .map_err(error)?;
    let missing: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM temp.projection_exact WHERE missing_links=1)",
            [],
            |row| row.get(0),
        )
        .map_err(error)?;
    Ok(Rollup {
        own_summary: summary(connection)?,
        resolved_guest_namespace: metadata.guest_namespace.clone().or_else(|| {
            if ambiguous {
                None
            } else {
                namespace
            }
        }),
        has_stored_turns: turns != 0,
        sourced_request_count: u64::try_from(sourced).map_err(error)?,
        missing_turn_links: missing,
        has_unread_bytes: unread,
        has_ownership_problem: ownership_problem,
        usage_revision: importer::revision(connection, record_id, &metadata, metadata.available)?,
        offline_usage_revision: importer::revision(connection, record_id, &metadata, false)?,
    })
}
