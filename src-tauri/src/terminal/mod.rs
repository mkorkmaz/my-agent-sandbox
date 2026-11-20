use crate::{platform::command, projects};
use pty::Pty;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::io::{Read, Write};
use std::ops::AsyncFnOnce;
use std::path::Path;
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
use target::{is_codex, same_environment_scope};
#[cfg(test)]
pub(crate) use title::TitleParser;

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
        overlay_x: f64,
        overlay_y: f64,
        overlay_width: f64,
        overlay_height: f64,
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
        let activity = if title.busy {
            ChatActivity::Busy
        } else {
            ChatActivity::Idle
        };
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
    pub fn belongs_to_environment(
        &self,
        project_id: &str,
        environment_id: &projects::EnvironmentId,
    ) -> bool {
        self.project_id == project_id && self.environment_id.as_ref() == Some(environment_id)
    }

    pub fn belongs_to(&self, project: &projects::Project) -> bool {
        self.belongs_to_environment(&project.id, &project.environment_id)
            && self.agent_kind == Some(project.agent_kind)
    }

    fn stops_with_environment(
        &self,
        project_id: &str,
        environment_id: &projects::EnvironmentId,
    ) -> bool {
        self.running
            && self.belongs_to_environment(project_id, environment_id)
            && runs_in_guest(self.purpose)
    }
}

pub fn is_chat(purpose: &str) -> bool {
    ["codex", "resume", "claude", "claude-resume"].contains(&purpose)
}

