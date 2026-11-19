use crate::{
    agents::AgentKind,
    environment::{managed_profile, shared_runtime, CheckedProject},
    platform::command,
    projects::Project,
};
use std::{sync::atomic::AtomicBool, time::Duration};

const SCRIPT: &str = include_str!("guest_credentials.py");

fn managed(checked: &CheckedProject) -> Result<(), String> {
    if checked.project.agent_kind != AgentKind::ClaudeCode
        || !managed_profile::shared(&checked.project)
    {
        return Err("Shared Claude sign-in requires a verified Tugboat project guest.".into());
    }
    shared_runtime::verify_claude(checked)
}

pub(super) fn publish(checked: &CheckedProject, bytes: Vec<u8>) -> Result<(), String> {
    managed(checked)?;
    execute(checked, "write", Some(bytes))
}

pub(super) fn remove(checked: &CheckedProject) -> Result<(), String> {
    managed(checked)?;
    execute(checked, "remove", None)
}

pub(super) fn remove_using(
    brig: &crate::platform::brig::Brig,
    project: &Project,
) -> Result<(), String> {
    if !managed_profile::shared(project) {
        return Err("The shared Claude copy has an unsupported profile.".into());
    }
    execute_using(brig, project, "remove", None)
}

fn execute(
    checked: &CheckedProject,
    operation: &str,
    input: Option<Vec<u8>>,
) -> Result<(), String> {
    execute_using(&checked.brig, &checked.project, operation, input)
}

fn execute_using(
    brig: &crate::platform::brig::Brig,
    project: &Project,
    operation: &str,
    input: Option<Vec<u8>>,
) -> Result<(), String> {
    let arguments = [
        "exec".into(),
        project.reference(),
        "--".into(),
        "python3".into(),
        "-c".into(),
        SCRIPT.into(),
        operation.into(),
    ];
    let result = command::run_with_input(
        &brig.executable,
        &arguments,
        None,
        &brig.environment,
        Duration::from_secs(20),
        &AtomicBool::new(false),
        input,
    )?;
    if result.exit_code != Some(0) || result.timed_out || result.cancelled || result.output_exceeded
    {
        return Err("Could not update this Claude environment's shared sign-in. Stop and reopen its environment, then retry.".into());
    }
    Ok(())
}

pub(super) fn require_idle(checked: &CheckedProject) -> Result<(), String> {
    if in_use(&checked.project) {
        return Err(
            "End Claude chats and guest terminals before changing the shared Claude account."
                .into(),
        );
    }
    if checked.project.stage != crate::projects::ProjectStage::EnvironmentReady {
        return Ok(());
    }
    let arguments = ["exec".into(), checked.project.reference(), "--".into(), "sh".into(), "-c".into(),
        r#"for file in /proc/[0-9]*/comm; do IFS= read -r name < "$file" 2>/dev/null || continue; case "$name" in claude|claude-*) printf busy; exit 0;; esac; done; printf idle"#.into()];
    let result = checked.brig.run(&arguments, None)?;
    if result.exit_code != Some(0)
        || result.timed_out
        || result.cancelled
        || result.output_exceeded
        || result.stdout != "idle"
    {
        return Err("A Claude process still uses the shared account. End it, or stop its environment, before changing accounts.".into());
    }
    Ok(())
}

pub(super) fn in_use(project: &Project) -> bool {
    crate::terminal::terminal_sessions(project.id.clone())
        .iter()
        .any(|process| {
            process.belongs_to(project)
                && process.running
                && matches!(process.purpose, "claude" | "claude-resume" | "login")
                || (process.running
                    && process.purpose == "shell"
                    && process.belongs_to_environment(&project.id, &project.environment_id))
        })
}
