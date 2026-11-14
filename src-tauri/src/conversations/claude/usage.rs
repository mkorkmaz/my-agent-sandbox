use crate::conversations::*;

const TOKEN_LIMIT: u64 = 1_000_000_000_000;
const PENDING: &str = "claude unrecorded response";

#[derive(Clone, Copy, Eq, PartialEq)]
enum ResponseMerge { New, Unmeasured, FirstMeasurement, Conflict, Update }

fn response_merge(previous: Option<&usage::TurnUsage>, current: &usage::TurnUsage) -> ResponseMerge {
    let Some(previous) = previous else { return ResponseMerge::New; };
    let Some(next) = current.requests.first() else { return ResponseMerge::Unmeasured; };
    let Some(request) = previous.requests.first() else { return ResponseMerge::FirstMeasurement; };
    if request.tokens.input != next.tokens.input
        || request.tokens.cached != next.tokens.cached
        || request.tokens.cache_write != next.tokens.cache_write
        || request.model.as_ref().zip(next.model.as_ref()).is_some_and(|(previous, next)| previous != next)
    { return ResponseMerge::Conflict; }
    ResponseMerge::Update
}

enum CanonicalSource { Saved, RecoverSaved(String, String), Observed }

fn canonical_source(candidates: &[(String, String)], known: &BTreeSet<(String, String)>) -> CanonicalSource {
    if known.contains(&candidates[0]) { return CanonicalSource::Saved; }
    if let Some((owner, turn)) = candidates.iter().find(|candidate| known.contains(candidate)) {
        return CanonicalSource::RecoverSaved(owner.clone(), turn.clone());
    }
    CanonicalSource::Observed
}
pub(in crate::conversations) const COVERAGE_NOTE: &str = "Claude usage includes recorded main-session and discovered subagent messages. Missing transcripts, unverified relationships, and unrecorded work limit coverage. User prompt attribution is unavailable.";

fn identifier(value: &Value) -> Option<&str> {
    value.as_str().filter(|value| {
        !value.is_empty() && value.len() <= 256 && value.bytes().all(|byte| byte.is_ascii_graphic())
    })
}

fn tokens(value: &Value) -> Option<usage::Tokens> {
    let count = |name: &str| value[name].as_u64().filter(|count| *count <= TOKEN_LIMIT);
    let ordinary = count("input_tokens")?;
    let cached = count("cache_read_input_tokens")?;
    let cache_write = count("cache_creation_input_tokens")?;
    let output = count("output_tokens")?;
    let input = ordinary.checked_add(cached)?.checked_add(cache_write)?;
    if input > TOKEN_LIMIT {
        return None;
    }
    Some(usage::Tokens {
        input,
        cached,
        cache_write,
        output,
        cache_known: true,
        cached_known: Some(true),
        cache_write_known: Some(true),
        // Anthropic's one-hour cache metadata is not OpenAI's twelve-hour class.
        write_12h: None,
        ..Default::default()
    })
}

pub(in crate::conversations) fn ingest(
    turns: &mut BTreeMap<String, usage::TurnUsage>,
    value: &Value,
    session: &str,
    namespace: &str,
) -> bool {
    let message = &value["message"];
    let message_id = identifier(&message["id"]);
    let id = message_id
        .or_else(|| value["uuid"].as_str().filter(|id| valid_id(id)))
        .unwrap_or("claude unidentified response");
    let complete = message["stop_reason"]
        .as_str()
        .is_some_and(|value| !value.is_empty());
    let mut turn = usage::TurnUsage {
        id: id.into(),
        status: if complete { usage::TurnStatus::Complete } else { usage::TurnStatus::InProgress },
        origin_thread_id: Some(session.into()),
        ..Default::default()
    };
    if let Some((id, tokens)) = message_id.zip(tokens(&message["usage"])) {
        let metadata = usage::anthropic::parse(&message["usage"], tokens.cache_write);
        let mut request = usage::RequestUsage {
            key: format!("anthropic:{id}"),
            at: value["timestamp"].as_str().unwrap_or_default().into(),
            model: identifier(&message["model"]).map(str::to_owned),
            tokens,
            rates: None,
            cost_usd: None,
            partial: !complete,
            unpriced_reason: None,
            service_tier: None,
            tier_source: None,
            rate_correction: None,
            provider_correction: None,
            source: Some(usage::RequestSource {
                guest_namespace: namespace.into(),
                model_provider: "anthropic".into(),
                origin_thread_id: session.into(),
                origin_turn_id: Some(id.into()),
                root_turn_id: None,
                response_id: id.into(),
            }),
            amount_correction: None,
            anthropic: Some(metadata),
            anthropic_corrections: Vec::new(),
        };
        usage::anthropic::refresh(&mut request);
        turn.requests.push(request);
    }
    merge(turns, BTreeMap::from([(id.into(), turn)]))
}

