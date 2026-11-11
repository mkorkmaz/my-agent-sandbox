use crate::{
    environment::{self, CheckedProject},
    settings::Network,
    terminal::{self, Layout},
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, WebviewWindow};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Login,
    Codex,
    Resume,
    Shell,
}

impl Action {
    fn purpose(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Codex => "codex",
            Self::Resume => "resume",
            Self::Shell => "shell",
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginStatus {
    pub method: String,
    pub message: String,
}

fn classify_login(exit_code: Option<i32>, text: &str) -> LoginStatus {
    let text = text.to_ascii_lowercase();
    if exit_code == Some(0) && text.contains("chatgpt") {
        return LoginStatus {
            method: "chatgpt".into(),
            message: "Signed in with ChatGPT.".into(),
        };
    }
    if exit_code == Some(0) && (text.contains("api key") || text.contains("api_key")) {
        return LoginStatus {
            method: "api-key".into(),
            message: "Guest Codex is using an API key. Sign in with ChatGPT.".into(),
        };
    }
    if text.contains("not logged in") || text.contains("not authenticated") {
        return LoginStatus {
            method: "logged-out".into(),
            message: "Sign in with ChatGPT.".into(),
        };
    }
    LoginStatus {
        method: "unknown".into(),
        message: "Could not read sign-in status. Prepare the environment and check again.".into(),
    }
}

fn login(checked: &CheckedProject) -> Result<LoginStatus, String> {
    if checked.project.stage != "environment-ready" {
        return Err("Prepare the Brig environment before checking sign-in status.".into());
    }
    let output = checked.brig.run(
        &[
            "sh".into(),
            checked.project.reference(),
            "codex".into(),
            "login".into(),
            "status".into(),
        ],
        Some(&checked.project.path),
    )?;
    // Status can print part of an API key. Return only the classified method.
    Ok(classify_login(
        output.exit_code,
        &(output.stdout + &output.stderr),
    ))
}

#[tauri::command]
pub async fn login_status(app: AppHandle, identifier: String) -> Result<LoginStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let checked = environment::checked_project(&app, &identifier, None)?;
        login(&checked)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn arguments(
    checked: &CheckedProject,
    action: Action,
    use_brig_boundary: bool,
    session_id: Option<&str>,
) -> Result<Vec<String>, String> {
    if checked.project.stage != "environment-ready" {
        return Err("Prepare the environment and verify the guest Codex version first.".into());
    }
    if !matches!(action, Action::Shell) && checked.settings.network == Network::Offline {
        return Err("Sign-in and online Codex sessions are unavailable in offline mode.".into());
    }
    if let Some(session_id) = session_id {
        if !matches!(action, Action::Resume) || !crate::conversations::valid_id(session_id) {
            return Err("This saved session does not belong to the selected project. Refresh the session list.".into());
        }
    }
    if matches!(action, Action::Shell) {
        return Ok(vec!["sh".into(), checked.project.reference()]);
    }
    if matches!(action, Action::Login) {
        let help = checked.brig.checked(
            &[
                "sh".into(),
                checked.project.reference(),
                "codex".into(),
                "login".into(),
                "--help".into(),
            ],
            None,
        )?;
        if !help.stdout.contains("--device-auth") {
            return Err(
                "Guest Codex does not support device sign-in. Check the image version.".into(),
            );
        }
        return Ok(vec![
            "sh".into(),
            checked.project.reference(),
            "codex".into(),
            "login".into(),
            "--device-auth".into(),
        ]);
    }
    if login(checked)?.method != "chatgpt" {
        return Err("Sign in with ChatGPT in the guest before starting Codex.".into());
    }
    let help = checked.brig.checked(
        &[
            "sh".into(),
            checked.project.reference(),
            "codex".into(),
            "--help".into(),
        ],
        None,
    )?;
    if !help.stdout.contains("--sandbox") || !help.stdout.contains("--ask-for-approval") {
        return Err(
            "Guest Codex does not support the required sandbox and approval options.".into(),
        );
    }
    let mut codex_arguments = Vec::new();
    if matches!(action, Action::Resume) {
        codex_arguments.push("resume");
        if let Some(session_id) = session_id {
            codex_arguments.push(session_id);
        }
    }
    codex_arguments.extend([
        "-c",
        "tui.terminal_title=[\"session-id\",\"status\"]",
        "--sandbox",
        if use_brig_boundary {
            "danger-full-access"
        } else {
            "workspace-write"
        },
        "--ask-for-approval",
        "on-request",
    ]);
    if !use_brig_boundary {
        codex_arguments.extend(["-c", "sandbox_workspace_write.network_access=false"]);
    }
    Ok(environment::run_arguments(checked, &codex_arguments))
}

#[tauri::command]
pub async fn start_codex_session(
    window: WebviewWindow,
    identifier: String,
    fingerprint: String,
    action: Action,
    use_brig_boundary: bool,
    session_id: Option<String>,
    layout: Layout,
) -> Result<u64, String> {
    let app = window.app_handle().clone();
    let project_id = identifier.clone();
    let conversation_id = session_id.clone();
    let command = tauri::async_runtime::spawn_blocking(move || {
        let checked = environment::checked_project(&app, &identifier, Some(&fingerprint))?;
        if use_brig_boundary && matches!(action, Action::Codex | Action::Resume) {
            crate::boundaries::require_pass(&app, &identifier, &checked.fingerprint)?;
        }
        crate::conversations::observe(&app, &checked)?;
        if matches!(action, Action::Resume) {
            let selected = session_id
                .as_deref()
                .ok_or("Choose a root chat to resume.")?;
            crate::conversations::require_resume(&app, &checked, selected)?;
        }
        let arguments = arguments(&checked, action, use_brig_boundary, session_id.as_deref())?;
        Ok::<_, String>(environment::command(&checked, arguments))
    })
    .await
    .map_err(|error| error.to_string())??;
    terminal::start(
        window,
        project_id,
        action.purpose(),
        command,
        layout,
        conversation_id,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authentication_classification_never_returns_key_fragments() {
        let chatgpt = classify_login(Some(0), "Logged in using ChatGPT");
        assert_eq!(chatgpt.method, "chatgpt");
        let api = classify_login(Some(0), "Logged in using an API key - sk-private-fragment");
        assert_eq!(api.method, "api-key");
        assert!(!api.message.contains("sk-"));
        assert_eq!(
            classify_login(Some(1), "Not logged in").method,
            "logged-out"
        );
    }
}
