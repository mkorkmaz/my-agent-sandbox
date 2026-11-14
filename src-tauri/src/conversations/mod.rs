use crate::{
    agents::AgentKind,
    environment::{self, CheckedProject},
    projects, storage, terminal, usage,
};
use graph::Graph;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;
use tauri::{AppHandle, Emitter, Manager};

mod claude;
mod environment_history;
pub(crate) mod graph;
pub(crate) mod guest_records;
mod history_state;
pub(crate) mod lineage;
mod membership;
mod source_state;
use history_state::{HistoryObservation, PublicationState};
use membership::MembershipState;
use source_state::SourceState;
pub(crate) mod record_summary;
mod sql_backend;
pub(crate) mod sql_projection;
pub(crate) mod sql_reconnect;
mod sql_scan;
mod usage_details;
pub use usage_details::ConversationUsageDetails;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: String,
    #[serde(default)]
    pub environment_id: Option<projects::EnvironmentId>,
    #[serde(default = "codex_kind")]
    pub agent_kind: AgentKind,
    #[serde(default = "usage_supported")]
    pub usage_available: bool,
    pub title: String,
    pub updated_at: u64,
    #[serde(default)]
    pub state: SessionState,
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub usage: usage::Summary,
    #[serde(default)]
    pub warning: Option<String>,
    #[serde(default)]
    pub lineage: lineage::Lineage,
    #[serde(default)]
    pub relation_status: graph::Status,
    #[serde(default)]
    pub root_session_id: Option<String>,
    #[serde(default)]
    pub subagent_count: u64,
    #[serde(default)]
    pub family_usage: usage::Summary,
    #[serde(default)]
    pub coverage: usage::Coverage,
    #[serde(default)]
    pub usage_revision: String,
    #[serde(skip)]
    source: PathBuf,
    #[serde(skip)]
    title_known: bool,
    #[serde(skip)]
    cwd: PathBuf,
    #[serde(skip)]
    lineage_problem: Option<lineage::Problem>,
    #[serde(skip)]
    source_root_id: Option<String>,
}

fn codex_kind() -> AgentKind {
    AgentKind::Codex
}

fn usage_supported() -> bool {
    true
}

pub fn valid_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                return byte == b'-';
            }
            byte.is_ascii_hexdigit()
        })
}

#[derive(Deserialize)]
struct IndexEntry {
    id: String,
    thread_name: String,
}

fn titles(directory: &Path) -> Result<(HashMap<String, String>, bool), String> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory.join("session_index.jsonl"))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((HashMap::new(), false))
        }
        Err(_) => return Err("Could not read saved session names. Check the guest home.".into()),
    };
    let mut source = String::new();
    file.take(16 * 1024 * 1024 + 1)
        .read_to_string(&mut source)
        .map_err(|_| "Could not read saved session names.")?;
    if source.len() > 16 * 1024 * 1024 {
        return Err("The saved session index is too large to read.".into());
    }
    let mut result = HashMap::new();
    let mut incomplete = false;
    let mut lines = source.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim().is_empty() {
            continue;
        }
        let entry = match serde_json::from_str::<IndexEntry>(line) {
            Ok(entry) => entry,
            Err(error) if error.is_eof() && lines.peek().is_none() && !source.ends_with('\n') => {
                break
            }
            Err(_) => {
                incomplete = true;
                continue;
            }
        };
        if valid_id(&entry.id) && !entry.thread_name.trim().is_empty() {
            result.insert(entry.id, entry.thread_name);
        }
    }
    Ok((result, incomplete))
}

struct RolloutMetadata {
    id: String,
    updated_at: u64,
    cwd: PathBuf,
    lineage: lineage::Lineage,
    problem: Option<lineage::Problem>,
    source_root_id: Option<String>,
}

fn rollout(path: &Path) -> Result<Option<RolloutMetadata>, String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| "Could not read a saved session. Refresh the session list.")?;
    let modified = file
        .metadata()
        .and_then(|metadata| metadata.modified())
        .map_err(|_| "Could not read the saved session date.")?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let mut header = Vec::new();
    BufReader::new(file)
        .take(1_048_577)
        .read_until(b'\n', &mut header)
        .map_err(|_| "Could not read saved session metadata.")?;
    if header.len() > 1_048_576 {
        return Err("The saved session metadata is too large to read.".into());
    }
    if header.is_empty() {
        return Ok(None);
    }
    let document: Value = serde_json::from_slice(&header).map_err(|_| {
        "Saved session metadata is incomplete. Refresh after Codex finishes writing."
    })?;
    if document["type"] != "session_meta" {
        return Ok(None);
    }
    let Some(id) = document["payload"]["id"].as_str() else {
        return Ok(None);
    };
    let Some(directory) = document["payload"]["cwd"].as_str() else {
        return Ok(None);
    };
    let directory = Path::new(directory);
    if !valid_id(id)
        || !directory.is_absolute()
        || directory
            .components()
            .any(|part| part == Component::ParentDir)
    {
        return Ok(None);
    }
    let (lineage, mut problem) = lineage::parse(&document["payload"]);
    let root_hint = &document["payload"]["session_id"];
    let source_root_id = root_hint
        .as_str()
        .filter(|id| valid_id(id))
        .map(str::to_owned);
    if !root_hint.is_null() && source_root_id.is_none() {
        problem = Some(lineage::Problem::Malformed);
    }
    Ok(Some(RolloutMetadata {
        id: id.into(),
        updated_at: modified,
        cwd: directory.into(),
        lineage,
        problem,
        source_root_id,
    }))
}

#[derive(Default)]
struct HistoryScan {
    sessions: Vec<Conversation>,
    turns: BTreeMap<String, BTreeMap<String, usage::TurnUsage>>,
    claude_sources: BTreeMap<String, claude::subagents::Source>,
    #[cfg(test)]
    metadata_visits: usize,
    problems: BTreeMap<PathBuf, String>,
    warnings: Vec<String>,
    lineage_problems: BTreeMap<String, lineage::Problem>,
    exclusions: BTreeMap<String, ProjectExclusion>,
    positive_members: BTreeSet<String>,
}

#[cfg(test)]
fn read_history(home: &Path, guest_project: &Path) -> Result<Vec<Conversation>, String> {
    Ok(scan_history(home, guest_project)?.sessions)
}

#[cfg(test)]
fn scan_history(home: &Path, guest_project: &Path) -> Result<HistoryScan, String> {
    scan_history_known(home, guest_project, &BTreeMap::new())
}

fn scan_history_known(
    home: &Path,
    guest_project: &Path,
    known: &BTreeMap<String, SessionRecord>,
) -> Result<HistoryScan, String> {
    #[cfg(test)]
    let _benchmark_phase = sqlite_benchmark::Phase::parser();
    let mut result = HistoryScan::default();
    let directory = home.join(".codex");
    if !directory.exists() {
        return Ok(result);
    }
    let home = fs::canonicalize(home).map_err(|_| "The guest home is unavailable.")?;
    let namespace = guest_namespace(&home);
    let directory = fs::canonicalize(directory).map_err(|_| "The Codex history is unavailable.")?;
    if !directory.starts_with(&home) {
        return Err("The Codex history points outside this guest home.".into());
    }
    let (names, incomplete_names) = match titles(&directory) {
        Ok(names) => names,
        Err(message) => {
            result.warnings.push(message);
            (HashMap::new(), false)
        }
    };
    if incomplete_names {
        result.warnings.push(
            "Some saved session names could not be read. The last known titles are preserved."
                .into(),
        );
    }
    let mut pending = Vec::new();
    for name in ["sessions", "archived_sessions"] {
        let root = directory.join(name);
        let root = match fs::canonicalize(&root) {
            Ok(root) => root,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                result
                    .problems
                    .insert(root, "This saved session directory is unreadable.".into());
                continue;
            }
        };
        if !root.starts_with(&directory) {
            return Err("The saved sessions point outside the Codex history directory.".into());
        }
        pending.push((root.clone(), 0, root));
    }
    let mut sessions: HashMap<String, Conversation> = HashMap::new();
    let mut visited = HashSet::new();
    while let Some((directory, depth, root)) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => {
                result.problems.insert(
                    directory,
                    "This saved session directory is unreadable.".into(),
                );
                continue;
            }
        };
        for entry in entries {
            let Ok(entry) = entry else {
                result.problems.insert(
                    directory.clone(),
                    "Some directory entries could not be read.".into(),
                );
                continue;
            };
            let Ok(kind) = entry.file_type() else {
                result.problems.insert(
                    entry.path(),
                    "This saved session entry is unreadable.".into(),
                );
                continue;
            };
            if kind.is_dir() && depth < 3 {
                pending.push((entry.path(), depth + 1, root.clone()));
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !kind.is_file() || !name.starts_with("rollout-") || !name.ends_with(".jsonl") {
                continue;
            }
            let path = match fs::canonicalize(entry.path()) {
                Ok(path) => path,
                Err(_) => {
                    result.problems.insert(
                        entry.path(),
                        "A saved session moved. Refresh the list.".into(),
                    );
                    continue;
                }
            };
            if !path.starts_with(&root) {
                return Err("A saved session points outside this project's history.".into());
            }
            if !visited.insert(path.clone()) {
                continue;
            }
            #[cfg(test)]
            {
                result.metadata_visits += 1;
            }
            let metadata = match rollout(&path) {
                Ok(metadata) => metadata,
                Err(message) => {
                    result.problems.insert(path, message);
                    continue;
                }
            };
            if let Some(metadata) = metadata {
                let id = metadata.id;
                let updated_at = metadata.updated_at;
                let title_known = names.contains_key(&id);
                let title = names
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| "Untitled session".into());
                let previous = sessions.entry(id.clone()).or_insert(Conversation {
                    id,
                    environment_id: None,
                    agent_kind: AgentKind::Codex,
                    usage_available: true,
                    title,
                    updated_at,
                    state: SessionState::Active,
                    available: true,
                    usage: usage::Summary::default(),
                    warning: metadata
                        .problem
                        .as_ref()
                        .map(|problem| problem.message().into()),
                    lineage: metadata.lineage.clone(),
                    relation_status: graph::Status::Unknown,
                    root_session_id: None,
                    subagent_count: 0,
                    family_usage: usage::Summary::default(),
                    coverage: usage::Coverage::Unknown,
                    usage_revision: String::new(),
                    source: path.clone(),
                    title_known,
                    cwd: metadata.cwd.clone(),
                    lineage_problem: metadata.problem.clone(),
                    source_root_id: metadata.source_root_id.clone(),
                });
                if previous.lineage != metadata.lineage || previous.cwd != metadata.cwd {
                    previous.lineage_problem = Some(lineage::Problem::Conflicting);
                    previous.warning = Some(lineage::Problem::Conflicting.message().into());
                }
                if updated_at >= previous.updated_at {
                    previous.source = path;
                }
                previous.updated_at = previous.updated_at.max(updated_at);
            }
        }
    }
    let mut nodes = BTreeMap::new();
    for (id, record) in known {
        if record.cursors.keys().any(|source| {
            source.starts_with(&directory)
                && !source.components().any(|part| part == Component::ParentDir)
        }) {
            nodes.insert(
                id.clone(),
                graph::Node {
                    agent_kind: record.agent_kind,
                    guest_namespace: Some(namespace.clone()),
                    lineage: record.lineage.clone(),
                    problem: record.lineage_problem.clone(),
                    cwd: record
                        .project_exclusion
                        .as_ref()
                        .filter(|exclusion| exclusion.root_session_id == *id)
                        .map(|exclusion| exclusion.root_cwd.clone())
                        .unwrap_or_else(|| guest_project.into()),
                    source_root_id: record.source_root_id.clone(),
                },
            );
        }
    }
    for (id, session) in &mut sessions {
        if let Some(record) = known.get(id).filter(|record| {
            record
                .guest_namespace
                .as_ref()
                .is_none_or(|owner| owner == &namespace)
        }) {
            let (lineage, problem) = lineage::merge(
                &record.lineage,
                &record.lineage_problem,
                session.lineage.clone(),
                session.lineage_problem.clone(),
            );
            session.lineage = lineage;
            session.lineage_problem = problem;
            session.warning = session
                .lineage_problem
                .as_ref()
                .map(|problem| problem.message().to_owned());
            session.source_root_id = session
                .source_root_id
                .clone()
                .or_else(|| record.source_root_id.clone());
        }
        nodes.insert(
            id.clone(),
            graph::Node {
                agent_kind: Some(session.agent_kind),
                guest_namespace: Some(namespace.clone()),
                lineage: session.lineage.clone(),
                problem: session.lineage_problem.clone(),
                cwd: session.cwd.clone(),
                source_root_id: session.source_root_id.clone(),
            },
        );
    }
    let graph = Graph::new(nodes);
    let mut members = graph.project_members(guest_project);
    let graph_ids: BTreeSet<_> = sessions.keys().chain(known.keys()).cloned().collect();
    for id in &graph_ids {
        result.record_membership(id, graph.resolved_root_cwd(id), guest_project);
        let saved = known.get(id).filter(|record| record.guest_namespace.as_deref() == Some(&namespace));
        if result.membership(id, saved) == MembershipState::RetainedExclusion {
            members.remove(id);
        }
        if let Some(problem) = graph.problem(id) {
            result.lineage_problems.insert(id.clone(), problem.clone());
            if graph.blocked(id) {
                if let Some(session) = sessions.get(id) {
                    result
                        .problems
                        .insert(session.source.clone(), problem.message().into());
                }
            }
        }
    }
    let mut sessions: Vec<_> = sessions
        .into_values()
        .filter(|session| members.contains(&session.id))
        .collect();
    apply_graph(&mut sessions, &graph);
    sessions.sort_by(|first, second| {
        second
            .updated_at
            .cmp(&first.updated_at)
            .then_with(|| first.id.cmp(&second.id))
    });
    result.sessions = sessions;
    if !result.problems.is_empty() {
        result.warnings.push("Some saved session metadata could not be read. Other sessions and saved usage remain available.".into());
    }
    Ok(result)
}

