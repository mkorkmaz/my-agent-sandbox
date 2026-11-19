use crate::agents::{AgentKind, SessionId};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    env,
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Fixture {
    home: PathBuf,
    codex: Transcript,
    claude: Transcript,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Transcript {
    pub(super) session_id: String,
    pub(super) path: String,
    pub(super) marker: String,
}

impl Transcript {
    fn validate(&self, agent: AgentKind) -> Result<(), String> {
        SessionId::new(&self.session_id)?;
        if !(6..=96).contains(&self.marker.len())
            || !self
                .marker
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err("A saved chat marker must use 6 to 96 ASCII letters, digits, hyphens or underscores.".into());
        }
        let valid = match agent {
            AgentKind::Codex => self.valid_codex_path(),
            AgentKind::ClaudeCode => {
                self.path
                    == format!(
                        ".claude/projects/-work-sample-project/{}.jsonl",
                        self.session_id
                    )
            }
        };
        if !valid {
            return Err(
                "A saved chat path does not match the agent and its native session ID.".into(),
            );
        }
        Ok(())
    }

    fn valid_codex_path(&self) -> bool {
        let Some((_, filename)) = self.path.rsplit_once('/') else {
            return false;
        };
        let suffix = format!("-{}.jsonl", self.session_id);
        let Some(stamp) = filename
            .strip_prefix("rollout-")
            .and_then(|value| value.strip_suffix(&suffix))
        else {
            return false;
        };
        let Ok(time) = chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%dT%H-%M-%S") else {
            return false;
        };
        time.format("%Y-%m-%dT%H-%M-%S").to_string() == stamp
            && self.path == format!(".codex/sessions/{}/{filename}", time.format("%Y/%m/%d"))
    }

    fn verify_marker(&self, agent: AgentKind, bytes: &[u8]) -> Result<(), String> {
        let latest_reply = bytes.rsplit(|byte| *byte == b'\n').find_map(|line| {
            let event: Value = serde_json::from_slice(line).ok()?;
            let message = match agent {
                AgentKind::Codex if event["type"] == "response_item" => &event["payload"],
                AgentKind::ClaudeCode
                    if event["type"] == "assistant" && event["sessionId"] == self.session_id =>
                {
                    &event["message"]
                }
                _ => return None,
            };
            if message["role"] != "assistant" {
                return None;
            }
            let mut text = message["content"]
                .as_array()?
                .iter()
                .filter_map(|part| part["text"].as_str());
            let first = text.next()?;
            Some(first.contains(&self.marker) || text.any(|part| part.contains(&self.marker)))
        });
        if latest_reply != Some(true) {
            return Err("A saved chat marker must occur in the latest assistant reply.".into());
        }
        Ok(())
    }
}

impl Fixture {
    pub(super) fn load() -> Result<Self, String> {
        let manifest =
            PathBuf::from(env::var_os("TUGBOAT_IMAGE_TEST_TRANSCRIPT_MANIFEST").ok_or(
                "Set TUGBOAT_IMAGE_TEST_TRANSCRIPT_MANIFEST to the private proof manifest.",
            )?);
        if !manifest.starts_with("/private/tmp") {
            return Err("The saved chat manifest must be in a private temporary directory.".into());
        }
        let fixture: Self = serde_json::from_slice(&bounded_file(&manifest, 4096, true)?)
            .map_err(|_| "The saved chat manifest is invalid.")?;
        fixture.verify_home()?;
        fixture.codex.validate(AgentKind::Codex)?;
        fixture.claude.validate(AgentKind::ClaudeCode)?;
        if fixture.codex.session_id == fixture.claude.session_id {
            return Err("Select a separate native saved chat for each proof agent.".into());
        }
        Ok(fixture)
    }

