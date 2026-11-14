use super::source_state::SourceState;
use crate::usage::{self, Coverage, Summary, TurnUsage};
use crate::{conversations::graph::Status, conversations::lineage::Lineage};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[cfg(test)]
pub(super) fn from_ledger(
    ledger: &super::Ledger,
    session_id: &str,
) -> Result<ConversationUsageDetails, String> {
    from_ledger_graph(ledger, session_id, &super::ledger_graph(ledger))
}

pub(super) fn from_ledger_graph(
    ledger: &super::Ledger,
    session_id: &str,
    graph: &crate::conversations::graph::Graph,
) -> Result<ConversationUsageDetails, String> {
    let selected = super::project_record(ledger, session_id)
        .ok_or("Saved session not found in this project.")?;
    let claude = selected.agent_kind == Some(crate::agents::AgentKind::ClaudeCode);
    let root_session_id = graph.relation(session_id).root_session_id;
    let root_turns = root_session_id
        .as_ref()
        .filter(|_| !claude)
        .and_then(|root| {
            super::project_record(ledger, root).map(|record| usage::own_turns(&record.turns, root))
        });
    let mut agents = Vec::new();
    let mut warnings = Vec::new();
    if claude {
        warnings.push(super::claude::usage::COVERAGE_NOTE.into());
    }
    for id in std::iter::once(session_id.to_owned()).chain(graph.descendants(session_id)) {
        let Some(record) = super::project_record(ledger, &id) else {
            continue;
        };
        let turns = usage::own_turns(&record.turns, &id);
        let relation = graph.relation(&id);
        if let Some(warning) = &record.warning {
            warnings.push(format!("{}: {warning}", record.title));
        }
        if record.source_state() == SourceState::Unavailable {
            warnings.push(format!(
                "{}: Session file unavailable; saved usage is retained.",
                record.title
            ));
        }
        let relation_warning = match relation.status {
            Status::Orphan => Some("Parent session is unavailable; own usage is retained."),
            Status::Invalid => {
                Some("Session relationship is invalid; family attribution is unverified.")
            }
            Status::Unknown
                if record.lineage.kind == crate::conversations::lineage::Kind::Subagent =>
            {
                Some("Subagent relationship could not be verified.")
            }
            _ => None,
        };
        if let Some(warning) = relation_warning {
            warnings.push(format!("{}: {warning}", record.title));
        }
        agents.push(AgentUsageDetails {
            agent_kind: record.agent_kind,
            session_id: id.clone(),
            title: record.title.clone(),
            available: record.available,
            lineage: record.lineage.clone(),
            relation_status: relation.status,
            own_usage: usage::summarize(&turns),
            coverage: super::own_coverage(&id, record, &graph),
            turns: turns.into_values().collect(),
        });
    }
    Ok(build(
        session_id,
        root_session_id,
        agents,
        root_turns.as_ref(),
        super::family_coverage(session_id, ledger, &graph),
        warnings,
    ))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsageDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_kind: Option<crate::agents::AgentKind>,
    pub session_id: String,
    pub title: String,
    pub available: bool,
    pub lineage: Lineage,
    pub relation_status: Status,
    pub own_usage: Summary,
    pub coverage: Coverage,
    pub turns: Vec<TurnUsage>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestRef {
    pub session_id: String,
    pub turn_id: String,
    pub request_key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnRef {
    pub session_id: String,
    pub turn_id: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnassignedUsage {
    pub usage: Summary,
    pub request_refs: Vec<RequestRef>,
    pub unmeasured_turn_refs: Vec<TurnRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptUsage {
    pub turn_id: String,
    pub status: String,
    pub own_usage: Summary,
    pub descendants_usage: Summary,
    pub combined_usage: Summary,
    pub request_refs: Vec<RequestRef>,
    pub unmeasured_turn_refs: Vec<TurnRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationUsageDetails {
    pub session_id: String,
    pub root_session_id: Option<String>,
    pub own_usage: Summary,
    pub descendants_usage: Summary,
    pub combined_usage: Summary,
    pub coverage: Coverage,
    pub warnings: Vec<String>,
    pub agents: Vec<AgentUsageDetails>,
    pub user_prompt_count: Option<u64>,
    pub agent_turn_count: u64,
    pub prompts: Vec<PromptUsage>,
    pub unassigned: UnassignedUsage,
}

// Attribution is a partition of canonical turns, never another stored spending copy.
// Missing or conflicting request attribution leaves the entire turn unassigned.
fn attributed_prompt<'a>(
    session: &str,
    turn: &'a TurnUsage,
    root_turns: &BTreeMap<String, TurnUsage>,
) -> Option<&'a str> {
    if turn
        .origin_thread_id
        .as_deref()
        .is_some_and(|owner| owner != session)
    {
        return None;
    }
    let mut candidate = turn.root_turn_id.as_deref();
    if turn.requests.is_empty() && turn.origin_thread_id.as_deref() != Some(session) {
        return None;
    }
    for request in &turn.requests {
        let source = request.source.as_ref()?;
        if source.origin_thread_id != session || source.origin_turn_id.as_deref() != Some(&turn.id)
        {
            return None;
        }
        let root = source.root_turn_id.as_deref()?;
        if candidate.is_some_and(|previous| previous != root) {
            return None;
        }
        candidate = Some(root);
    }
    candidate.filter(|root| root_turns.contains_key(*root))
}

fn references(agent: &AgentUsageDetails, turn: &TurnUsage) -> (Vec<RequestRef>, Vec<TurnRef>) {
    let requests = turn
        .requests
        .iter()
        .map(|request| RequestRef {
            session_id: agent.session_id.clone(),
            turn_id: turn.id.clone(),
            request_key: request.key.clone(),
        })
        .collect();
    let empty = if turn.requests.is_empty() {
        vec![TurnRef {
            session_id: agent.session_id.clone(),
            turn_id: turn.id.clone(),
        }]
    } else {
        Vec::new()
    };
    (requests, empty)
}

pub fn build(
    session_id: &str,
    root_session_id: Option<String>,
    agents: Vec<AgentUsageDetails>,
    root_turns: Option<&BTreeMap<String, TurnUsage>>,
    coverage: Coverage,
    warnings: Vec<String>,
) -> ConversationUsageDetails {
    let mut own_usage = Summary::default();
    let mut descendants_usage = Summary::default();
    let mut combined_usage = Summary::default();
    let mut prompts = BTreeMap::new();
    let mut unassigned = UnassignedUsage::default();
    let mut agent_turn_count = 0;
    if root_session_id.as_deref() == Some(session_id) {
        if let Some(turns) = root_turns {
            for turn in turns.values() {
                prompts.insert(turn.id.clone(), prompt(turn));
            }
        }
    }
    for agent in &agents {
        if agent.session_id == session_id {
            own_usage.add(&agent.own_usage);
        } else {
            descendants_usage.add(&agent.own_usage);
        }
        combined_usage.add(&agent.own_usage);
        agent_turn_count += agent.turns.len() as u64;
        for turn in &agent.turns {
            let summary = usage::summarize(&BTreeMap::from([(turn.id.clone(), turn.clone())]));
            let (mut request_refs, mut empty_refs) = references(agent, turn);
            let attributed =
                root_turns.and_then(|turns| attributed_prompt(&agent.session_id, turn, turns));
            if let Some(root_turn) = attributed {
                let root = &root_turns.expect("attribution requires root turns")[root_turn];
                let prompt = prompts
                    .entry(root_turn.to_owned())
                    .or_insert_with(|| prompt(root));
                if agent.session_id == session_id {
                    prompt.own_usage.add(&summary);
                } else {
                    prompt.descendants_usage.add(&summary);
                }
                prompt.combined_usage.add(&summary);
                prompt.request_refs.append(&mut request_refs);
                prompt.unmeasured_turn_refs.append(&mut empty_refs);
            } else {
                unassigned.usage.add(&summary);
                unassigned.request_refs.append(&mut request_refs);
                unassigned.unmeasured_turn_refs.append(&mut empty_refs);
            }
        }
    }
    ConversationUsageDetails {
        session_id: session_id.into(),
        root_session_id,
        own_usage,
        descendants_usage,
        combined_usage,
        coverage,
        warnings,
        agents,
        user_prompt_count: root_turns.map(|_| prompts.len() as u64),
        agent_turn_count,
        prompts: prompts.into_values().collect(),
        unassigned,
    }
}

fn prompt(turn: &TurnUsage) -> PromptUsage {
    PromptUsage {
        turn_id: turn.id.clone(),
        status: turn.status.as_str().into(),
        own_usage: Summary::default(),
        descendants_usage: Summary::default(),
        combined_usage: Summary::default(),
        request_refs: Vec::new(),
        unmeasured_turn_refs: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::collections::BTreeSet;

    fn examples() -> Vec<Value> {
        serde_json::from_str::<Value>(include_str!(
            "../../../tests/fixtures/codex-subagents/contracts/v2-examples.json"
        ))
        .unwrap()["examples"]
            .as_array()
            .unwrap()
            .clone()
    }

    fn agents(details: &Value) -> Vec<AgentUsageDetails> {
        details["agents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|agent| AgentUsageDetails {
                agent_kind: None,
                session_id: agent["sessionId"].as_str().unwrap().into(),
                title: agent["title"].as_str().unwrap().into(),
                available: agent["available"].as_bool().unwrap(),
                lineage: serde_json::from_value(agent["lineage"].clone()).unwrap(),
                relation_status: serde_json::from_value(agent["relationStatus"].clone()).unwrap(),
                own_usage: serde_json::from_value(agent["ownUsage"].clone()).unwrap(),
                coverage: serde_json::from_value(agent["coverage"].clone()).unwrap(),
                turns: serde_json::from_value(agent["turns"].clone()).unwrap(),
            })
            .collect()
    }

    fn owned(turns: &[TurnUsage]) -> BTreeMap<String, TurnUsage> {
        turns
            .iter()
            .map(|turn| (turn.id.clone(), turn.clone()))
            .collect()
    }

    #[test]
    fn frozen_examples_keep_selected_scope_and_partition_each_request_once() {
        let examples = examples();
        let family = examples
            .iter()
            .find(|example| example["name"] == "root-two-children-grandchild-followup")
            .unwrap();
        let family_agents = agents(&family["details"]);
        for example in &examples {
            let details = &example["details"];
            let session = details["sessionId"].as_str().unwrap();
            let root = details["rootSessionId"].as_str().map(str::to_owned);
            let agents = agents(details);
            let root_turns = root.as_ref().map(|root| {
                let source = agents
                    .iter()
                    .chain(&family_agents)
                    .find(|agent| &agent.session_id == root)
                    .unwrap();
                owned(&source.turns)
            });
            let actual = build(
                session,
                root,
                agents,
                root_turns.as_ref(),
                serde_json::from_value(details["coverage"].clone()).unwrap(),
                serde_json::from_value(details["warnings"].clone()).unwrap(),
            );
            let value = serde_json::to_value(&actual).unwrap();
            let expected: ConversationUsageDetails =
                serde_json::from_value(details.clone()).unwrap();
            assert_eq!(
                value,
                serde_json::to_value(expected).unwrap(),
                "{}",
                example["name"]
            );
            let expected: BTreeSet<_> = actual
                .agents
                .iter()
                .flat_map(|agent| {
                    agent.turns.iter().flat_map(move |turn| {
                        turn.requests
                            .iter()
                            .map(move |request| (&agent.session_id, &turn.id, &request.key))
                    })
                })
                .collect();
            let refs: Vec<_> = actual
                .prompts
                .iter()
                .flat_map(|prompt| &prompt.request_refs)
                .chain(&actual.unassigned.request_refs)
                .map(|r| (&r.session_id, &r.turn_id, &r.request_key))
                .collect();
            assert_eq!(refs.len(), expected.len());
            assert_eq!(refs.into_iter().collect::<BTreeSet<_>>(), expected);
        }
    }

    #[test]
    fn missing_or_conflicting_request_roots_leave_the_whole_turn_unassigned() {
        let family = examples()
            .into_iter()
            .find(|example| example["name"] == "root-two-children-grandchild-followup")
            .unwrap();
        let mut agents = agents(&family["details"]);
        let root_turns = owned(&agents[0].turns);
        let selected = agents[0].session_id.clone();
        let mut extra = agents[1].turns[0].requests[0].clone();
        extra.key = "second-request".into();
        extra.source.as_mut().unwrap().response_id = "second-response".into();
        extra.source.as_mut().unwrap().root_turn_id = None;
        agents[1].turns[0].requests.push(extra);
        agents[1].own_usage = usage::summarize(&owned(&agents[1].turns));
        let actual = build(
            &selected,
            Some(selected.clone()),
            agents,
            Some(&root_turns),
            Coverage::Partial,
            Vec::new(),
        );
        assert_eq!(actual.user_prompt_count, Some(2));
        assert_eq!(actual.unassigned.request_refs.len(), 2);
        let unassigned = &actual.unassigned.request_refs[0];
        assert!(actual.prompts.iter().all(|prompt| prompt
            .request_refs
            .iter()
            .all(|r| r.session_id != unassigned.session_id || r.turn_id != unassigned.turn_id)));
        assert_eq!(actual.unassigned.usage.requests, 2);
    }

    #[test]
    fn equal_local_turn_ids_in_different_agents_keep_separate_request_references() {
        let example = examples()
            .into_iter()
            .find(|e| e["name"] == "root-two-children-grandchild-followup")
            .unwrap();
        let mut agents = agents(&example["details"]);
        let selected = agents[0].session_id.clone();
        let root_turns = owned(&agents[0].turns);
        let shared = agents[1].turns[0].id.clone();
        agents[3].turns[0].id = shared.clone();
        for request in &mut agents[3].turns[0].requests {
            request.source.as_mut().unwrap().origin_turn_id = Some(shared.clone());
        }
        let details = build(
            &selected,
            Some(selected.clone()),
            agents,
            Some(&root_turns),
            Coverage::Complete,
            Vec::new(),
        );
        let refs: Vec<_> = details
            .prompts
            .iter()
            .flat_map(|p| &p.request_refs)
            .chain(&details.unassigned.request_refs)
            .collect();
        assert_eq!(refs.len() as u64, details.combined_usage.requests);
        let owners: BTreeSet<_> = refs
            .iter()
            .filter(|r| r.turn_id == shared)
            .map(|r| &r.session_id)
            .collect();
        assert_eq!(owners.len(), 2);
    }

    #[test]
    fn selected_child_scope_requires_local_membership_and_survives_source_loss() {
        let example = examples()
            .into_iter()
            .find(|example| example["name"] == "root-two-children-grandchild-followup")
            .unwrap();
        let agents = agents(&example["details"]);
        let root_id = agents[0].session_id.clone();
        let child_id = agents[1].session_id.clone();
        let mut ledger = super::super::Ledger::default();
        for agent in agents {
            ledger.sessions.insert(
                agent.session_id.clone(),
                super::super::SessionRecord {
                    title: agent.title,
                    available: agent.available,
                    lineage: agent.lineage,
                    turns: owned(&agent.turns),
                    ..Default::default()
                },
            );
        }
        let details = from_ledger(&ledger, &child_id).unwrap();
        assert_eq!(details.root_session_id.as_deref(), Some(root_id.as_str()));
        assert_eq!(details.agents.len(), 2);
        assert_eq!(details.agents[0].session_id, child_id);
        assert!(details
            .agents
            .iter()
            .all(|agent| agent.session_id != root_id));
        assert_eq!(details.combined_usage.total, 6_600);
        assert_eq!(details.user_prompt_count, Some(2));
        ledger.sessions.get_mut(&child_id).unwrap().available = false;
        let missing = from_ledger(&ledger, &child_id).unwrap();
        assert_eq!(missing.combined_usage, details.combined_usage);
        assert!(!missing.agents[0].available);
        assert!(missing
            .warnings
            .iter()
            .any(|warning| warning.contains("file unavailable")));
        assert!(from_ledger(&super::super::Ledger::default(), &child_id).is_err());
        ledger
            .sessions
            .get_mut(&child_id)
            .unwrap()
            .project_exclusion = Some(super::super::ProjectExclusion {
            root_session_id: root_id,
            root_cwd: "/other/project".into(),
        });
        assert!(from_ledger(&ledger, &child_id).is_err());
    }
}