fn apply_graph(sessions: &mut [Conversation], graph: &Graph) {
    for session in sessions {
        let relation = graph.relation(&session.id);
        session.lineage_problem = graph.problem(&session.id);
        session.relation_status = relation.status;
        session.root_session_id = relation.root_session_id;
        session.subagent_count = graph.descendants(&session.id).len() as u64;
        let warning = match session.relation_status {
            graph::Status::Orphan => Some("Parent session is unavailable; own usage is retained."),
            graph::Status::Invalid => {
                Some("Session relationship is invalid; family totals are not verified.")
            }
            _ => None,
        };
        if let Some(warning) = warning {
            session.warning = Some(
                session
                    .warning
                    .take()
                    .map(|previous| format!("{previous} {warning}"))
                    .unwrap_or_else(|| warning.into()),
            );
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    #[default]
    Active,
    Archived,
    Deleted,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) agent_kind: Option<AgentKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) claude_source: Option<claude::subagents::Source>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) guest_namespace: Option<String>,
    pub(crate) title: String,
    pub(crate) updated_at: u64,
    pub(crate) state: SessionState,
    pub(crate) turns: BTreeMap<String, usage::TurnUsage>,
    pub(crate) cursors: BTreeMap<PathBuf, usage::Cursor>,
    #[serde(default)]
    pub(crate) warning: Option<String>,
    #[serde(default)]
    pub(crate) parser_version: u32,
    #[serde(default)]
    pub(crate) available: bool,
    #[serde(default)]
    pub(crate) lineage: lineage::Lineage,
    #[serde(default)]
    pub(crate) lineage_problem: Option<lineage::Problem>,
    #[serde(default)]
    pub(crate) source_root_id: Option<String>,
    #[serde(default)]
    pub(crate) project_exclusion: Option<ProjectExclusion>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectExclusion {
    pub(crate) root_session_id: String,
    pub(crate) root_cwd: PathBuf,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Ledger {
    pub(crate) schema_version: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) variants: guest_records::Variants,
    pub(crate) sessions: BTreeMap<String, SessionRecord>,
    #[serde(default)]
    pub(crate) warnings: Vec<String>,
}
impl Default for Ledger {
    fn default() -> Self {
        Self {
            schema_version: 2,
            variants: BTreeMap::new(),
            sessions: BTreeMap::new(),
            warnings: Vec::new(),
        }
    }
}
pub(crate) fn prepare_saved_claude(record: &mut SessionRecord) -> bool {
    let before = record.clone();
    claude::usage::prepare(record);
    // Compare a single record. Never serialize an environment to detect a change.
    before.agent_kind != record.agent_kind || before.turns != record.turns
}
#[cfg(test)]
fn load_ledger(file: &Path) -> Result<Ledger, String> {
    let Some(bytes) = storage::read_regular_bytes(file)? else {
        return Ok(Ledger::default());
    };
    let document: Value =
        serde_json::from_slice(&bytes).map_err(|_| "Could not read the saved usage ledger.")?;
    if !matches!(document["schemaVersion"].as_u64(), Some(1 | 2)) {
        return Err("This usage ledger version is unsupported.".into());
    }
    serde_json::from_value(document).map_err(|_| "Could not read the saved usage ledger.".into())
}

#[cfg(test)]
fn write_ledger(file: &Path, ledger: &mut Ledger) -> Result<(), String> {
    if ledger.schema_version == 1 {
        storage::immutable_v1_backup(file)?;
        ledger.schema_version = 2;
    }
    if ledger.schema_version != 2 {
        return Err("This usage ledger version is unsupported.".into());
    }
    storage::write(file, ledger)
}
#[derive(Clone)]
struct Watch {
    target: Option<environment_history::Target>,
    records_directory: Option<crate::storage::data_location::RecordsDirectory>,
    store: Option<crate::usage_store::SharedStore>,
    home: PathBuf,
    project: PathBuf,
    #[cfg(test)]
    ledger: PathBuf,
}
impl Watch {
    fn check_storage(&self) -> Result<(), String> {
        match &self.records_directory {
            Some(directory) => directory.verify(),
            None => Ok(()),
        }
    }
    fn namespace(&self) -> String {
        guest_namespace(&fs::canonicalize(&self.home).unwrap_or_else(|_| self.home.clone()))
    }
    fn selected_ledger(&self) -> Result<Ledger, String> {
        self.check_storage()?;
        let mut ledger = self.load()?;
        ledger.select_guest(&self.namespace());
        Ok(ledger)
    }
}
#[derive(Default, Clone)]
pub struct History {
    lock: Arc<Mutex<()>>,
    watches: Arc<Mutex<BTreeMap<String, Watch>>>,
    published: Arc<Mutex<HashMap<String, Dashboard>>>,
    latest: Arc<Mutex<HashMap<String, Dashboard>>>,
}
impl History {
    fn visit(
        &self,
        mut work: impl FnMut(&str, &Watch) -> Result<(), String>,
    ) -> Result<(), String> {
        let _guard = self.lock.lock().map_err(|error| error.to_string())?;
        let watches = self
            .watches
            .lock()
            .map_err(|error| error.to_string())?
            .clone();
        let mut errors = Vec::new();
        for (id, watch) in &watches {
            if let Err(error) = work(id, watch) {
                errors.push(format!("Project {id}: {error}"));
            }
        }
        if !errors.is_empty() {
            return Err(errors.join("\n"));
        }
        Ok(())
    }
    pub(crate) fn flush(&self) -> Result<(), String> {
        self.visit(|_, watch| synchronize(watch).map(|_| ()))
    }
    fn unobserve(&self, id: &str) -> Result<(), String> {
        self.remove(id, None, false)
    }
    fn publication_state(&self, id: &str, dashboard: &Dashboard) -> Result<PublicationState, String> {
        let published = self.published.lock().map_err(|error| error.to_string())?;
        Ok(PublicationState::observe(published.get(id), dashboard))
    }
    #[cfg(test)]
    fn should_publish(&self, id: &str, dashboard: &Dashboard) -> Result<bool, String> {
        Ok(self.publication_state(id, dashboard)?.permits_publication())
    }
    fn record_published(&self, id: &str, dashboard: &Dashboard) -> Result<(), String> {
        self.published
            .lock()
            .map_err(|error| error.to_string())?
            .insert(id.into(), dashboard.clone());
        Ok(())
    }
}
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dashboard {
    pub usage_available: bool,
    pub conversations: Vec<Conversation>,
    pub usage: usage::Summary,
    pub pricing_date: String,
    pub anthropic_pricing_date: String,
    pub warnings: Vec<String>,
    pub coverage: usage::Coverage,
}

#[derive(Clone, Serialize)]
pub struct DashboardUpdate {
    #[serde(flatten)]
    dashboard: Dashboard,
    sequence: u64,
}

// Callers hold the history lock while reading and numbering each result.
fn dashboard_sequence() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl DashboardUpdate {
    fn new(dashboard: Dashboard) -> Self {
        Self {
            dashboard,
            sequence: dashboard_sequence(),
        }
    }
}

fn paths(app: &AppHandle, checked: &CheckedProject) -> Result<Watch, String> {
    let store = app.state::<crate::usage_store::Runtime>().wait()?;
    let records = crate::storage::data_location::records_directory(app)?;
    Ok(Watch {
        target: Some(environment_history::Target::for_project(&checked.project)),
        #[cfg(test)]
        ledger: checked.project.ledger_file(records.path())?,
        records_directory: Some(records),
        store: Some(store),
        home: checked.paths.host_home.clone(),
        project: checked.paths.guest_project.clone(),
    })
}

fn synchronize(watch: &Watch) -> Result<(Dashboard, bool), String> {
    let (mut dashboard, changed) = match watch.target.as_ref().map(|target| target.agent_kind) {
        Some(AgentKind::ClaudeCode) => claude::history::synchronize(watch)?,
        _ => synchronize_codex(watch)?,
    };
    if let Some(target) = &watch.target {
        target.annotate(&mut dashboard);
    }
    Ok((dashboard, changed))
}

fn synchronize_codex(watch: &Watch) -> Result<(Dashboard, bool), String> {
    watch.check_storage()?;
    let _file_guard = watch.legacy_lock()?;
    let (mut ledger, mut scan_state) = watch.scan_ledger()?;
    let namespace = watch.namespace();
    scan_state.prepare_selection(watch, &mut ledger, &namespace)?;
    let selection_changed = ledger.select_guest(&namespace);
    let scan = scan_history_known(&watch.home, &watch.project, &ledger.sessions)?;
    let mut needed = BTreeSet::new();
    for session in &scan.sessions {
        let current = ledger.sessions.get(&session.id).is_some_and(|record| {
            record.parser_version >= usage::PARSER_VERSION
                && record.cursors.get(&session.source).is_some_and(|cursor| {
                    cursor
                        .is_current_file(&session.source, &namespace, &session.id)
                        .unwrap_or(false)
                })
        });
        if !current {
            needed.insert(session.id.clone());
        }
    }
    scan_state.hydrate(watch, &mut ledger, needed, Some(&namespace))?;
    let mut changed =
        selection_changed || ledger.schema_version != 2 || ledger.warnings != scan.warnings;
    ledger.warnings = scan.warnings;
    for row in ledger.sessions.values_mut().chain(
        ledger
            .variants
            .values_mut()
            .flat_map(|rows| rows.values_mut()),
    ) {
        for request in row.turns.values_mut().flat_map(|turn| &mut turn.requests) {
            changed |= usage::correct_provider_price(request);
        }
    }
    let mut sources = HashMap::new();
    if !scan.sessions.is_empty() {
        let directory = fs::canonicalize(watch.home.join(".codex"))
            .map_err(|_| "The Codex history is unavailable.")?;
        for record in ledger.sessions.values_mut() {
            for (source, cursor) in &mut record.cursors {
                if source.starts_with(&directory)
                    && !source.components().any(|part| part == Component::ParentDir)
                {
                    changed |= cursor.bind_guest_namespace_if_missing(&namespace);
                }
            }
        }
    }
    for (id, exclusion) in &scan.exclusions {
        if let Some(record) = ledger
            .sessions
            .get_mut(id)
            .filter(|record| record.guest_namespace.as_deref() == Some(&namespace))
        {
            changed |= record.exclude(exclusion);
        }
    }
    for session in scan.sessions {
        sources.insert(session.id.clone(), session.clone());
        let owner = session.id.clone();
        ledger.admit_guest(&owner, &namespace);
        let record = ledger.sessions.get_mut(&owner).expect("admitted session");
        let membership = match scan.positive_members.contains(&owner) {
            true => MembershipState::Confirmed,
            false => MembershipState::Unresolved,
        };
        changed |= record.confirm_membership(membership);
        changed |= record.observe_source(SourceState::Available);
        let (lineage, problem) = lineage::merge(
            &record.lineage,
            &record.lineage_problem,
            session.lineage,
            session.lineage_problem,
        );
        changed |= record.lineage != lineage || record.lineage_problem != problem;
        record.lineage = lineage;
        record.lineage_problem = problem;
        let root_hint = session
            .source_root_id
            .or_else(|| record.source_root_id.clone());
        changed |= record.source_root_id != root_hint;
        record.source_root_id = root_hint;
        if record.parser_version < usage::PARSER_VERSION {
            record.cursors.clear();
            record.parser_version = usage::PARSER_VERSION;
            changed = true;
        }
        if (session.title_known || record.title.is_empty()) && record.title != session.title {
            record.title = session.title;
            changed = true;
        }
        if record.updated_at != session.updated_at {
            record.updated_at = session.updated_at;
            changed = true;
        }
        let cursor = record.cursors.entry(session.source.clone()).or_default();
        let before = cursor.clone();
        let synchronized = scan_state.sync_codex_source(
            &owner,
            &session.source,
            &namespace,
            cursor,
            &mut record.turns,
        );
        let usage_warning = match synchronized {
            Ok(updated) => {
                changed |= updated;
                cursor.skipped_large_record().then(|| {
                    "An oversized session record was skipped. Its usage could not be verified."
                        .into()
                })
            }
            Err(message) => Some(format!(
                "Usage partially read: {message}. Refresh after the session file is repaired."
            )),
        };
        let metadata_warning = record
            .lineage_problem
            .as_ref()
            .map(|problem| problem.message().to_owned());
        let warning = match (metadata_warning, usage_warning) {
            (Some(metadata), Some(usage)) => Some(format!("{metadata} {usage}")),
            (metadata, usage) => metadata.or(usage),
        };
        changed |= *cursor != before || record.warning != warning;
        record.warning = warning;
    }
    for (id, record) in &mut ledger.sessions {
        if sources.contains_key(id) {
            continue;
        }
        changed |= record.observe_source(SourceState::Unavailable);
        if let Some(problem) = scan
            .lineage_problems
            .get(id)
            .filter(|_| record.guest_namespace.as_deref() == Some(&namespace))
        {
            changed |= record.lineage_problem.as_ref() != Some(problem);
            record.lineage_problem = Some(problem.clone());
        }
        let warning = record
            .cursors
            .keys()
            .find_map(|source| source.ancestors().find_map(|path| scan.problems.get(path)))
            .cloned()
            .or_else(|| {
                record
                    .lineage_problem
                    .as_ref()
                    .map(|problem| problem.message().to_owned())
            });
        changed |= record.warning != warning;
        record.warning = warning;
    }
    scan_state.finish(watch, ledger, sources, changed)
}

