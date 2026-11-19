use crate::projects::{EnvironmentId, Project};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::net::{Ipv4Addr, SocketAddrV4};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Mapping {
    pub host_port: u16,
    pub guest_port: u16,
}

impl Mapping {
    pub(super) fn validate(self) -> Result<(), String> {
        if self.host_port == 0 || self.guest_port == 0 {
            return Err("Use a host and guest port from 1 to 65535.".into());
        }
        Ok(())
    }

    pub(super) fn address(self) -> SocketAddrV4 {
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.host_port)
    }

    pub(super) fn publication(self) -> String {
        format!("{}:{}", self.address(), self.guest_port)
    }

    pub(super) fn url(self) -> String {
        format!("http://{}/", self.address())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Phase {
    Publishing,
    Published,
    Closing,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Approval {
    pub(crate) environment_id: EnvironmentId,
    pub(super) reference: String,
    pub(super) mapping: Mapping,
    pub(super) phase: Phase,
}

impl Approval {
    pub(crate) fn validate(&self, reference: &str) -> Result<(), String> {
        self.mapping.validate()?;
        if self.reference != reference {
            return Err("Close the local preview before changing its Brig session.".into());
        }
        Ok(())
    }

    pub(super) fn new(project: &Project, mapping: Mapping) -> Self {
        Self {
            environment_id: project.environment_id.clone(),
            reference: project.reference(),
            mapping,
            phase: Phase::Publishing,
        }
    }
}

pub(super) const REVIEW: &str =
    "Brig reported a port outside the approved local preview. Review the session before continuing.";

pub(super) fn planned(value: &Value, approved: Option<Mapping>) -> Result<bool, String> {
    let ports = value
        .as_array()
        .ok_or("The Brig plan is missing its port list.")?;
    match ports.as_slice() {
        [] => Ok(false),
        [port] if approved.is_some_and(|mapping| *port == mapping.publication()) => Ok(true),
        _ => Err(REVIEW.into()),
    }
}

pub(crate) fn validate_plan_ports(project: &Project, value: &Value) -> Result<(), String> {
    let approval = project
        .preview
        .as_ref()
        .filter(|approval| approval.environment_id == project.environment_id);
    let has_port = planned(value, approval.map(|value| value.mapping))?;
    if let Some(approval) = approval {
        if approval.phase != Phase::Published || !has_port {
            return Err("Finish the local preview change in Project settings before preparing this environment.".into());
        }
        approval.validate(&project.reference())?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Observed {
    Absent,
    Recorded,
    Live,
}

pub(super) fn observed(
    data: &Value,
    sandbox: &str,
    approved: Option<Mapping>,
) -> Result<Observed, String> {
    if data["sandbox"] != sandbox {
        return Err("The Brig port list belongs to a different sandbox.".into());
    }
    let ports = data["ports"]
        .as_array()
        .ok_or("The Brig port list is missing.")?;
    match ports.as_slice() {
        [] => Ok(Observed::Absent),
        [port]
            if approved.is_some_and(|mapping| {
                port["host"] == mapping.address().to_string()
                    && port["guest"] == mapping.guest_port
                    && port["protocol"] == "tcp"
            }) =>
        {
            match port["live"].as_bool() {
                Some(true) => Ok(Observed::Live),
                Some(false) => Ok(Observed::Recorded),
                None => Err("Brig could not confirm the project preview port state.".into()),
            }
        }
        _ => Err(REVIEW.into()),
    }
}
