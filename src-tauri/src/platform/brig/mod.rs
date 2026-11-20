use crate::{desktop::settings::Settings, platform::command, platform::host, projects::Project};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
mod keychain_scope;
mod paths;
mod shared;
pub use paths::PlanPaths;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct Brig {
    pub executable: PathBuf,
    pub environment: BTreeMap<String, String>,
}

impl Brig {
    pub fn new(settings: &Settings) -> Result<Self, String> {
        Ok(Self {
            executable: settings.brig()?,
            environment: settings.environment(),
        })
    }

    pub fn for_project(
        &self,
        project: &Project,
        profiles: &std::path::Path,
    ) -> Result<Self, String> {
        if !crate::environment::managed_profile::shared(project) {
            return Err("The project does not use the managed shared Tugboat profile.".into());
        }
        let mut brig = self.with_shared_profile(profiles)?;
        brig.environment.extend([
            ("BRIG_VERIFY".into(), "require".into()),
            ("BRIG_IMAGE".into(), project.image_ref.clone()),
        ]);
        Ok(brig)
    }

    pub(crate) fn with_shared_profile(&self, profiles: &std::path::Path) -> Result<Self, String> {
        crate::environment::managed_profile::install(profiles)?;
        let mut brig = self.clone();
        brig.environment.insert(
            "BRIG_PROFILE_DIR".into(),
            profiles.to_string_lossy().into_owned(),
        );
        Ok(brig)
    }

    pub(crate) fn for_registered_project(
        &self,
        project: &Project,
        records: &std::path::Path,
    ) -> Result<Self, String> {
        let mut brig = self.for_project(project, &records.join("profiles"))?;
        if project.github_access.is_some() {
            brig.enable_host_keychain(records)?;
        }
        Ok(brig)
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
            COMMAND_TIMEOUT,
            &AtomicBool::new(false),
        )
    }

    pub fn checked(
        &self,
        arguments: &[String],
        directory: Option<&std::path::Path>,
    ) -> Result<command::CommandOutput, String> {
        let output = self.run(arguments, directory)?;
        check_completion(&output)?;
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
        arguments.extend(self.project_arguments(project, settings)?);
        let plan = self.json(&arguments, "Plan", Some(&project.path))?;
        self.verify_shared_plan(project, &plan)?;
        Ok(plan)
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

    pub fn agent_profile(&self, name: &str) -> Result<Value, String> {
        let output = self.checked(
            &["agent".into(), "show".into(), name.into(), "--json".into()],
            None,
        )?;
        let profile: Value =
            serde_json::from_str(&output.stdout).map_err(|_| "Could not read the Brig profile.")?;
        Ok(profile)
    }
}

fn check_completion(output: &command::CommandOutput) -> Result<(), String> {
    if output.cancelled {
        return Err("The Brig command was cancelled.".into());
    }
    if output.timed_out {
        return Err(format!(
            "Brig did not respond within {} seconds.",
            COMMAND_TIMEOUT.as_secs()
        ));
    }
    if output.output_exceeded {
        return Err("Brig output exceeded the size limit.".into());
    }
    if output.exit_code != Some(0) {
        let detail: String = output.stderr.chars().take(2000).collect();
        if !detail.trim().is_empty() {
            return Err(format!("Brig operation failed. {detail}"));
        }
        return Err(match output.exit_code {
            Some(code) => format!("Brig exited with status {code}."),
            None => "Brig terminated without an exit status.".into(),
        });
    }
    Ok(())
}

pub fn fingerprint(value: &Value) -> Result<String, String> {
    Ok(crate::hash::hex(Sha256::digest(
        serde_json::to_vec(value).map_err(|error| error.to_string())?,
    )))
}

pub(crate) const MISSING_PROJECT_POLICY: &str =
    "The selected policy is missing from the Brig plan.";

