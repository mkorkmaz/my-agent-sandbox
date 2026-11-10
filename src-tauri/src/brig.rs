use crate::{command, host, projects::Project, settings::Settings};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

pub struct Brig {
    pub executable: PathBuf,
    pub environment: BTreeMap<String, String>,
}

impl Brig {
    pub fn new(settings: &Settings) -> Result<Self, String> {
        if crate::hull::executable().is_none() {
            return Err("The app runtime helper is missing. Reinstall Tugboat.".into());
        }
        Ok(Self {
            executable: settings.brig()?,
            environment: settings.environment(),
        })
    }

    pub fn run(
        &self,
        arguments: &[String],
        directory: Option<&std::path::Path>,
    ) -> Result<command::CommandOutput, String> {
        command::run(
            &self.executable,
            arguments,
            directory,
            &self.environment,
            Duration::from_secs(30),
            &AtomicBool::new(false),
        )
    }

    pub fn checked(
        &self,
        arguments: &[String],
        directory: Option<&std::path::Path>,
    ) -> Result<command::CommandOutput, String> {
        let output = self.run(arguments, directory)?;
        if output.cancelled
            || output.timed_out
            || output.output_exceeded
            || output.exit_code != Some(0)
        {
            let detail: String = output.stderr.chars().take(2000).collect();
            return Err(format!("Brig operation failed. {detail}"));
        }
        Ok(output)
    }

    pub fn json(
        &self,
        arguments: &[String],
        kind: &str,
        directory: Option<&std::path::Path>,
    ) -> Result<Value, String> {
        host::envelope(&self.checked(arguments, directory)?, kind)
    }

    pub fn plan(&self, project: &Project, settings: &Settings) -> Result<Value, String> {
        let mut arguments = vec!["--json".into(), "plan".into()];
        arguments.extend(settings.arguments());
        arguments.extend([
            project.reference(),
            project.path.to_string_lossy().into_owned(),
        ]);
        self.json(&arguments, "Plan", Some(&project.path))
    }

    pub fn policy(&self, name: &str) -> Result<Option<Value>, String> {
        let output = self.run(
            &["policy".into(), "show".into(), name.into(), "--json".into()],
            None,
        )?;
        if output.timed_out || output.cancelled || output.output_exceeded {
            return Err("Could not read the policy. Try again.".into());
        }
        if output.exit_code != Some(0) {
            if output.stderr.contains("unknown policy") {
                return Ok(None);
            }
            return Err("Could not read the Brig policy directory. Run the Mac check.".into());
        }
        let document: Value = serde_json::from_str(&output.stdout)
            .map_err(|_| "The policy response is not valid JSON.")?;
        if document["apiVersion"] != "brig.sh/v1alpha1"
            || document["kind"] != "Policy"
            || document["metadata"]["name"] != name
            || !document["spec"].is_object()
        {
            return Err("The policy document has an unsupported version or name.".into());
        }
        Ok(Some(document))
    }

    pub fn validate_codex_profile(&self, name: &str) -> Result<(), String> {
        let output = self.checked(
            &["agent".into(), "show".into(), name.into(), "--json".into()],
            None,
        )?;
        let profile: Value =
            serde_json::from_str(&output.stdout).map_err(|_| "Could not read the Brig profile.")?;
        if profile["name"] != name || profile["binary"] != "codex" || profile["kind"] != "agent" {
            return Err("Select an agent profile that runs Codex.".into());
        }
        Ok(())
    }
}

pub fn fingerprint(value: &Value) -> Result<String, String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(|error| error.to_string())?)
    ))
}

