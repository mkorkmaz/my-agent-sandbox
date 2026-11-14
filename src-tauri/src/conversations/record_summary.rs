//! In-memory facts saved for one conversation record.
use super::{record_namespace, SessionRecord};
use crate::{agents::AgentKind, usage, usage_store::Rollup};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(crate) fn rollup(id: &str, record: &SessionRecord) -> Result<Rollup, String> {
    #[cfg(test)]
    let _benchmark_detail = crate::conversations::sqlite_benchmark::Detail::new("rollup");
    let turns = usage::own_turns(&record.turns, id);
    let requests: Vec<_> = turns.values().flat_map(|turn| &turn.requests).collect();
    let cursor_states: BTreeMap<_, _> = record
        .cursors
        .iter()
        .map(|(path, cursor)| (path, cursor.revision_state()))
        .collect();
    let revision = |available: bool| {
        serde_json::to_vec(&(
            record.parser_version,
            &cursor_states,
            &record.lineage,
            &record.lineage_problem,
            &record.warning,
            available,
            &record.source_root_id,
            &record.title,
            (record.agent_kind == Some(AgentKind::ClaudeCode)).then_some(&record.turns),
        ))
        .map_err(|error| error.to_string())
    };
    Ok(Rollup {
        own_summary: usage::summarize(&turns),
        resolved_guest_namespace: record_namespace(id, record),
        has_stored_turns: !record.turns.is_empty(),
        sourced_request_count: requests
            .iter()
            .filter(|request| request.source.is_some())
            .count() as u64,
        missing_turn_links: requests.iter().any(|request| {
            request.source.as_ref().is_some_and(|source| {
                source.origin_turn_id.is_none() || source.root_turn_id.is_none()
            })
        }),
        has_unread_bytes: record.cursors.values().any(usage::Cursor::has_unread_bytes),
        has_ownership_problem: record
            .cursors
            .values()
            .any(usage::Cursor::has_ownership_problem),
        usage_revision: crate::hash::hex(Sha256::digest(revision(record.available)?)),
        offline_usage_revision: crate::hash::hex(Sha256::digest(revision(false)?)),
    })
}