    fn verify_home(&self) -> Result<(), String> {
        let root = self
            .home
            .ancestors()
            .nth(3)
            .ok_or("The Phase 1 proof path is invalid.")?;
        let root_info =
            fs::symlink_metadata(root).map_err(|_| "The Phase 1 proof root is unavailable.")?;
        if root.parent() != Some(Path::new("/private/tmp"))
            || !root
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("tgp1."))
            || !root_info.is_dir()
            || root_info.uid() != unsafe { libc::geteuid() }
            || root_info.mode() & 0o077 != 0
            || self.home != root.join("state/homes/brig-tugboat-project-proof-proof")
            || self.home.canonicalize().ok().as_ref() != Some(&self.home)
        {
            return Err(
                "The saved chats must come from an authorized private Phase 1 proof home.".into(),
            );
        }
        Ok(())
    }

    pub(super) fn entries(&self) -> [(AgentKind, &Transcript); 2] {
        [
            (AgentKind::Codex, &self.codex),
            (AgentKind::ClaudeCode, &self.claude),
        ]
    }

    pub(super) fn sessions(&self) -> [(AgentKind, &str); 2] {
        self.entries()
            .map(|(agent, transcript)| (agent, transcript.session_id.as_str()))
    }

    pub(super) fn copy_to(&self, home: &Path) -> Result<[[u8; 32]; 2], String> {
        let mut hashes = [[0; 32]; 2];
        for ((agent, transcript), hash) in self.entries().into_iter().zip(&mut hashes) {
            let bytes = bounded_file(&self.home.join(&transcript.path), 4 * 1024 * 1024, false)?;
            transcript.verify_marker(agent, &bytes)?;
            let path = home.join(&transcript.path);
            let directory = path
                .parent()
                .ok_or("The transcript directory is invalid.")?;
            let parent = directory
                .ancestors()
                .find(|path| path.exists())
                .ok_or("The transcript directory is unavailable.")?;
            if !parent.starts_with(home) || parent.canonicalize().ok().as_deref() != Some(parent) {
                return Err("The destination transcript directory changed.".into());
            }
            fs::create_dir_all(directory).map_err(|error| error.to_string())?;
            if directory.canonicalize().ok().as_deref() != Some(directory) {
                return Err("The destination transcript directory changed.".into());
            }
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
                .and_then(|mut file| file.write_all(&bytes))
                .map_err(|_| "Could not install an isolated proof transcript.")?;
            *hash = Sha256::digest(bytes).into();
        }
        Ok(hashes)
    }

    pub(super) fn verify_copies(&self, home: &Path, hashes: &[[u8; 32]; 2]) -> Result<(), String> {
        for ((_, transcript), expected) in self.entries().into_iter().zip(hashes) {
            for directory in [&self.home, home] {
                let bytes =
                    bounded_file(&directory.join(&transcript.path), 4 * 1024 * 1024, false)?;
                if Sha256::digest(bytes)[..] != expected[..] {
                    return Err("A saved chat transcript changed across the image update.".into());
                }
            }
        }
        Ok(())
    }

    pub(super) fn verify_existing(&self, home: &Path) -> Result<(), String> {
        for (agent, transcript) in self.entries() {
            let original = bounded_file(&self.home.join(&transcript.path), 4 * 1024 * 1024, false)?;
            transcript.verify_marker(agent, &original)?;
            let current = bounded_file(&home.join(&transcript.path), 4 * 1024 * 1024, false)?;
            if current != original {
                return Err(
                    "The candidate transcript no longer matches its original proof.".into(),
                );
            }
        }
        Ok(())
    }
}

fn bounded_file(path: &Path, limit: u64, owner_only: bool) -> Result<Vec<u8>, String> {
    if path.canonicalize().ok().as_deref() != Some(path) {
        return Err("A saved chat proof file is missing or redirected.".into());
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "A saved chat proof file cannot be opened.")?;
    let metadata = file
        .metadata()
        .map_err(|_| "A saved chat proof file cannot be checked.")?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || (owner_only && metadata.mode() & 0o077 != 0)
        || metadata.len() > limit
    {
        return Err("A saved chat proof file does not match its private fixture limits.".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "A saved chat proof file cannot be read.")?;
    if bytes.len() as u64 > limit {
        return Err("A saved chat proof file exceeds the supported size.".into());
    }
    Ok(bytes)
}

#[test]
fn transcript_manifest_only_admits_exact_native_paths() {
    let mut transcript = Transcript {
        session_id: "3c852422-233e-4625-9268-641be30add87".into(),
        path: ".codex/sessions/2026/11/19/rollout-2026-11-19T20-00-05-3c852422-233e-4625-9268-641be30add87.jsonl".into(),
        marker: "proof-one".into(),
    };
    assert!(transcript.validate(AgentKind::Codex).is_ok());
    assert!(transcript.validate(AgentKind::ClaudeCode).is_err());
    transcript.path = transcript.path.replace("/11/19/", "/11/20/");
    assert!(transcript.validate(AgentKind::Codex).is_err());
    transcript.path = format!(
        ".claude/projects/-work-sample-project/{}.jsonl",
        transcript.session_id
    );
    assert!(transcript.validate(AgentKind::ClaudeCode).is_ok());
    transcript.path = transcript
        .path
        .replace("projects/", "projects/../projects/");
    assert!(transcript.validate(AgentKind::ClaudeCode).is_err());
}

#[test]
fn marker_must_be_in_the_latest_assistant_reply() {
    let bytes = br#"{"type":"response_item","payload":{"role":"assistant","content":[{"text":"earlier-proof"}]}}
{"type":"response_item","payload":{"role":"assistant","content":[{"text":"latest-proof"}]}}
{"type":"response_item","payload":{"role":"user","content":[{"text":"user-only"}]}}
"#;
    let mut transcript = Transcript {
        session_id: "3c852422-233e-4625-9268-641be30add87".into(),
        path: String::new(),
        marker: "latest-proof".into(),
    };
    assert!(transcript.verify_marker(AgentKind::Codex, bytes).is_ok());
    for marker in ["earlier-proof", "user-only"] {
        transcript.marker = marker.into();
        assert!(transcript.verify_marker(AgentKind::Codex, bytes).is_err());
    }
}
