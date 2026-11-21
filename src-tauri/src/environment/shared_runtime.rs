use super::{codex_package, CheckedProject};
use crate::{
    agents::{AgentCommand, AgentKind, GuestCommand},
    projects::{Project, DEFAULT_ENVIRONMENT_IMAGE, PREVIOUS_ENVIRONMENT_IMAGE},
};

use serde::Serialize;

const CLAUDE_PROGRAM: &str = "/usr/local/bin/claude";
const PREVIOUS_CLAUDE_SHA256: &str =
    "53a0d7e219d7b0804ecba5dbc002745e89f786cc946d2de85a6470d5b2d1fb06";

const DEFAULT_CLAUDE_SHA256: &str =
    "98d56f6ffe53469767a74864a38eaf359d62ef557a8de1b1ad524d61a66ec119";

#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Versions {
    pub(crate) codex: String,
    pub(crate) claude_code: String,
}

pub(crate) fn program(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Codex => codex_package::PROGRAM,
        AgentKind::ClaudeCode => CLAUDE_PROGRAM,
    }
}

pub(crate) fn interactive(project: &Project, command: GuestCommand) -> Vec<String> {
    command.in_guest(project.reference())
}

pub(crate) fn exec(project: &Project, program: &str, arguments: &[&str]) -> Vec<String> {
    let mut command = vec![
        "exec".into(),
        project.reference(),
        "--".into(),
        program.into(),
    ];
    command.extend(arguments.iter().map(|argument| (*argument).to_owned()));
    command
}

pub(crate) fn install_codex(checked: &CheckedProject) -> Result<(), String> {
    if checked.paths.host_home != checked.brig.shared_project_home(&checked.project)? {
        return Err("The Codex guest installation path does not match this project.".into());
    }
    let running = super::listed_as_running(&checked.brig, &checked.project)?;
    codex_package::install(&checked.brig, &checked.paths.host_home, running)
}

pub(crate) fn verify_codex(checked: &CheckedProject) -> Result<(), String> {
    codex_package::verify(&checked.paths.host_home)
}

pub(crate) fn pinned_versions(image: &str) -> Result<Versions, String> {
    let claude_code = match image {
        PREVIOUS_ENVIRONMENT_IMAGE => "2.1.318 (Claude Code)",
        DEFAULT_ENVIRONMENT_IMAGE => "2.1.326 (Claude Code)",
        _ => return Err("The guest image has no approved Claude Code release.".into()),
    };
    Ok(Versions {
        codex: codex_package::VERSION.into(),
        claude_code: claude_code.into(),
    })
}

pub(crate) fn verify_versions(checked: &CheckedProject) -> Result<Versions, String> {
    verify_codex(checked)?;
    verify_claude(checked)?;
    let version = |kind: AgentKind| {
        let arguments = kind
            .command(AgentCommand::Version)?
            .in_output(checked.project.reference());
        checked
            .brig
            .checked(&arguments, None)
            .map(|output| output.stdout.trim().to_owned())
    };
    let versions = Versions {
        codex: version(AgentKind::Codex)?,
        claude_code: version(AgentKind::ClaudeCode)?,
    };
    if versions != pinned_versions(&checked.project.image_ref)? {
        return Err("The guest did not run both pinned agent versions.".into());
    }
    Ok(versions)
}

pub(crate) fn verify_claude(checked: &CheckedProject) -> Result<(), String> {
    let mounts = checked.brig.checked(
        &exec(
            &checked.project,
            "/usr/bin/findmnt",
            &["-n", "-o", "TARGET,FSTYPE", "-T", "/brig/claude"],
        ),
        None,
    )?;
    if mounts.stdout.trim() != "/brig/claude tmpfs" {
        return Err("The guest Claude access directory is not mounted on temporary memory.".into());
    }
    let expected = match checked.project.image_ref.as_str() {
        PREVIOUS_ENVIRONMENT_IMAGE => PREVIOUS_CLAUDE_SHA256,
        DEFAULT_ENVIRONMENT_IMAGE => DEFAULT_CLAUDE_SHA256,
        _ => return Err("The guest image has no approved Claude executable.".into()),
    };
    let executable = checked.brig.checked(
        &exec(&checked.project, "/usr/bin/sha256sum", &[CLAUDE_PROGRAM]),
        None,
    )?;
    if executable.stdout.split_whitespace().next() != Some(expected) {
        return Err(
            "The guest Claude executable changed. Prepare the verified guest again.".into(),
        );
    }
    Ok(())
}
