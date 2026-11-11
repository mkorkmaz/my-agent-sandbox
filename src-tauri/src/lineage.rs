use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Primary,
    Subagent,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lineage {
    pub kind: Kind,
    pub parent_session_id: Option<String>,
    pub agent_path: Option<String>,
    pub nickname: Option<String>,
    pub source_version: Option<String>,
    pub lineage_version: u32,
}

impl Default for Lineage {
    fn default() -> Self {
        Self {
            kind: Kind::Unknown,
            parent_session_id: None,
            agent_path: None,
            nickname: None,
            source_version: None,
            lineage_version: 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Problem {
    Malformed,
    UnsupportedVersion,
    UnsupportedSource,
    Conflicting,
    MissingSource,
}

impl Problem {
    pub fn message(&self) -> &'static str {
        match self {
            Self::Malformed => {
                "Subagent relationship metadata is malformed; own usage is retained."
            }
            Self::UnsupportedVersion => {
                "This Codex source version has unverified relationship coverage."
            }
            Self::UnsupportedSource => {
                "This Codex source kind has unverified relationship coverage."
            }
            Self::Conflicting => "Saved sources disagree about this session's relationship.",
            Self::MissingSource => {
                "Relationship source metadata is missing; the last verified relation is retained."
            }
        }
    }
}

fn optional_text(value: &Value, limit: usize) -> Result<Option<String>, Problem> {
    if value.is_null() {
        return Ok(None);
    }
    let text = value.as_str().ok_or(Problem::Malformed)?;
    if text.len() > limit || text.chars().any(char::is_control) {
        return Err(Problem::Malformed);
    }
    Ok(Some(text.into()))
}

pub fn parse(payload: &Value) -> (Lineage, Option<Problem>) {
    let mut result = Lineage::default();
    let source = &payload["source"];
    if source.get("subagent").is_some()
        || source.get("subAgent").is_some()
        || source
            .as_str()
            .is_some_and(|source| source == "subagent" || source.starts_with("subAgent"))
    {
        result.kind = Kind::Subagent;
    }
    result.source_version = match optional_text(&payload["cli_version"], 128) {
        Ok(version) => version,
        Err(problem) => return (result, Some(problem)),
    };
    if source.is_null() {
        let problem = result
            .source_version
            .as_ref()
            .map(|_| Problem::MissingSource);
        return (result, problem);
    }
    if result.source_version.as_deref() != Some("0.167.0") {
        return (result, Some(Problem::UnsupportedVersion));
    }
    if let Some(source) = source.as_str() {
        if matches!(source, "cli" | "vscode" | "exec" | "appServer") {
            result.kind = Kind::Primary;
            return (result, None);
        }
        return (result, Some(Problem::UnsupportedSource));
    }
    let spawn = source
        .get("subagent")
        .or_else(|| source.get("subAgent"))
        .and_then(|agent| agent.get("thread_spawn"));
    let Some(spawn) = spawn else {
        let problem = if result.kind == Kind::Subagent {
            Problem::Malformed
        } else {
            Problem::UnsupportedSource
        };
        return (result, Some(problem));
    };
    let parent = spawn["parent_thread_id"].as_str();
    if !parent.is_some_and(crate::conversations::valid_id)
        || !spawn["depth"]
            .as_u64()
            .is_some_and(|depth| depth > 0 && depth <= 64)
    {
        return (result, Some(Problem::Malformed));
    }
    result.parent_session_id = parent.map(str::to_owned);
    result.agent_path = match optional_text(&spawn["agent_path"], 4096) {
        Ok(path) if path.as_deref().is_none_or(|path| path.starts_with('/')) => path,
        _ => return (result, Some(Problem::Malformed)),
    };
    result.nickname = match optional_text(&spawn["agent_nickname"], 256) {
        Ok(nickname) => nickname,
        Err(problem) => return (result, Some(problem)),
    };
    result.kind = Kind::Subagent;
    (result, None)
}

pub fn merge(
    previous: &Lineage,
    previous_problem: &Option<Problem>,
    next: Lineage,
    problem: Option<Problem>,
) -> (Lineage, Option<Problem>) {
    let absent = problem == Some(Problem::MissingSource)
        || (next.kind == Kind::Unknown && next.source_version.is_none() && problem.is_none());
    if absent && previous.kind != Kind::Unknown {
        return (
            previous.clone(),
            previous_problem.clone().or(Some(Problem::MissingSource)),
        );
    }
    if previous.kind == Kind::Subagent && next.kind != Kind::Subagent {
        return (previous.clone(), Some(Problem::Conflicting));
    }
    if previous.kind == Kind::Primary && next.kind == Kind::Subagent {
        return (next, Some(Problem::Conflicting));
    }
    if previous.kind == Kind::Subagent
        && next.kind == Kind::Subagent
        && previous.parent_session_id.is_some()
        && next.parent_session_id.is_some()
        && previous.parent_session_id != next.parent_session_id
    {
        return (previous.clone(), Some(Problem::Conflicting));
    }
    (next, problem)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(source: &str) -> Value {
        serde_json::from_str::<Value>(source.lines().next().unwrap()).unwrap()["payload"].clone()
    }

    #[test]
    fn real_guest_subagent_and_normal_fork_have_different_kinds() {
        let child = metadata(include_str!(
            "../../tests/fixtures/codex-subagents/v0.167.0/child-alpha.jsonl"
        ));
        let fork = metadata(include_str!(
            "../../tests/fixtures/codex-subagents/v0.167.0/normal-fork.jsonl"
        ));
        let (child, problem) = parse(&child);
        assert!(problem.is_none());
        assert_eq!(child.kind, Kind::Subagent);
        assert_eq!(
            child.parent_session_id.as_deref(),
            Some("00000000-0000-4000-8000-000000000001")
        );
        assert_eq!(parse(&fork).0.kind, Kind::Primary);
    }

    #[test]
    fn declared_child_role_survives_conflicting_source_and_later_source_loss() {
        let payload = metadata(include_str!(
            "../../tests/fixtures/codex-subagents/v0.167.0/child-alpha.jsonl"
        ));
        let mut primary_payload = payload.clone();
        primary_payload["source"] = "cli".into();
        let (primary, primary_problem) = parse(&primary_payload);
        assert_eq!(primary.kind, Kind::Primary);
        assert!(primary_problem.is_none());

        let mut malformed = payload.clone();
        malformed["source"]["subagent"]["thread_spawn"]["parent_thread_id"] = 17.into();
        let mut future = payload.clone();
        future["cli_version"] = "99.0.0".into();
        for payload in [payload, malformed, future] {
            let (child, child_problem) = parse(&payload);
            assert_eq!(child.kind, Kind::Subagent);
            let (retained, problem) = merge(&child, &child_problem, primary.clone(), None);
            assert_eq!(retained, child);
            assert_eq!(problem, Some(Problem::Conflicting));
            assert_eq!(
                merge(&retained, &problem, Lineage::default(), None),
                (child.clone(), Some(Problem::Conflicting))
            );
            assert_eq!(
                merge(&primary, &None, child.clone(), child_problem),
                (child, Some(Problem::Conflicting))
            );
        }
    }

    #[test]
    fn legacy_unknown_upgrades_to_verified_primary_or_subagent() {
        let child_payload = metadata(include_str!(
            "../../tests/fixtures/codex-subagents/v0.167.0/child-alpha.jsonl"
        ));
        let mut primary_payload = child_payload.clone();
        primary_payload["source"] = "cli".into();
        for payload in [primary_payload, child_payload] {
            let (verified, problem) = parse(&payload);
            assert!(problem.is_none());
            assert_eq!(
                merge(&Lineage::default(), &None, verified.clone(), None),
                (verified, None)
            );
        }
    }

    #[test]
    fn malformed_parent_is_typed_and_legacy_absence_has_no_false_warning() {
        let mut payload = metadata(include_str!(
            "../../tests/fixtures/codex-subagents/v0.167.0/child-alpha.jsonl"
        ));
        payload["source"]["subagent"]["thread_spawn"]["parent_thread_id"] = "--last".into();
        assert_eq!(parse(&payload).1, Some(Problem::Malformed));
        assert_eq!(parse(&payload).0.kind, Kind::Subagent);
        assert_eq!(parse(&serde_json::json!({})), (Lineage::default(), None));
        payload["cli_version"] = "99.0.0".into();
        assert_eq!(parse(&payload).1, Some(Problem::UnsupportedVersion));
        assert_eq!(parse(&payload).0.kind, Kind::Subagent);
        assert!(parse(&payload).0.parent_session_id.is_none());
    }
}
