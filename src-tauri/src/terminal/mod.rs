use crate::{platform::command, projects};
use pty::Pty;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::io::{Read, Write};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

pub(crate) mod pty;
mod surface;
mod target;
mod title;
use surface::Surface;
pub use target::AgentTarget;
use target::{is_codex, same_process_scope};

unsafe extern "C" {
    fn brig_terminal_create(
        window: *mut c_void,
        identifier: u64,
        input: extern "C" fn(u64, *const u8, usize),
        resize: extern "C" fn(u64, u16, u16, u16, u16),
    ) -> *mut c_void;
    fn brig_terminal_layout(
        view: *mut c_void,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        visible: bool,
    );
    fn brig_terminal_feed(view: *mut c_void, bytes: *const u8, length: usize);
    fn brig_terminal_focus(view: *mut c_void);
    fn brig_terminal_show(view: *mut c_void, visible: bool);
    fn brig_terminal_destroy(view: *mut c_void);
}

struct Session {
    app: AppHandle,
    project_id: String,
    target: Option<AgentTarget>,
    purpose: &'static str,
    chat: Mutex<ChatState>,
    exit_code: Mutex<Option<i32>>,
    process: Arc<pty::Process>,
    view: Surface,
    input: mpsc::SyncSender<Vec<u8>>,
}

#[derive(Default)]
struct ChatState {
    conversation_id: Option<String>,
    prefix: Option<String>,
    activity: ChatActivity,
    identities: HashMap<String, Option<String>>,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum ChatActivity {
    #[default]
    Idle,
    Busy,
}

enum IdentityResolution {
    Unobserved,
    AwaitingMetadata,
    Ambiguous,
    Resolved(Option<String>),
}

impl ChatState {
    fn matches(&self, id: &str) -> bool {
        if let Some(known) = &self.conversation_id {
            return known == id;
        }
        self.prefix
            .as_ref()
            .is_some_and(|prefix| id.starts_with(prefix))
    }

    fn bind(&mut self, conversations: &[crate::conversations::Conversation]) -> bool {
        use crate::{conversations::graph::Status, conversations::lineage::Kind};
        let roots: HashSet<_> = conversations
            .iter()
            .filter(|conversation| {
                conversation.lineage.kind != Kind::Subagent
                    && conversation.relation_status == Status::Root
            })
            .map(|conversation| conversation.id.as_str())
            .collect();
        self.identities = conversations
            .iter()
            .map(|conversation| {
                let identity = match (&conversation.lineage.kind, &conversation.relation_status) {
                    (_, Status::Invalid) => None,
                    (Kind::Subagent, Status::Resolved) => conversation
                        .root_session_id
                        .clone()
                        .filter(|root| roots.contains(root.as_str())),
                    (Kind::Subagent, _) => None,
                    _ => Some(conversation.id.clone()),
                };
                (conversation.id.clone(), identity)
            })
            .collect();
        self.resolve_prefix()
    }

    fn resolve_prefix(&mut self) -> bool {
        let IdentityResolution::Resolved(identity) = self.identity_resolution() else {
            return false;
        };
        if identity == self.conversation_id {
            return false;
        }
        self.conversation_id = identity;
        true
    }

    fn identity_resolution(&self) -> IdentityResolution {
        let Some(prefix) = &self.prefix else {
            return IdentityResolution::Unobserved;
        };
        let mut matches = self
            .identities
            .iter()
            .filter(|(id, _)| id.starts_with(prefix));
        let Some((_, identity)) = matches.next() else {
            return IdentityResolution::AwaitingMetadata;
        };
        if matches.next().is_some() {
            return IdentityResolution::Ambiguous;
        }
        IdentityResolution::Resolved(identity.clone())
    }

