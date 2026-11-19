use crate::platform::brig::Brig;
use serde_json::Value;
use std::{
    fs,
    io::{Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt},
        net::UnixStream,
    },
    path::{Path, PathBuf},
    time::Duration,
};

const UNVERIFIED: &str =
    "Could not confirm revocation in the Brig credential broker. Retry revoking access.";

fn control_path(brig: &Brig) -> Result<PathBuf, String> {
    let directory = brig
        .environment
        .get("BRIG_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            brig.environment
                .get("HOME")
                .map(|home| Path::new(home).join(".brig"))
        })
        .filter(|path| path.is_absolute())
        .ok_or(UNVERIFIED)?;
    Ok(directory.join("host/broker/control.sock"))
}

pub(super) fn revoke(brig: &Brig, sandbox: &str) -> Result<(), String> {
    if !sandbox.starts_with("brig-") || !crate::desktop::settings::valid_identifier(sandbox) {
        return Err(UNVERIFIED.into());
    }
    let socket = control_path(brig)?;
    if !registered(&socket, sandbox)? {
        return Ok(());
    }
    let result = request(&socket, "DELETE", &format!("/sandboxes/{sandbox}"))?;
    if result.is_some_and(|response| response["removed"].as_bool().is_none())
        || registered(&socket, sandbox)?
    {
        return Err(UNVERIFIED.into());
    }
    Ok(())
}

pub(super) fn serves(brig: &Brig, sandbox: &str) -> Result<bool, String> {
    registered(&control_path(brig)?, sandbox)
}

fn registered(socket: &Path, sandbox: &str) -> Result<bool, String> {
    let Some(response) = request(socket, "GET", "/status")? else {
        return Ok(false);
    };
    let sandboxes = response["sandboxes"].as_array().ok_or(UNVERIFIED)?;
    if sandboxes.iter().any(|id| !id.is_string()) {
        return Err(UNVERIFIED.into());
    }
    Ok(sandboxes.iter().any(|id| id == sandbox))
}

fn request(socket: &Path, method: &str, resource: &str) -> Result<Option<Value>, String> {
    let identity = match fs::symlink_metadata(socket) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(UNVERIFIED.into()),
    };
    if !identity.file_type().is_socket()
        || identity.uid() != unsafe { libc::geteuid() }
        || identity.mode() & 0o077 != 0
    {
        return Err(UNVERIFIED.into());
    }
    let mut stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(UNVERIFIED.into()),
    };
    let limit = Some(Duration::from_secs(4));
    stream.set_read_timeout(limit).map_err(|_| UNVERIFIED)?;
    stream.set_write_timeout(limit).map_err(|_| UNVERIFIED)?;
    write!(
        stream,
        "{method} {resource} HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .map_err(|_| UNVERIFIED)?;
    let mut response = String::new();
    stream
        .take(65_537)
        .read_to_string(&mut response)
        .map_err(|_| UNVERIFIED)?;
    let (headers, body) = response.split_once("\r\n\r\n").ok_or(UNVERIFIED)?;
    if response.len() > 65_536 || headers.split_whitespace().nth(1) != Some("200") {
        return Err(UNVERIFIED.into());
    }
    serde_json::from_str(body)
        .map(Some)
        .map_err(|_| UNVERIFIED.into())
}
