use crate::{agents::SessionId, environment::CheckedProject};
use serde_json::Value;
use std::ffi::{CString, OsStr};
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::{ffi::OsStrExt, fs::OpenOptionsExt},
};
use std::path::{Component, Path};

const HEADER_LIMIT: u64 = 1_048_576;

fn open_at(parent: &File, name: &OsStr, directory: bool) -> Result<Option<File>, String> {
    let name = CString::new(name.as_bytes()).map_err(|_| "Invalid Claude transcript name.")?;
    let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
    let flags = flags | if directory { libc::O_DIRECTORY } else { 0 };
    let descriptor = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if descriptor >= 0 {
        return Ok(Some(unsafe { File::from_raw_fd(descriptor) }));
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::NotFound {
        return Ok(None);
    }
    Err("Could not safely open this Claude transcript. Symbolic links and special files are unsupported.".into())
}

fn owns_header(file: File, session: &SessionId, guest_project: &Path) -> Result<bool, String> {
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("The Claude transcript is not a regular file.".into());
    }
    let mut bytes = Vec::new();
    file.take(HEADER_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read the Claude transcript header.")?;
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if !line.ends_with(b"\n") {
            break;
        }
        let value: Value =
            serde_json::from_slice(line).map_err(|_| "The Claude transcript header is invalid.")?;
        if value
            .get("sessionId")
            .is_some_and(|value| value != &session.argument())
        {
            return Err("The Claude transcript session ID does not match its file.".into());
        }
        if !matches!(value["type"].as_str(), Some("user" | "assistant")) {
            continue;
        }
        if value["isSidechain"] != false
            || value.get("agentId").is_some_and(|value| !value.is_null())
        {
            return Err(
                "Choose a main Claude chat. Subagent transcripts cannot be resumed here.".into(),
            );
        }
        if value["sessionId"] != session.argument() {
            return Err("The Claude transcript session ID is missing or invalid.".into());
        }
        let directory = value["cwd"]
            .as_str()
            .ok_or("The Claude transcript project path is missing.")?;
        let directory = Path::new(directory);
        return Ok(directory.is_absolute()
            && !directory
                .components()
                .any(|part| part == Component::ParentDir)
            && directory.starts_with(guest_project));
    }
    if bytes.len() as u64 > HEADER_LIMIT {
        return Err("The Claude transcript header exceeds the supported size.".into());
    }
    Err(
        "The Claude transcript has no complete main-chat header. Wait and refresh before resuming."
            .into(),
    )
}

pub(super) fn require(checked: &CheckedProject, session: &SessionId) -> Result<(), String> {
    let home = &checked.paths.host_home;
    let anchor = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(home)
        .map_err(|_| "The selected Claude guest home is unavailable.")?;
    let configuration = open_at(&anchor, OsStr::new(".claude"), true)?
        .ok_or("No saved Claude chats exist in this environment.")?;
    let projects = open_at(&configuration, OsStr::new("projects"), true)?
        .ok_or("No saved Claude chats exist in this environment.")?;
    let entries = fs::read_dir(home.join(".claude/projects"))
        .map_err(|_| "Could not read this environment's Claude projects.")?;
    let mut count = 0;
    let mut found = false;
    let mut transcript_count = 0;
    for entry in entries {
        count += 1;
        if count > 4096 {
            return Err("The Claude project list exceeds the supported size.".into());
        }
        let entry = entry.map_err(|_| "Could not read a saved Claude project.")?;
        if !entry
            .file_type()
            .map_err(|_| "Could not verify a saved Claude project.")?
            .is_dir()
        {
            continue;
        }
        let Some(directory) = open_at(&projects, &entry.file_name(), true)? else {
            continue;
        };
        let name = format!("{}.jsonl", session.argument());
        let Some(file) = open_at(&directory, OsStr::new(&name), false)? else {
            continue;
        };
        transcript_count += 1;
        if transcript_count > 1 {
            return Err("Multiple Claude transcripts use this session ID. Resolve the copies before resuming.".into());
        }
        found = owns_header(file, session, &checked.paths.guest_project)?;
    }
    if !found {
        return Err(
            "This Claude chat does not belong to the selected environment and project.".into(),
        );
    }
    Ok(())
}