    fn update_title(&mut self, title: title::ChatTitle) -> bool {
        let changed_prefix = title.prefix.is_some() && title.prefix != self.prefix;
        let activity = if title.busy { ChatActivity::Busy } else { ChatActivity::Idle };
        let changed = changed_prefix || self.activity != activity;
        if changed_prefix {
            self.prefix = title.prefix;
            // Retain an established root until trusted metadata resolves a new title.
            // A full child UUID is not an independent chat identity.
            self.resolve_prefix();
        }
        self.activity = activity;
        changed
    }

    fn matches_any(&self, ids: &[String]) -> bool {
        ids.iter().any(|id| {
            self.matches(id)
                || self
                    .prefix
                    .as_ref()
                    .is_some_and(|prefix| id.starts_with(prefix))
        })
    }
}

impl Session {
    fn status(&self, identifier: u64) -> TerminalStatus {
        let running = self.process.running();
        let chat = self.chat.lock().unwrap_or_else(|error| error.into_inner());
        TerminalStatus {
            identifier,
            project_id: self.project_id.clone(),
            environment_id: self
                .target
                .as_ref()
                .map(|target| target.environment_id.clone()),
            agent_kind: self.target.as_ref().map(|target| target.agent_kind),
            purpose: self.purpose,
            running,
            exit_code: self.exit_code.lock().ok().and_then(|code| *code),
            conversation_id: chat.conversation_id.clone(),
            busy: running && chat.activity == ChatActivity::Busy,
        }
    }

    fn update_title(&self, title: title::ChatTitle) {
        let mut chat = self.chat.lock().unwrap_or_else(|error| error.into_inner());
        if !chat.update_title(title) {
            return;
        }
        drop(chat);
        let _ = self.app.emit("terminal-sessions-changed", &self.project_id);
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalStatus {
    pub identifier: u64,
    pub project_id: String,
    pub environment_id: Option<projects::EnvironmentId>,
    pub agent_kind: Option<crate::agents::AgentKind>,
    pub purpose: &'static str,
    pub running: bool,
    pub exit_code: Option<i32>,
    pub conversation_id: Option<String>,
    pub busy: bool,
}

impl TerminalStatus {
    pub fn belongs_to(&self, project: &projects::Project) -> bool {
        self.project_id == project.id
            && self.environment_id.as_ref() == Some(&project.environment_id)
            && self.agent_kind == Some(project.agent_kind)
    }
}

pub fn is_chat(purpose: &str) -> bool {
    ["codex", "resume", "claude", "claude-resume"].contains(&purpose)
}

pub fn conversations_running(project_id: &str, conversation_ids: &[String]) -> bool {
    sessions().lock().is_ok_and(|states| {
        states.values().any(|session| {
            session.project_id == project_id
                && is_codex(session.target.as_ref())
                && session.process.running()
                && session
                    .chat
                    .lock()
                    .is_ok_and(|chat| chat.matches_any(conversation_ids))
        })
    })
}

pub fn bind_conversations(
    project_id: &str,
    conversations: &[crate::conversations::Conversation],
) -> bool {
    let Ok(states) = sessions().lock() else {
        return false;
    };
    let mut changed = false;
    for session in states.values().filter(|session| {
        session.project_id == project_id
            && is_codex(session.target.as_ref())
            && is_chat(session.purpose)
    }) {
        if let Ok(mut chat) = session.chat.lock() {
            changed |= chat.bind(conversations);
        }
    }
    changed
}

pub fn status(project_id: &str) -> Option<TerminalStatus> {
    status_for(project_id, None, None)
}

fn purpose_group(purpose: &str) -> &'static str {
    if [
        "local",
        "environment",
        "login",
        "codex",
        "resume",
        "claude",
        "claude-resume",
        "shell",
    ]
    .contains(&purpose)
    {
        "primary"
    } else {
        "auxiliary"
    }
}

fn status_for(
    project_id: &str,
    group: Option<&str>,
    target: Option<&AgentTarget>,
) -> Option<TerminalStatus> {
    let states = sessions().lock().ok()?;
    states
        .iter()
        .filter(|(_, session)| {
            session.project_id == project_id
                && same_process_scope(session.target.as_ref(), target)
                && group.is_none_or(|group| purpose_group(session.purpose) == group)
        })
        .max_by_key(|(identifier, session)| {
            (
                (session.process.running() as u8) * 2
                    + (purpose_group(session.purpose) == "primary") as u8,
                *identifier,
            )
        })
        .map(|(identifier, session)| session.status(*identifier))
}

#[tauri::command]
pub fn terminal_sessions(project_id: String) -> Vec<TerminalStatus> {
    let Ok(states) = sessions().lock() else {
        return Vec::new();
    };
    let mut statuses: Vec<_> = states
        .iter()
        .filter(|(_, session)| session.project_id == project_id)
        .map(|(identifier, session)| session.status(*identifier))
        .collect();
    statuses.sort_by_key(|status| status.identifier);
    statuses
}

#[tauri::command]
pub fn terminal_snapshot(identifier: u64) -> Option<TerminalStatus> {
    let session = session(identifier)?;
    Some(session.status(identifier))
}

pub fn running_conversation(project: &projects::Project, conversation_id: &str) -> Option<u64> {
    let target = AgentTarget::for_project(project);
    sessions()
        .lock()
        .ok()?
        .iter()
        .find_map(|(identifier, session)| {
            (session.project_id == project.id
                && session.target.as_ref() == Some(&target)
                && session.process.running()
                && session
                    .chat
                    .lock()
                    .is_ok_and(|chat| chat.matches(conversation_id)))
            .then_some(*identifier)
        })
}

static SESSIONS: OnceLock<Mutex<HashMap<u64, Arc<Session>>>> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
pub(crate) fn reserve_identifier() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::SeqCst)
}
fn sessions() -> &'static Mutex<HashMap<u64, Arc<Session>>> {
    SESSIONS.get_or_init(Mutex::default)
}

