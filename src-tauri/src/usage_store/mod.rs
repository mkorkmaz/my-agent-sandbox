//! Private, dataset-bound storage for usage facts. Native transcripts remain external.
pub(crate) mod codecs;
mod connection;
mod exact_json;
pub(crate) mod import_rollup;
pub(crate) mod importer;
pub(crate) mod move_support;
mod queries;
pub(crate) mod runtime;
mod writes;
pub(crate) use runtime::{Runtime, SharedStore};
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod record_reads {
    use std::cell::RefCell;
    thread_local! {static READS:RefCell<Option<Vec<i64>>>=const {RefCell::new(None)};}
    pub(crate) fn record(record_id: i64) {
        READS.with(|reads| {
            if let Some(reads) = reads.borrow_mut().as_mut() {
                reads.push(record_id);
            }
        });
    }
    pub(crate) fn begin() {
        READS.with(|reads| *reads.borrow_mut() = Some(Vec::new()));
    }
    pub(crate) fn finish() -> Vec<i64> {
        READS.with(|reads| reads.borrow_mut().take().unwrap_or_default())
    }
}

use crate::{agents::AgentKind, projects::EnvironmentId, usage};
pub(crate) use connection::Store;
use serde::{Deserialize, Serialize};

pub(crate) const APPLICATION_ID: i64 = 0x54554755;
pub(crate) const SCHEMA_VERSION: i64 = 1;
pub(crate) const CLAUDE_PRICING_VERSION: i64 = 1;
pub(crate) const SCHEMA: &str = include_str!("migrations/001_initial.sql");

#[derive(Clone, Debug)]
pub(crate) struct Environment {
    pub project_id: String,
    pub environment_id: EnvironmentId,
    pub agent_kind: AgentKind,
}

impl Environment {
    pub(crate) fn new(
        project_id: String,
        environment_id: EnvironmentId,
        agent_kind: AgentKind,
    ) -> Result<Self, String> {
        if project_id.len() != 32
            || !project_id.bytes().all(|byte| byte.is_ascii_hexdigit())
            || environment_id != EnvironmentId::for_agent(&project_id, agent_kind)
        {
            return Err("The usage environment identity is invalid.".into());
        }
        Ok(Self {
            project_id,
            environment_id,
            agent_kind,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct RecordSlot {
    pub session_id: String,
    pub variant_key: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Rollup {
    pub own_summary: usage::Summary,
    pub resolved_guest_namespace: Option<String>,
    pub has_stored_turns: bool,
    pub sourced_request_count: u64,
    pub missing_turn_links: bool,
    pub has_unread_bytes: bool,
    pub has_ownership_problem: bool,
    pub usage_revision: String,
    pub offline_usage_revision: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoredSummary {
    #[serde(flatten)]
    pub summary: usage::Summary,
    pub offline_usage_revision: String,
}

pub(crate) fn error(error: impl std::fmt::Display) -> String {
    format!("Usage database operation failed: {error}")
}

pub(crate) fn integer(value: u64) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| "A usage count exceeds the SQLite integer range.".into())
}
