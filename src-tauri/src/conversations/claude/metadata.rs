use crate::conversations::*;
use std::fs::File;

const FILE_LIMIT: u64 = 64 * 1024 * 1024;
const LINE_LIMIT: u64 = 1_048_576;

pub(in crate::conversations) struct Transcript {
    pub session: Conversation,
    pub turns: BTreeMap<String, usage::TurnUsage>,
    pub source: claude::subagents::Source,
    pub parent: Result<claude::subagents::Parent, lineage::Problem>,
}

fn title(value: &Value) -> Option<String> {
    let text = value.as_str()?;
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return None;
    }
    Some(text.chars().take(160).collect())
}

fn user_title(value: &Value) -> Option<String> {
    let content = &value["message"]["content"];
    if let Some(text) = title(content) {
        return Some(text);
    }
    content.as_array()?.iter().find_map(|block| {
        (block["type"] == "text")
            .then(|| title(&block["text"]))
            .flatten()
    })
}

pub(in crate::conversations) fn read(
    file: File,
    path: PathBuf,
    expected: &str,
    guest_project: &Path,
    namespace: &str,
    agent: Option<&str>,
) -> Result<Option<Transcript>, String> {
    let metadata = file
        .metadata()
        .map_err(|_| "Could not read the Claude chat date.")?;
    if !metadata.is_file() || metadata.len() > FILE_LIMIT {
        return Err("The Claude chat is not a supported regular transcript file.".into());
    }
    let mut source = BufReader::new(file);
    let mut first = None;
    let mut custom_title = None;
    let mut updated_at = 0;
    let mut consumed = 0;
    let mut turns = BTreeMap::new();
    let mut usage_conflict = false;
    let mut records = 0;
    let mut tool_use_ids = BTreeSet::new();
    let mut project_scope = None;
    let session_id = agent
        .map(|agent| claude::subagents::session_id(expected, agent))
        .unwrap_or_else(|| expected.into());
    let incomplete = loop {
        let mut line = Vec::new();
        let length = source
            .by_ref()
            .take(LINE_LIMIT + 1)
            .read_until(b'\n', &mut line)
            .map_err(|_| "Could not read a saved Claude chat.")?;
        consumed += length as u64;
        records += usize::from(length > 0);
        if consumed > FILE_LIMIT || length as u64 > LINE_LIMIT || records > 131_072 {
            return Err("The Claude transcript exceeds the supported record size.".into());
        }
        if !line.ends_with(b"\n") {
            break !line.is_empty();
        }
        let value: Value = serde_json::from_slice(&line)
            .map_err(|_| "A complete Claude transcript record is invalid.")?;
        if value
            .get("sessionId")
            .is_some_and(|value| value != expected)
        {
            return Err("The Claude transcript session ID does not match its file.".into());
        }
        if value["type"] == "custom-title" && value["sessionId"] == expected {
            custom_title = title(&value["customTitle"]).or(custom_title);
        }
        if !matches!(value["type"].as_str(), Some("user" | "assistant")) {
            continue;
        }
        if let Some(agent) = agent {
            if value["isSidechain"] != true || value["agentId"] != agent {
                return Err(
                    "The Claude subagent identity does not match its transcript file.".into(),
                );
            }
        } else if value["isSidechain"] != false
            || value.get("agentId").is_some_and(|value| !value.is_null())
        {
            continue;
        }
        let directory = value["cwd"]
            .as_str()
            .ok_or("The Claude chat project path is missing.")?;
        let directory = Path::new(directory);
        if value["sessionId"] != expected
            || !directory.is_absolute()
            || directory
                .components()
                .any(|part| part == Component::ParentDir)
        {
            return Ok(None);
        }
        let belongs = directory.starts_with(guest_project);
        if project_scope.is_some_and(|previous| previous != belongs) {
            return Err("Claude transcript records disagree about their project path.".into());
        }
        project_scope = Some(belongs);
        if let Some(timestamp) = value["timestamp"].as_str() {
            let timestamp = chrono::DateTime::parse_from_rfc3339(timestamp)
                .map_err(|_| "The Claude chat date is invalid.")?;
            let millis = u64::try_from(timestamp.timestamp_millis())
                .map_err(|_| "The Claude chat date is outside the supported range.")?;
            updated_at = updated_at.max(millis);
        }
        if first.is_none() {
            first = Some((
                directory.to_owned(),
                value["version"].as_str().map(str::to_owned),
                None,
            ));
        }
        if value["type"] == "user" {
            let (_, _, initial_title) = first.as_mut().expect("main message checked");
            if initial_title.is_none() {
                *initial_title = user_title(&value);
            }
        }
        if value["type"] == "assistant" {
            usage_conflict |= claude::usage::ingest(&mut turns, &value, &session_id, namespace);
            for block in value["message"]["content"].as_array().into_iter().flatten() {
                if block["type"] == "tool_use"
                    && matches!(block["name"].as_str(), Some("Agent" | "Task"))
                {
                    if let Some(id) = block["id"].as_str().filter(|id| {
                        !id.is_empty()
                            && id.len() <= 256
                            && id.bytes().all(|byte| byte.is_ascii_graphic())
                    }) {
                        tool_use_ids.insert(id.into());
                    }
                }
            }
        }
    };
    let Some((cwd, version, initial_title)) = first else {
        return Ok(None);
    };
    if updated_at == 0 {
        updated_at = metadata
            .modified()
            .map_err(|_| "Could not read the Claude chat date.")?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
    }
    let usage = usage::summarize(&turns);
    let source = claude::subagents::Source {
        path: path.clone(),
        cwd: cwd.clone(),
        root_session_id: expected.into(),
        agent_id: agent.map(str::to_owned),
        tool_use_ids,
    };
    let warning = if usage_conflict {
        Some(
            "Conflicting Claude message usage was skipped. Known recorded usage is retained."
                .into(),
        )
    } else {
        incomplete.then(|| {
            "The last Claude transcript record is incomplete. Known recorded usage is retained."
                .into()
        })
    };
    Ok(Some(Transcript {
        session: Conversation {
            id: session_id.clone(),
            environment_id: None,
            agent_kind: AgentKind::ClaudeCode,
            usage_available: true,
            title: custom_title
                .or(initial_title)
                .unwrap_or_else(|| "Untitled Claude chat".into()),
            updated_at,
            state: SessionState::Active,
            available: true,
            usage: usage.clone(),
            family_usage: usage,
            warning,
            lineage: lineage::Lineage {
                kind: if agent.is_some() {
                    lineage::Kind::Subagent
                } else {
                    lineage::Kind::Primary
                },
                agent_path: agent.map(|agent| format!("/{agent}")),
                source_version: version,
                ..Default::default()
            },
            relation_status: graph::Status::Root,
            root_session_id: agent.is_none().then_some(expected.into()),
            subagent_count: 0,
            coverage: usage::Coverage::Unknown,
            usage_revision: format!("{}:{updated_at}", metadata.len()),
            source: path,
            title_known: true,
            cwd,
            lineage_problem: None,
            source_root_id: agent.map(|_| expected.into()),
        },
        turns,
        source,
        parent: Err(lineage::Problem::MissingSource),
    }))
}
