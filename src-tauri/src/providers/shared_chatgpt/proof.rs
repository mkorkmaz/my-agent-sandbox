use super::{directory::Directory, guest, store};
use crate::{
    environment::{self, shared_runtime, CheckedProject},
    platform::brig,
    projects::project_image_update::{proof_policy, CANDIDATE_IMAGE},
};
use serde_json::Value;
use std::{env, fs, path::PathBuf};

pub(crate) fn with_access_copy<T>(
    checked: &CheckedProject,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let observed = checked.brig.plan(&checked.project, &checked.settings)?;
    let data = &observed["data"];
    let policy = checked
        .project
        .policy
        .as_ref()
        .ok_or("The disposable project has no registered policy.")?;
    let document = checked
        .brig
        .policy(&policy.name)?
        .ok_or("The disposable project's registered policy is unavailable.")?;
    if checked.project.image_ref != CANDIDATE_IMAGE
        || data["image"]["ref"] != CANDIDATE_IMAGE
        || checked.project.policy_detached
        || checked.project.github_access.is_some()
        || policy.fingerprint.as_deref() != Some(brig::fingerprint(&document)?.as_str())
        || !environment::is_running(&checked.brig, &checked.project, &checked.paths)?
    {
        return Err("The ChatGPT replay proof requires its running signed candidate.".into());
    }
    proof_policy::verify(data, &document, &policy.name)?;
    shared_runtime::verify_codex(checked)?;
    let records = PathBuf::from(
        env::var_os("TUGBOAT_IMAGE_TEST_CHATGPT_RECORDS")
            .ok_or("Set TUGBOAT_IMAGE_TEST_CHATGPT_RECORDS to the authorized Dev records.")?,
    );
    let expected = env::home_dir()
        .ok_or("The home directory is unavailable.")?
        .join("Library/Application Support/Tugboat Dev");
    let provider = records.join("chatgpt");
    if records != expected || fs::canonicalize(&provider).ok().as_ref() != Some(&provider) {
        return Err("Use the authorized default Dev ChatGPT account for this proof.".into());
    }
    let document = store::read_private(&provider.join("session.json"))?
        .ok_or("The authorized Dev ChatGPT account is unavailable.")?;
    let session: store::Session = serde_json::from_slice(&document)
        .map_err(|_| "The authorized Dev ChatGPT record is invalid.")?;
    session.validate()?;
    if session.disconnected || session.reconnect_required || session.pending_login.is_some() {
        return Err("The authorized Dev ChatGPT account needs a completed sign-in.".into());
    }
    let credentials = session
        .credentials
        .ok_or("The authorized Dev ChatGPT account is disconnected.")?;
    if credentials.expires_at()? <= chrono::Utc::now().timestamp() + 180 {
        return Err("The authorized Dev ChatGPT access has expired or will expire shortly.".into());
    }
    guest::require_idle(checked, false)?;
    let path = guest::directory(checked)?;
    let directory = Directory::open(&path, None)?;
    if directory.read()?.is_some() {
        return Err("The disposable project already has a ChatGPT access copy.".into());
    }
    let projection = credentials.guest();
    if projection["tokens"]["refresh_token"] != "" {
        return Err("The proof requires access without renewal credentials.".into());
    }
    let mut copy = AccessCopy {
        directory,
        projection,
        removed: false,
    };
    let bytes = serde_json::to_vec(&copy.projection)
        .map_err(|_| "Could not prepare the ChatGPT access copy.")?;
    copy.directory.write(&bytes)?;
    let result = operation();
    copy.remove()?;
    result
}

struct AccessCopy {
    directory: Directory,
    projection: Value,
    removed: bool,
}

impl AccessCopy {
    fn remove(&mut self) -> Result<(), String> {
        if let Some(bytes) = self.directory.read()? {
            if !guest::same_access(&bytes, &self.projection) {
                return Err(
                    "The disposable ChatGPT access copy changed. Review its cleanup.".into(),
                );
            }
            self.directory.remove()?;
        }
        self.removed = self.directory.read()?.is_none();
        if !self.removed {
            return Err("The disposable ChatGPT access copy remains after cleanup.".into());
        }
        Ok(())
    }
}

impl Drop for AccessCopy {
    fn drop(&mut self) {
        if !self.removed && self.remove().is_err() {
            eprintln!("Verify cleanup of the disposable ChatGPT access copy.");
        }
    }
}