// A response's output count is cumulative across repeated content/stream records.
// Keep one stable message identity, and retain measured counts on replay or loss.
pub(in crate::conversations) fn merge(
    saved: &mut BTreeMap<String, usage::TurnUsage>,
    current: BTreeMap<String, usage::TurnUsage>,
) -> bool {
    let mut conflict = false;
    for (id, turn) in current {
        let state = response_merge(saved.get(&id), &turn);
        if state == ResponseMerge::New {
            saved.insert(id, turn);
            continue;
        }
        let previous = saved.get_mut(&id).expect("saved Claude response");
        if state == ResponseMerge::Conflict {
            conflict = true;
            continue;
        }
        previous.status.observe_completion(turn.status);
        if state == ResponseMerge::Unmeasured { continue; }
        if state == ResponseMerge::FirstMeasurement {
            previous.requests = turn.requests;
            continue;
        }
        let next = turn.requests.first().expect("measured Claude response");
        let request = previous.requests.first_mut().expect("saved Claude measurement");
        let correction = usage::anthropic::evidence(request, "Recovered recorded Anthropic model or billing metadata; original dated tariff retained when available.");
        let previous_metadata = request.anthropic.clone();
        let previous_model = request.model.clone();
        if let Some(next_metadata) = &next.anthropic {
            let mut metadata = request.anthropic.clone().unwrap_or_default();
            if usage::anthropic::merge(&mut metadata, next_metadata) {
                conflict = true;
            }
            request.anthropic = Some(metadata);
        }
        if request.model.is_none() {
            request.model = next.model.clone();
        }
        if request.anthropic != previous_metadata || request.model != previous_model {
            request.anthropic_corrections.push(correction);
        }
        request.tokens.output = request.tokens.output.max(next.tokens.output);
        request.partial &= next.partial;
        usage::anthropic::refresh(request);
    }
    if saved.len() > 1 {
        saved.remove(PENDING);
    }
    conflict
}

pub(in crate::conversations) fn prepare(record: &mut SessionRecord) {
    for request in record
        .turns
        .values_mut()
        .flat_map(|turn| &mut turn.requests)
    {
        usage::anthropic::refresh(request);
    }
    prepare_saved(record);
}

pub(in crate::conversations) fn prepare_saved(record: &mut SessionRecord) {
    record.agent_kind = Some(AgentKind::ClaudeCode);
    for request in record
        .turns
        .values_mut()
        .flat_map(|turn| &mut turn.requests)
    {
        usage::correct_provider_price(request);
    }
    if record.turns.is_empty() {
        record.turns.insert(
            PENDING.into(),
            usage::TurnUsage {
                id: PENDING.into(),
                status: usage::TurnStatus::Unknown,
                ..Default::default()
            },
        );
    }
}

// Parent tool-result summaries are never ingested. If a transcript also copies
// an assistant response, the explicit child transcript owns that message once.
fn set_owner(turn: &mut usage::TurnUsage, owner: &str, turn_id: &str) {
    turn.id = turn_id.into();
    turn.origin_thread_id = Some(owner.into());
    for request in &mut turn.requests {
        if let Some(source) = &mut request.source {
            source.origin_thread_id = owner.into();
            source.origin_turn_id = Some(turn_id.into());
        }
    }
}

