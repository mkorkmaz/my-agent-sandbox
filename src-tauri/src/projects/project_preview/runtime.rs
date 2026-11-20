use super::{
    settings,
    state::{self, Mapping, Observed},
    Brig, Project,
};
use crate::platform::command;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::atomic::AtomicBool,
    time::Duration,
};

pub(super) struct Snapshot {
    pub(super) ports: Observed,
    pub(super) running: bool,
    present: bool,
    isolated: bool,
}

impl Snapshot {
    pub(super) fn require_publishable(&self) -> Result<(), String> {
        if !self.present {
            return Err("Prepare the project environment before publishing a preview.".into());
        }
        if !self.isolated {
            return Err(
                "Use an isolated project with an accepted Brig plan to publish a preview.".into(),
            );
        }
        Ok(())
    }
}

pub(super) fn inspect(
    brig: &Brig,
    project: &Project,
    settings: &settings::Settings,
    approval: Option<Mapping>,
) -> Result<Snapshot, String> {
    let plan = brig
        .plan(project, settings)
        .map_err(|_| "Could not read the selected project's Brig preview plan.")?;
    let data = &plan["data"];
    if data["runtime"]["kind"] != "hull"
        || data["runtime"]["backend"] != "hvi"
        || data["runtime"]["available"] != true
    {
        return Err("The Hull runtime is unavailable. Check the Mac setup and try again.".into());
    }
    if data["profile"] != project.profile
        || data["session"] != project.label
        || data["project"]["host"].as_str() != project.path.to_str()
        || !data["projectRefused"].is_null()
    {
        return Err("The Brig plan does not match the selected project. Review the session before continuing.".into());
    }
    let sandbox = data["sandbox"]
        .as_str()
        .filter(|name| !name.is_empty())
        .ok_or("The Brig plan is missing its sandbox name.")?;
    let paths = crate::platform::brig::PlanPaths::from_plan(&plan)?;
    let planned = state::planned(&data["ports"], approval)?;
    let running = verify_sandbox(brig, project, sandbox, &paths.host_home)?;
    let report = brig
        .json(&args(project, "ls"), "Ports", Some(&project.path))
        .map_err(|_| {
            "Could not read the project's Brig port list. Check the environment and retry."
        })?;
    let ports = state::observed(&report["data"], sandbox, approval)?;
    if planned != (ports != Observed::Absent) || (ports == Observed::Live && running != Some(true))
    {
        return Err("Brig's plan and the current preview state disagree. Review the session before continuing.".into());
    }
    Ok(Snapshot {
        ports,
        running: running == Some(true),
        present: running.is_some(),
        isolated: data["posture"] == "isolated" && data["refused"].is_null(),
    })
}

pub(in crate::projects) fn verify_sandbox(
    brig: &Brig,
    project: &Project,
    sandbox: &str,
    home: &std::path::Path,
) -> Result<Option<bool>, String> {
    const WRONG_TARGET: &str =
        "The Brig guest does not match the selected project. Review the session before continuing.";
    let inventory =
        crate::projects::operations::inventory::SandboxInventory::read(brig).map_err(|_| {
            "Could not verify the project's Brig guest. Check the environment and retry."
        })?;
    let Some(running) = inventory.running(project, sandbox, home)? else {
        return Ok(None);
    };
    let info = super::super::operations::verify_target(brig, project).map_err(|_| WRONG_TARGET)?;
    if info["data"]["workspace"].as_str() != home.to_str() {
        return Err(WRONG_TARGET.into());
    }
    Ok(Some(running))
}

pub(super) fn publish(brig: &Brig, project: &Project, mapping: Mapping) -> Result<(), String> {
    mutate(brig, project, "publish", mapping.publication())
}

pub(super) fn unpublish(brig: &Brig, project: &Project, mapping: Mapping) -> Result<(), String> {
    mutate(brig, project, "unpublish", mapping.address().to_string())
}

fn args(project: &Project, verb: &str) -> Vec<String> {
    vec![
        "--json".into(),
        "network".into(),
        verb.into(),
        project.reference(),
    ]
}

fn mutate(brig: &Brig, project: &Project, verb: &str, port: String) -> Result<(), String> {
    const ERROR: &str = "Brig could not confirm the preview change. Retry in Project settings.";
    let mut args = args(project, verb);
    args.push(port);
    let result = brig.run(&args, Some(&project.path)).map_err(|_| ERROR)?;
    if result.cancelled || result.timed_out || result.output_exceeded || result.exit_code != Some(0)
    {
        if verb == "publish" && result.stderr.contains("address already in use") {
            return Err("The host port is already in use.".into());
        }
        return Err(ERROR.into());
    }
    Ok(())
}

pub(super) fn require_free_port(mapping: Mapping) -> Result<(), String> {
    TcpListener::bind(mapping.address())
        .map(drop)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AddrInUse {
                return "The host port is already in use.".into();
            }
            "The host port is unavailable.".into()
        })
}

pub(super) fn server_responds(mapping: Mapping) -> bool {
    let timeout = Duration::from_millis(600);
    let probe = || -> std::io::Result<_> {
        let mut stream = TcpStream::connect_timeout(&mapping.address().into(), timeout)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        write!(
            stream,
            "HEAD / HTTP/1.0\r\nHost: {}\r\nConnection: close\r\n\r\n",
            mapping.address()
        )?;
        let mut prefix = [0; 7];
        stream.read_exact(&mut prefix)?;
        Ok(&prefix == b"HTTP/1.")
    };
    probe().unwrap_or(false)
}

pub(super) fn open(mapping: Mapping) -> Result<(), String> {
    let result = command::run(
        std::path::Path::new("/usr/bin/open"),
        &["--".into(), mapping.url()],
        None,
        &command::environment(),
        Duration::from_secs(10),
        &AtomicBool::new(false),
    )
    .map_err(|_| "Could not open the local preview in the browser.")?;
    if result.exit_code != Some(0) || result.timed_out || result.cancelled || result.output_exceeded
    {
        return Err("Could not open the local preview in the browser.".into());
    }
    Ok(())
}
