mod account;
mod claude;
pub(crate) mod codex;
mod launch;
mod resume;

use crate::{
    agents::{AgentKind, SessionId},
    environment,
    projects::{self, EnvironmentId, Project, Registry},
    terminal::{self, Layout},
};
pub use account::{Accounts, State, Status};
use serde::Deserialize;
use std::process::Command;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Login,
    NewChat,
    Resume,
    Shell,
}

impl Action {
    fn purpose(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::NewChat => "claude",
            Self::Resume => "claude-resume",
            Self::Shell => "shell",
        }
    }

    fn codex(self) -> codex::Action {
        match self {
            Self::Login => codex::Action::Login,
            Self::NewChat => codex::Action::Codex,
            Self::Resume => codex::Action::Resume,
            Self::Shell => codex::Action::Shell,
        }
    }

    fn session(self, supplied: Option<&str>) -> Result<Option<SessionId>, String> {
        if self == Self::Resume {
            return Ok(Some(SessionId::new(
                supplied.ok_or("Choose a saved main chat to resume.")?,
            )?));
        }
        if supplied.is_some() {
            return Err("This operation does not accept a saved session ID.".into());
        }
        if self == Self::NewChat {
            return Ok(Some(SessionId::generate()?));
        }
        Ok(None)
    }
}

fn selected(
    app: &AppHandle,
    identifier: &str,
    environment_id: Option<&EnvironmentId>,
) -> Result<Project, String> {
    Registry::load(&projects::file(app)?)?.resolve_environment(identifier, environment_id)
}

fn require_claude(project: &Project) -> Result<(), String> {
    if project.agent_kind != AgentKind::ClaudeCode {
        return Err("Use the shared ChatGPT controls for Codex accounts.".into());
    }
    Ok(())
}

fn announce(app: &AppHandle, status: &Status) {
    if app.emit("agent-account-changed", status).is_err() {
        eprintln!("Could not deliver a Claude account update.");
    }
}

pub fn invalidate(app: &AppHandle, project: &Project) -> Result<(), String> {
    if project.agent_kind != AgentKind::ClaudeCode {
        return Ok(());
    }
    let status = app.state::<Accounts>().invalidate(project)?;
    announce(app, &status);
    Ok(())
}

enum Launch {
    Existing(u64),
    Prepared {
        command: Command,
        project: Project,
        conversation: Option<String>,
    },
}

#[tauri::command]
pub async fn start_agent_session(
    window: WebviewWindow,
    identifier: String,
    environment_id: Option<EnvironmentId>,
    fingerprint: String,
    action: Action,
    use_brig_boundary: Option<bool>,
    session_id: Option<String>,
    layout: Layout,
) -> Result<u64, String> {
    if action == Action::Login {
        return Err("Connect the provider in Settings, Providers. Sign-in does not require a project.".into());
    }
    let app = window.app_handle().clone();
    let project = selected(&app, &identifier, environment_id.as_ref())?;
    if project.agent_kind == AgentKind::Codex {
        return codex::start_codex_session(
            window,
            identifier,
            fingerprint,
            action.codex(),
            use_brig_boundary,
            session_id,
            layout,
        )
        .await;
    }
    let operation = app.state::<Accounts>().operation(&project)?;
    let _guard = operation.lock_owned().await;
    crate::providers::provider_login::require_idle(
        &app,
        crate::providers::provider_login::Provider::Claude,
    )?;
    let mut account = app
        .state::<crate::providers::shared_claude::SharedClaude>()
        .0
        .clone()
        .lock_owned()
        .await;
    let terminal_identifier = terminal::reserve_identifier();
    let prepare_app = app.clone();
    let prepared = tauri::async_runtime::spawn_blocking(move || {
        let session = action.session(session_id.as_deref())?;
        let checked = environment::checked_environment(
            &prepare_app,
            &project.id,
            Some(&project.environment_id),
            Some(&fingerprint),
        )?;
        if checked.project.stage != crate::projects::ProjectStage::EnvironmentReady {
            return Err("Prepare the selected Claude environment first.".into());
        }
        if action == Action::Resume {
            if let Some(identifier) = terminal::running_conversation(
                &checked.project,
                &session.as_ref().unwrap().argument(),
            ) {
                return Ok((Launch::Existing(identifier), account));
            }
        }
        if matches!(action, Action::NewChat | Action::Resume) {
            let status = crate::providers::shared_claude::ensure_locked(
                &prepare_app,
                &mut account,
                &checked,
            )?;
            if status.state != "signed-in" {
                return Err(status.message);
            }
        }
        let arguments = claude::arguments(&checked, action, session.as_ref(), use_brig_boundary)?;
        if action == Action::Resume {
            resume::require(&checked, session.as_ref().unwrap())?;
        }
        crate::conversations::observe(&prepare_app, &checked)?;
        let command = environment::command(&checked, arguments);
        Ok::<_, String>((Launch::Prepared {
            command,
            project: checked.project,
            conversation: session.map(|session| session.argument()),
        }, account))
    })
    .await
    .map_err(|_| "Could not prepare this agent session.")??;
    let (prepared, _account_guard) = prepared;
    let (command, project, conversation) = match prepared {
        Launch::Existing(identifier) => return Ok(identifier),
        Launch::Prepared {
            command,
            project,
            conversation,
        } => (command, project, conversation),
    };
    terminal::start_reserved(
        window,
        project.id.clone(),
        action.purpose(),
        command,
        layout,
        conversation,
        terminal::Reservation {
            identifier: terminal_identifier,
            on_exit: None,
            target: Some(terminal::AgentTarget::for_project(&project)),
        },
    )
    .await
}

#[tauri::command]
pub async fn agent_account_status(
    app: AppHandle,
    identifier: String,
    environment_id: EnvironmentId,
) -> Result<Status, String> {
    let project = selected(&app, &identifier, Some(&environment_id))?;
    require_claude(&project)?;
    let operation = app.state::<Accounts>().operation(&project)?;
    let _guard = operation.lock_owned().await;
    tauri::async_runtime::spawn_blocking(move || {
        let accounts = app.state::<Accounts>();
        if let Some(pending) = accounts.pending(&project)? {
            return Ok(pending);
        }
        let checked = environment::checked_environment(
            &app,
            &project.id,
            Some(&project.environment_id),
            None,
        )?;
        if checked.project.stage != crate::projects::ProjectStage::EnvironmentReady {
            return accounts.publish(
                &project,
                Status::new(
                    &project,
                    State::EnvironmentStopped,
                    "Prepare this Claude environment before checking its account.",
                    None,
                ),
            );
        }
        let shared = crate::providers::shared_claude::ensure(&app, &checked)?;
        let status = Status::new(&project, if shared.state == "signed-in" && claude::signed_in(&checked)? { State::SignedIn } else { State::SignedOut }, &shared.message, None);
        accounts.publish(&project, status)
    })
    .await
    .map_err(|_| "Could not read this Claude account.")?
}

#[tauri::command]
pub async fn agent_logout(
    app: AppHandle,
    identifier: String,
    environment_id: EnvironmentId,
) -> Result<Status, String> {
    let _ = (app, identifier, environment_id);
    Err("Use Sign out of Tugboat in Settings, Providers to disconnect the shared Claude account.".into())
}

#[cfg(test)]
mod tests;