pub(in crate::conversations) fn deduplicate(
    ledger: &mut Ledger,
    known_turns: &BTreeSet<(String, String)>,
) -> bool {
    let mut identities: BTreeMap<_, Vec<(String, String)>> = BTreeMap::new();
    for (id, record) in &ledger.sessions {
        let root = record
            .claude_source
            .as_ref()
            .map(|source| source.root_session_id.as_str())
            .unwrap_or(id);
        for (turn_id, turn) in &record.turns {
            for request in &turn.requests {
                if let Some(source) = request
                    .source
                    .as_ref()
                    .filter(|source| source.model_provider == "anthropic")
                {
                    identities
                        .entry((
                            source.guest_namespace.clone(),
                            root.to_owned(),
                            source.response_id.clone(),
                        ))
                        .or_default()
                        .push((id.clone(), turn_id.clone()));
                }
            }
        }
    }
    let graph = ledger_graph(ledger);
    let depths: BTreeMap<_, _> = ledger
        .sessions
        .keys()
        .map(|id| {
            let mut current = id.as_str();
            let mut depth = 0;
            if graph.relation(id).status != graph::Status::Resolved {
                return (id.clone(), depth);
            }
            while let Some(parent) = ledger
                .sessions
                .get(current)
                .and_then(|record| record.lineage.parent_session_id.as_deref())
            {
                depth += 1;
                if depth > 64 {
                    return (id.clone(), 0);
                }
                current = parent;
            }
            (id.clone(), depth)
        })
        .collect();
    let mut changed = false;
    for candidates in identities
        .values_mut()
        .filter(|candidates| candidates.len() > 1)
    {
        candidates.sort_by(|first, second| {
            let child = |id: &str| ledger.sessions[id].lineage.kind == lineage::Kind::Subagent;
            child(&second.0)
                .cmp(&child(&first.0))
                .then_with(|| depths[&second.0].cmp(&depths[&first.0]))
                .then_with(|| first.cmp(second))
        });
        let (owner, owner_turn) = &candidates[0];
        if let CanonicalSource::RecoverSaved(saved_owner, saved_turn) = canonical_source(candidates, known_turns) {
            let mut recorded = ledger.sessions[&saved_owner].turns[&saved_turn].clone();
            set_owner(&mut recorded, owner, owner_turn);
            let record = ledger.sessions.get_mut(owner).expect("canonical Claude request");
            let current = record.turns.insert(owner_turn.clone(), recorded).expect("current Claude request");
            if merge(&mut record.turns, BTreeMap::from([(owner_turn.clone(), current)])) {
                record.warning = Some("Conflicting copies of a Claude message were counted once. The first saved measurement is retained.".into());
            }
        }
        for (copy, copy_turn) in &candidates[1..] {
            let ambiguous = ledger.sessions[copy].lineage.kind == lineage::Kind::Subagent
                && ledger.sessions[owner].lineage.kind == lineage::Kind::Subagent
                && !graph.descendants(copy).contains(owner)
                && !graph.descendants(owner).contains(copy);
            let mut turn = ledger
                .sessions
                .get_mut(copy)
                .expect("copied Claude request")
                .turns
                .remove(copy_turn)
                .expect("copied Claude turn");
            set_owner(&mut turn, owner, owner_turn);
            let record = ledger
                .sessions
                .get_mut(owner)
                .expect("canonical Claude request");
            let conflict = merge(
                &mut record.turns,
                BTreeMap::from([(owner_turn.clone(), turn)]),
            );
            if conflict || ambiguous {
                record.warning = Some("Conflicting copies of a Claude message were counted once. Message ownership or counts are partially verified.".into());
            }
            changed = true;
        }
    }
    changed
}


#[cfg(test)]
mod transition_review {
    use super::*;
    use serde_json::json;
    #[test]
    fn recheck_conflicting_measurement_cannot_complete_a_partial_response() {
        let mut saved = BTreeMap::new();
        let mut value = json!({"message":{"id":"response-a","model":"model-a","usage":{"input_tokens":8,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":1}}});
        assert!(!ingest(&mut saved, &value, "session-a","fixture"));
        assert_eq!(saved["response-a"].status, usage::TurnStatus::InProgress);
        value["message"]["usage"]["input_tokens"] = json!(80);
        value["message"]["stop_reason"] = json!("end_turn");
        assert!(ingest(&mut saved, &value,"session-a","fixture"));
        assert_eq!(saved["response-a"].requests[0].tokens.input, 8);
        assert!(saved["response-a"].requests[0].partial);
        assert_eq!(saved["response-a"].status, usage::TurnStatus::InProgress);
    }
}