fn dashboard_from_ledger(
    ledger: Ledger,
    mut sources: HashMap<String, Conversation>,
) -> Result<Dashboard, String> {
    let graph = ledger_graph(&ledger);
    let mut usage = usage::Summary::default();
    let mut project_coverage = Vec::new();
    for (id, variants) in &ledger.variants {
        for record in variants
            .values()
            .filter(|record| record.project_exclusion.is_none())
        {
            let summary = usage::summarize(&usage::own_turns(&record.turns, id));
            if summary.requests > 0 || !record.turns.is_empty() {
                usage.add(&summary);
                project_coverage.push(usage::Coverage::Partial);
            }
        }
    }
    let mut conversations = Vec::new();
    for (id, record) in &ledger.sessions {
        if record.project_exclusion.is_some() {
            continue;
        }
        let summary = usage::summarize(&usage::own_turns(&record.turns, id));
        usage.add(&summary);
        let coverage = family_coverage(id, &ledger, &graph);
        project_coverage.push(coverage);
        let mut family_usage = summary.clone();
        for child in graph.descendants(id) {
            if let Some(record) = project_record(&ledger, &child) {
                family_usage.add(&usage::summarize(&usage::own_turns(&record.turns, &child)));
            }
        }
        let available = record.source_state() == SourceState::Available;
        let metadata = sources.remove(id);
        let cursor_states: BTreeMap<_, _> = record
            .cursors
            .iter()
            .map(|(path, cursor)| (path, cursor.revision_state()))
            .collect();
        let revision = serde_json::to_vec(&(
            record.parser_version,
            cursor_states,
            &record.lineage,
            &record.lineage_problem,
            &record.warning,
            available,
            &record.source_root_id,
            &record.title,
            (record.agent_kind == Some(AgentKind::ClaudeCode)).then_some(&record.turns),
        ))
        .map_err(|error| error.to_string())?;
        conversations.push(Conversation {
            available,
            source: metadata
                .as_ref()
                .map(|session| session.source.clone())
                .unwrap_or_default(),
            id: id.clone(),
            environment_id: None,
            agent_kind: record.agent_kind.unwrap_or(AgentKind::Codex),
            usage_available: true,
            title: record.title.clone(),
            updated_at: record.updated_at,
            state: record.state,
            usage: summary,
            warning: record.warning.clone(),
            lineage: record.lineage.clone(),
            relation_status: graph::Status::Unknown,
            root_session_id: None,
            subagent_count: 0,
            family_usage,
            coverage,
            usage_revision: crate::hash::hex(Sha256::digest(revision)),
            title_known: true,
            cwd: metadata
                .as_ref()
                .map(|session| session.cwd.clone())
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

fn project_record<'a>(ledger: &'a Ledger, id: &str) -> Option<&'a SessionRecord> {
    ledger
        .sessions
        .get(id)
        .filter(|record| record.project_exclusion.is_none())
}

fn ledger_graph(ledger: &Ledger) -> Graph {
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
                        guest_namespace: record_namespace(id, record),
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

fn guest_namespace(canonical_home: &Path) -> String {
    format!(
        "guest:{}",
        crate::hash::hex(Sha256::digest(canonical_home.as_os_str().as_encoded_bytes()))
    )
}

fn record_namespace(id: &str, record: &SessionRecord) -> Option<String> {
    if let Some(namespace) = &record.guest_namespace {
        return Some(namespace.clone());
    }
    let namespaces: BTreeSet<_> = record
        .cursors
        .values()
        .filter_map(usage::Cursor::guest_namespace)
        .chain(
            record
                .turns
                .values()
                .flat_map(|turn| &turn.requests)
                .filter_map(|request| request.source.as_ref())
                .filter(|source| source.origin_thread_id == id)
                .map(|source| source.guest_namespace.as_str()),
        )
        .filter(|namespace| !namespace.is_empty())
        .collect();
    if namespaces.len() != 1 {
        return None;
    }
    namespaces.first().map(|namespace| (*namespace).to_owned())
}

fn own_coverage(id: &str, record: &SessionRecord, graph: &Graph) -> usage::Coverage {
    let turns = usage::own_turns(&record.turns, id);
    let summary = usage::summarize(&turns);
    if record.agent_kind == Some(AgentKind::ClaudeCode) {
        return if summary.requests > 0 {
            usage::Coverage::Partial
        } else {
            usage::Coverage::Unknown
        };
    }
    let requests: Vec<_> = turns.values().flat_map(|turn| &turn.requests).collect();
    let known = requests
        .iter()
        .filter(|request| request.source.is_some())
        .count();
    if record.lineage.kind == lineage::Kind::Unknown
        || record.lineage.source_version.as_deref() != Some("0.167.0")
        || record
            .cursors
            .values()
            .any(usage::Cursor::has_ownership_problem)
    {
        return usage::Coverage::Unknown;
    }
    let gap = record.cursors.values().any(usage::Cursor::has_unread_bytes)
        || record.source_state() == SourceState::Unavailable
        || record.warning.is_some()
        || record.lineage_problem.is_some()
        || summary.unmeasured_turns > 0
        || known != requests.len()
        || requests.iter().any(|request| {
            request.source.as_ref().is_some_and(|source| {
                source.origin_turn_id.is_none() || source.root_turn_id.is_none()
            })
        })
        || !matches!(
            graph.relation(id).status,
            graph::Status::Root | graph::Status::Resolved
        );
    if gap {
        return if known > 0 {
            usage::Coverage::Partial
        } else {
            usage::Coverage::Unknown
        };
    }
    usage::Coverage::Complete
}

fn family_coverage(id: &str, ledger: &Ledger, graph: &Graph) -> usage::Coverage {
    let ids = std::iter::once(id.to_owned()).chain(graph.descendants(id));
    combined_coverage(
        &ids.filter_map(|id| {
            project_record(ledger, &id).map(|record| own_coverage(&id, record, graph))
        })
        .collect::<Vec<_>>(),
    )
}

fn combined_coverage(values: &[usage::Coverage]) -> usage::Coverage {
    if values
        .iter()
        .all(|value| *value == usage::Coverage::Complete)
    {
        return usage::Coverage::Complete;
    }
    if values
        .iter()
        .all(|value| *value == usage::Coverage::Unknown)
    {
        return usage::Coverage::Unknown;
    }
    usage::Coverage::Partial
}

fn root_family(ledger: &Ledger, session_id: &str) -> Result<Vec<String>, String> {
    if !valid_id(session_id) {
        return Err("Invalid saved session identity.".into());
    }
    let record = project_record(ledger, session_id)
        .ok_or("This saved session does not belong to the selected project.")?;
    if record.lineage.kind == lineage::Kind::Subagent {
        return Err("Subagents are managed through their root chat.".into());
    }
    let graph = ledger_graph(ledger);
    if graph.relation(session_id).status == graph::Status::Invalid || graph.blocked(session_id) {
        return Err(
            "This saved session has an invalid relationship. Refresh the session list.".into(),
        );
    }
    Ok(std::iter::once(session_id.to_owned())
        .chain(graph.descendants(session_id))
        .collect())
}

fn resume_dashboard(watch: &Watch, session_id: &str) -> Result<Dashboard, String> {
    let (dashboard, _) = synchronize(watch)?;
    let _file_guard = watch.legacy_lock()?;
    let ledger = watch.stored_metadata()?;
    root_family(&ledger, session_id)?;
    if !project_record(&ledger, session_id).is_some_and(|record| record.source_state() == SourceState::Available) {
        return Err("This saved session file is unavailable. Refresh the session list.".into());
    }
    Ok(dashboard)
}

pub fn require_resume(
    app: &AppHandle,
    checked: &CheckedProject,
    session_id: &str,
) -> Result<(), String> {
    let watch = paths(app, checked)?;
    let history = app.state::<History>();
    let _guard = history.lock.lock().map_err(|error| error.to_string())?;
    let dashboard = resume_dashboard(&watch, session_id)?;
    environment_history::bind(app, &checked.project.id, &dashboard)
}

pub fn observe(app: &AppHandle, checked: &CheckedProject) -> Result<(), String> {
    let history = app.state::<History>();
    let watch = paths(app, checked)?;
    let _guard = history.lock.lock().map_err(|error| error.to_string())?;
    let project = projects::Registry::load(&projects::file(app)?)?
        .environment(&checked.project.id, &checked.project.environment_id)?;
    if project.stage == crate::projects::ProjectStage::Removing {
        return Err("This project is being removed.".into());
    }
    history
        .watches
        .lock()
        .map_err(|error| error.to_string())?
        .insert(
            environment_history::Target::for_project(&checked.project).key(),
            watch,
        );
    Ok(())
}

pub fn flush(app: &AppHandle) -> Result<(), String> {
    app.state::<History>().flush()
}

pub fn unobserve(app: &AppHandle, identifier: &str) -> Result<(), String> {
    app.state::<History>().unobserve(identifier)
}

pub fn unobserve_environment(
    app: &AppHandle,
    identifier: &str,
    environment_id: &projects::EnvironmentId,
) -> Result<(), String> {
    app.state::<History>()
        .remove(identifier, Some(environment_id), true)
}

pub fn monitor(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(2));
        if crate::desktop::lifecycle::is_closing() {
            continue;
        }
        let history = app.state::<History>();
        let result = history.visit(|key, watch| {
            if crate::desktop::lifecycle::is_closing() {
                return Ok(());
            }
            let id = watch
                .target
                .as_ref()
                .map(|target| target.project_id.as_str())
                .unwrap_or(key);
            history.refresh(key, watch)?;
            let dashboard = history.merged(id)?;
            environment_history::bind(&app, id, &dashboard)?;
            if history.publication_state(id, &dashboard)?.permits_publication() {
                let update = DashboardUpdate::new(dashboard);
                app.emit("project-usage-updated", (&id, &update))
                    .map_err(|error| error.to_string())?;
                history.record_published(id, &update.dashboard)?;
            }
            Ok(())
        });
        if let Err(error) = result {
            eprintln!("Could not synchronize session usage: {error}");
        }
    });
}