const LIVE_LIMIT: usize = 16;
const FINISHED_LIMIT: usize = 32;

#[derive(Debug, PartialEq)]
enum Capacity { Available, Full }

#[derive(Debug, PartialEq)]
struct Retention {
    capacity: Capacity,
    expired: Vec<u64>,
}

fn retention(statuses: &[(u64, bool)]) -> Retention {
    let can_start = statuses.iter().filter(|(_, running)| *running).count() < LIVE_LIMIT;
    let mut finished: Vec<_> = statuses
        .iter()
        .filter(|(_, running)| !running)
        .map(|(id, _)| *id)
        .collect();
    finished.sort_unstable();
    finished.truncate(finished.len().saturating_sub(FINISHED_LIMIT));
    let capacity = if can_start { Capacity::Available } else { Capacity::Full };
    Retention { capacity, expired: finished }
}

fn trim_finished(app: &AppHandle) {
    let _ = app.run_on_main_thread(move || {
        let Ok(mut states) = sessions().lock() else {
            return;
        };
        let statuses: Vec<_> = states
            .iter()
            .map(|(id, state)| (*id, state.process.running()))
            .collect();
        for id in retention(&statuses).expired {
            if let Some(state) = states.remove(&id) {
                let pointer = state.view.close();
                if pointer != 0 {
                    unsafe {
                        brig_terminal_destroy(pointer as *mut c_void);
                    }
                }
                let _ = state
                    .app
                    .emit("terminal-sessions-changed", &state.project_id);
            }
        }
    });
}
fn session(identifier: u64) -> Option<Arc<Session>> {
    sessions().lock().ok()?.get(&identifier).cloned()
}

extern "C" fn input(identifier: u64, bytes: *const u8, length: usize) {
    if bytes.is_null() || length > 1_048_576 {
        return;
    }
    if let Some(session) = session(identifier) {
        let bytes = unsafe { std::slice::from_raw_parts(bytes, length) };
        // Native callbacks must never block the AppKit thread.
        if session.input.try_send(bytes.to_vec()).is_err() {
            let _ = session.app.emit("terminal-input-error", serde_json::json!({ "identifier": identifier, "message": "The terminal input queue is full. Wait for the process and try again." }));
        }
    }
}