pub fn validate_plan(plan: &Value, project: &Project, settings: &Settings) -> Result<(), String> {
    let data = &plan["data"];
    crate::projects::validate_guest_path(
        data["project"]["guest"]
            .as_str()
            .ok_or("The guest project path is missing.")?,
    )?;
    if plan["apiVersion"] != "brig.sh/v1alpha1" || plan["kind"] != "Plan" {
        return Err("The Brig plan version is unsupported.".into());
    }
    if data.get("refused").is_some_and(|value| !value.is_null()) {
        return Err("Brig refused this plan. Check the policy and runtime.".into());
    }
    if data["runtime"]["available"] != true
        || data["runtime"]["kind"] != "hull"
        || data["runtime"]["backend"] != "hvi"
    {
        return Err("This plan requires an available hull/hvi runtime.".into());
    }
    if data["profile"] != project.profile
        || data["session"] != project.label
        || data["project"]["host"].as_str() != project.path.to_str()
        || data["project"]["mode"] != "read-write"
    {
        return Err("The Brig plan project or session does not match the selected folder.".into());
    }
    if data["posture"] != settings.network_name()
        || data["limits"]["cpus"] != settings.cpus
        || data["limits"]["mem"] != settings.memory_mb
    {
        return Err("The Brig plan network or resource limits do not match the settings.".into());
    }
    let verification =
        serde_json::to_value(&settings.verification).map_err(|error| error.to_string())?;
    if data["verify"]["mode"] != verification {
        return Err("Image verification was not applied to the Brig plan.".into());
    }
    if data["noPolicy"] != false || data["egress"]["default"] != "deny" {
        return Err("Attach a network policy that denies access by default.".into());
    }
    let policy = project
        .policy
        .as_ref()
        .ok_or("The project policy binding is missing.")?;
    let policies = data["policies"]
        .as_array()
        .ok_or("The plan policy list is missing.")?;
    if !policies.iter().any(|name| name == &policy.name) {
        return Err("The selected policy is missing from the Brig plan.".into());
    }
    let credentials = data["credentials"]
        .as_array()
        .ok_or("The plan credential list is missing.")?;
    if credentials
        .iter()
        .any(|credential| credential["state"] != "unresolved" && credential["state"] != "withheld")
    {
        return Err(
            "The plan forwards a host credential to the guest. Check the settings and profile."
                .into(),
        );
    }
    if !data["skills"].as_array().is_some_and(Vec::is_empty) {
        return Err("The plan includes unexpected host skills.".into());
    }
    if !data["ports"].as_array().is_some_and(Vec::is_empty) {
        return Err("This session has published host ports. Unpublish them in Brig first.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projects::{Policy, Registry};
    #[test]
    fn successful_refused_and_mismatched_plans_are_checked_by_fields() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/brig/v0.5.2/ready.json"))
                .unwrap();
        let mut plan: Value = serde_json::from_str(fixture["stdout"].as_str().unwrap()).unwrap();
        let directory = std::env::temp_dir().join(format!("brig-plan-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let mut registry = Registry::load(&directory.join("registry.json")).unwrap();
        let mut project = registry.register(&directory, "codex").unwrap();
        project.label = "bd01-fixture".into();
        project.policy = Some(Policy {
            name: "bd01-fixture".into(),
            owned: true,
            fingerprint: None,
        });
        plan["data"]["project"]["host"] = project.path.to_string_lossy().into_owned().into();
        plan["data"]["project"]["guest"] = "/work/sample-project".into();
        let settings = Settings {
            verification: crate::settings::Verification::Warn,
            ..Settings::default()
        };
        assert!(validate_plan(&plan, &project, &settings).is_ok());
        plan["data"]["project"]["guest"] = "/work/sample project".into();
        assert!(validate_plan(&plan, &project, &settings).is_ok());
        let mut unsupported = plan.clone();
        unsupported["data"]["project"]["guest"] = "/work/sample\tproject".into();
        assert!(validate_plan(&unsupported, &project, &settings).is_err());
        for (field, value) in [
            ("refused", serde_json::json!({"reason":"denied"})),
            (
                "credentials",
                serde_json::json!([{"name":"OPENAI_API_KEY","state":"resolved"}]),
            ),
            ("posture", serde_json::json!("shared")),
        ] {
            let mut changed = plan.clone();
            changed["data"][field] = value;
            assert!(validate_plan(&changed, &project, &settings).is_err());
        }
        plan["data"]["project"]["host"] = "/another/project".into();
        assert!(validate_plan(&plan, &project, &settings).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
