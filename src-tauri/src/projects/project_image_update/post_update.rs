use super::{state::NamedHome, CANDIDATE_IMAGE};
use crate::{
    agent_sessions::shared_project_proof,
    agents::{AgentKind, SessionId},
    conversations::{self, Dashboard},
    environment::{shared_runtime, CheckedProject},
    platform::brig,
    projects::{
        project_preview::{self, tests::live::image_update_witness as preview_witness},
        EnvironmentId, Registry, PREVIOUS_ENVIRONMENT_IMAGE,
    },
    storage::data_location::RecordsDirectory,
    usage,
};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::{Ipv4Addr, TcpListener},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

mod transcripts;

const GUEST_PROJECT: &str = "/work/sample project";

pub(super) struct Retained {
    scope: Scope,
    preview: (project_preview::Approval, project_preview::Mapping),
    transcripts: transcripts::Fixture,
    hashes: [[u8; 32]; 2],
    usage: [usage::Summary; 2],
    records: RecordsDirectory,
}

#[derive(PartialEq)]
struct Scope {
    project: EnvironmentId,
    reference: String,
    home: NamedHome,
    policy: Value,
}

fn private_fixture(root: &Path, file: &Path, checked: &CheckedProject) -> Result<(), String> {
    let metadata = fs::symlink_metadata(root).map_err(|error| error.to_string())?;
    if root.parent() != Some(Path::new("/private/tmp"))
        || root.canonicalize().ok().as_deref() != Some(root)
        || !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || file != root.join("records/projects.json")
        || file.canonicalize().ok().as_deref() != Some(file)
        || checked.project.path != root.join("project-a/sample project")
        || checked.paths.guest_project != Path::new(GUEST_PROJECT)
        || !checked
            .paths
            .host_home
            .starts_with(root.join("state/tugboat-project-homes"))
        || checked
            .brig
            .environment
            .get("BRIG_STATE_DIR")
            .map(Path::new)
            != Some(root.join("state").as_path())
        || checked.brig.environment.get("HOME").map(Path::new) != Some(root.join("h").as_path())
    {
        return Err("The image update check requires its own private A fixture.".into());
    }
    checked.project.check_path()?;
    checked.project.require_active()
}

fn scope(file: &Path, checked: &CheckedProject, image: &str) -> Result<Scope, String> {
    let project =
        Registry::load(file)?.environment(&checked.project.id, &checked.project.environment_id)?;
    let policy = project
        .policy
        .as_ref()
        .ok_or("The image proof project has no registered policy.")?;
    let document = checked
        .brig
        .policy(&policy.name)?
        .ok_or("The registered image proof policy is unavailable.")?;
    let plan = &checked.plan["data"];
    if project.image_ref != image
        || checked.project.image_ref != image
        || project.image_update.is_some()
        || project.policy_detached
        || project.github_access.is_some()
        || project.reference() != checked.project.reference()
        || policy.fingerprint.as_deref() != Some(brig::fingerprint(&document)?.as_str())
    {
        return Err("The image proof requires the registered signed image and policy.".into());
    }
    super::proof_policy::verify(plan, &document, &policy.name)?;
    Ok(Scope {
        reference: project.reference(),
        project: project.environment_id,
        home: NamedHome::read(&checked.paths.host_home)?,
        policy: json!({ "registration": policy, "document": document, "egress": plan["egress"] }),
    })
}

fn summaries(
    checked: &CheckedProject,
    dashboard: &Dashboard,
    sessions: [(AgentKind, &str); 2],
    require_available: bool,
) -> Result<[usage::Summary; 2], String> {
    if dashboard.conversations.len() != sessions.len() {
        return Err("The usage database did not select exactly both saved chats.".into());
    }
    let retained = |(agent, id): (AgentKind, &str)| {
        dashboard
            .conversations
            .iter()
            .find(|chat| {
                chat.id == id
                    && chat.agent_kind == agent
                    && chat.environment_id.as_ref() == Some(&checked.project.environment_id)
                    && chat.usage_available
                    && chat.usage.requests > 0
                    && (!require_available || chat.available)
            })
            .map(|chat| chat.usage.clone())
            .ok_or_else(|| {
                format!(
                    "The saved {} chat or its attributed usage is unavailable.",
                    agent.binary()
                )
            })
    };
    Ok([retained(sessions[0])?, retained(sessions[1])?])
}