extern "C" fn resize(identifier: u64, columns: u16, rows: u16, width: u16, height: u16) {
    if let Some(session) = session(identifier) {
        session.process.resize(columns, rows, width, height);
    }
}

pub(crate) async fn on_main<T: Send + 'static>(
    app: AppHandle,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (send, receive) = mpsc::sync_channel(1);
        app.run_on_main_thread(move || {
            let _ = send.send(work());
        })
        .map_err(|error| error.to_string())?;
        receive.recv().map_err(|error| error.to_string())?
    })
    .await
    .map_err(|error| error.to_string())?
}

#[derive(Deserialize)]
pub struct Layout {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    visible: bool,
}

impl Layout {
    fn validate(&self) -> Result<(), String> {
        if [self.x, self.y, self.width, self.height]
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0 || *value > 32_768.0)
        {
            return Err("Invalid terminal size.".into());
        }
        Ok(())
    }
}

#[tauri::command]
pub async fn terminal_start(
    window: WebviewWindow,
    project_id: String,
    layout: Layout,
) -> Result<u64, String> {
    let project =
        projects::Registry::load(&projects::file(window.app_handle())?)?.project(&project_id)?;
    project.require_active()?;
    project.check_path()?;
    let mut command = Command::new("/bin/zsh");
    command
        .args(["-f", "-i"])
        .current_dir(&project.path)
        .env_clear()
        .envs(command::environment())
        .env("TERM", "xterm-256color")
        .env("PROMPT", "%1~ %# ");
    start(window, project_id, "local", command, layout, None).await
}

pub async fn start(
    window: WebviewWindow,
    project_id: String,
    purpose: &'static str,
    command: Command,
    layout: Layout,
    conversation_id: Option<String>,
) -> Result<u64, String> {
    let target = (purpose != "local").then(|| AgentTarget::codex(&project_id));
    start_reserved(
        window,
        project_id,
        purpose,
        command,
        layout,
        conversation_id,
        Reservation {
            identifier: reserve_identifier(),
            on_exit: None,
            target,
        },
    )
    .await
}

pub(crate) type ExitHandler = Box<dyn FnOnce(Option<i32>) + Send>;
pub(crate) struct Reservation {
    pub identifier: u64,
    pub on_exit: Option<ExitHandler>,
    pub target: Option<AgentTarget>,
}