pub fn validate_plan(
    plan: &Value,
    project: &Project,
    settings: &Settings,
) -> Result<PlanPaths, String> {
    validate_plan_context(plan, project, settings)?;
    let data = &plan["data"];
    let policy = project
        .policy
        .as_ref()
        .ok_or("The project policy binding is missing.")?;
    let policies = checked_policy_bindings(plan, project)?;
    let grant = crate::projects::project_github_access::approved_policy(project);
    if !policies.iter().any(|name| name == &policy.name) {
        return Err(MISSING_PROJECT_POLICY.into());
    }
    if data["noPolicy"] != false || data["egress"]["default"] != "deny" {
        return Err("Attach a network policy that denies access by default.".into());
    }
    if grant.is_some_and(|grant| policies.len() != 2 || !policies.iter().any(|name| name == grant))
    {
        return Err("The GitHub access policy is missing from the Brig plan.".into());
    }
    PlanPaths::from_plan(plan)
}

pub(crate) fn checked_policy_bindings<'a>(
    plan: &'a Value,
    project: &Project,
) -> Result<&'a [Value], String> {
    let policies = plan["data"]["policies"]
        .as_array()
        .ok_or("The plan policy list is missing.")?;
    let expected = [
        project.policy.as_ref().map(|policy| policy.name.as_str()),
        crate::projects::project_github_access::approved_policy(project),
    ];
    if policies.len() > expected.iter().flatten().count()
        || policies.iter().enumerate().any(|(index, name)| {
            policies[..index].contains(name)
                || name
                    .as_str()
                    .is_none_or(|name| !expected.contains(&Some(name)))
        })
    {
        return Err("Brig reports unexpected policies for this session. Review the policy bindings before retrying setup.".into());
    }
    Ok(policies)
}

pub fn validate_plan_context(
    plan: &Value,
    project: &Project,
    settings: &Settings,
) -> Result<(), String> {
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
    if data["verify"]["mode"] != "require"
        || data["verify"]["policy"] != "brig"
        || data["image"]["ref"] != project.image_ref
    {
        return Err(
            "This project requires its pinned image and the default Brig signature policy.".into(),
        );
    }
    let credentials = data["credentials"]
        .as_array()
        .ok_or("The plan credential list is missing.")?;
    if !credentials.is_empty() {
        return Err("The plan contains an unexpected host credential source.".into());
    }
    if !data["skills"].as_array().is_some_and(Vec::is_empty) {
        return Err("The plan includes unexpected host skills.".into());
    }
    crate::projects::project_preview::validate_plan_ports(project, &data["ports"])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projects::{Policy, Registry};
    #[test]
    fn a_command_timeout_with_empty_stderr_reports_the_time_limit() {
        let output = command::CommandOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            cancelled: false,
            timed_out: true,
            output_exceeded: false,
        };
        assert_eq!(
            check_completion(&output).unwrap_err(),
            "Brig did not respond within 30 seconds."
        );
    }

    #[test]
    fn successful_refused_and_mismatched_plans_are_checked_by_fields() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/brig/v0.5.2/ready.json"
        ))
        .unwrap();
        let mut plan: Value = serde_json::from_str(fixture["stdout"].as_str().unwrap()).unwrap();
        let directory = std::env::temp_dir().join(format!("brig-plan-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let mut registry = Registry::load(&directory.join("registry.json")).unwrap();
        let mut project = registry.register(&directory).unwrap();
        project.policy = Some(Policy {
            name: "bd01-fixture".into(),
            owned: true,
            fingerprint: None,
        });
        plan["data"]["project"]["host"] = project.path.to_string_lossy().into_owned().into();
        plan["data"]["project"]["guest"] = "/work/sample-project".into();
        plan["data"]["profile"] = serde_json::json!(project.profile);
        plan["data"]["session"] = serde_json::json!(project.label);
        plan["data"]["verify"]["mode"] = serde_json::json!("require");
        plan["data"]["image"]["ref"] = serde_json::json!(project.image_ref);
        plan["data"]["credentials"] = serde_json::json!([]);
        let settings = Settings::default();
        assert!(validate_plan(&plan, &project, &settings).is_ok());
        let mut unexpected = plan.clone();
        unexpected["data"]["policies"] = serde_json::json!(["bd01-fixture", "old-policy"]);
        assert!(validate_plan(&unexpected, &project, &settings)
            .unwrap_err()
            .contains("unexpected policies"));
        let mut missing = plan.clone();
        missing["data"]["policies"] = serde_json::json!([]);
        assert_eq!(
            validate_plan(&missing, &project, &settings).unwrap_err(),
            "The selected policy is missing from the Brig plan."
        );
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
