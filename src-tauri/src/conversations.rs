use crate::{
    environment::{self, CheckedProject},
    projects, storage, terminal, usage,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub updated_at: u64,
    #[serde(default)]
    pub state: SessionState,
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub usage: usage::Summary,
    #[serde(skip)]
    source: PathBuf,
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

fn titles(directory: &Path) -> Result<HashMap<String, String>, String> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory.join("session_index.jsonl"))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
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
    let mut lines = source.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim().is_empty() {
            continue;
        }
        let entry = match serde_json::from_str::<IndexEntry>(line) {
            Ok(entry) => entry,
            Err(error) if error.is_eof() && lines.peek().is_none() && !source.ends_with('\n') => break,
            Err(_) => return Err("The saved session index has an unsupported record. Refresh after Codex finishes writing.".into()),
        };
        if valid_id(&entry.id) && !entry.thread_name.trim().is_empty() {
            result.insert(entry.id, entry.thread_name);
        }
    }
    Ok(result)
}

fn rollout(path: &Path, guest_project: &Path) -> Result<Option<(String, u64)>, String> {
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
        || !directory.starts_with(guest_project)
    {
        return Ok(None);
    }
    Ok(Some((id.into(), modified)))
}

fn read_history(home: &Path, guest_project: &Path) -> Result<Vec<Conversation>, String> {
    let directory = home.join(".codex");
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let home = fs::canonicalize(home).map_err(|_| "The guest home is unavailable.")?;
    let directory = fs::canonicalize(directory).map_err(|_| "The Codex history is unavailable.")?;
    if !directory.starts_with(&home) {
        return Err("The Codex history points outside this guest home.".into());
    }
    let names = titles(&directory)?;
    let root = directory.join("sessions");
    if !root.exists() {
        return Ok(Vec::new());
    }
    let root = fs::canonicalize(root).map_err(|_| "The saved sessions are unavailable.")?;
    if !root.starts_with(&directory) {
        return Err("The saved sessions point outside the Codex history directory.".into());
    }
    let mut pending = vec![(root.clone(), 0)];
    let mut sessions: HashMap<String, Conversation> = HashMap::new();
    while let Some((directory, depth)) = pending.pop() {
        let entries = fs::read_dir(directory).map_err(|_| "Could not list saved sessions.")?;
        for entry in entries {
            let entry = entry.map_err(|_| "Could not read a saved session entry.")?;
            let kind = entry
                .file_type()
                .map_err(|_| "Could not read a saved session entry.")?;
            if kind.is_dir() && depth < 3 {
                pending.push((entry.path(), depth + 1));
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !kind.is_file() || !name.starts_with("rollout-") || !name.ends_with(".jsonl") {
                continue;
            }
            let path = fs::canonicalize(entry.path())
                .map_err(|_| "A saved session moved. Refresh the list.")?;
            if !path.starts_with(&root) {
                return Err("A saved session points outside this project's history.".into());
            }
            if let Some((id, updated_at)) = rollout(&path, guest_project)? {
                let title = names
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| "Untitled session".into());
                let previous = sessions.entry(id.clone()).or_insert(Conversation {
                    id,
                    title,
                    updated_at,
                    state: SessionState::Active,
                    available: true,
                    usage: usage::Summary::default(),
                    source: path.clone(),
                });
                if updated_at >= previous.updated_at {
                    previous.source = path;
                }
                previous.updated_at = previous.updated_at.max(updated_at);
            }
        }
    }
    let mut sessions: Vec<_> = sessions.into_values().collect();
    sessions.sort_by(|first, second| {
        second
            .updated_at
            .cmp(&first.updated_at)
            .then_with(|| first.id.cmp(&second.id))
    });
    Ok(sessions)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    #[default]
    Active,
    Archived,
    Deleted,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionRecord {
    title: String,
    updated_at: u64,
    state: SessionState,
    turns: BTreeMap<String, usage::TurnUsage>,
    cursors: BTreeMap<PathBuf, usage::Cursor>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ledger {
    schema_version: u32,
    sessions: BTreeMap<String, SessionRecord>,
}
impl Default for Ledger {
    fn default() -> Self {
        Self {
            schema_version: 1,
            sessions: BTreeMap::new(),
        }
    }
}
fn load_ledger(file: &Path) -> Result<Ledger, String> {
    let ledger = storage::read::<Ledger>(file)?.unwrap_or_default();
    if ledger.schema_version != 1 {
        return Err("This usage ledger version is unsupported.".into());
    }
    Ok(ledger)
}
#[derive(Clone)]
struct Watch {
    home: PathBuf,
    project: PathBuf,
    ledger: PathBuf,
}
#[derive(Default, Clone)]
pub struct History(pub Arc<Mutex<()>>, Arc<Mutex<HashMap<String, Watch>>>);
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dashboard {
    pub conversations: Vec<Conversation>,
    pub usage: usage::Summary,
    pub pricing_date: String,
}

fn paths(checked: &CheckedProject, file: PathBuf) -> Result<Watch, String> {
    let data = &checked.plan["data"];
    let home = data["home"]["host"]
        .as_str()
        .ok_or("The guest home is missing from the project plan.")?;
    let project = data["project"]["guest"]
        .as_str()
        .ok_or("The guest project path is missing from the plan.")?;
    Ok(Watch {
        home: home.into(),
        project: project.into(),
        ledger: file,
    })
}
fn ledger_file(app: &AppHandle, id: &str) -> Result<PathBuf, String> {
    if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid project identity.".into());
    }
    Ok(app
        .path()
        .app_config_dir()
        .map_err(|error| error.to_string())?
        .join("conversation-ledgers")
        .join(format!("{id}.json")))
}
fn synchronize(watch: &Watch) -> Result<(Dashboard, bool), String> {
    let sessions = read_history(&watch.home, &watch.project)?;
    let mut ledger = load_ledger(&watch.ledger)?;
    let mut changed = false;
    let mut sources = HashMap::new();
    for session in sessions {
        sources.insert(session.id.clone(), session.source.clone());
        let record = ledger.sessions.entry(session.id).or_default();
        if record.title != session.title || record.updated_at != session.updated_at {
            record.title = session.title;
            record.updated_at = session.updated_at;
            changed = true;
        }
        let cursor = record.cursors.entry(session.source.clone()).or_default();
        changed |= usage::sync_file(&session.source, cursor, &mut record.turns)?;
    }
    if changed {
        storage::write(&watch.ledger, &ledger)?;
    }
    let mut usage = usage::Summary::default();
    let mut conversations = Vec::new();
    for (id, record) in ledger.sessions {
        let summary = usage::summarize(&record.turns);
        usage.add(&summary);
        let available = sources.contains_key(&id);
        conversations.push(Conversation {
            available,
            source: sources.remove(&id).unwrap_or_default(),
            id,
            title: record.title,
            updated_at: record.updated_at,
            state: record.state,
            usage: summary,
        });
    }
    conversations.sort_by(|first, second| {
        second
            .updated_at
            .cmp(&first.updated_at)
            .then_with(|| first.id.cmp(&second.id))
    });
    Ok((
        Dashboard {
            conversations,
            usage,
            pricing_date: usage::PRICING_DATE.into(),
        },
        changed,
    ))
}

pub fn list(checked: &CheckedProject) -> Result<Vec<Conversation>, String> {
    let watch = paths(checked, PathBuf::new())?;
    read_history(&watch.home, &watch.project)
}

pub fn observe(app: &AppHandle, checked: &CheckedProject) -> Result<(), String> {
    let history = app.state::<History>();
    let watch = paths(checked, ledger_file(app, &checked.project.id)?)?;
    history
        .1
        .lock()
        .map_err(|error| error.to_string())?
        .insert(checked.project.id.clone(), watch);
    Ok(())
}

pub fn flush(app: &AppHandle) -> Result<(), String> {
    let history = app.state::<History>();
    let watches = history.1.lock().map_err(|error| error.to_string())?.clone();
    let _guard = history.0.lock().map_err(|error| error.to_string())?;
    for watch in watches.values() {
        synchronize(watch)?;
    }
    Ok(())
}

pub fn monitor(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let history = app.state::<History>();
        let result = (|| -> Result<(), String> {
            let watches = history.1.lock().map_err(|error| error.to_string())?.clone();
            let _guard = history.0.lock().map_err(|error| error.to_string())?;
            for (id, watch) in watches {
                match synchronize(&watch) {
                    Ok((dashboard, changed)) => {
                        if terminal::bind_conversations(&id, &dashboard.conversations) {
                            app.emit("terminal-sessions-changed", &id)
                                .map_err(|error| error.to_string())?;
                        }
                        if changed {
                            app.emit("project-usage-updated", (&id, dashboard))
                                .map_err(|error| error.to_string())?;
                        }
                    }
                    Err(message) => {
                        app.emit("project-usage-error", (&id, message))
                            .map_err(|error| error.to_string())?;
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("Could not synchronize session usage: {error}");
        }
    });
}

#[tauri::command]
pub async fn project_dashboard(app: AppHandle, identifier: String) -> Result<Dashboard, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let checked = environment::checked_project(&app, &identifier, None)?;
        observe(&app, &checked)?;
        let history = app.state::<History>();
        let _guard = history.0.lock().map_err(|error| error.to_string())?;
        let watch = paths(&checked, ledger_file(&app, &identifier)?)?;
        let (dashboard, _) = synchronize(&watch)?;
        if terminal::bind_conversations(&identifier, &dashboard.conversations) {
            app.emit("terminal-sessions-changed", &identifier)
                .map_err(|error| error.to_string())?;
        }
        Ok(dashboard)
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
) -> Result<(), String> {
    if !valid_id(&session_id) {
        return Err("Invalid saved session identity.".into());
    }
    if state != SessionState::Active && terminal::conversation_running(&identifier, &session_id) {
        return Err("End this session's terminal process before archiving or deleting it.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        projects::Registry::load(&projects::file(&app)?)?.project(&identifier)?;
        let history = app.state::<History>();
        let _guard = history.0.lock().map_err(|error| error.to_string())?;
        let file = ledger_file(&app, &identifier)?;
        let mut ledger = load_ledger(&file)?;
        let record = ledger
            .sessions
            .get_mut(&session_id)
            .ok_or("This saved session does not belong to the selected project.")?;
        record.state = state;
        storage::write(&file, &ledger)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn conversation_usage(
    app: AppHandle,
    identifier: String,
    session_id: String,
) -> Result<Vec<usage::TurnUsage>, String> {
    if !valid_id(&session_id) {
        return Err("Invalid saved session identity.".into());
    }
    projects::Registry::load(&projects::file(&app)?)?.project(&identifier)?;
    let history = app.state::<History>();
    let _guard = history.0.lock().map_err(|error| error.to_string())?;
    let ledger = load_ledger(&ledger_file(&app, &identifier)?)?;
    let record = ledger
        .sessions
        .get(&session_id)
        .ok_or("Saved session not found.")?;
    Ok(record.turns.values().cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::time::Duration;

    const FIRST: &str = "01a1c5fd-894f-7e42-b36c-bd1a0dfef8e7";
    const SECOND: &str = "01a1c5fd-894f-7e42-b36c-bd1a0dfef8e8";
    const OTHER: &str = "01a1c5fd-894f-7e42-b36c-bd1a0dfef8e9";

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
        assert!(read_history(&home, Path::new("/work/project")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ledger_preserves_totals_and_archive_state_across_restarts_and_missing_rollouts() {
        let root = std::env::temp_dir().join(format!("tugboat-ledger-{}", std::process::id()));
        let watch = Watch {
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
        fs::remove_file(path).unwrap();
        let (result, _) = synchronize(&watch).unwrap();
        assert_eq!(result.usage.total, 1050);
        assert!(!result.conversations[0].available);
        assert_eq!(result.usage.cost_usd, original_cost);
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
}
