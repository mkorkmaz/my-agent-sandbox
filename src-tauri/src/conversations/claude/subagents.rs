use crate::conversations::*;
use std::fs::File;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Source {
    pub path: PathBuf,
    pub cwd: PathBuf,
    pub root_session_id: String,
    pub agent_id: Option<String>,
    pub tool_use_ids: BTreeSet<String>,
}

#[derive(Clone)]
pub(in crate::conversations) struct Parent {
    pub agent_id: Option<String>,
    pub tool_use_id: String,
}

pub(in crate::conversations) fn valid_agent_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

// Claude agent IDs are not UUIDs. Keep the root/agent pair stable within each
// environment, while existing ledger guest variants keep different guests apart.
pub(in crate::conversations) fn session_id(root: &str, agent: &str) -> String {
    let digest = crate::hash::hex(Sha256::digest(format!("claude-subagent:{root}:{agent}")));
    format!(
        "{}-{}-{}-{}-{}",
        &digest[..8],
        &digest[8..12],
        &digest[12..16],
        &digest[16..20],
        &digest[20..32]
    )
}

impl Source {
    fn family_directory(&self) -> PathBuf {
        if self.agent_id.is_none() {
            return self.path.with_extension("");
        }
        self.path
            .ancestors()
            .find(|path| path.file_name().is_some_and(|name| name == "subagents"))
            .and_then(Path::parent)
            .unwrap_or(Path::new(""))
            .into()
    }

    fn valid(&self, home: &Path, id: &str) -> bool {
        let Ok(relative) = self.path.strip_prefix(home.join(".claude/projects")) else {
            return false;
        };
        let parts: Vec<_> = relative.components().collect();
        if parts
            .iter()
            .any(|part| !matches!(part, Component::Normal(_)))
            || !valid_id(&self.root_session_id)
            || !self.cwd.is_absolute()
            || self
                .cwd
                .components()
                .any(|part| part == Component::ParentDir)
        {
            return false;
        }
        let Some(agent) = &self.agent_id else {
            return parts.len() == 2
                && id == self.root_session_id
                && self
                    .path
                    .file_name()
                    .is_some_and(|name| name == format!("{id}.jsonl").as_str());
        };
        parts.len() >= 4
            && parts.len() <= 12
            && valid_agent_id(agent)
            && id == session_id(&self.root_session_id, agent)
            && parts[1].as_os_str() == self.root_session_id.as_str()
            && parts[2].as_os_str() == "subagents"
            && self
                .path
                .file_name()
                .is_some_and(|name| name == format!("agent-{agent}.jsonl").as_str())
    }
}

pub(in crate::conversations) fn metadata(
    directory: &File,
    agent: &str,
    root: &str,
) -> Result<Parent, lineage::Problem> {
    let name = format!("agent-{agent}.meta.json");
    let file = claude::history::open_at(directory, name.as_ref(), false)
        .map_err(|_| lineage::Problem::Malformed)?
        .ok_or(lineage::Problem::MissingSource)?;
    let stat = file.metadata().map_err(|_| lineage::Problem::Malformed)?;
    if !stat.is_file() || stat.len() > 65_536 {
        return Err(lineage::Problem::Malformed);
    }
    let mut bytes = Vec::new();
    file.take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|_| lineage::Problem::Malformed)?;
    if bytes.len() > 65_536 {
        return Err(lineage::Problem::Malformed);
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| lineage::Problem::Malformed)?;
    if !value.is_object()
        || value.get("sessionId").is_some_and(|id| id != root)
        || value.get("agentId").is_some_and(|id| id != agent)
    {
        return Err(lineage::Problem::Malformed);
    }
    let tool_use_id = value["toolUseId"]
        .as_str()
        .filter(|id| {
            !id.is_empty() && id.len() <= 256 && id.bytes().all(|byte| byte.is_ascii_graphic())
        })
        .ok_or(lineage::Problem::MissingSource)?
        .to_owned();
    let agent_id = match &value["parentAgentId"] {
        Value::Null => None,
        Value::String(id) if valid_agent_id(id) => Some(id.clone()),
        _ => return Err(lineage::Problem::Malformed),
    };
    if value.get("spawnDepth").is_some_and(|depth| {
        !depth
            .as_u64()
            .is_some_and(|depth| (1..=64).contains(&depth))
    }) {
        return Err(lineage::Problem::Malformed);
    }
    Ok(Parent {
        agent_id,
        tool_use_id,
    })
}