pub(crate) async fn start_reserved(
    window: WebviewWindow,
    project_id: String,
    purpose: &'static str,
    mut command: Command,
    layout: Layout,
    conversation_id: Option<String>,
    reservation: Reservation,
) -> Result<u64, String> {
    let Reservation {
        identifier,
        on_exit,
        target,
    } = reservation;
    if crate::desktop::lifecycle::is_closing() {
        return Err("The app is closing.".into());
    }
    layout.validate()?;
    if let Some(previous) = status_for(&project_id, Some(purpose_group(purpose)), target.as_ref()) {
        if previous.running && !(is_chat(purpose) && is_chat(previous.purpose)) {
            if previous.purpose == purpose && on_exit.is_none() {
                return Ok(previous.identifier);
            }
            return Err("This project has an open terminal. End that process first.".into());
        }
    }
    let pty = Pty::open().map_err(|error| error.to_string())?;
    let mut writer = pty.master.try_clone().map_err(|error| error.to_string())?;
    let mut reader = pty.master.try_clone().map_err(|error| error.to_string())?;
    let (send, receive) = mpsc::sync_channel::<Vec<u8>>(256);
    let state = Arc::new(Session {
        app: window.app_handle().clone(),
        project_id: project_id.clone(),
        target: target.clone(),
        purpose,
        chat: Mutex::new(ChatState {
            conversation_id: conversation_id.clone(),
            ..Default::default()
        }),
        exit_code: Mutex::new(None),
        process: Arc::new(pty::Process::new(&pty).map_err(|error| error.to_string())?),
        view: Surface::default(),
        input: send,
    });
    {
        let mut states = sessions().lock().map_err(|error| error.to_string())?;
        if crate::desktop::lifecycle::is_closing() {
            return Err("The app is closing.".into());
        }
        if let Some((identifier, _)) = states.iter().find(|(_, session)| {
            session.project_id == project_id
                && session.target == target
                && session.process.running()
                && conversation_id.as_ref().is_some_and(|id| {
                    session
                        .chat
                        .lock()
                        .is_ok_and(|chat| chat.conversation_id.as_ref() == Some(id))
                })
        }) {
            return Ok(*identifier);
        }
        if states.values().any(|session| {
            session.project_id == project_id
                && same_process_scope(session.target.as_ref(), target.as_ref())
                && purpose_group(session.purpose) == purpose_group(purpose)
                && !(is_chat(session.purpose) && is_chat(purpose))
                && session.process.running()
        }) {
            return Err("This project terminal is already starting.".into());
        }
        let statuses: Vec<_> = states
            .iter()
            .map(|(id, state)| (*id, state.process.running()))
            .collect();
        if retention(&statuses).capacity == Capacity::Full {
            return Err(
                "There are 16 live terminal processes. End an unused process first.".into(),
            );
        }
        states.insert(identifier, state.clone());
    }
    let app = window.app_handle().clone();
    let creating = state.clone();
    let view = on_main(app.clone(), move || {
        let pointer = unsafe {
            brig_terminal_create(
                window.ns_window().map_err(|error| error.to_string())?,
                identifier,
                input,
                resize,
            )
        };
        if pointer.is_null() {
            return Err(
                "Could not start the terminal. Check libghostty and the bundled terminal font."
                    .into(),
            );
        }
        if let Err(error) = creating.view.attach(pointer as usize) {
            unsafe { brig_terminal_destroy(pointer); }
            return Err(error);
        }
        unsafe {
            brig_terminal_layout(
                pointer,
                layout.x,
                layout.y,
                layout.width,
                layout.height,
                false,
            );
        }
        Ok(())
    })
    .await;
    match view {
        Ok(()) => {},
        Err(error) => {
            state.process.stop(libc::SIGHUP);
            if let Ok(mut states) = sessions().lock() {
                states.remove(&identifier);
            }
            return Err(error);
        }
    }
    let child = state.process.spawn(&pty, &mut command);
    if let Err(error) = child {
        close_state(app, identifier, state).await?;
        return Err(error.to_string());
    }
    let mut child = child.unwrap();
    std::thread::spawn(move || {
        while let Ok(bytes) = receive.recv() {
            if writer.write_all(&bytes).is_err() {
                break;
            }
        }
    });
    drop(pty);
    let reader_app = app.clone();
    let reader_state = state.clone();
    std::thread::spawn(move || {
        let mut buffer = [0; 32_768];
        let mut titles = title::TitleParser::default();
        loop {
            let count = match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            };
            let bytes = buffer[..count].to_vec();
            if is_codex(reader_state.target.as_ref()) && is_chat(reader_state.purpose) {
                for title in titles.feed(&bytes) {
                    reader_state.update_title(title);
                }
            }
            let current = reader_state.clone();
            let (acknowledge, received) = mpsc::sync_channel(1);
            if reader_app
                .run_on_main_thread(move || {
                    let pointer = current.view.pointer();
                    if pointer != 0 {
                        unsafe {
                            brig_terminal_feed(pointer as *mut c_void, bytes.as_ptr(), bytes.len());
                        }
                    }
                    let _ = acknowledge.send(());
                })
                .is_err()
            {
                break;
            }
            if received.recv().is_err() {
                break;
            }
        }
    });
    let _ = app.emit("terminal-sessions-changed", &state.project_id);
    std::thread::spawn(move || {
        let status = child.wait();
        let code = status.ok().and_then(|status| status.code());
        state.process.child_exited();
        if let Ok(mut stored) = state.exit_code.lock() {
            *stored = code;
        }
        if let Some(on_exit) = on_exit {
            on_exit(code);
        }
        state.process.exited();
        trim_finished(&app);
        let _ = app.emit(
            "terminal-exit",
            serde_json::json!({ "identifier": identifier, "projectId": project_id, "environmentId":target.as_ref().map(|target| &target.environment_id), "agentKind":target.as_ref().map(|target| target.agent_kind), "purpose": purpose, "code": code }),
        );
    });
    Ok(identifier)
}

