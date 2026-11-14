use crate::{
    agents::{self, AgentCommand, AgentKind, AgentOperation, SessionId},
    environment::{self, CheckedProject},
    terminal::{self, Layout},
};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, WebviewWindow};

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
    checked.project.require_codex()?;
    if checked.project.stage != crate::projects::ProjectStage::EnvironmentReady {
        return Err("Prepare the Brig environment before checking sign-in status.".into());
    }
    let arguments = AgentKind::Codex
        .command(AgentCommand::LoginStatus)?
        .in_guest(checked.project.reference());
    let output = checked.brig.run(&arguments, Some(&checked.project.path))?;
    // Status can print part of an API key. Return only the classified method.
    Ok(classify_login(
        output.exit_code,
        &(output.stdout + &output.stderr),
    ))
}

fn arguments(
    checked: &CheckedProject,
    action: Action,
    use_brig_boundary: Option<bool>,
    session_id: Option<&str>,
) -> Result<Vec<String>, String> {
    checked.project.require_codex()?;
    use crate::agent_sessions::launch::{Admission, LaunchMode};
    let mode = match action {
        Action::Shell => LaunchMode::Shell,
        Action::Login => LaunchMode::Login,
        _ => LaunchMode::Chat,
    };
    let mut admission = Admission::begin(&checked.project, AgentKind::Codex, mode,
        "Prepare the environment and verify the guest Codex version first.")?;
    if mode != LaunchMode::Shell {
        admission.online(&checked.settings.network, "Sign-in and online Codex sessions are unavailable in offline mode.")?;
    }
    if session_id.is_some() && !matches!(action, Action::Resume) {
        return Err("This action does not accept a saved session ID.".into());
    }
    let session = session_id.map(SessionId::new).transpose()?;
    let operation = match action {
        Action::Shell => return admission.prepare(vec!["sh".into(), checked.project.reference()]),
        Action::Login => AgentOperation::Login,
        Action::Codex => AgentOperation::NewChat,
        Action::Resume => AgentOperation::Resume,
    };
    let request = operation.command(session.as_ref())?;
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
        admission.capabilities()?;
        return admission.prepare(AgentKind::Codex
            .command(request)?
            .in_guest(checked.project.reference()));
    }

    admission.authorize(login(checked)?.method == "chatgpt", "This project's Codex configuration did not accept the shared ChatGPT account. Check its Codex settings or reconnect the account in Settings.")?;
    let help = checked.brig.checked(
        &[
            "sh".into(),
            checked.project.reference(),
            "codex".into(),
            "--help".into(),
        ],
        None,
    )?;
    if !help.stdout.contains("--sandbox")
        || !help.stdout.contains("--ask-for-approval")
        || !help.stdout.contains("--no-daemon")
    {
        return Err(
            "Guest Codex does not support the required sandbox and approval options.".into(),
        );
    }
    let boundary = match use_brig_boundary.unwrap_or(true) {
        true => agents::codex::Boundary::Brig,
        false => agents::codex::Boundary::Workspace,
    };
    let arguments = agents::codex::command(request, boundary)?.in_profile();
    admission.capabilities()?;
    admission.prepare(environment::run_arguments(checked, &arguments))
}

