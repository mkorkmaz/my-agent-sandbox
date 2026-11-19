use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Read;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Phase {
    Saving,
    Saved,
    Revoking,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Grant {
    pub(super) repository: String,
    pub(super) name: String,
    pub(super) phase: Phase,
}

impl Grant {
    pub(super) fn new(project: &str, repository: String) -> Result<Self, String> {
        let mut random = [0; 10];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut source| source.read_exact(&mut random))
            .map_err(|_| "Could not create a private GitHub grant identity.")?;
        let name = format!("{}{}", prefix(project), crate::hash::hex(random));
        let grant = Self {
            repository,
            name,
            phase: Phase::Saving,
        };
        grant.validate(project)?;
        Ok(grant)
    }

    pub(crate) fn validate(&self, project: &str) -> Result<(), String> {
        let suffix = self.name.strip_prefix(&prefix(project));
        if !suffix.is_some_and(|suffix| {
            suffix.len() == 20
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        }) || !crate::desktop::settings::valid_identifier(&self.name)
            || repository(&self.repository).as_deref() != Ok(self.repository.as_str())
        {
            return Err("The GitHub access grant does not match this project.".into());
        }
        Ok(())
    }

    pub(super) fn source(&self) -> Value {
        json!({"apiVersion": "brig.sh/v1alpha1", "kind": "Policy", "metadata": {"name": self.name}, "spec": {
            "egress": {"default": "deny"}, "routes": [{
                "name": "github", "preset": "github-token", "credential": {"secretRef": {"name": self.name}},
                "request": {"allow": {
                    "methods": ["GET", "POST"],
                    "paths": [format!("/{}.git/info/refs", self.repository), format!("/{}.git/git-upload-pack", self.repository)],
                    "repos": [self.repository]
                }}
            }]
        }})
    }

    pub(super) fn matches_policy(&self, document: &Value) -> bool {
        let mut expected = self.source();
        let fields = expected["spec"]["routes"][0]
            .as_object_mut()
            .expect("checked route source");
        fields.insert("mode".into(), json!("endpoint"));
        fields.insert("upstream".into(), json!("https://github.com"));
        fields.insert("inject".into(), json!({"headers": {"remove": ["Authorization"]}, "basicAuth": {"username": "x-access-token"}}));
        fields.insert(
            "guest".into(),
            json!({"git": {"insteadOf": ["https://github.com/", "git@github.com:"]}}),
        );
        *document == expected
    }
}

fn prefix(project: &str) -> String {
    let build = if cfg!(dev) { "dev" } else { "app" };
    format!("tgb-gh-{build}-{project}-")
}

pub(super) fn repository(value: &str) -> Result<String, String> {
    let (owner, name) = value
        .split_once('/')
        .ok_or("Enter a GitHub repository as owner/name.")?;
    let valid_owner = owner.len() <= 39
        && owner.starts_with(|character: char| character.is_ascii_alphanumeric())
        && owner.ends_with(|character: char| character.is_ascii_alphanumeric())
        && owner
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    let valid_name = !name.is_empty()
        && !matches!(name, "." | "..")
        && name.len() <= 100
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte));
    if !valid_owner || !valid_name {
        return Err("Enter a GitHub repository as owner/name.".into());
    }
    // The Brig route grammar compares repository spelling. Preserve the user's selected URL spelling.
    Ok(value.into())
}