#[tauri::command]
pub async fn terminal_layout(
    app: AppHandle,
    identifier: u64,
    layout: Layout,
) -> Result<(), String> {
    layout.validate()?;
    on_main(app, move || {
        if layout.visible {
            let states = sessions().lock().map_err(|error| error.to_string())?;
            for (other_id, other) in states.iter() {
                let pointer = other.view.pointer();
                if *other_id != identifier && pointer != 0 {
                    unsafe {
                        brig_terminal_show(pointer as *mut c_void, false);
                    }
                }
            }
        }
        let state = session(identifier).ok_or("Terminal closed.")?;
        let pointer = state.view.pointer();
        if pointer != 0 {
            unsafe {
                brig_terminal_layout(
                    pointer as *mut c_void,
                    layout.x,
                    layout.y,
                    layout.width,
                    layout.height,
                    layout.visible,
                );
            }
        }
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn terminal_focus(window: WebviewWindow, identifier: u64) -> Result<(), String> {
    on_main(window.app_handle().clone(), move || {
        let state = session(identifier).ok_or("Terminal closed.")?;
        let pointer = state.view.pointer();
        if pointer != 0 {
            unsafe {
                brig_terminal_focus(pointer as *mut c_void);
            }
        }
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn terminal_close(app: AppHandle, identifier: u64) -> Result<(), String> {
    let Some(state) = session(identifier) else {
        return Ok(());
    };
    close_state(app, identifier, state).await
}

async fn close_state(app: AppHandle, identifier: u64, state: Arc<Session>) -> Result<(), String> {
    state.process.stop(libc::SIGHUP);
    let closing = state.clone();
    on_main(app, move || {
        let pointer = closing.view.close();
        if pointer != 0 {
            unsafe {
                brig_terminal_destroy(pointer as *mut c_void);
            }
        }
        Ok(())
    })
    .await?;
    tauri::async_runtime::spawn_blocking(move || {
        pty::terminate(
            std::slice::from_ref(&state.process),
            std::time::Duration::from_millis(500),
        )?;
        sessions()
            .lock()
            .map_err(|error| error.to_string())?
            .remove(&identifier);
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

pub fn shutdown() -> Result<(), String> {
    let processes: Vec<_> = sessions()
        .lock()
        .map_err(|error| error.to_string())?
        .values()
        .map(|state| state.process.clone())
        .collect();
    pty::terminate(&processes, std::time::Duration::from_secs(1))
}

pub fn any_running() -> bool {
    sessions()
        .lock()
        .is_ok_and(|sessions| sessions.values().any(|session| session.process.running()))
}

pub fn chatgpt_in_use(skip_finished_login: bool) -> bool {
    sessions().lock().is_ok_and(|states| {
        states.values().any(|session| {
            session.process.running()
                && is_codex(session.target.as_ref())
                && matches!(session.purpose, "codex" | "resume" | "login" | "shell")
                && !(skip_finished_login
                    && session.purpose == "login"
                    && session.exit_code.lock().is_ok_and(|code| code.is_some()))
        })
    })
}

pub fn claude_in_use() -> bool {
    sessions().lock().is_ok_and(|states| states.values().any(|session| {
        session.process.running() && matches!(session.purpose, "claude" | "claude-resume" | "login" | "shell") && session.target.as_ref()
            .is_some_and(|target| target.agent_kind == crate::agents::AgentKind::ClaudeCode)
    }))
}

fn runs_in_guest(purpose: &str) -> bool {
    matches!(
        purpose,
        "environment"
            | "login"
            | "codex"
            | "resume"
            | "claude"
            | "claude-resume"
            | "shell"
            | "git-status"
            | "git-diff"
            | "git-staged-diff"
            | "tests"
    )
}

pub fn stop_environment_processes(project: &projects::Project) -> Result<(), String> {
    let processes: Vec<_> = sessions()
        .lock()
        .map_err(|error| error.to_string())?
        .values()
        .filter(|session| {
            let status = session.status(0);
            status.belongs_to(project) && runs_in_guest(status.purpose) && status.running
        })
        .map(|session| session.process.clone())
        .collect();
    // Keep the native view and its scrollback after the selected processes exit.
    pty::terminate(&processes, std::time::Duration::from_millis(500))
}

pub async fn close_environment(app: AppHandle, project: &projects::Project) -> Result<(), String> {
    let identifiers: Vec<_> = sessions()
        .lock()
        .map_err(|error| error.to_string())?
        .iter()
        .filter(|(identifier, session)| session.status(**identifier).belongs_to(project))
        .map(|(identifier, _)| *identifier)
        .collect();
    for identifier in identifiers {
        terminal_close(app.clone(), identifier).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_outputs_do_not_consume_live_capacity_and_retention_never_evicts_running_processes()
    {
        let mut states: Vec<_> = (1..=16).map(|id| (id, false)).collect();
        assert_eq!(retention(&states), Retention { capacity: Capacity::Available, expired: Vec::new() });
        states.extend((17..=32).map(|id| (id, true)));
        assert_eq!(retention(&states), Retention { capacity: Capacity::Full, expired: Vec::new() });
        states.extend((33..=50).map(|id| (id, false)));
        assert_eq!(retention(&states), Retention { capacity: Capacity::Full, expired: vec![1, 2] });
        states.retain(|(id, _)| *id != 17);
        assert_eq!(retention(&states), Retention { capacity: Capacity::Available, expired: vec![1, 2] });
    }

    fn conversation(id: &str) -> crate::conversations::Conversation {
        serde_json::from_value(
            serde_json::json!({"id": id, "title": "Fixture chat", "updatedAt": 0}),
        )
        .unwrap()
    }

    #[test]
    fn shortened_titles_bind_only_to_one_matching_conversation_and_keep_known_ids() {
        let first = "01a1c5fd-894f-7e42-b36c-bd1a0dfef8e7";
        let second = "01a1c5fd-894f-7e42-b36c-bd1a0dfef8e8";
        let mut chat = ChatState {
            prefix: Some(first[..31].into()),
            ..Default::default()
        };
        assert!(!chat.bind(&[conversation(second), conversation(first)]));
        assert!(chat.conversation_id.is_none());
        assert!(chat.bind(&[conversation(first)]));
        assert_eq!(chat.conversation_id.as_deref(), Some(first));
        assert!(chat.matches(first));
        assert!(!chat.matches(second));
        assert!(!chat.bind(&[]));
        assert_eq!(chat.conversation_id.as_deref(), Some(first));
    }

    fn primary(id: &str) -> crate::conversations::Conversation {
        let mut result = conversation(id);
        result.lineage.kind = crate::conversations::lineage::Kind::Primary;
        result.lineage.source_version = Some("0.167.0".into());
        result.relation_status = crate::conversations::graph::Status::Root;
        result.root_session_id = Some(id.into());
        result
    }

    fn child(id: &str, root: Option<&str>) -> crate::conversations::Conversation {
        let mut result = conversation(id);
        result.lineage.kind = crate::conversations::lineage::Kind::Subagent;
        result.lineage.source_version = Some("0.167.0".into());
        result.relation_status = if root.is_some() {
            crate::conversations::graph::Status::Resolved
        } else {
            crate::conversations::graph::Status::Orphan
        };
        result.root_session_id = root.map(str::to_owned);
        result
    }

    #[test]
    fn child_titles_keep_root_identity_busy_state_and_duplicate_resume_match() {
        let root = "10000000-0000-4000-8000-000000000001";
        let agent = "20000000-0000-4000-8000-000000000001";
        let conversations = [primary(root), child(agent, Some(root))];
        let mut chat = ChatState {
            conversation_id: Some(root.into()),
            ..Default::default()
        };
        // A root reservation exists before its process spawns or the monitor binds metadata.
        chat.update_title(title::ChatTitle {
            prefix: Some(agent.into()),
            busy: true,
        });
        assert_eq!(chat.conversation_id.as_deref(), Some(root));
        assert!(chat.matches(root));
        assert!(!chat.matches(agent));
        assert!(chat.matches_any(&[agent.into()]));
        assert!(chat.activity == ChatActivity::Busy);
        assert!(!chat.bind(&conversations));
        assert!(chat.update_title(title::ChatTitle {
            prefix: Some(agent[..25].into()),
            busy: false
        }));
        assert_eq!(chat.conversation_id.as_deref(), Some(root));
        assert!(chat.activity == ChatActivity::Idle);
        assert!(chat.matches_any(&[root.into(), agent.into()]));
        let mut observed = ChatState {
            prefix: Some(agent.into()),
            ..Default::default()
        };
        assert!(!observed.matches(root));
        assert!(observed.matches_any(&[root.into(), agent.into()]));
        assert!(observed.bind(&conversations));
        assert_eq!(observed.conversation_id.as_deref(), Some(root));
        observed.update_title(title::ChatTitle {
            prefix: Some(agent[..25].into()),
            busy: true,
        });
        assert_eq!(observed.conversation_id.as_deref(), Some(root));
    }

    #[test]
    fn orphan_unsupported_and_invalid_child_titles_never_bind_to_a_primary_name() {
        let root = "10000000-0000-4000-8000-000000000001";
        let agent = "20000000-0000-4000-8000-000000000001";
        for status in [
            crate::conversations::graph::Status::Orphan,
            crate::conversations::graph::Status::Unknown,
            crate::conversations::graph::Status::Invalid,
        ] {
            let mut orphan = child(agent, None);
            orphan.relation_status = status;
            let mut chat = ChatState {
                conversation_id: Some(root.into()),
                ..Default::default()
            };
            chat.update_title(title::ChatTitle {
                prefix: Some(agent.into()),
                busy: true,
            });
            assert!(chat.bind(&[primary(root), orphan]));
            assert!(chat.conversation_id.is_none());
            assert!(chat.activity == ChatActivity::Busy);
            assert!(!chat.matches(root));
            assert!(chat.matches(agent));
            assert!(!chat.bind(&[primary(root)]));
            assert!(chat.conversation_id.is_none());
        }
    }

    #[test]
    fn ambiguous_child_prefix_cannot_select_a_root_but_blocks_family_archival() {
        let first = "20000000-0000-4000-8000-000000000001";
        let second = "20000000-0000-4000-8000-000000000002";
        let root = "10000000-0000-4000-8000-000000000001";
        let mut chat = ChatState {
            prefix: Some(first[..25].into()),
            ..Default::default()
        };
        assert!(!chat.bind(&[
            primary(root),
            child(first, Some(root)),
            child(second, Some(root))
        ]));
        assert!(chat.conversation_id.is_none());
        assert!(chat.matches_any(&[root.into(), first.into(), second.into()]));
        assert!(!chat.matches(root));
    }
}