#[tauri::command]
pub async fn saved_project_dashboard(
    app: AppHandle,
    identifier: String,
) -> Result<Dashboard, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let registry = projects::Registry::load(&projects::file(&app)?)?;
        let environments = registry.environments(&identifier)?;
        let mut targets = Vec::new();
        for environment in environments.iter() {
            let project = registry.environment(&identifier, &environment.environment_id)?;
            targets.push(environment_history::Target::for_project(&project));
        }
        let store = app.state::<crate::usage_store::Runtime>().wait()?;
        environment_history::saved_snapshot(&targets, &store)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn recorded_project_dashboard(
    app: AppHandle,
    identifier: String,
) -> Result<RecordedDashboardUpdate, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let registry = projects::Registry::load(&projects::file(&app)?)?;
        let environments = registry.environments(&identifier)?;
        let mut targets = Vec::new();
        for environment in environments.iter() {
            let project = registry.environment(&identifier, &environment.environment_id)?;
            targets.push(environment_history::Target::for_project(&project));
        }
        let store = app.state::<crate::usage_store::Runtime>().wait()?;
        let history = app.state::<History>();
        let _guard = history.lock.lock().map_err(|error| error.to_string())?;
        let mut dashboard = environment_history::recorded_snapshot(&targets, &store)?;
        let verified = history
            .published
            .lock()
            .map_err(|error| error.to_string())?
            .get(&identifier)
            .cloned();
        environment_history::retain_verified_availability(&mut dashboard, verified.as_ref());
        Ok(RecordedDashboardUpdate {
            update: DashboardUpdate::new(dashboard),
            availability_verified: verified.is_some(),
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedDashboardUpdate {
    #[serde(flatten)]
    update: DashboardUpdate,
    availability_verified: bool,
}

#[tauri::command]
pub async fn project_dashboard(
    app: AppHandle,
    identifier: String,
) -> Result<DashboardUpdate, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let registry = projects::Registry::load(&projects::file(&app)?)?;
        let environments = registry.environments(&identifier)?;
        let store = app.state::<crate::usage_store::Runtime>().wait()?;
        let mut checked_watches = Vec::new();
        for environment in environments.iter() {
            let project = registry.environment(&identifier, &environment.environment_id)?;
            let target = environment_history::Target::for_project(&project);
            let watch = environment::checked_environment(
                &app,
                &identifier,
                Some(&project.environment_id),
                None,
            )
            .and_then(|checked| paths(&app, &checked));
            checked_watches.push((target, watch));
        }
        let history = app.state::<History>();
        let _guard = history.lock.lock().map_err(|error| error.to_string())?;
        let keys = checked_watches
            .iter()
            .map(|(target, _)| target.key())
            .collect();
        history.retain_project_keys(&identifier, &keys)?;
        for (target, watch) in checked_watches {
            let key = target.key();
            match watch {
                Ok(watch) => {
                    history
                        .watches
                        .lock()
                        .map_err(|error| error.to_string())?
                        .insert(key.clone(), watch.clone());
                    history.refresh(&key, &watch)?;
                }
                Err(message) => {
                    history
                        .watches
                        .lock()
                        .map_err(|error| error.to_string())?
                        .remove(&key);
                    history.saved_result(
                        &key,
                        &target,
                        &store,
                        format!("{} history: {message}", target.agent_kind.binary()),
                    )?;
                }
            }
        }
        let dashboard = history.merged(&identifier)?;
        history.record_published(&identifier, &dashboard)?;
        environment_history::bind(&app, &identifier, &dashboard)?;
        Ok(DashboardUpdate::new(dashboard))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn change_conversation_state(
    app: AppHandle,
    identifier: String,
    session_id: String,
    state: SessionState,
    environment_id: Option<projects::EnvironmentId>,
) -> Result<(), String> {
    if !valid_id(&session_id) {
        return Err("Invalid saved session identity.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let checked =
            environment::checked_environment(&app, &identifier, environment_id.as_ref(), None)?;
        let watch = paths(&app, &checked)?;
        let history = app.state::<History>();
        let _guard = history.lock.lock().map_err(|error| error.to_string())?;
        synchronize(&watch)?;
        change_state(&watch, &session_id, state, |ids| {
            if checked.project.agent_kind == AgentKind::Codex {
                return terminal::conversations_running(&identifier, ids);
            }
            terminal::terminal_sessions(identifier.clone())
                .iter()
                .any(|process| {
                    process.belongs_to(&checked.project)
                        && process.running
                        && process
                            .conversation_id
                            .as_ref()
                            .is_some_and(|id| ids.contains(id))
                })
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

fn change_state(
    watch: &Watch,
    session_id: &str,
    state: SessionState,
    running: impl Fn(&[String]) -> bool,
) -> Result<(), String> {
    if watch.store.is_some() {
        return sql_backend::change_state(watch, session_id, state, running);
    }
    watch.check_storage()?;
    let _file_guard = watch.legacy_lock()?;
    let mut ledger = watch.selected_ledger()?;
    let family = root_family(&ledger, session_id)?;
    if state != SessionState::Active && running(&family) {
        return Err(
            "End this chat's root and subagent terminal processes before archiving or deleting it."
                .into(),
        );
    }
    let record = ledger
        .sessions
        .get_mut(session_id)
        .ok_or("This saved session does not belong to the selected project.")?;
    record.state = state;
    watch.check_storage()?;
    watch.save(&mut ledger).map(|_| ())
}

#[tauri::command]
pub async fn conversation_usage(
    app: AppHandle,
    identifier: String,
    session_id: String,
    environment_id: Option<projects::EnvironmentId>,
) -> Result<ConversationUsageDetails, String> {
    if !valid_id(&session_id) {
        return Err("Invalid saved session identity.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let store = app.state::<crate::usage_store::Runtime>().wait()?;
        let project = projects::Registry::load(&projects::file(&app)?)?
            .resolve_environment(&identifier, environment_id.as_ref())?;
        let history = app.state::<History>();
        let _guard = history.lock.lock().map_err(|error| error.to_string())?;
        let target = environment_history::Target::for_project(&project);
        let watch = history
            .watches
            .lock()
            .map_err(|error| error.to_string())?
            .get(&target.key())
            .cloned();
        sql_backend::details(&target, &store, &session_id, |has_variants| {
            if let Some(watch) = watch {
                return Ok(Some(watch.namespace()));
            }
            if !has_variants {
                return Ok(None);
            }
            let checked = environment::checked_environment(
                &app,
                &identifier,
                Some(&project.environment_id),
                None,
            )?;
            let home = &checked.paths.host_home;
            Ok(Some(guest_namespace(
                &fs::canonicalize(home).unwrap_or_else(|_| home.into()),
            )))
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::time::Duration;

    const FIRST: &str = "01a1c5fd-894f-7e42-b36c-bd1a0dfef8e7";
    const SECOND: &str = "01a1c5fd-894f-7e42-b36c-bd1a0dfef8e8";
    const OTHER: &str = "01a1c5fd-894f-7e42-b36c-bd1a0dfef8e9";

    #[test]
    fn deterministic_large_history_scans_each_canonical_rollout_once() {
        const SESSION_COUNT: usize = 512;
        let nonce = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home = std::env::temp_dir().join(format!(
            "tugboat-large-history-{}-{nonce}",
            std::process::id()
        ));
        let folder = home.join(".codex/sessions/2026/11/11");
        fs::create_dir_all(&folder).unwrap();
        let expected: BTreeSet<_> = (0..SESSION_COUNT)
            .map(|index| format!("{index:08x}-0000-4000-8000-000000000001"))
            .collect();
        let root_id = expected.first().unwrap();
        let project = Path::new("/work/deterministic-project");
        for (index, id) in expected.iter().enumerate() {
            let mut payload = serde_json::json!({
                "id": id,
                "cwd": project,
                "cli_version": "0.167.0",
                "source": "cli"
            });
            if index > 0 {
                payload["cwd"] = "/work/deterministic-worktree".into();
                payload["source"] = serde_json::json!({
                    "subagent": {
                        "thread_spawn": {
                            "parent_thread_id": root_id,
                            "depth": 1,
                            "agent_path": format!("/root/child-{index:04}")
                        }
                    }
                });
            }
            let header = serde_json::json!({"type": "session_meta", "payload": payload});
            fs::write(
                folder.join(format!("rollout-{id}.jsonl")),
                format!("{header}\n"),
            )
            .unwrap();
        }
        let archive = home.join(".codex/archived_sessions");
        symlink(home.join(".codex/sessions"), &archive).unwrap();
        assert_eq!(
            fs::canonicalize(&archive).unwrap(),
            fs::canonicalize(home.join(".codex/sessions")).unwrap()
        );

        let started = std::time::Instant::now();
        let scan = scan_history(&home, project).unwrap();
        let elapsed = started.elapsed();
        assert_eq!(scan.metadata_visits, SESSION_COUNT);
        assert_eq!(scan.sessions.len(), SESSION_COUNT);
        assert_eq!(
            scan.sessions
                .iter()
                .map(|session| session.id.clone())
                .collect::<BTreeSet<_>>(),
            expected
        );
        let root = scan
            .sessions
            .iter()
            .find(|session| &session.id == root_id)
            .unwrap();
        assert_eq!(root.relation_status, graph::Status::Root);
        assert_eq!(root.subagent_count, (SESSION_COUNT - 1) as u64);
        for child in scan
            .sessions
            .iter()
            .filter(|session| &session.id != root_id)
        {
            assert_eq!(child.relation_status, graph::Status::Resolved);
            assert_eq!(child.root_session_id.as_deref(), Some(root_id.as_str()));
            assert_eq!(child.subagent_count, 0);
        }
        assert!(scan.problems.is_empty());
        assert!(scan.lineage_problems.is_empty());
        assert!(scan.warnings.is_empty());
        eprintln!(
            "P12 large history: sessions={SESSION_COUNT}, history_roots=2, canonical_metadata_visits={}, scan_elapsed_us={}",
            scan.metadata_visits,
            elapsed.as_micros()
        );
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn unobserve_waits_for_the_current_pass_and_prevents_later_reads() {
        let history = History::default();
        let watch = Watch {
            target: None,
            records_directory: None,
            store: None,
            home: "/fixture/home".into(),
            project: "/work/project".into(),
            ledger: "/fixture/ledger.json".into(),
        };
        history
            .watches
            .lock()
            .unwrap()
            .insert("removed".into(), watch.clone());
        history
            .watches
            .lock()
            .unwrap()
            .insert("remaining".into(), watch);
        history
            .record_published(
                "removed",
                &Dashboard {
                    usage_available: true,
                    conversations: Vec::new(),
                    usage: usage::Summary::default(),
                    pricing_date: usage::PRICING_DATE.into(),
                    anthropic_pricing_date: usage::anthropic::PRICING_DATE.into(),
                    warnings: Vec::new(),
                    coverage: usage::Coverage::Unknown,
                },
            )
            .unwrap();
        let (entered, started) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let reading = history.clone();
        let reader = std::thread::spawn(move || {
            reading
                .visit(|id, _| {
                    if id == "removed" {
                        entered.send(()).unwrap();
                        released.recv().unwrap();
                    }
                    Ok(())
                })
                .unwrap()
        });
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        let (completed, completion) = std::sync::mpsc::channel();
        let removing = history.clone();
        let remover = std::thread::spawn(move || {
            removing.unobserve("removed").unwrap();
            completed.send(()).unwrap();
        });
        assert!(completion.recv_timeout(Duration::from_millis(30)).is_err());
        release.send(()).unwrap();
        reader.join().unwrap();
        completion.recv_timeout(Duration::from_secs(2)).unwrap();
        remover.join().unwrap();
        let mut read = Vec::new();
        history
            .visit(|id, _| {
                read.push(id.to_owned());
                Ok(())
            })
            .unwrap();
        assert_eq!(read, ["remaining"]);
        assert!(!history.published.lock().unwrap().contains_key("removed"));
    }

    #[test]
    fn final_usage_is_flushed_after_the_terminal_writer_exits() {
        use std::io::{Read, Write};
        let root = std::env::temp_dir().join(format!("tugboat-exit-usage-{}", std::process::id()));
        let watch = Watch {
            target: None,
            records_directory: None,
            store: None,
            home: root.join("home"),
            project: "/work/project".into(),
            ledger: root.join("ledger.json"),
        };
        write_rollout(&watch.home, FIRST, "/work/project", 100);
        let source = watch.home.join(format!(
            ".codex/sessions/2026/11/10/rollout-fixture-{FIRST}.jsonl"
        ));
        writeln!(OpenOptions::new().append(true).open(&source).unwrap(), "{}", serde_json::json!({"type":"turn_context","payload":{"turn_id":"last","model":"gpt-6.1-sol"}})).unwrap();
        let history = History::default();
        history
            .watches
            .lock()
            .unwrap()
            .insert("project".into(), watch.clone());
        history.flush().unwrap();
        assert_eq!(
            usage::summarize(&load_ledger(&watch.ledger).unwrap().sessions[FIRST].turns).requests,
            0
        );
        let tokens = serde_json::json!({"input_tokens":100,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":10,"total_tokens":110});
        let event = serde_json::json!({"timestamp":"2026-11-11T01:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":tokens,"last_token_usage":tokens}}}).to_string();
        let pty = crate::terminal::pty::Pty::open().unwrap();
        let process = Arc::new(crate::terminal::pty::Process::new(&pty).unwrap());
        let mut command = std::process::Command::new("/bin/sh");
        command.args(["-c", r#"trap 'printf "%s\n" "$2" >> "$1"; exit 0' HUP; printf ready; while :; do read line; done"#, "fixture"]).arg(&source).arg(event);
        let mut child = process.spawn(&pty, &mut command).unwrap();
        let mut reader = pty.master.try_clone().unwrap();
        let mut ready = [0; 5];
        reader.read_exact(&mut ready).unwrap();
        drop(pty);
        let drain = std::thread::spawn(move || {
            let _ = reader.read_to_end(&mut Vec::new());
        });
        let waiting = process.clone();
        let waiter = std::thread::spawn(move || {
            child.wait().unwrap();
            waiting.exited();
        });
        crate::terminal::pty::terminate(&[process], Duration::from_secs(1)).unwrap();
        waiter.join().unwrap();
        drain.join().unwrap();
        history.flush().unwrap();
        assert_eq!(
            usage::summarize(&load_ledger(&watch.ledger).unwrap().sessions[FIRST].turns).requests,
            1
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn final_flush_records_other_projects_after_a_source_error() {
        let root = std::env::temp_dir().join(format!("tugboat-final-flush-{}", std::process::id()));
        let bad = Watch {
            target: None,
            records_directory: None,
            store: None,
            home: root.join("bad"),
            project: "/work/project".into(),
            ledger: root.join("bad-ledger.json"),
        };
        let good = Watch {
            target: None,
            records_directory: None,
            store: None,
            home: root.join("good"),
            project: "/work/project".into(),
            ledger: root.join("good-ledger.json"),
        };
        fs::create_dir_all(&bad.home).unwrap();
        fs::create_dir_all(root.join("outside")).unwrap();
        symlink(root.join("outside"), bad.home.join(".codex")).unwrap();
        write_rollout(&good.home, FIRST, "/work/project", 100);
        let path = good.home.join(format!(
            ".codex/sessions/2026/11/10/rollout-fixture-{FIRST}.jsonl"
        ));
        use std::io::Write;
        let mut output = OpenOptions::new().append(true).open(path).unwrap();
        writeln!(output, "{}", serde_json::json!({"type":"turn_context","payload":{"turn_id":"one","model":"gpt-6.1-sol"}})).unwrap();
        let tokens = serde_json::json!({"input_tokens":100,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":10,"total_tokens":110});
        writeln!(output, "{}", serde_json::json!({"timestamp":"2026-11-11T01:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":tokens,"last_token_usage":tokens}}})).unwrap();
        let history = History::default();
        history.watches.lock().unwrap().insert("01-bad".into(), bad);
        history
            .watches
            .lock()
            .unwrap()
            .insert("02-good".into(), good.clone());
        let error = history.flush().unwrap_err();
        assert!(error.contains("01-bad"));
        let ledger = load_ledger(&good.ledger).unwrap();
        assert_eq!(usage::summarize(&ledger.sessions[FIRST].turns).requests, 1);
        history.unobserve("02-good").unwrap();
        assert!(good.ledger.is_file());
        fs::remove_dir_all(root).unwrap();
    }

    fn write_rollout(root: &Path, id: &str, directory: &str, modified: u64) {
        let folder = root.join(".codex/sessions/2026/11/10");
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join(format!("rollout-fixture-{id}.jsonl"));
        let header = serde_json::json!({"type":"session_meta","payload":{"id":id,"cwd":directory}});
        fs::write(
            &path,
            format!("{header}\n{{\"type\":\"private-conversation-body\"}}\n"),
        )
        .unwrap();
        fs::File::open(path)
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(modified))
            .unwrap();
    }

    fn write_linked_rollout(home: &Path, id: &str, cwd: &str, parent: Option<&str>) -> PathBuf {
        write_rollout(home, id, cwd, 100);
        let path = home.join(format!(
            ".codex/sessions/2026/11/10/rollout-fixture-{id}.jsonl"
        ));
        let source = parent.map(|parent| serde_json::json!({"subagent":{"thread_spawn":{"parent_thread_id":parent,"depth":1,"agent_path":"/root/child"}}})).unwrap_or_else(|| "cli".into());
        let header = serde_json::json!({"type":"session_meta","payload":{"id":id,"cwd":cwd,"cli_version":"0.167.0","source":source}});
        fs::write(&path, format!("{header}\n")).unwrap();
        path
    }

    #[test]
    fn root_lifecycle_keeps_three_descendant_states_and_all_spending_unchanged() {
        let root =
            std::env::temp_dir().join(format!("tugboat-root-actions-{}", std::process::id()));
        let watch = Watch {
            target: None,
            records_directory: None,
            store: None,
            home: root.join("home"),
            project: "/work/fixture-project".into(),
            ledger: root.join("ledger.json"),
        };
        let folder = watch.home.join(".codex/sessions");
        fs::create_dir_all(&folder).unwrap();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures/codex-subagents/v0.167.0");
        for name in [
            "primary.jsonl",
            "child-alpha.jsonl",
            "child-beta.jsonl",
            "grandchild-gamma.jsonl",
        ] {
            fs::copy(fixtures.join(name), folder.join(format!("rollout-{name}"))).unwrap();
        }
        let (dashboard, _) = synchronize(&watch).unwrap();
        let selected = dashboard
            .conversations
            .iter()
            .find(|conversation| conversation.lineage.kind == lineage::Kind::Primary)
            .unwrap()
            .id
            .clone();
        let mut ledger = load_ledger(&watch.ledger).unwrap();
        let family = root_family(&ledger, &selected).unwrap();
        assert_eq!(family.len(), 4);
        let descendants: Vec<_> = family
            .iter()
            .filter(|id| **id != selected)
            .cloned()
            .collect();
        ledger.sessions.get_mut(&descendants[0]).unwrap().state = SessionState::Archived;
        ledger.sessions.get_mut(&descendants[1]).unwrap().state = SessionState::Deleted;
        write_ledger(&watch.ledger, &mut ledger).unwrap();
        let baseline = serde_json::to_value(&ledger).unwrap();
        for state in [
            SessionState::Archived,
            SessionState::Deleted,
            SessionState::Active,
        ] {
            change_state(&watch, &selected, state, |_| false).unwrap();
            let after = load_ledger(&watch.ledger).unwrap();
            assert_eq!(after.sessions[&selected].state, state);
            for id in &descendants {
                assert_eq!(
                    serde_json::to_value(&after.sessions[id]).unwrap(),
                    baseline["sessions"][id]
                );
            }
            assert_eq!(
                serde_json::to_value(&after.sessions[&selected].turns).unwrap(),
                baseline["sessions"][&selected]["turns"]
            );
            for child in &descendants {
                assert!(change_state(&watch, child, state, |_| false).is_err());
            }
        }
        let saved = fs::read(&watch.ledger).unwrap();
        for running in &family {
            assert!(
                change_state(&watch, &selected, SessionState::Archived, |ids| ids
                    .contains(running))
                .is_err()
            );
            assert!(
                change_state(&watch, &selected, SessionState::Deleted, |ids| ids
                    .contains(running))
                .is_err()
            );
            assert_eq!(fs::read(&watch.ledger).unwrap(), saved);
        }
        for name in [
            "child-alpha.jsonl",
            "child-beta.jsonl",
            "grandchild-gamma.jsonl",
        ] {
            fs::remove_file(folder.join(format!("rollout-{name}"))).unwrap();
        }
        for child in &descendants {
            assert!(resume_dashboard(&watch, child)
                .err()
                .unwrap()
                .contains("Subagents"));
        }
        assert!(resume_dashboard(&watch, &selected).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn declared_malformed_future_and_orphan_children_cannot_resume_or_change_state() {
        for (label, parent, version) in [
            ("malformed", "not-an-id", "0.167.0"),
            ("future", FIRST, "9.0.0"),
            ("orphan", FIRST, "0.167.0"),
        ] {
            let root = std::env::temp_dir().join(format!(
                "tugboat-child-guard-{}-{label}",
                std::process::id()
            ));
            let watch = Watch {
                target: None,
                records_directory: None,
                store: None,
                home: root.join("home"),
                project: "/work/project".into(),
                ledger: root.join("ledger.json"),
            };
            let path = write_linked_rollout(&watch.home, SECOND, "/work/project", Some(parent));
            let mut header: Value =
                serde_json::from_str(fs::read_to_string(&path).unwrap().trim()).unwrap();
            header["payload"]["cli_version"] = version.into();
            fs::write(&path, format!("{header}\n")).unwrap();
            assert!(resume_dashboard(&watch, SECOND)
                .err()
                .unwrap()
                .contains("Subagents"));
            for state in [
                SessionState::Archived,
                SessionState::Deleted,
                SessionState::Active,
            ] {
                assert!(change_state(&watch, SECOND, state, |_| false).is_err());
            }
            // A raw header that now claims a primary cannot erase a persisted child role.
            header["payload"]["source"] = "cli".into();
            header["payload"]["cli_version"] = "0.167.0".into();
            fs::write(&path, format!("{header}\n")).unwrap();
            assert!(resume_dashboard(&watch, SECOND).is_err());
            assert_eq!(
                load_ledger(&watch.ledger).unwrap().sessions[SECOND]
                    .lineage
                    .kind,
                lineage::Kind::Subagent
            );
            fs::remove_file(&path).unwrap();
            assert!(resume_dashboard(&watch, SECOND)
                .err()
                .unwrap()
                .contains("Subagents"));
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn legacy_root_remains_usable_but_invalid_and_project_excluded_records_are_denied() {
        let root =
            std::env::temp_dir().join(format!("tugboat-legacy-action-{}", std::process::id()));
        let watch = Watch {
            target: None,
            records_directory: None,
            store: None,
            home: root.join("home"),
            project: "/work/project".into(),
            ledger: root.join("ledger.json"),
        };
        write_rollout(&watch.home, FIRST, "/work/project", 100);
        assert!(resume_dashboard(&watch, FIRST).is_ok());
        change_state(&watch, FIRST, SessionState::Archived, |_| false).unwrap();
        change_state(&watch, FIRST, SessionState::Active, |_| false).unwrap();
        let mut ledger = load_ledger(&watch.ledger).unwrap();
        ledger.sessions.get_mut(FIRST).unwrap().lineage = lineage::Lineage {
            kind: lineage::Kind::Primary,
            parent_session_id: Some(SECOND.into()),
            source_version: Some("0.167.0".into()),
            ..Default::default()
        };
        assert!(root_family(&ledger, FIRST).is_err());
        ledger.sessions.get_mut(FIRST).unwrap().lineage = lineage::Lineage::default();
        ledger.sessions.get_mut(FIRST).unwrap().project_exclusion = Some(ProjectExclusion {
            root_session_id: FIRST.into(),
            root_cwd: "/work/other".into(),
        });
        write_ledger(&watch.ledger, &mut ledger).unwrap();
        assert!(change_state(&watch, FIRST, SessionState::Active, |_| false).is_err());
        fs::remove_file(watch.home.join(format!(
            ".codex/sessions/2026/11/10/rollout-fixture-{FIRST}.jsonl"
        )))
        .unwrap();
        assert!(resume_dashboard(&watch, FIRST).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn real_guest_project_uses_own_requests_and_family_totals_with_scoped_revisions() {
        let root =
            std::env::temp_dir().join(format!("tugboat-real-project-{}", std::process::id()));
        let watch = Watch {
            target: None,
            records_directory: None,
            store: None,
            home: root.join("home"),
            project: "/work/fixture-project".into(),
            ledger: root.join("ledger.json"),
        };
        let directory = watch.home.join(".codex/sessions");
        fs::create_dir_all(&directory).unwrap();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures/codex-subagents/v0.167.0");
        for name in [
            "primary.jsonl",
            "child-alpha.jsonl",
            "child-beta.jsonl",
            "grandchild-gamma.jsonl",
            "normal-fork.jsonl",
            "p02/history-primary.jsonl",
            "p02/history-child.jsonl",
        ] {
            fs::copy(
                fixtures.join(name),
                directory.join(format!(
                    "rollout-{}",
                    Path::new(name).file_name().unwrap().to_string_lossy()
                )),
            )
            .unwrap();
        }
        let (before, _) = synchronize(&watch).unwrap();
        assert_eq!(before.conversations.len(), 7);
        assert_eq!(
            (
                before.usage.input,
                before.usage.cached,
                before.usage.cache_write,
                before.usage.output,
                before.usage.reasoning,
                before.usage.total,
                before.usage.requests
            ),
            (319136, 296448, 0, 1646, 808, 320782, 21)
        );
        assert_eq!(before.coverage, usage::Coverage::Complete);
        let find = |dashboard: &Dashboard, suffix: &str| {
            dashboard
                .conversations
                .iter()
                .find(|conversation| conversation.id.ends_with(suffix))
                .unwrap()
                .clone()
        };
        let primary = find(&before, "001");
        assert_eq!(
            (
                primary.usage.total,
                primary.family_usage.total,
                primary.subagent_count
            ),
            (124036, 229351, 3)
        );
        assert_eq!(
            (
                find(&before, "008").family_usage.total,
                find(&before, "009").usage.completed_turns
            ),
            (75549, 1)
        );
        assert_eq!(find(&before, "005").usage.total, 15882);
        let ledger = load_ledger(&watch.ledger).unwrap();
        assert_eq!(
            usage::own_turns(
                &ledger.sessions[&find(&before, "009").id].turns,
                &find(&before, "009").id
            )
            .len(),
            1
        );

        fn assert_details_partition(details: &usage_details::ConversationUsageDetails) {
            let expected: BTreeSet<_> = details
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
            let references: Vec<_> = details
                .prompts
                .iter()
                .flat_map(|prompt| &prompt.request_refs)
                .chain(&details.unassigned.request_refs)
                .map(|reference| {
                    (
                        &reference.session_id,
                        &reference.turn_id,
                        &reference.request_key,
                    )
                })
                .collect();
            assert_eq!(references.len() as u64, details.combined_usage.requests);
            assert_eq!(references.len(), expected.len());
            assert_eq!(references.into_iter().collect::<BTreeSet<_>>(), expected);
            assert_eq!(
                details
                    .prompts
                    .iter()
                    .map(|prompt| prompt.combined_usage.total)
                    .sum::<u64>()
                    + details.unassigned.usage.total,
                details.combined_usage.total
            );
        }
        const ROOT_FIRST_TURN: &str = "10000000-0000-4000-8000-000000000001";
        const ROOT_SECOND_TURN: &str = "10000000-0000-4000-8000-000000000002";
        const ALPHA_FIRST_TURN: &str = "10000000-0000-4000-8000-000000000003";
        let alpha = find(&before, "002");
        let gamma = find(&before, "004");
        let details = usage_details::from_ledger(&ledger, &primary.id).unwrap();
        assert_eq!(details.combined_usage.total, primary.family_usage.total);
        assert_eq!(
            (details.user_prompt_count, details.agent_turn_count),
            (Some(2), 6)
        );
        let prompt_totals: BTreeMap<_, _> = details
            .prompts
            .iter()
            .map(|prompt| (prompt.turn_id.as_str(), prompt.combined_usage.total))
            .collect();
        assert_eq!(
            prompt_totals,
            BTreeMap::from([(ROOT_FIRST_TURN, 166475), (ROOT_SECOND_TURN, 62876)])
        );
        assert_eq!(details.unassigned.usage.total, 0);
        assert_details_partition(&details);

        let selected = usage_details::from_ledger(&ledger, &alpha.id).unwrap();
        assert_eq!(
            selected.root_session_id.as_deref(),
            Some(primary.id.as_str())
        );
        assert_eq!(
            selected
                .agents
                .iter()
                .map(|agent| agent.session_id.as_str())
                .collect::<Vec<_>>(),
            vec![alpha.id.as_str(), gamma.id.as_str()]
        );
        assert_eq!(
            (
                selected.own_usage.total,
                selected.descendants_usage.total,
                selected.combined_usage.total
            ),
            (75884, 14655, 90539)
        );
        assert_eq!(
            (selected.user_prompt_count, selected.agent_turn_count),
            (Some(2), 3)
        );
        assert_details_partition(&selected);

        let inherited = usage_details::from_ledger(&ledger, &find(&before, "009").id).unwrap();
        assert_eq!(
            inherited.root_session_id.as_deref(),
            Some(find(&before, "008").id.as_str())
        );
        assert_eq!(
            (
                inherited.agents.len(),
                inherited.agent_turn_count,
                inherited.user_prompt_count,
                inherited.combined_usage.total
            ),
            (1, 1, Some(1), 15729)
        );
        assert_eq!(inherited.agents[0].turns.len(), 1);
        assert_eq!(inherited.prompts.len(), 1);
        assert_eq!(
            inherited.prompts[0].turn_id,
            "10000000-0000-4000-8000-000000000019"
        );
        assert_eq!(inherited.prompts[0].combined_usage.total, 15729);
        assert_details_partition(&inherited);

        let mut conflicting = load_ledger(&watch.ledger).unwrap();
        conflicting
            .sessions
            .get_mut(&alpha.id)
            .unwrap()
            .turns
            .get_mut(ALPHA_FIRST_TURN)
            .unwrap()
            .root_turn_id = Some(ROOT_SECOND_TURN.into());
        let unresolved = usage_details::from_ledger(&conflicting, &primary.id).unwrap();
        assert_eq!(unresolved.combined_usage, details.combined_usage);
        assert_eq!(unresolved.user_prompt_count, Some(2));
        assert_eq!(unresolved.unassigned.usage.total, 60391);
        assert!(unresolved
            .unassigned
            .request_refs
            .iter()
            .all(|reference| reference.session_id == alpha.id
                && reference.turn_id == ALPHA_FIRST_TURN));
        assert_details_partition(&unresolved);

        let mut missing_root_turn = load_ledger(&watch.ledger).unwrap();
        missing_root_turn
            .sessions
            .get_mut(&primary.id)
            .unwrap()
            .turns
            .remove(ROOT_FIRST_TURN);
        let unassigned = usage_details::from_ledger(&missing_root_turn, &alpha.id).unwrap();
        assert_eq!(unassigned.combined_usage, selected.combined_usage);
        assert_eq!(unassigned.user_prompt_count, Some(1));
        assert_eq!(unassigned.unassigned.usage.total, 75046);
        assert_details_partition(&unassigned);

        // A request-level tier observation changes provenance without changing tokens or price.
        let path = directory.join("rollout-child-alpha.jsonl");
        let mut body = fs::read_to_string(&path).unwrap();
        let mut notification: Value = body
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .rfind(|record| record["payload"]["type"] == "token_count")
            .unwrap();
        notification["payload"]["info"]["service_tier"] = "default".into();
        body.push_str(&format!("{notification}\n"));
        fs::write(&path, body).unwrap();
        let history = History::default();
        history.record_published("real", &before).unwrap();
        let (after, _) = synchronize(&watch).unwrap();
        assert_eq!(before.usage, after.usage);
        assert!(history.should_publish("real", &after).unwrap());
        assert_ne!(
            find(&before, "002").usage_revision,
            find(&after, "002").usage_revision
        );
        assert_eq!(
            find(&before, "005").usage_revision,
            find(&after, "005").usage_revision
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn late_foreign_parent_exclusion_survives_missing_child_sources_and_restart() {
        for missing_before_parent in [false, true] {
            let root = std::env::temp_dir().join(format!(
                "tugboat-foreign-parent-{}-{missing_before_parent}",
                std::process::id()
            ));
            let watch = Watch {
                target: None,
                records_directory: None,
                store: None,
                home: root.join("home"),
                project: "/work/project-b".into(),
                ledger: root.join("ledger-b.json"),
            };
            let child = write_linked_rollout(&watch.home, SECOND, "/work/project-b", Some(FIRST));
            let body = fs::read_to_string(&child).unwrap();
            let context = serde_json::json!({"type":"turn_context","payload":{"turn_id":"10000000-0000-4000-8000-000000000001","model":"gpt-6.1-sol"}});
            let tokens = serde_json::json!({"input_tokens":100,"cached_input_tokens":20,"cache_write_input_tokens":0,"output_tokens":10,"reasoning_output_tokens":2,"total_tokens":110});
            let notification = serde_json::json!({"type":"event_msg","timestamp":"2026-11-01T00:00:00Z","payload":{"type":"token_count","info":{"last_token_usage":tokens,"total_token_usage":tokens}}});
            fs::write(&child, format!("{body}{context}\n{notification}\n")).unwrap();
            let (orphan, _) = synchronize(&watch).unwrap();
            assert_eq!(orphan.usage.total, 110);
            assert_eq!(
                orphan.conversations[0].relation_status,
                graph::Status::Orphan
            );
            let raw =
                serde_json::to_value(&load_ledger(&watch.ledger).unwrap().sessions[SECOND].turns)
                    .unwrap();
            if missing_before_parent {
                fs::remove_file(&child).unwrap();
            }
            let parent = write_linked_rollout(&watch.home, FIRST, "/work/project-a", None);
            let (excluded, _) = synchronize(&watch).unwrap();
            assert!(excluded.conversations.is_empty());
            assert_eq!(excluded.usage.total, 0);
            let ledger = load_ledger(&watch.ledger).unwrap();
            assert_eq!(
                serde_json::to_value(&ledger.sessions[SECOND].turns).unwrap(),
                raw
            );
            let proof = ledger.sessions[SECOND].project_exclusion.as_ref().unwrap();
            assert_eq!(proof.root_session_id, FIRST);
            assert_eq!(proof.root_cwd, Path::new("/work/project-a"));
            assert!(project_record(&ledger, SECOND).is_none());
            if !missing_before_parent {
                let foreign = Watch {
                    target: None,
                    records_directory: None,
                    store: None,
                    home: watch.home.clone(),
                    project: "/work/project-a".into(),
                    ledger: root.join("ledger-a.json"),
                };
                let (belongs, _) = synchronize(&foreign).unwrap();
                assert_eq!(belongs.usage.total, 110);
                assert_eq!(belongs.conversations.len(), 2);
                fs::remove_file(child).unwrap();
            }
            fs::remove_file(parent).unwrap();
            let (restart, _) = synchronize(&watch).unwrap();
            assert!(restart.conversations.is_empty());
            assert_eq!(restart.usage.total, 0);
            assert!(load_ledger(&watch.ledger).unwrap().sessions[SECOND]
                .project_exclusion
                .is_some());
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn measured_zero_empty_turn_and_unknown_price_have_separate_coverage() {
        let root = std::env::temp_dir().join(format!("tugboat-measurement-{}", std::process::id()));
        let watch = Watch {
            target: None,
            records_directory: None,
            store: None,
            home: root.join("home"),
            project: "/work/synthetic-project".into(),
            ledger: root.join("ledger.json"),
        };
        let path = watch.home.join(".codex/sessions/rollout-zero.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures/codex-subagents/v0.167.0/p02/synthetic-cases.json");
        let cases: Value = serde_json::from_slice(&fs::read(fixtures).unwrap()).unwrap();
        let records = cases["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == "zero-request-and-empty-own-turn")
            .unwrap()["records"]
            .as_array()
            .unwrap();
        let measured: Vec<_> = records.iter().take(7).cloned().collect();
        let body = |records: &[Value]| {
            records
                .iter()
                .map(|record| format!("{record}\n"))
                .collect::<String>()
        };
        fs::write(&path, body(&measured)).unwrap();
        let (zero, _) = synchronize(&watch).unwrap();
        assert_eq!(
            (
                zero.usage.requests,
                zero.usage.total,
                zero.usage.unmeasured_turns
            ),
            (1, 0, 0)
        );
        assert_eq!(zero.coverage, usage::Coverage::Complete);
        let mut ledger = load_ledger(&watch.ledger).unwrap();
        let selected = zero.conversations[0].id.clone();
        let request = ledger
            .sessions
            .get_mut(&selected)
            .unwrap()
            .turns
            .values_mut()
            .flat_map(|turn| &mut turn.requests)
            .next()
            .unwrap();
        request.rates = None;
        request.cost_usd = None;
        request.unpriced_reason = Some("Historical price unavailable".into());
        assert_eq!(
            own_coverage(
                &selected,
                &ledger.sessions[&selected],
                &ledger_graph(&ledger)
            ),
            usage::Coverage::Complete
        );
        ledger
            .sessions
            .get_mut(&selected)
            .unwrap()
            .turns
            .values_mut()
            .flat_map(|turn| &mut turn.requests)
            .next()
            .unwrap()
            .source
            .as_mut()
            .unwrap()
            .root_turn_id = None;
        assert_eq!(
            own_coverage(
                &selected,
                &ledger.sessions[&selected],
                &ledger_graph(&ledger)
            ),
            usage::Coverage::Partial
        );
        fs::write(&path, body(records)).unwrap();
        let (empty, _) = synchronize(&watch).unwrap();
        assert_eq!(
            (
                empty.usage.requests,
                empty.usage.total,
                empty.usage.unmeasured_turns
            ),
            (1, 0, 1)
        );
        assert_eq!(empty.coverage, usage::Coverage::Partial);
        fs::remove_dir_all(root).unwrap();
    }

    fn legacy_v1_bytes() -> Vec<u8> {
        let rates = serde_json::json!({"date":"2025-01-01","source":"fixture saved rates","input":17.0,"cached":3.0,"write30m":21.0,"write12h":42.0,"output":91.0,"tier":"legacy-special","tierAssumed":false,"longContext":false});
        let request = serde_json::json!({"key":"immutable-old-key","at":"2025-01-01T00:00:00Z","model":"historical-model","tokens":{"input":1000,"cached":500,"cacheWrite":0,"output":100,"reasoning":20,"write12h":0,"cacheKnown":true},"rates":rates,"costUsd":0.123456,"partial":false,"unpricedReason":null,"serviceTier":"legacy-special","tierSource":"request","rateCorrection":{"previousRates":rates,"previousCostUsd":0.654321,"reason":"old tier evidence"}});
        let record = |state| serde_json::json!({"title":"Preserved title","updatedAt":321,"state":state,"turns":{"old-turn":{"id":"old-turn","status":"complete","requests":[request]}},"cursors":{},"parserVersion":1,"available":false});
        format!(" \n{}\n  ", serde_json::json!({"schemaVersion":1,"sessions":{FIRST:record("archived"),SECOND:record("deleted")},"warnings":["Retained warning"]})).into_bytes()
    }

    #[test]
    fn v1_read_is_non_mutating_and_migration_has_byte_exact_immutable_backup() {
        use std::os::unix::fs::MetadataExt;
        let root = std::env::temp_dir().join(format!("tugboat-v1-migrate-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let file = root.join("ledger.json");
        let bytes = legacy_v1_bytes();
        fs::write(&file, &bytes).unwrap();
        let before = load_ledger(&file).unwrap();
        assert_eq!(before.schema_version, 1);
        assert_eq!(fs::read(&file).unwrap(), bytes);
        assert!(!storage::v1_backup_path(&file).exists());
        let saved = serde_json::to_value(&before).unwrap();
        let _lock = storage::lock(&file).unwrap();
        storage::immutable_v1_backup(&file).unwrap(); // Resume after a crash between backup and replace.
        let backup_inode = fs::metadata(storage::v1_backup_path(&file)).unwrap().ino();
        let mut migrated = load_ledger(&file).unwrap();
        write_ledger(&file, &mut migrated).unwrap();
        assert_eq!(migrated.schema_version, 2);
        assert_eq!(fs::read(storage::v1_backup_path(&file)).unwrap(), bytes);
        assert_eq!(
            fs::metadata(storage::v1_backup_path(&file)).unwrap().ino(),
            backup_inode
        );
        let after = serde_json::to_value(load_ledger(&file).unwrap()).unwrap();
        assert_eq!(after["sessions"], saved["sessions"]);
        assert_eq!(after["warnings"], saved["warnings"]);
        assert_eq!(after["sessions"][FIRST]["lineage"]["kind"], "unknown");
        write_ledger(&file, &mut migrated).unwrap();
        assert_eq!(fs::read(storage::v1_backup_path(&file)).unwrap(), bytes);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_v1_writers_migrate_once_without_losing_either_change() {
        let root =
            std::env::temp_dir().join(format!("tugboat-v1-concurrent-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let file = root.join("ledger.json");
        let bytes = legacy_v1_bytes();
        fs::write(&file, &bytes).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let writers: Vec<_> = [" first", " second"]
            .into_iter()
            .map(|suffix| {
                let file = file.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let _lock = storage::lock(&file).unwrap();
                    let mut ledger = load_ledger(&file).unwrap();
                    ledger
                        .sessions
                        .get_mut(FIRST)
                        .unwrap()
                        .title
                        .push_str(suffix);
                    write_ledger(&file, &mut ledger).unwrap();
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        let ledger = load_ledger(&file).unwrap();
        assert_eq!(ledger.schema_version, 2);
        assert!(ledger.sessions[FIRST].title.contains(" first"));
        assert!(ledger.sessions[FIRST].title.contains(" second"));
        assert_eq!(ledger.sessions[FIRST].state, SessionState::Archived);
        assert_eq!(fs::read(storage::v1_backup_path(&file)).unwrap(), bytes);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mismatched_backup_future_schema_and_symlinks_never_overwrite_inputs() {
        let root = std::env::temp_dir().join(format!("tugboat-v1-refuse-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let file = root.join("ledger.json");
        let original = legacy_v1_bytes();
        fs::write(&file, &original).unwrap();
        let _lock = storage::lock(&file).unwrap();
        storage::immutable_v1_backup(&file).unwrap();
        let changed = String::from_utf8(original.clone())
            .unwrap()
            .replace("Preserved title", "Changed title")
            .into_bytes();
        fs::write(&file, &changed).unwrap();
        assert!(write_ledger(&file, &mut load_ledger(&file).unwrap()).is_err());
        assert_eq!(fs::read(&file).unwrap(), changed);
        assert_eq!(fs::read(storage::v1_backup_path(&file)).unwrap(), original);
        let future = b"{\"schemaVersion\":99,\"sessions\":{}}";
        fs::write(&file, future).unwrap();
        assert!(load_ledger(&file).is_err());
        assert_eq!(fs::read(&file).unwrap(), future);
        let alias = root.join("alias.json");
        symlink(&file, &alias).unwrap();
        assert!(load_ledger(&alias).is_err());
        let fresh = root.join("fresh.json");
        fs::write(&fresh, legacy_v1_bytes()).unwrap();
        symlink(&file, storage::v1_backup_path(&fresh)).unwrap();
        assert!(storage::immutable_v1_backup(&fresh).is_err());
        assert_eq!(fs::read(&file).unwrap(), future);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn different_cwd_closure_excludes_other_project_and_preserves_omitted_child_source() {
        let root = std::env::temp_dir().join(format!("tugboat-cwd-closure-{}", std::process::id()));
        let watch = Watch {
            target: None,
            records_directory: None,
            store: None,
            home: root.join("home"),
            project: "/work/project".into(),
            ledger: root.join("ledger.json"),
        };
        write_linked_rollout(&watch.home, FIRST, "/work/project", None);
        let child = write_linked_rollout(&watch.home, SECOND, "/work/child-worktree", Some(FIRST));
        write_linked_rollout(&watch.home, OTHER, "/work/other-project", None);
        write_linked_rollout(
            &watch.home,
            "01a1c5fd-894f-7e42-b36c-bd1a0dfef8e6",
            "/work/project",
            Some(OTHER),
        );
        let (dashboard, _) = synchronize(&watch).unwrap();
        assert_eq!(dashboard.conversations.len(), 2);
        let selected_child = dashboard
            .conversations
            .iter()
            .find(|session| session.id == SECOND)
            .unwrap();
        assert_eq!(selected_child.root_session_id.as_deref(), Some(FIRST));
        assert_eq!(selected_child.relation_status, graph::Status::Resolved);
        assert_eq!(
            dashboard
                .conversations
                .iter()
                .find(|session| session.id == FIRST)
                .unwrap()
                .subagent_count,
            1
        );
        let mut header: Value =
            serde_json::from_str(fs::read_to_string(&child).unwrap().trim()).unwrap();
        header["payload"].as_object_mut().unwrap().remove("source");
        fs::write(child, format!("{header}\n")).unwrap();
        let (retained, _) = synchronize(&watch).unwrap();
        let selected_child = retained
            .conversations
            .iter()
            .find(|session| session.id == SECOND)
            .unwrap();
        assert!(selected_child.available);
        assert_eq!(selected_child.lineage.kind, lineage::Kind::Subagent);
        assert_eq!(selected_child.root_session_id.as_deref(), Some(FIRST));
        assert_eq!(
            selected_child.lineage_problem,
            Some(lineage::Problem::MissingSource)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn late_parent_changes_metadata_and_publishes_without_token_changes() {
        let root = std::env::temp_dir().join(format!("tugboat-late-parent-{}", std::process::id()));
        let watch = Watch {
            target: None,
            records_directory: None,
            store: None,
            home: root.join("home"),
            project: "/work/project".into(),
            ledger: root.join("ledger.json"),
        };
        write_linked_rollout(&watch.home, SECOND, "/work/project", Some(FIRST));
        let (before, _) = synchronize(&watch).unwrap();
        assert_eq!(
            before.conversations[0].relation_status,
            graph::Status::Orphan
        );
        let history = History::default();
        history.record_published("project", &before).unwrap();
        write_linked_rollout(&watch.home, FIRST, "/work/project", None);
        let (after, _) = synchronize(&watch).unwrap();
        assert_eq!(before.usage, after.usage);
        assert!(history.should_publish("project", &after).unwrap());
        let child = after
            .conversations
            .iter()
            .find(|session| session.id == SECOND)
            .unwrap();
        assert_eq!(child.relation_status, graph::Status::Resolved);
        assert_eq!(child.root_session_id.as_deref(), Some(FIRST));
        assert!(child.warning.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cached_project_parent_with_unknown_role_admits_child_without_guessing_a_root() {
        let root =
            std::env::temp_dir().join(format!("tugboat-cached-parent-{}", std::process::id()));
        let watch = Watch {
            target: None,
            records_directory: None,
            store: None,
            home: root.join("home"),
            project: "/work/project".into(),
            ledger: root.join("ledger.json"),
        };
        write_linked_rollout(&watch.home, SECOND, "/work/child-worktree", Some(FIRST));
        let mut record = SessionRecord::default();
        record.cursors.insert(
            fs::canonicalize(watch.home.join(".codex"))
                .unwrap()
                .join("sessions/rollout-missing-parent.jsonl"),
            usage::Cursor::default(),
        );
        let ledger = Ledger {
            sessions: BTreeMap::from([(FIRST.into(), record)]),
            ..Default::default()
        };
        storage::write(&watch.ledger, &ledger).unwrap();
        let (dashboard, _) = synchronize(&watch).unwrap();
        let child = dashboard
            .conversations
            .iter()
            .find(|session| session.id == SECOND)
            .unwrap();
        assert!(child.available);
        assert_eq!(child.relation_status, graph::Status::Unknown);
        assert!(child.root_session_id.is_none());
        assert_eq!(child.lineage.kind, lineage::Kind::Subagent);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_different_known_session_root_is_a_negative_cross_check_and_body_is_not_imported() {
        let root =
            std::env::temp_dir().join(format!("tugboat-conflicting-root-{}", std::process::id()));
        let watch = Watch {
            target: None,
            records_directory: None,
            store: None,
            home: root.join("home"),
            project: "/work/project".into(),
            ledger: root.join("ledger.json"),
        };
        write_linked_rollout(&watch.home, FIRST, "/work/project", None);
        write_linked_rollout(&watch.home, OTHER, "/work/other-project", None);
        let child = write_linked_rollout(&watch.home, SECOND, "/work/child-worktree", Some(FIRST));
        let mut header: Value =
            serde_json::from_str(fs::read_to_string(&child).unwrap().trim()).unwrap();
        header["payload"]["session_id"] = OTHER.into();
        fs::write(
            &child,
            format!("{header}\nthis body must never be parsed\n"),
        )
        .unwrap();
        let scan = scan_history(&watch.home, &watch.project).unwrap();
        assert_eq!(scan.sessions.len(), 1);
        assert_eq!(
            scan.lineage_problems.get(SECOND),
            Some(&lineage::Problem::Conflicting)
        );
        let (dashboard, _) = synchronize(&watch).unwrap();
        assert_eq!(dashboard.conversations.len(), 1);
        assert_eq!(dashboard.conversations[0].id, FIRST);
        assert!(!load_ledger(&watch.ledger)
            .unwrap()
            .sessions
            .contains_key(SECOND));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cached_parent_from_a_different_guest_cannot_admit_an_outside_cwd_child() {
        let root = std::env::temp_dir().join(format!("tugboat-other-guest-{}", std::process::id()));
        let home = root.join("home");
        write_linked_rollout(&home, SECOND, "/work/other-worktree", Some(FIRST));
        let mut record = SessionRecord {
            lineage: lineage::Lineage {
                kind: lineage::Kind::Primary,
                source_version: Some("0.167.0".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        record.cursors.insert(
            PathBuf::from("/different-guest/.codex/sessions/rollout-parent.jsonl"),
            usage::Cursor::default(),
        );
        let scan = scan_history_known(
            &home,
            Path::new("/work/project"),
            &BTreeMap::from([(FIRST.into(), record)]),
        )
        .unwrap();
        assert!(scan.sessions.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shared_ledger_keeps_different_guest_spending_without_cross_guest_family_binding() {
        let root =
            std::env::temp_dir().join(format!("tugboat-guest-boundary-{}", std::process::id()));
        let first = Watch {
            target: None,
            records_directory: None,
            store: None,
            home: root.join("first-home"),
            project: "/work/fixture-project".into(),
            ledger: root.join("ledger.json"),
        };
        let second = Watch {
            target: None,
            records_directory: None,
            store: None,
            home: root.join("second-home"),
            project: first.project.clone(),
            ledger: first.ledger.clone(),
        };
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures/codex-subagents/v0.167.0");
        let root_id = "00000000-0000-4000-8000-000000000001";
        let child_id = "00000000-0000-4000-8000-000000000002";
        let parent_path = first.home.join(".codex/sessions/rollout-parent.jsonl");
        let child_path = second.home.join(".codex/sessions/rollout-child.jsonl");
        fs::create_dir_all(parent_path.parent().unwrap()).unwrap();
        fs::create_dir_all(child_path.parent().unwrap()).unwrap();
        fs::copy(fixtures.join("primary.jsonl"), &parent_path).unwrap();
        synchronize(&first).unwrap();
        let saved_root =
            serde_json::to_value(&load_ledger(&first.ledger).unwrap().sessions[root_id].turns)
                .unwrap();
        fs::copy(fixtures.join("child-alpha.jsonl"), &child_path).unwrap();
        let scan = scan_history_known(
            &second.home,
            &second.project,
            &load_ledger(&second.ledger).unwrap().sessions,
        )
        .unwrap();
        assert_eq!(scan.sessions.len(), 1);
        assert_eq!(scan.sessions[0].relation_status, graph::Status::Orphan);
        for missing in [false, true] {
            if missing {
                fs::remove_file(&child_path).unwrap();
                fs::remove_file(&parent_path).unwrap();
            }
            let (dashboard, _) = synchronize(&second).unwrap();
            assert_eq!(dashboard.usage.total, 124036 + 75884);
            let parent = dashboard
                .conversations
                .iter()
                .find(|conversation| conversation.id == root_id)
                .unwrap();
            let child = dashboard
                .conversations
                .iter()
                .find(|conversation| conversation.id == child_id)
                .unwrap();
            assert_eq!(parent.family_usage.total, 124036);
            assert_eq!(parent.subagent_count, 0);
            assert_eq!(child.relation_status, graph::Status::Orphan);
            assert!(child.root_session_id.is_none());
            let ledger = load_ledger(&second.ledger).unwrap();
            assert_eq!(
                serde_json::to_value(&ledger.sessions[root_id].turns).unwrap(),
                saved_root
            );
            let details = usage_details::from_ledger(&ledger, child_id).unwrap();
            assert!(details.root_session_id.is_none());
            assert_eq!(details.agents.len(), 1);
            assert_eq!(details.combined_usage.total, 75884);
            assert_eq!(
                usage_details::from_ledger(&ledger, root_id)
                    .unwrap()
                    .combined_usage
                    .total,
                124036
            );
        }
        // A new process can load the same durable edge boundary after all source files disappear.
        let graph = ledger_graph(&load_ledger(&second.ledger).unwrap());
        assert_eq!(graph.relation(child_id).status, graph::Status::Orphan);
        assert!(graph.descendants(root_id).is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn archived_only_history_pins_the_first_real_inherited_child_header() {
        let root =
            std::env::temp_dir().join(format!("tugboat-archived-lineage-{}", std::process::id()));
        let watch = Watch {
            target: None,
            records_directory: None,
            store: None,
            home: root.join("home"),
            project: "/work/fixture-project".into(),
            ledger: root.join("ledger.json"),
        };
        let folder = watch.home.join(".codex/archived_sessions");
        fs::create_dir_all(&folder).unwrap();
        fs::write(
            folder.join("rollout-child.jsonl"),
            include_str!(
                "../../../tests/fixtures/codex-subagents/v0.167.0/p02/history-child.jsonl"
            ),
        )
        .unwrap();
        let sessions = read_history(&watch.home, &watch.project).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "00000000-0000-4000-8000-000000000009");
        assert_eq!(sessions[0].lineage.kind, lineage::Kind::Subagent);
        assert_eq!(
            sessions[0].lineage.parent_session_id.as_deref(),
            Some("00000000-0000-4000-8000-000000000008")
        );
        assert!(sessions[0]
            .source
            .starts_with(fs::canonicalize(folder).unwrap()));
        let (dashboard, _) = synchronize(&watch).unwrap();
        assert_eq!(dashboard.usage.total, 15729);
        assert!(dashboard.conversations[0].available);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_lineage_keeps_own_usage_and_a_typed_problem() {
        let root =
            std::env::temp_dir().join(format!("tugboat-malformed-lineage-{}", std::process::id()));
        let watch = Watch {
            target: None,
            records_directory: None,
            store: None,
            home: root.join("home"),
            project: "/work/project".into(),
            ledger: root.join("ledger.json"),
        };
        write_rollout(&watch.home, FIRST, "/work/project", 100);
        let path = watch.home.join(format!(
            ".codex/sessions/2026/11/10/rollout-fixture-{FIRST}.jsonl"
        ));
        let header = serde_json::json!({"type":"session_meta","payload":{"id":FIRST,"cwd":"/work/project","cli_version":"0.167.0","source":{"subagent":{"thread_spawn":{"parent_thread_id":17,"depth":1}}}}});
        let context = serde_json::json!({"type":"turn_context","payload":{"turn_id":"own","model":"gpt-6.1-sol"}});
        let tokens = serde_json::json!({"input_tokens":100,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":10,"total_tokens":110});
        let event = serde_json::json!({"timestamp":"2026-11-11T01:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":tokens,"last_token_usage":tokens}}});
        fs::write(path, format!("{header}\n{context}\n{event}\n")).unwrap();
        let (dashboard, _) = synchronize(&watch).unwrap();
        assert_eq!(dashboard.usage.total, 110);
        assert_eq!(
            dashboard.conversations[0].lineage_problem,
            Some(lineage::Problem::Malformed)
        );
        assert!(dashboard.conversations[0]
            .warning
            .as_ref()
            .unwrap()
            .contains("malformed"));
        assert!(dashboard.conversations[0].available);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn omitted_source_preserves_verified_child_and_problem_across_missing_source_restart() {
        let root =
            std::env::temp_dir().join(format!("tugboat-lineage-retain-{}", std::process::id()));
        let watch = Watch {
            target: None,
            records_directory: None,
            store: None,
            home: root.join("home"),
            project: "/work/project".into(),
            ledger: root.join("ledger.json"),
        };
        write_rollout(&watch.home, FIRST, "/work/project", 100);
        let path = watch.home.join(format!(
            ".codex/sessions/2026/11/10/rollout-fixture-{FIRST}.jsonl"
        ));
        let header = serde_json::json!({"type":"session_meta","payload":{"id":FIRST,"cwd":"/work/project","cli_version":"0.167.0","source":{"subagent":{"thread_spawn":{"parent_thread_id":SECOND,"depth":1}}}}});
        fs::write(&path, format!("{header}\n")).unwrap();
        synchronize(&watch).unwrap();
        let mut omitted = header;
        omitted["payload"].as_object_mut().unwrap().remove("source");
        fs::write(&path, format!("{omitted}\n")).unwrap();
        let (dashboard, _) = synchronize(&watch).unwrap();
        assert_eq!(
            dashboard.conversations[0].lineage.kind,
            lineage::Kind::Subagent
        );
        assert_eq!(
            dashboard.conversations[0]
                .lineage
                .parent_session_id
                .as_deref(),
            Some(SECOND)
        );
        assert_eq!(
            dashboard.conversations[0].lineage_problem,
            Some(lineage::Problem::MissingSource)
        );
        fs::remove_file(path).unwrap();
        let (reopened, _) = synchronize(&watch).unwrap();
        assert!(!reopened.conversations[0].available);
        assert_eq!(
            reopened.conversations[0].lineage.kind,
            lineage::Kind::Subagent
        );
        assert_eq!(
            reopened.conversations[0].lineage_problem,
            Some(lineage::Problem::MissingSource)
        );
        assert_eq!(
            load_ledger(&watch.ledger).unwrap().sessions[FIRST].lineage_problem,
            Some(lineage::Problem::MissingSource)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn archived_history_symlink_cannot_escape_the_verified_guest_home() {
        let root =
            std::env::temp_dir().join(format!("tugboat-archive-symlink-{}", std::process::id()));
        fs::create_dir_all(root.join("home/.codex")).unwrap();
        fs::create_dir_all(root.join("outside")).unwrap();
        symlink(
            root.join("outside"),
            root.join("home/.codex/archived_sessions"),
        )
        .unwrap();
        assert!(read_history(&root.join("home"), Path::new("/work/project")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn history_uses_latest_titles_and_keeps_only_this_project_in_recent_order() {
        let home = std::env::temp_dir().join(format!("tugboat-history-{}", std::process::id()));
        write_rollout(&home, FIRST, "/work/project", 100);
        write_rollout(&home, SECOND, "/work/project/src", 200);
        write_rollout(&home, OTHER, "/work/project-other", 300);
        let first = serde_json::json!({"id":FIRST,"thread_name":"Old title"});
        let latest = serde_json::json!({"id":FIRST,"thread_name":"A long session title with Türkçe and 日本語 that must remain complete in its tooltip"});
        let second = serde_json::json!({"id":SECOND,"thread_name":"Second session"});
        fs::write(
            home.join(".codex/session_index.jsonl"),
            format!("{first}\n{latest}\n{second}\n{{\"id\":"),
        )
        .unwrap();
        let sessions = read_history(&home, Path::new("/work/project")).unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id, SECOND);
        assert_eq!(sessions[1].id, FIRST);
        assert!(sessions[1].title.ends_with("complete in its tooltip"));
        assert!(sessions[1].title.contains("日本語"));
        assert_eq!(sessions[0].updated_at, 200_000);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn missing_history_is_empty_and_history_symlinks_cannot_escape_the_guest_home() {
        let root =
            std::env::temp_dir().join(format!("tugboat-history-links-{}", std::process::id()));
        let home = root.join("home");
        let outside = root.join("outside");
        fs::create_dir_all(&home).unwrap();
        assert!(read_history(&home, Path::new("/work/project"))
            .unwrap()
            .is_empty());
        write_rollout(&outside, FIRST, "/work/project", 100);
        symlink(outside.join(".codex"), home.join(".codex")).unwrap();
        assert!(read_history(&home, Path::new("/work/project")).is_err());
        fs::remove_file(home.join(".codex")).unwrap();
        fs::create_dir(home.join(".codex")).unwrap();
        fs::write(root.join("private-index"), "unrelated private data").unwrap();
        symlink(
            root.join("private-index"),
            home.join(".codex/session_index.jsonl"),
        )
        .unwrap();
        let scan = scan_history(&home, Path::new("/work/project")).unwrap();
        assert!(!scan.warnings.is_empty());
        assert!(scan.sessions.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ledger_preserves_totals_and_archive_state_across_restarts_and_missing_rollouts() {
        let root = std::env::temp_dir().join(format!("tugboat-ledger-{}", std::process::id()));
        let watch = Watch {
            target: None,
            records_directory: None,
            store: None,
            home: root.join("home"),
            project: "/work/project".into(),
            ledger: root.join("ledger.json"),
        };
        write_rollout(&watch.home, FIRST, "/work/project", 100);
        let path = watch.home.join(format!(
            ".codex/sessions/2026/11/10/rollout-fixture-{FIRST}.jsonl"
        ));
        let context = serde_json::json!({"type":"turn_context","payload":{"turn_id":"turn-one","model":"gpt-6.1-sol"}});
        let tokens = serde_json::json!({"input_tokens":1000,"cached_input_tokens":100,"cache_write_input_tokens":0,"output_tokens":50,"reasoning_output_tokens":0,"total_tokens":1050});
        let event = serde_json::json!({"timestamp":"2026-11-10T10:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":tokens,"last_token_usage":tokens}}});
        let complete = serde_json::json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"turn-one"}});
        let mut body = fs::read_to_string(&path).unwrap();
        body += &format!("{context}\n{event}\n{complete}\n");
        fs::write(&path, body).unwrap();
        let (first, _) = synchronize(&watch).unwrap();
        assert_eq!(first.usage.total, 1050);
        assert_eq!(first.usage.completed_turns, 1);
        assert!(first.usage.cost_usd > 0.0);
        let original_cost = first.usage.cost_usd;
        for state in [
            SessionState::Archived,
            SessionState::Deleted,
            SessionState::Active,
        ] {
            let mut ledger = load_ledger(&watch.ledger).unwrap();
            ledger.sessions.get_mut(FIRST).unwrap().state = state;
            storage::write(&watch.ledger, &ledger).unwrap();
            let (result, _) = synchronize(&watch).unwrap();
            assert_eq!(result.conversations[0].state, state);
            assert_eq!(result.usage.total, 1050);
            assert_eq!(result.usage.cost_usd, original_cost);
            assert_eq!(result.usage.requests, 1);
        }
        assert!(!fs::read_to_string(&watch.ledger)
            .unwrap()
            .contains("private-conversation-body"));
        let history = History::default();
        history.record_published("project", &first).unwrap();
        let held = root.join("held-rollout.jsonl");
        fs::rename(&path, &held).unwrap();
        let (result, changed) = synchronize(&watch).unwrap();
        assert!(changed);
        assert_eq!(result.usage.total, 1050);
        assert!(!result.conversations[0].available);
        assert_eq!(result.usage.cost_usd, original_cost);
        // Another process may already have saved the availability transition.
        let (unchanged, changed) = synchronize(&watch).unwrap();
        assert!(!changed);
        assert!(history.should_publish("project", &unchanged).unwrap());
        history.record_published("project", &unchanged).unwrap();
        assert!(!history
            .should_publish("project", &synchronize(&watch).unwrap().0)
            .unwrap());
        fs::rename(held, path).unwrap();
        let (restored, changed) = synchronize(&watch).unwrap();
        assert!(changed);
        assert!(restored.conversations[0].available);
        assert_eq!(restored.usage.requests, 1);
        assert_eq!(restored.usage.cost_usd, original_cost);
        assert!(history.should_publish("project", &restored).unwrap());
        history.record_published("project", &restored).unwrap();
        assert!(!history
            .should_publish("project", &synchronize(&watch).unwrap().0)
            .unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_archive_and_usage_writers_preserve_both_changes() {
        let root =
            std::env::temp_dir().join(format!("tugboat-ledger-concurrent-{}", std::process::id()));
        let watch = Watch {
            target: None,
            records_directory: None,
            store: None,
            home: root.join("home"),
            project: "/work/project".into(),
            ledger: root.join("ledger.json"),
        };
        write_rollout(&watch.home, FIRST, "/work/project", 100);
        synchronize(&watch).unwrap();
        let path = watch.home.join(format!(
            ".codex/sessions/2026/11/10/rollout-fixture-{FIRST}.jsonl"
        ));
        let mut output = OpenOptions::new().append(true).open(path).unwrap();
        use std::io::Write;
        writeln!(output, "{}", serde_json::json!({"type":"turn_context","payload":{"turn_id":"new-turn","model":"gpt-6.1-sol"}})).unwrap();
        let tokens = serde_json::json!({"input_tokens":1000,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":100,"total_tokens":1100});
        writeln!(output, "{}", serde_json::json!({"timestamp":"2026-11-11T05:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":tokens,"last_token_usage":tokens}}})).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let updating = watch.clone();
        let start = barrier.clone();
        let synchronize_thread = std::thread::spawn(move || {
            start.wait();
            synchronize(&updating).unwrap();
        });
        barrier.wait();
        change_state(&watch, FIRST, SessionState::Archived, |_| false).unwrap();
        synchronize_thread.join().unwrap();
        let (dashboard, _) = synchronize(&watch).unwrap();
        assert_eq!(dashboard.conversations[0].state, SessionState::Archived);
        assert_eq!(dashboard.usage.requests, 1);
        assert_eq!(dashboard.usage.total, 1100);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn damaged_sessions_and_titles_do_not_block_healthy_usage_and_repair_replays_once() {
        let root =
            std::env::temp_dir().join(format!("tugboat-history-repair-{}", std::process::id()));
        let watch = Watch {
            target: None,
            records_directory: None,
            store: None,
            home: root.join("home"),
            project: "/work/project".into(),
            ledger: root.join("ledger.json"),
        };
        write_rollout(&watch.home, FIRST, "/work/project", 100);
        write_rollout(&watch.home, SECOND, "/work/project", 200);
        let folder = watch.home.join(".codex/sessions/2026/11/10");
        let first = folder.join(format!("rollout-fixture-{FIRST}.jsonl"));
        let second = folder.join(format!("rollout-fixture-{SECOND}.jsonl"));
        let context = serde_json::json!({"type":"turn_context","payload":{"turn_id":"one","model":"gpt-6.1-sol"}});
        let event = |at: &str, input: u64, last_input: u64| {
            let tokens = |input| serde_json::json!({"input_tokens":input,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":input/10,"total_tokens":input+input/10});
            serde_json::json!({"timestamp":at,"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":tokens(input),"last_token_usage":tokens(last_input)}}})
        };
        let initial = event("2026-11-11T01:00:00Z", 100, 100);
        use std::io::Write;
        for path in [&first, &second] {
            writeln!(
                OpenOptions::new().append(true).open(path).unwrap(),
                "{context}\n{initial}"
            )
            .unwrap();
        }
        let index = watch.home.join(".codex/session_index.jsonl");
        fs::write(
            &index,
            format!(
                "{}\n",
                serde_json::json!({"id":FIRST,"thread_name":"Preserved title"})
            ),
        )
        .unwrap();
        assert_eq!(synchronize(&watch).unwrap().0.usage.total, 220);
        let repaired = event("2026-11-11T02:00:00Z", 200, 100);
        let mut damaged = repaired.clone();
        damaged["payload"]["info"]["last_token_usage"]
            .as_object_mut()
            .unwrap()
            .remove("input_tokens");
        writeln!(
            OpenOptions::new().append(true).open(&first).unwrap(),
            "{damaged}"
        )
        .unwrap();
        writeln!(
            OpenOptions::new().append(true).open(&second).unwrap(),
            "{}",
            event("2026-11-11T02:00:00Z", 300, 200)
        )
        .unwrap();
        let bad_header = folder.join("rollout-invalid-header.jsonl");
        fs::write(&bad_header, "{bad header}\n").unwrap();
        fs::write(
            &index,
            format!(
                "{{invalid record}}\n{}\n",
                serde_json::json!({"id":SECOND,"thread_name":"Healthy title"})
            ),
        )
        .unwrap();
        let (partial, changed) = synchronize(&watch).unwrap();
        assert!(changed);
        assert_eq!(partial.conversations.len(), 2);
        assert_eq!(partial.usage.total, 440);
        assert_eq!(partial.usage.requests, 3);
        let damaged_session = partial
            .conversations
            .iter()
            .find(|session| session.id == FIRST)
            .unwrap();
        assert_eq!(damaged_session.title, "Preserved title");
        assert!(damaged_session
            .warning
            .as_ref()
            .unwrap()
            .contains("Missing input token count"));
        let healthy = partial
            .conversations
            .iter()
            .find(|session| session.id == SECOND)
            .unwrap();
        assert!(healthy.available);
        assert!(healthy.warning.is_none());
        assert!(!partial.warnings.is_empty());
        assert!(!synchronize(&watch).unwrap().1);
        fs::write(
            &first,
            fs::read_to_string(&first)
                .unwrap()
                .replace(&damaged.to_string(), &repaired.to_string()),
        )
        .unwrap();
        fs::remove_file(bad_header).unwrap();
        fs::write(
            &index,
            format!(
                "{}\n{}\n",
                serde_json::json!({"id":FIRST,"thread_name":"Repaired title"}),
                serde_json::json!({"id":SECOND,"thread_name":"Healthy title"})
            ),
        )
        .unwrap();
        let (repaired_dashboard, _) = synchronize(&watch).unwrap();
        assert_eq!(repaired_dashboard.usage.total, 550);
        assert_eq!(repaired_dashboard.usage.requests, 4);
        assert!(repaired_dashboard.warnings.is_empty());
        assert!(repaired_dashboard
            .conversations
            .iter()
            .all(|session| session.warning.is_none()));
        assert_eq!(synchronize(&watch).unwrap().0.usage.requests, 4);
        let original = fs::read_to_string(&first).unwrap();
        fs::write(&first, "{incomplete metadata}\n").unwrap();
        let (metadata_error, _) = synchronize(&watch).unwrap();
        let damaged_session = metadata_error
            .conversations
            .iter()
            .find(|session| session.id == FIRST)
            .unwrap();
        assert!(!damaged_session.available);
        assert!(damaged_session.warning.is_some());
        assert_eq!(metadata_error.usage.total, 550);
        fs::write(&first, original).unwrap();
        let (final_dashboard, _) = synchronize(&watch).unwrap();
        assert_eq!(final_dashboard.usage.requests, 4);
        assert!(final_dashboard
            .conversations
            .iter()
            .all(|session| session.available && session.warning.is_none()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_cache_knowledge_is_recovered_without_changing_prices_or_archive_state() {
        let root =
            std::env::temp_dir().join(format!("tugboat-cache-migration-{}", std::process::id()));
        let watch = Watch {
            target: None,
            records_directory: None,
            store: None,
            home: root.join("home"),
            project: "/work/project".into(),
            ledger: root.join("ledger.json"),
        };
        write_rollout(&watch.home, FIRST, "/work/project", 100);
        let path = watch.home.join(format!(
            ".codex/sessions/2026/11/10/rollout-fixture-{FIRST}.jsonl"
        ));
        use std::io::Write;
        let mut output = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(output, "{}", serde_json::json!({"type":"turn_context","payload":{"turn_id":"one","model":"gpt-6.1-sol"}})).unwrap();
        let tokens = serde_json::json!({"input_tokens":300,"cached_input_tokens":75,"output_tokens":30,"total_tokens":330});
        writeln!(output, "{}", serde_json::json!({"timestamp":"2026-11-11T01:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":tokens,"last_token_usage":tokens}}})).unwrap();
        synchronize(&watch).unwrap();
        change_state(&watch, FIRST, SessionState::Archived, |_| false).unwrap();
        let mut legacy = serde_json::to_value(load_ledger(&watch.ledger).unwrap()).unwrap();
        legacy["sessions"][FIRST]["parserVersion"] = 1.into();
        let record = &mut legacy["sessions"][FIRST]["turns"]["one"]["requests"][0];
        let old_rates = record["rates"].clone();
        for field in ["cachedKnown", "cacheWriteKnown"] {
            record["tokens"].as_object_mut().unwrap().remove(field);
        }
        storage::write(&watch.ledger, &legacy).unwrap();
        let (dashboard, _) = synchronize(&watch).unwrap();
        assert_eq!(dashboard.usage.cached, 75);
        assert_eq!(dashboard.usage.missing_cached_requests, 0);
        assert_eq!(dashboard.usage.missing_cache_write_requests, 1);
        assert_eq!(dashboard.usage.unpriced_requests, 1);
        assert_eq!(dashboard.usage.requests, 1);
        assert_eq!(dashboard.conversations[0].state, SessionState::Archived);
        let saved = serde_json::to_value(load_ledger(&watch.ledger).unwrap()).unwrap();
        assert_eq!(
            saved["sessions"][FIRST]["turns"]["one"]["requests"][0]["rates"],
            old_rates
        );
        assert!(!synchronize(&watch).unwrap().1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saved_session_ids_cannot_be_flags_or_paths() {
        assert!(valid_id(FIRST));
        for id in [
            "--last",
            "../../another-project",
            "01a1c5fd-894f-7e42-b36c-bd1a0dfef8ez",
            "",
        ] {
            assert!(!valid_id(id));
        }
    }

    #[test]
    fn legacy_ledger_replays_recorded_tiers_without_counting_requests_twice() {
        let root =
            std::env::temp_dir().join(format!("tugboat-tier-migration-{}", std::process::id()));
        let watch = Watch {
            target: None,
            records_directory: None,
            store: None,
            home: root.join("home"),
            project: "/work/project".into(),
            ledger: root.join("ledger.json"),
        };
        write_rollout(&watch.home, FIRST, "/work/project", 100);
        let path = watch.home.join(format!(
            ".codex/sessions/2026/11/10/rollout-fixture-{FIRST}.jsonl"
        ));
        use std::io::Write;
        let mut output = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(output, "{}", serde_json::json!({"type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"service_tier":"priority"}}})).unwrap();
        writeln!(output, "{}", serde_json::json!({"type":"turn_context","payload":{"turn_id":"one","model":"gpt-6.1-sol"}})).unwrap();
        let tokens = serde_json::json!({"input_tokens":1000,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":100,"total_tokens":1100});
        writeln!(output, "{}", serde_json::json!({"timestamp":"2026-11-11T01:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":tokens,"last_token_usage":tokens}}})).unwrap();
        assert!((synchronize(&watch).unwrap().0.usage.cost_usd - 0.006).abs() < 1e-12);
        let mut old = serde_json::to_value(load_ledger(&watch.ledger).unwrap()).unwrap();
        let session = &mut old["sessions"][FIRST];
        session.as_object_mut().unwrap().remove("parserVersion");
        let request = &mut session["turns"]["one"]["requests"][0];
        for field in ["tierSource", "serviceTier", "rateCorrection"] {
            request.as_object_mut().unwrap().remove(field);
        }
        let rates = &mut request["rates"];
        for field in ["input", "cached", "write30m", "write12h", "output"] {
            rates[field] = (rates[field].as_f64().unwrap() / 2.0).into();
        }
        rates["tier"] = "standard".into();
        rates["tierAssumed"] = true.into();
        rates["date"] = "2026-10-01".into();
        request["costUsd"] = 0.003.into();
        storage::write(&watch.ledger, &old).unwrap();
        let (migrated, changed) = synchronize(&watch).unwrap();
        assert!(changed);
        assert_eq!(migrated.usage.requests, 1);
        assert!((migrated.usage.cost_usd - 0.006).abs() < 1e-12);
        let ledger = load_ledger(&watch.ledger).unwrap();
        let request = &ledger.sessions[FIRST].turns["one"].requests[0];
        assert_eq!(request.tier_source.as_deref(), Some("thread-settings"));
        assert_eq!(request.rates.as_ref().unwrap().date, "2026-10-01");
        let evidence = serde_json::to_value(request).unwrap();
        assert_eq!(evidence["rateCorrection"]["previousCostUsd"], 0.003);
        assert!(!synchronize(&watch).unwrap().1);
        assert_eq!(synchronize(&watch).unwrap().0.usage.requests, 1);
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod history_tests;

#[cfg(test)]
mod environment_history_tests;

#[cfg(test)]
pub(crate) mod sqlite_benchmark;
