//! Derived usage facts. The saved digest uses the same inputs as the JSON reader.
use super::*;
use crate::usage_store::{RecordSlot, Rollup};

// Keep the comparison independent of the summary used by store writes.
#[cfg(test)]
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

fn selected_slot(id: &str) -> RecordSlot {
    RecordSlot {
        session_id: id.into(),
        variant_key: None,
    }
}

pub(super) fn graph(ledger: &Ledger, rollups: &BTreeMap<RecordSlot, Rollup>) -> Graph {
    Graph::new(
        ledger
            .sessions
            .iter()
            .filter(|(_, record)| record.project_exclusion.is_none())
            .map(|(id, record)| {
                (
                    id.clone(),
                    graph::Node {
                        agent_kind: record.agent_kind,
                        guest_namespace: record_namespace(id, record).or_else(|| {
                            rollups
                                .get(&selected_slot(id))
                                .and_then(|rollup| rollup.resolved_guest_namespace.clone())
                        }),
                        lineage: record.lineage.clone(),
                        problem: record.lineage_problem.clone(),
                        cwd: PathBuf::new(),
                        source_root_id: record.source_root_id.clone(),
                    },
                )
            })
            .collect(),
    )
}

fn coverage(id: &str, record: &SessionRecord, value: &Rollup, graph: &Graph) -> usage::Coverage {
    let summary = &value.own_summary;
    if record.agent_kind == Some(AgentKind::ClaudeCode) {
        return if summary.requests > 0 {
            usage::Coverage::Partial
        } else {
            usage::Coverage::Unknown
        };
    }
    if record.lineage.kind == lineage::Kind::Unknown
        || record.lineage.source_version.as_deref() != Some("0.167.0")
        || value.has_ownership_problem
    {
        return usage::Coverage::Unknown;
    }
    let gap = value.has_unread_bytes
        || !record.available
        || record.warning.is_some()
        || record.lineage_problem.is_some()
        || summary.unmeasured_turns > 0
        || value.sourced_request_count != summary.requests
        || value.missing_turn_links
        || !matches!(
            graph.relation(id).status,
            graph::Status::Root | graph::Status::Resolved
        );
    if gap {
        return if value.sourced_request_count > 0 {
            usage::Coverage::Partial
        } else {
            usage::Coverage::Unknown
        };
    }
    usage::Coverage::Complete
}

pub(super) fn dashboard(
    ledger: Ledger,
    rollups: BTreeMap<RecordSlot, Rollup>,
    mut sources: HashMap<String, Conversation>,
) -> Result<Dashboard, String> {
    let graph = graph(&ledger, &rollups);
    let mut usage = usage::Summary::default();
    let mut project_coverage = Vec::new();
    for (id, variants) in &ledger.variants {
        for (key, _) in variants
            .iter()
            .filter(|(_, record)| record.project_exclusion.is_none())
        {
            let value = rollups
                .get(&RecordSlot {
                    session_id: id.clone(),
                    variant_key: Some(key.clone()),
                })
                .ok_or("The saved usage summary is missing. Reopen the verified records folder.")?;
            if value.own_summary.requests > 0 || value.has_stored_turns {
                usage.add(&value.own_summary);
                project_coverage.push(usage::Coverage::Partial);
            }
        }
    }
    let mut conversations = Vec::new();
    for (id, record) in ledger
        .sessions
        .iter()
        .filter(|(_, record)| record.project_exclusion.is_none())
    {
        let value = rollups
            .get(&selected_slot(id))
            .ok_or("The saved usage summary is missing. Reopen the verified records folder.")?;
        usage.add(&value.own_summary);
        let family: Vec<_> = std::iter::once(id.clone())
            .chain(graph.descendants(id))
            .collect();
        let mut family_usage = usage::Summary::default();
        let mut family_coverage = Vec::new();
        for member in family {
            if let (Some(record), Some(value)) = (
                project_record(&ledger, &member),
                rollups.get(&selected_slot(&member)),
            ) {
                family_usage.add(&value.own_summary);
                family_coverage.push(coverage(&member, record, value, &graph));
            }
        }
        let coverage = combined_coverage(&family_coverage);
        project_coverage.push(coverage);
        let metadata = sources.remove(id);
        conversations.push(Conversation {
            id: id.clone(),
            environment_id: None,
            agent_kind: record.agent_kind.unwrap_or(AgentKind::Codex),
            usage_available: true,
            title: record.title.clone(),
            updated_at: record.updated_at,
            state: record.state,
            available: record.available,
            usage: value.own_summary.clone(),
            warning: record.warning.clone(),
            lineage: record.lineage.clone(),
            relation_status: graph::Status::Unknown,
            root_session_id: None,
            subagent_count: 0,
            family_usage,
            coverage,
            usage_revision: value.usage_revision.clone(),
            source: metadata
                .as_ref()
                .map(|value| value.source.clone())
                .unwrap_or_default(),
            title_known: true,
            cwd: metadata
                .as_ref()
                .map(|value| value.cwd.clone())
                .unwrap_or_default(),
            lineage_problem: record.lineage_problem.clone(),
            source_root_id: record.source_root_id.clone(),
        });
    }
    apply_graph(&mut conversations, &graph);
    conversations.sort_by(|first, second| {
        second
            .updated_at
            .cmp(&first.updated_at)
            .then_with(|| first.id.cmp(&second.id))
    });
    Ok(Dashboard {
        usage_available: true,
        conversations,
        usage,
        pricing_date: usage::PRICING_DATE.into(),
        anthropic_pricing_date: usage::anthropic::PRICING_DATE.into(),
        warnings: ledger.warnings,
        coverage: combined_coverage(&project_coverage),
    })
}