pub(in crate::conversations) fn resolve(
    scan: &mut HistoryScan,
    sessions: &mut BTreeMap<String, claude::metadata::Transcript>,
    known: &BTreeMap<String, SessionRecord>,
    home: &Path,
    project: &Path,
    namespace: &str,
) {
    let mut sources: BTreeMap<_, _> = known
        .iter()
        .filter(|(id, record)| {
            record.guest_namespace.as_deref() == Some(namespace)
                && !scan.lineage_problems.contains_key(*id)
        })
        .filter_map(|(id, record)| {
            record
                .claude_source
                .as_ref()
                .filter(|source| source.valid(home, id))
                .map(|source| (id.clone(), source.clone()))
        })
        .collect();
    for (id, transcript) in sessions.iter_mut() {
        if let Some(previous) = sources.get(id).filter(|source| {
            source.path == transcript.source.path && source.cwd == transcript.source.cwd
        }) {
            transcript
                .source
                .tool_use_ids
                .extend(previous.tool_use_ids.clone());
        }
        sources.insert(id.clone(), transcript.source.clone());
    }
    for transcript in sessions
        .values_mut()
        .filter(|transcript| transcript.source.agent_id.is_some())
    {
        let relation = transcript.parent.clone().and_then(|parent| {
            let root = &transcript.source.root_session_id;
            let parent_id = parent
                .agent_id
                .as_ref()
                .map(|agent| session_id(root, agent))
                .unwrap_or_else(|| root.clone());
            let Some(source) = sources.get(&parent_id) else {
                transcript.session.lineage.parent_session_id = Some(parent_id);
                return Err(lineage::Problem::MissingSource);
            };
            if source.family_directory() != transcript.source.family_directory()
                || source.root_session_id != *root
                || !source.tool_use_ids.contains(&parent.tool_use_id)
                || sources.iter().any(|(id, candidate)| {
                    id != &parent_id
                        && candidate.agent_id.is_some()
                        && candidate.family_directory() == source.family_directory()
                        && candidate.tool_use_ids.contains(&parent.tool_use_id)
                })
            {
                return Err(lineage::Problem::Conflicting);
            }
            transcript.session.lineage.parent_session_id = Some(parent_id);
            Ok(())
        });
        transcript.session.lineage_problem = relation.err();
    }
    let mut nodes = BTreeMap::new();
    for (id, record) in known
        .iter()
        .filter(|(_, record)| record.guest_namespace.as_deref() == Some(namespace))
    {
        if let Some(source) = sources.get(id) {
            nodes.insert(
                id.clone(),
                graph::Node {
                    agent_kind: Some(AgentKind::ClaudeCode),
                    guest_namespace: Some(namespace.into()),
                    lineage: record.lineage.clone(),
                    problem: record.lineage_problem.clone(),
                    cwd: record
                        .project_exclusion
                        .as_ref()
                        .filter(|exclusion| exclusion.root_session_id == *id)
                        .map(|exclusion| exclusion.root_cwd.clone())
                        .unwrap_or_else(|| source.cwd.clone()),
                    source_root_id: record.source_root_id.clone(),
                },
            );
        }
    }
    for (id, transcript) in sessions.iter_mut() {
        if let Some(record) = known
            .get(id)
            .filter(|record| record.guest_namespace.as_deref() == Some(namespace))
        {
            let (lineage, problem) = lineage::merge(
                &record.lineage,
                &record.lineage_problem,
                transcript.session.lineage.clone(),
                transcript.session.lineage_problem.clone(),
            );
            transcript.session.lineage = lineage;
            transcript.session.lineage_problem = problem;
        }
        let session = &transcript.session;
        nodes.insert(
            id.clone(),
            graph::Node {
                agent_kind: Some(AgentKind::ClaudeCode),
                guest_namespace: Some(namespace.into()),
                lineage: session.lineage.clone(),
                problem: session.lineage_problem.clone(),
                cwd: session.cwd.clone(),
                source_root_id: session.source_root_id.clone(),
            },
        );
    }
    let graph = Graph::new(nodes);
    let mut members = graph.project_members(project);
    for id in known.keys().chain(sessions.keys()) {
        scan.record_membership(id, graph.resolved_root_cwd(id), project);
        if scan.membership(id, known.get(id)) == MembershipState::RetainedExclusion {
            members.remove(id);
        }
    }
    sessions.retain(|id, _| members.contains(id));
}
