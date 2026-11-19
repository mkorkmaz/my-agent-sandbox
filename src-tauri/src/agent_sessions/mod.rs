mod account;
mod claude;
mod codex;
mod launch;
mod resume;
#[cfg(test)]
pub(crate) mod shared_project_proof;

use crate::{
    agents::{AgentKind, SessionId},
    environment,
    projects::{self, EnvironmentId, Project, Registry},
    terminal::{self, Layout},
};
pub use account::Accounts;
use serde::Deserialize;
use std::process::Command;
use tauri::{AppHandle, Manager, WebviewWindow};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    NewChat,
    Resume,
    Shell,
}

impl Action {
    fn purpose(self) -> &'static str {
        match self {
            Self::NewChat => "claude",
            Self::Resume => "claude-resume",
            Self::Shell => "shell",
        }
    }

    fn codex(self) -> codex::Action {
        match self {
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
    environment_id: &EnvironmentId,
    agent_kind: AgentKind,
) -> Result<Project, String> {
    Registry::load(&projects::file(app)?)?.agent_environment(identifier, environment_id, agent_kind)
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
    environment_id: EnvironmentId,
    agent_kind: AgentKind,
    fingerprint: String,
    action: Action,
    use_brig_boundary: Option<bool>,
    session_id: Option<String>,
    layout: Layout,
) -> Result<u64, String> {
    let app = window.app_handle().clone();
    let project = selected(&app, &identifier, &environment_id, agent_kind)?;
    let _operation =
        environment::EnvironmentOperation::begin(&project.id, &project.environment_id)?;
    if action == Action::Shell {
        action.session(session_id.as_deref())?;
        let target = terminal::AgentTarget::for_project(&project);
        let shell_app = app.clone();
        let command = tauri::async_runtime::spawn_blocking(move || {
            let checked = environment::checked_agent_environment(
                &shell_app,
                &identifier,
                &environment_id,
                agent_kind,
                Some(&fingerprint),
            )?;
            if checked.project.stage != crate::projects::ProjectStage::EnvironmentReady {
                return Err(
                    "Prepare the shared project environment before opening its guest shell.".into(),
                );
            }
            let arguments = vec!["sh".into(), checked.project.reference()];
            Ok::<_, String>(environment::command(&checked, arguments))
        })
        .await
        .map_err(|_| "Could not prepare the guest shell.")??;
        return terminal::start_reserved(
            window,
            project.id,
            "shell",
            command,
            layout,
            None,
            terminal::Reservation {
                identifier: terminal::reserve_identifier(),
                on_exit: None,
                target: Some(target),
            },
        )
        .await;
    }
    if agent_kind == AgentKind::Codex {
        return codex::start_codex_session(
            window,
            project,
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
        let checked = environment::checked_agent_environment(
            &prepare_app,
            &project.id,
            &project.environment_id,
            agent_kind,
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
        Ok::<_, String>((
            Launch::Prepared {
                command,
                project: checked.project,
                conversation: session.map(|session| session.argument()),
            },
            account,
        ))
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

#[cfg(test)]
mod tests;