pub fn conversations_running(project: &projects::Project, conversation_ids: &[String]) -> bool {
    let target = AgentTarget::for_project(project);
    sessions().lock().is_ok_and(|states| {
        states.values().any(|session| {
            session.project_id == project.id
                && session.target.as_ref() == Some(&target)
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
                && same_environment_scope(session.target.as_ref(), target)
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
    #[cfg(test)]
    if let Some(statuses) = fixture_terminal_statuses(&project_id) {
        return statuses;
    }
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

#[cfg(test)]
thread_local! {
    static FIXTURE_STATUSES: std::cell::RefCell<Option<Vec<TerminalStatus>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn fixture_terminal_statuses(project_id: &str) -> Option<Vec<TerminalStatus>> {
    FIXTURE_STATUSES.with(|statuses| {
        statuses.borrow().as_ref().map(|statuses| {
            statuses
                .iter()
                .filter(|status| status.project_id == project_id)
                .cloned()
                .collect()
        })
    })
}

#[cfg(test)]
pub(crate) fn with_fixture_terminal_statuses<ResultValue>(
    statuses: Vec<TerminalStatus>,
    run: impl FnOnce() -> ResultValue,
) -> ResultValue {
    struct Restore(Option<Vec<TerminalStatus>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            FIXTURE_STATUSES.with(|statuses| statuses.replace(self.0.take()));
        }
    }
    let _restore = Restore(FIXTURE_STATUSES.with(|fixture| fixture.replace(Some(statuses))));
    run()
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
enum Capacity {
    Available,
    Full,
}

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
    let capacity = if can_start {
        Capacity::Available
    } else {
        Capacity::Full
    };
    Retention {
        capacity,
        expired: finished,
    }
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

pub(crate) async fn copy_text(app: AppHandle, text: String) -> Result<(), String> {
    on_main(app, move || {
        unsafe { brig_terminal_copy_text(text.as_ptr(), text.len()) };
        Ok(())
    })
    .await
}

unsafe extern "C" {
    fn brig_terminal_copy_text(bytes: *const u8, length: usize);
}

#[derive(Deserialize)]
pub struct Layout {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    visible: bool,
    #[serde(default)]
    overlay: Option<OverlayRectangle>,
}

#[derive(Clone, Copy, Default, Deserialize)]
struct OverlayRectangle {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl OverlayRectangle {
    fn validate(&self) -> Result<(), String> {
        if [self.x, self.y, self.width, self.height]
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0 || *value > 32_768.0)
        {
            return Err("Invalid terminal overlay.".into());
        }
        Ok(())
    }
}

impl Layout {
    fn validate(&self) -> Result<(), String> {
        if [self.x, self.y, self.width, self.height]
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0 || *value > 32_768.0)
        {
            return Err("Invalid terminal size.".into());
        }
        if let Some(overlay) = self.overlay {
            overlay.validate()?;
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
    let file = projects::file(window.app_handle())?;
    start_local_terminal(&file, project_id, async move |project_id, command| {
        start(window, project_id, "local", command, layout, None).await
    })
    .await
}

async fn start_local_terminal(
    registry_file: &Path,
    project_id: String,
    launch: impl AsyncFnOnce(String, Command) -> Result<u64, String>,
) -> Result<u64, String> {
    let environment_id = projects::EnvironmentId::for_project(&project_id);
    let _operation = crate::environment::EnvironmentOperation::begin(&project_id, &environment_id)?;
    let project =
        projects::Registry::load(registry_file)?.environment(&project_id, &environment_id)?;
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
    launch(project_id, command).await
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

pub(crate) type ExitHandler = Box<dyn FnOnce(Option<i32>, bool) + Send>;
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
            if previous.purpose == purpose
                && previous.agent_kind == target.as_ref().map(|target| target.agent_kind)
                && on_exit.is_none()
            {
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
                && same_environment_scope(session.target.as_ref(), target.as_ref())
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
            unsafe {
                brig_terminal_destroy(pointer);
            }
            return Err(error);
        }
        let overlay = layout.overlay.unwrap_or_default();
        unsafe {
            brig_terminal_layout(
                pointer,
                layout.x,
                layout.y,
                layout.width,
                layout.height,
                false,
                overlay.x,
                overlay.y,
                overlay.width,
                overlay.height,
            );
        }
        Ok(())
    })
    .await;
    match view {
        Ok(()) => {}
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
        let cancelled = state.process.child_exited();
        if let Ok(mut stored) = state.exit_code.lock() {
            *stored = code;
        }
        if let Some(on_exit) = on_exit {
            on_exit(code, cancelled);
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
            let overlay = layout.overlay.unwrap_or_default();
            unsafe {
                brig_terminal_layout(
                    pointer as *mut c_void,
                    layout.x,
                    layout.y,
                    layout.width,
                    layout.height,
                    layout.visible,
                    overlay.x,
                    overlay.y,
                    overlay.width,
                    overlay.height,
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
pub async fn cancel_environment_preparation(
    project_id: String,
    environment_id: projects::EnvironmentId,
    identifier: u64,
) -> Result<bool, String> {
    let Some(state) = session(identifier) else {
        return Ok(false);
    };
    if state.purpose != "environment"
        || !state
            .status(identifier)
            .belongs_to_environment(&project_id, &environment_id)
    {
        return Err("This terminal belongs to a different project operation.".into());
    }
    if !state.process.cancel_startup() {
        return Ok(false);
    }
    tauri::async_runtime::spawn_blocking(move || {
        pty::terminate(
            std::slice::from_ref(&state.process),
            std::time::Duration::from_millis(500),
        )
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(true)
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
    sessions().lock().is_ok_and(|states| {
        states.values().any(|session| {
            session.process.running()
                && matches!(
                    session.purpose,
                    "claude" | "claude-resume" | "login" | "shell"
                )
                && session
                    .target
                    .as_ref()
                    .is_some_and(|target| target.agent_kind == crate::agents::AgentKind::ClaudeCode)
        })
    })
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
            session
                .status(0)
                .stops_with_environment(&project.id, &project.environment_id)
        })
        .map(|session| session.process.clone())
        .collect();
    // Keep the native view and its scrollback after the selected processes exit.
    pty::terminate(&processes, std::time::Duration::from_millis(500))
}

fn environment_terminals(project: &projects::Project) -> Result<Vec<TerminalStatus>, String> {
    let mut statuses: Vec<_> = sessions()
        .lock()
        .map_err(|error| error.to_string())?
        .iter()
        .map(|(identifier, session)| session.status(*identifier))
        .filter(|status| status.belongs_to_environment(&project.id, &project.environment_id))
        .collect();
    statuses.sort_by_key(|status| status.identifier);
    Ok(statuses)
}

pub(crate) fn removal_terminal_summary(project: &projects::Project) -> Result<String, String> {
    let statuses = environment_terminals(project)?;
    if statuses.is_empty() {
        return Ok(String::new());
    }
    let names: Vec<_> = statuses
        .iter()
        .map(|status| {
            let purpose = match status.purpose {
                "codex" | "resume" => "Codex chat",
                "claude" | "claude-resume" => "Claude Code chat",
                "shell" => "Guest terminal",
                "environment" => "Environment startup",
                "git-status" => "Git status",
                "git-diff" => "Working tree diff",
                "git-staged-diff" => "Staged diff",
                "tests" => "Project tests",
                "doctor" => "Brig doctor",
                "info" => "Session info",
                "policy-check" => "Policy bindings",
                "logs" => "Guest logs",
                _ => "Operation",
            };
            format!("{purpose} · Terminal {}", status.identifier)
        })
        .collect();
    Ok(format!("\n\nTerminals to close\n{}", names.join("\n")))
}

pub async fn close_environment(app: AppHandle, project: &projects::Project) -> Result<(), String> {
    for status in environment_terminals(project)? {
        terminal_close(app.clone(), status.identifier).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_launch_and_environment_update_exclude_each_other_in_both_orders() {
        use crate::{environment::EnvironmentOperation, projects::Registry, storage};
        use std::{
            fs,
            future::Future,
            os::unix::fs::DirBuilderExt,
            pin::pin,
            task::{Context, Poll, Waker},
        };

        let root = std::env::temp_dir().join(format!(
            "tugboat-terminal-admission-{}-{}",
            std::process::id(),
            reserve_identifier()
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let first = root.join("first");
        let second = root.join("second");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        let file = root.join("projects.json");
        let mut registry = Registry::default();
        let project = registry.register(&first).unwrap();
        let sibling = registry.register(&second).unwrap();
        storage::write(&file, &registry).unwrap();
        let mut context = Context::from_waker(Waker::noop());

        {
            let mut starting = pin!(start_local_terminal(
                &file,
                project.id.clone(),
                async |_, command| {
                    assert_eq!(command.get_current_dir(), Some(first.as_path()));
                    std::future::pending().await
                }
            ));
            assert!(starting.as_mut().poll(&mut context).is_pending());
            assert!(EnvironmentOperation::begin(&project.id, &project.environment_id).is_err());
            assert!(EnvironmentOperation::begin(&sibling.id, &sibling.environment_id).is_ok());
        }

        let update = EnvironmentOperation::begin(&project.id, &project.environment_id).unwrap();
        let mut blocked = pin!(start_local_terminal(
            &file,
            project.id.clone(),
            async |_, _| { panic!("The current project operation must block local launch.") }
        ));
        assert!(matches!(
            blocked.as_mut().poll(&mut context),
            Poll::Ready(Err(error)) if error.contains("already in progress")
        ));
        let mut independent = pin!(start_local_terminal(
            &file,
            sibling.id.clone(),
            async |_, command| {
                assert_eq!(command.get_current_dir(), Some(second.as_path()));
                Ok(41)
            }
        ));
        assert_eq!(independent.as_mut().poll(&mut context), Poll::Ready(Ok(41)));
        drop(update);
        let mut retry = pin!(start_local_terminal(
            &file,
            project.id.clone(),
            async |_, _| { Ok(42) }
        ));
        assert_eq!(retry.as_mut().poll(&mut context), Poll::Ready(Ok(42)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stopping_a_guest_selects_both_agents_and_leaves_other_terminals_running() {
        use crate::{agents::AgentKind, projects::EnvironmentId};

        let project = "project-a";
        let shared = EnvironmentId::for_project(project);
        let sibling = EnvironmentId::for_agent(project, AgentKind::ClaudeCode);
        let terminal = |identifier,
                        project_id: &str,
                        environment_id: Option<&EnvironmentId>,
                        agent_kind,
                        purpose,
                        running| {
            TerminalStatus {
                identifier,
                project_id: project_id.into(),
                environment_id: environment_id.cloned(),
                agent_kind,
                purpose,
                running,
                exit_code: None,
                conversation_id: Some("same-native-id".into()),
                busy: false,
            }
        };
        let codex = Some(AgentKind::Codex);
        let claude = Some(AgentKind::ClaudeCode);
        let terminals = [
            terminal(1, project, Some(&shared), codex, "codex", true),
            terminal(2, project, Some(&shared), claude, "claude", true),
            terminal(3, project, Some(&sibling), claude, "claude", true),
            terminal(4, "project-b", Some(&shared), codex, "codex", true),
            terminal(5, project, None, None, "local", true),
            terminal(6, project, Some(&shared), codex, "doctor", true),
            terminal(7, project, Some(&shared), claude, "claude", false),
        ];
        let stopped: Vec<_> = terminals
            .iter()
            .filter(|terminal| terminal.stops_with_environment(project, &shared))
            .map(|terminal| terminal.identifier)
            .collect();
        let closed: Vec<_> = terminals
            .iter()
            .filter(|terminal| terminal.belongs_to_environment(project, &shared))
            .map(|terminal| terminal.identifier)
            .collect();
        assert_eq!(stopped, [1, 2]);
        assert_eq!(closed, [1, 2, 6, 7]);
    }

    #[test]
    fn terminal_layout_accepts_existing_requests_and_optional_overlay_rectangles() {
        let mut layout: Layout = serde_json::from_value(serde_json::json!({
            "x": 0, "y": 0, "width": 800, "height": 600, "visible": true
        }))
        .unwrap();
        assert!(layout.overlay.is_none());
        assert!(layout.validate().is_ok());
        layout.overlay = Some(OverlayRectangle {
            x: 600.0,
            y: 400.0,
            width: 400.0,
            height: 300.0,
        });
        assert!(layout.validate().is_ok());
        layout.overlay = Some(OverlayRectangle::default());
        assert!(layout.validate().is_ok());
        assert!(serde_json::from_value::<Layout>(serde_json::json!({
            "x": 0, "y": 0, "width": 800, "height": 600, "visible": true,
            "overlay": {"x": 0, "y": 0, "width": 200}
        }))
        .is_err());
    }

    #[test]
    fn terminal_overlay_rejects_nonfinite_negative_and_oversized_coordinates() {
        let rectangle = OverlayRectangle {
            x: 10.0,
            y: 20.0,
            width: 200.0,
            height: 100.0,
        };
        for overlay in [
            OverlayRectangle {
                x: f64::NAN,
                ..rectangle
            },
            OverlayRectangle {
                y: -1.0,
                ..rectangle
            },
            OverlayRectangle {
                width: f64::INFINITY,
                ..rectangle
            },
            OverlayRectangle {
                height: 32_769.0,
                ..rectangle
            },
        ] {
            let layout = Layout {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0,
                visible: true,
                overlay: Some(overlay),
            };
            assert_eq!(layout.validate().unwrap_err(), "Invalid terminal overlay.");
        }
    }

    #[test]
    fn completed_outputs_do_not_consume_live_capacity_and_retention_never_evicts_running_processes()
    {
        let mut states: Vec<_> = (1..=16).map(|id| (id, false)).collect();
        assert_eq!(
            retention(&states),
            Retention {
                capacity: Capacity::Available,
                expired: Vec::new()
            }
        );
        states.extend((17..=32).map(|id| (id, true)));
        assert_eq!(
            retention(&states),
            Retention {
                capacity: Capacity::Full,
                expired: Vec::new()
            }
        );
        states.extend((33..=50).map(|id| (id, false)));
        assert_eq!(
            retention(&states),
            Retention {
                capacity: Capacity::Full,
                expired: vec![1, 2]
            }
        );
        states.retain(|(id, _)| *id != 17);
        assert_eq!(
            retention(&states),
            Retention {
                capacity: Capacity::Available,
                expired: vec![1, 2]
            }
        );
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