fn guest_markers(checked: &CheckedProject) -> Result<(), String> {
    let output = checked.brig.checked(
        &shared_runtime::exec(&checked.project, "/usr/bin/bash", &["-ec", r#"test "$PWD" = "$BRIG_PROJECT"; cat "$BRIG_PROJECT/tugboat-image-project" "$HOME/tugboat-image-home""#]),
        Some(&checked.project.path),
    )?;
    if output.stdout != "project-markerhome-marker" {
        return Err("The guest cannot read both retained project and named-home markers.".into());
    }
    Ok(())
}

pub(super) fn before(
    root: &Path,
    file: &Path,
    checked: &CheckedProject,
) -> Result<Retained, String> {
    private_fixture(root, file, checked)?;
    let scope = scope(file, checked, PREVIOUS_ENVIRONMENT_IMAGE)?;
    guest_markers(checked)?;
    let records = RecordsDirectory::fixture(&root.join("records"));
    let transcripts = transcripts::Fixture::load()?;
    let hashes = transcripts.copy_to(&checked.paths.host_home)?;
    let config = checked.paths.host_home.join(".codex/config.toml");
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(config)
        .and_then(|mut file| {
            file.write_all(b"[projects.\"/work/sample project\"]\ntrust_level = \"trusted\"\n")
        })
        .map_err(|_| "Could not seed trust for the disposable proof workspace.")?;
    let sessions = transcripts.sessions();
    let (_, verified) = conversations::image_update_witness(checked, &records, sessions)?;
    let usage = summaries(checked, &verified, sessions, true)?;
    let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|listener| listener.local_addr())
        .map_err(|_| "Could not select an isolated loopback port.")?
        .port();
    let mapping = project_preview::Mapping {
        host_port: port,
        guest_port: port,
    };
    let approval = preview_witness(checked, file, project_preview::Action::Publish, mapping)?;
    println!("Recorded both original native chat IDs, measured usage, policy, home, project and one working preview before review.");
    Ok(Retained {
        scope,
        preview: (approval, mapping),
        transcripts,
        hashes,
        usage,
        records,
    })
}

pub(super) fn after(
    root: &Path,
    file: &Path,
    checked: &CheckedProject,
    before: &Retained,
) -> Result<(), String> {
    private_fixture(root, file, checked)?;
    if scope(file, checked, CANDIDATE_IMAGE)? != before.scope {
        return Err(
            "The image update changed the registered project, named home or policy.".into(),
        );
    }
    guest_markers(checked)?;
    let (approval, mapping) = &before.preview;
    if preview_witness(checked, file, project_preview::Action::Read, *mapping)? != *approval {
        return Err("The image update changed the approved preview mapping.".into());
    }
    before
        .transcripts
        .verify_copies(&checked.paths.host_home, &before.hashes)?;
    let sessions = before.transcripts.sessions();
    let (recorded, verified) =
        conversations::image_update_witness(checked, &before.records, sessions)?;
    if summaries(checked, &recorded, sessions, false)? != before.usage
        || summaries(checked, &verified, sessions, true)? != before.usage
    {
        return Err("The image update changed usage attributed to the saved chats.".into());
    }
    println!("Verified both chat IDs with unchanged measured usage, the exact policy and loopback preview, and both mounted markers on the candidate.");
    replay(checked, &before.transcripts)
}

pub(super) fn resume_existing(
    root: &Path,
    file: &Path,
    checked: &CheckedProject,
) -> Result<(), String> {
    private_fixture(root, file, checked)?;
    scope(file, checked, CANDIDATE_IMAGE)?;
    guest_markers(checked)?;
    let transcripts = transcripts::Fixture::load()?;
    transcripts.verify_existing(&checked.paths.host_home)?;
    let records = RecordsDirectory::fixture_existing(&root.join("records"), &"a".repeat(32));
    let sessions = transcripts.sessions();
    let (recorded, verified) = conversations::image_update_witness(checked, &records, sessions)?;
    if summaries(checked, &recorded, sessions, false)?
        != summaries(checked, &verified, sessions, true)?
    {
        return Err(
            "The signed candidate did not retain the measured usage of both selected chats.".into(),
        );
    }
    println!("Verified the retained signed candidate and both exact saved chat IDs at the approved plan fingerprint.");
    replay(checked, &transcripts)
}

fn replay(checked: &CheckedProject, transcripts: &transcripts::Fixture) -> Result<(), String> {
    let mut failures = Vec::new();
    for (agent, transcript) in transcripts.entries() {
        let id = SessionId::new(&transcript.session_id)?;
        match shared_project_proof::replay_without_message(checked, agent, &id, &transcript.marker)
        {
            Ok(()) => println!(
                "{} displayed its existing native chat without a new message.",
                agent.binary()
            ),
            Err(error) => failures.push(error),
        }
    }
    if !failures.is_empty() {
        return Err(failures.join(" "));
    }
    Ok(())
}