#[tauri::command]
pub async fn start_codex_session(
    window: WebviewWindow,
    identifier: String,
    fingerprint: String,
    action: Action,
    use_brig_boundary: Option<bool>,
    session_id: Option<String>,
    layout: Layout,
) -> Result<u64, String> {
    if matches!(action, Action::Login) {
        return Err(
            "Sign in with ChatGPT from Settings, Providers. Project sign-in is no longer used."
                .into(),
        );
    }
    let app = window.app_handle().clone();
    crate::providers::provider_login::require_idle(
        &app,
        crate::providers::provider_login::Provider::Chatgpt,
    )?;
    let project_id = identifier.clone();
    let conversation_id = session_id.clone();
    let runtime = app
        .state::<crate::providers::shared_chatgpt::SharedChatgpt>()
        .0
        .clone()
        .lock_owned()
        .await;
    let command_app = app.clone();
    let (command, mut runtime, login_nonce) = tauri::async_runtime::spawn_blocking(move || {
        let app = command_app;
        let mut runtime = runtime;
        let checked = environment::checked_project(&app, &identifier, Some(&fingerprint))?;
        checked.project.require_codex()?;
        crate::conversations::observe(&app, &checked)?;
        if matches!(action, Action::Resume) {
            let selected = session_id
                .as_deref()
                .ok_or("Choose a root chat to resume.")?;
            crate::conversations::require_resume(&app, &checked, selected)?;
        }
        if matches!(action, Action::Codex | Action::Resume) {
            let status = crate::providers::shared_chatgpt::ensure(&app, &mut runtime, &checked)?;
            if status.state != "signed-in" {
                return Err(format!(
                    "{} Open ChatGPT account in Settings.",
                    status.message
                ));
            }
        }
        let mut arguments = arguments(&checked, action, use_brig_boundary, session_id.as_deref())?;
        let nonce = if matches!(action, Action::Login) {
            let (nonce, guest_path) =
                crate::providers::shared_chatgpt::begin_login(&app, &mut runtime, &checked)?;
            let mut staged = vec![
                "sh".into(),
                checked.project.reference(),
                "sh".into(),
                "-c".into(),
                "export CODEX_HOME=\"$1\"; shift; exec codex \"$@\"".into(),
                "tugboat-chatgpt-login".into(),
                guest_path,
            ];
            staged.extend(arguments.into_iter().skip(3));
            arguments = staged;
            Some(nonce)
        } else {
            None
        };
        Ok::<_, String>((environment::command(&checked, arguments), runtime, nonce))
    })
    .await
    .map_err(|error| error.to_string())??;
    let terminal_id = terminal::reserve_identifier();
    if let Some(nonce) = &login_nonce {
        runtime.attach_login_process(nonce.clone(), terminal_id)?;
    }
    let on_exit: Option<terminal::ExitHandler> = login_nonce.as_ref().map(|nonce| {
        let app = app.clone();
        let project = project_id.clone();
        let nonce = nonce.clone();
        Box::new(move |code| {
            if let Err(message) = crate::providers::shared_chatgpt::finish_login(
                &app,
                &project,
                &nonce,
                code == Some(0),
            ) {
                let _ = app.emit("shared-chatgpt-error", message);
            }
        }) as terminal::ExitHandler
    });
    let result = terminal::start_reserved(
        window,
        project_id.clone(),
        action.purpose(),
        command,
        layout,
        conversation_id,
        terminal::Reservation {
            identifier: terminal_id,
            on_exit,
            target: Some(terminal::AgentTarget::codex(&project_id)),
        },
    )
    .await;
    drop(runtime);
    if result.is_err() {
        if let Some(nonce) = login_nonce {
            tauri::async_runtime::spawn_blocking(move || {
                if let Err(message) =
                    crate::providers::shared_chatgpt::finish_login(&app, &project_id, &nonce, false)
                {
                    let _ = app.emit("shared-chatgpt-error", message);
                }
            })
            .await
            .map_err(|_| "Could not roll back the ChatGPT sign-in.")?;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::settings::Network;

    #[test]
    fn terminal_status_serialization_matches_the_interface_chat_fixture() {
        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/terminal-status.json"))
                .unwrap();
        for (key, action, identifier, conversation) in [
            ("codex-new-chat", Action::Codex, 1, None),
            (
                "codex-resume",
                Action::Resume,
                2,
                Some("1a5bc5b5-af21-432d-bf07-e2218dd9f9bf"),
            ),
        ] {
            let status = terminal::TerminalStatus {
                identifier,
                project_id: "project-a".into(),
                environment_id: Some(
                    serde_json::from_value(serde_json::json!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"))
                        .unwrap(),
                ),
                agent_kind: Some(AgentKind::Codex),
                purpose: action.purpose(),
                running: true,
                exit_code: None,
                conversation_id: conversation.map(String::from),
                busy: false,
            };
            assert_eq!(serde_json::to_value(status).unwrap(), fixtures[key]);
        }
    }

    #[test]
    fn a_claude_only_project_cannot_use_legacy_codex_account_or_terminal_commands() {
        let root =
            std::env::temp_dir().join(format!("tugboat-claude-only-codex-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut registry = crate::projects::Registry::default();
        let codex = registry.register(&root, "codex").unwrap();
        registry
            .ensure_environment(
                &codex.id,
                AgentKind::ClaudeCode,
                &crate::desktop::settings::Settings::default(),
            )
            .unwrap();
        registry
            .remove_environment(&codex.id, &codex.environment_id)
            .unwrap();
        let mut project = registry.project(&codex.id).unwrap();
        assert_eq!(project.agent_kind, AgentKind::ClaudeCode);
        project.stage = crate::projects::ProjectStage::EnvironmentReady;
        let plan = serde_json::json!({"data":{"home":{"host":root.join("home")},"project":{"guest":"/work/project"}}});
        let checked = CheckedProject {
            project,
            settings: crate::desktop::settings::Settings::default(),
            brig: crate::platform::brig::Brig {
                executable: root.join("must-not-run"),
                environment: Default::default(),
            },
            paths: crate::platform::brig::PlanPaths::from_plan(&plan).unwrap(),
            plan,
            fingerprint: "fixture".into(),
        };
        assert!(login(&checked).err().unwrap().contains("only for Codex"));
        for (action, session) in [
            (Action::Login, None),
            (Action::Codex, None),
            (Action::Shell, None),
            (Action::Resume, Some("1a5bc5b5-af21-432d-bf07-e2218dd9f9bf")),
        ] {
            assert!(arguments(&checked, action, None, session)
                .unwrap_err()
                .contains("only for Codex"));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn new_and_resumed_chats_default_to_full_access_without_approvals() {
        use std::os::unix::fs::PermissionsExt;

        let root =
            std::env::temp_dir().join(format!("tugboat-codex-arguments-{}", std::process::id()));
        std::fs::create_dir_all(root.join("sample project")).unwrap();
        let mut project = crate::projects::Registry::load(&root.join("projects.json"))
            .unwrap()
            .register(&root.join("sample project"), "codex")
            .unwrap();
        project.stage = crate::projects::ProjectStage::EnvironmentReady;
        let executable = root.join("brig-fixture");
        std::fs::write(
            &executable,
            r#"#!/bin/sh
case "$*" in
    *" login status") printf '%s\n' 'Logged in using ChatGPT' ;;
    *" codex --help") printf '%s\n' '--sandbox --ask-for-approval --no-daemon' ;;
    *) exit 1 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let plan = serde_json::json!({"data":{"home":{"host":root.join("home")},"project":{"guest":"/work/project"}}});
        let mut checked = CheckedProject {
            project,
            settings: crate::desktop::settings::Settings::default(),
            brig: crate::platform::brig::Brig {
                executable,
                environment: crate::platform::command::environment(),
            },
            paths: crate::platform::brig::PlanPaths::from_plan(&plan).unwrap(),
            plan,
            fingerprint: "fixture".into(),
        };
        for (action, session_id) in [
            (Action::Codex, None),
            (Action::Resume, Some("01a1c5fd-894f-7e42-b36c-bd1a0dfef8e7")),
        ] {
            let defaults = arguments(&checked, action, None, session_id).unwrap();
            assert!(defaults.windows(4).any(|flags| flags
                == [
                    "--sandbox",
                    "danger-full-access",
                    "--ask-for-approval",
                    "never"
                ]));
            assert!(!defaults
                .iter()
                .any(|flag| flag == "sandbox_workspace_write.network_access=false"));
            assert_eq!(
                arguments(&checked, action, Some(true), session_id).unwrap(),
                defaults
            );
            let restricted = arguments(&checked, action, Some(false), session_id).unwrap();
            assert!(restricted.windows(4).any(|flags| flags
                == [
                    "--sandbox",
                    "workspace-write",
                    "--ask-for-approval",
                    "never"
                ]));
            assert!(restricted
                .iter()
                .any(|flag| flag == "sandbox_workspace_write.network_access=false"));
        }
        assert!(arguments(&checked, Action::Resume, None, None).is_err());
        assert!(arguments(&checked, Action::Resume, None, Some("--flag")).is_err());
        assert!(arguments(
            &checked,
            Action::Codex,
            None,
            Some("1a5bc5b5-af21-432d-bf07-e2218dd9f9bf")
        )
        .is_err());
        checked.settings.network = Network::Offline;
        assert!(arguments(&checked, Action::Login, None, None).is_err());
        assert!(arguments(&checked, Action::Codex, None, None).is_err());
        assert!(arguments(
            &checked,
            Action::Resume,
            None,
            Some("1a5bc5b5-af21-432d-bf07-e2218dd9f9bf")
        )
        .is_err());
        assert_eq!(
            arguments(&checked, Action::Shell, None, None).unwrap(),
            vec!["sh".to_owned(), checked.project.reference()]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

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
