use crate::projects;
use crate::terminal::{is_chat, TerminalStatus};
use serde::Serialize;
use std::sync::{Arc, Mutex, MutexGuard};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_updater::Update;

pub(super) struct Updates(pub Mutex<State>);

pub(super) struct State {
    pub phase: Phase,
    pub offer: Option<Offer>,
    pub revision: u64,
    pub sessions: Vec<TerminalStatus>,
    pub session_revision: u64,
}

pub(super) enum Phase {
    Disabled,
    Idle,
    Checking,
    Current,
    Available,
    Downloading(usize),
    Ready(Arc<Vec<u8>>, String),
    ShuttingDown,
    Failed {
        message: String,
        installer: bool,
    },
    Recovery {
        message: String,
        version: Option<String>,
        can_reopen: bool,
    },
}

#[derive(Clone)]
pub(super) struct Offer {
    pub id: u64,
    pub update: Arc<Update>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    revision: u64,
    installed_version: String,
    phase: &'static str,
    offer: Option<OfferStatus>,
    progress: Option<Progress>,
    message: Option<String>,
    automatic_offer: bool,
    installer_url: Option<&'static str>,
    can_reopen: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OfferStatus {
    id: u64,
    version: String,
    notes: String,
    session_revision: u64,
    sessions: Vec<AffectedSession>,
}

#[derive(Clone, Serialize)]
struct AffectedSession {
    kind: &'static str,
    label: String,
}

#[derive(Clone, Serialize)]
struct Progress {
    downloaded: usize,
}

impl State {
    pub fn new(enabled: bool) -> Self {
        Self {
            phase: if enabled {
                Phase::Idle
            } else {
                Phase::Disabled
            },
            offer: None,
            revision: 0,
            sessions: Vec::new(),
            session_revision: 0,
        }
    }

    pub fn lock<R: Runtime>(app: &AppHandle<R>) -> MutexGuard<'_, Self> {
        app.state::<Updates>()
            .inner()
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    pub fn refresh_sessions(&mut self, active: Vec<TerminalStatus>) {
        if self
            .sessions
            .iter()
            .map(|session| session.identifier)
            .ne(active.iter().map(|session| session.identifier))
        {
            self.session_revision += 1;
        }
        self.sessions = active;
    }

    pub fn has_offer(&self, id: u64) -> bool {
        self.offer.as_ref().is_some_and(|offer| offer.id == id)
    }

    pub fn publish<R: Runtime>(&mut self, app: &AppHandle<R>, automatic: bool) -> Status {
        self.revision += 1;
        let snapshot = self.snapshot(app, automatic);
        let _ = app.emit("app-update", &snapshot);
        snapshot
    }

    pub fn snapshot<R: Runtime>(&self, app: &AppHandle<R>, automatic_offer: bool) -> Status {
        let (phase, message, progress, installer, version) = match &self.phase {
            Phase::Disabled => ("disabled", None, None, false, None),
            Phase::Idle => ("idle", None, None, false, None),
            Phase::Checking => ("checking", None, None, false, None),
            Phase::Current => ("current", None, None, false, None),
            Phase::Available => ("available", None, None, false, None),
            Phase::Downloading(bytes) => (
                "downloading",
                None,
                Some(Progress { downloaded: *bytes }),
                false,
                None,
            ),
            Phase::Ready(_, message) => ("ready", Some(message.clone()), None, false, None),
            Phase::ShuttingDown => ("shuttingDown", None, None, false, None),
            Phase::Failed { message, installer } => {
                ("failed", Some(message.clone()), None, *installer, None)
            }
            Phase::Recovery {
                message, version, ..
            } => (
                "recovery",
                Some(message.clone()),
                None,
                true,
                version.clone(),
            ),
        };
        let can_reopen = matches!(
            self.phase,
            Phase::Recovery {
                can_reopen: true,
                ..
            }
        );
        let projects =
            if !self.sessions.is_empty() && matches!(phase, "available" | "ready" | "failed") {
                projects::file(app)
                    .and_then(|file| projects::Registry::load(&file))
                    .ok()
            } else {
                None
            };
        let installed_version = if phase == "recovery" {
            version.unwrap_or_else(|| "Unknown".into())
        } else {
            app.package_info().version.to_string()
        };
        Status {
            revision: self.revision,
            installed_version,
            phase,
            offer: self.offer.as_ref().map(|offer| OfferStatus {
                id: offer.id,
                version: offer.update.version.clone(),
                notes: offer.update.body.clone().unwrap_or_default(),
                session_revision: self.session_revision,
                sessions: self
                    .sessions
                    .iter()
                    .map(|session| affected_session(session, projects.as_ref()))
                    .collect(),
            }),
            progress,
            message,
            automatic_offer,
            installer_url: installer.then_some(super::INSTALLER),
            can_reopen,
        }
    }
}

fn affected_session(
    session: &TerminalStatus,
    projects: Option<&projects::Registry>,
) -> AffectedSession {
    let name = match session.purpose {
        "codex" | "resume" => "Codex chat",
        "claude" | "claude-resume" => "Claude Code chat",
        "local" => "Local terminal",
        "shell" => "Guest terminal",
        "login" => "Sign-in terminal",
        "environment" => "Environment startup",
        _ => "Project operation",
    };
    let project = projects.and_then(|registry| registry.project(&session.project_id).ok());
    let label = match project {
        Some(project) => format!("{} · {name} {}", project.name, session.identifier),
        None => format!("{name} {}", session.identifier),
    };
    AffectedSession {
        kind: if is_chat(session.purpose) {
            "chat"
        } else {
            "terminal"
        },
        label,
    }
}
