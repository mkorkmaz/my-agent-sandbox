use super::{Grant, Phase};
use crate::{
    desktop::settings::Settings,
    platform::brig::{Brig, PlanPaths},
    projects::{
        project_policy::{self, PolicyEditor},
        project_preview, Project,
    },
};
use serde_json::Value;

const REVIEW: &str =
    "The GitHub access policy changed outside Tugboat. Review it before continuing.";

pub(super) struct Scope {
    pub(super) running: bool,
    pub(super) sandbox: String,
    plan: Value,
}

impl Scope {
    pub(super) fn read(
        brig: &Brig,
        project: &Project,
        settings: &Settings,
    ) -> Result<Self, String> {
        let plan = brig
            .plan(project, settings)
            .map_err(|_| "Could not check the selected Brig session.")?;
        let data = &plan["data"];
        if data["runtime"]["kind"] != "hull"
            || data["runtime"]["backend"] != "hvi"
            || data["runtime"]["available"] != true
        {
            return Err("The Hull runtime is unavailable. Check Mac setup and retry.".into());
        }
        if data["profile"] != project.profile
            || data["session"] != project.label
            || data["project"]["host"].as_str() != project.path.to_str()
        {
            return Err("Brig reported a different project or session. Review the project before continuing.".into());
        }
        let sandbox = data["sandbox"]
            .as_str()
            .filter(|name| !name.is_empty())
            .ok_or("The Brig plan is missing its sandbox name.")?;
        let paths = PlanPaths::from_plan(&plan)?;
        let running = project_preview::verify_sandbox(brig, project, sandbox, &paths.host_home)?
            == Some(true);
        Ok(Self {
            running,
            sandbox: sandbox.into(),
            plan,
        })
    }

    pub(super) fn observe(
        &self,
        brig: &Brig,
        project: &Project,
        grant: &Grant,
    ) -> Result<(bool, bool), String> {
        let attached = self.is_attached(&grant.name)?;
        let policy = brig.policy(&grant.name).map_err(|_| REVIEW)?;
        if let Some(document) = &policy {
            if !grant.matches_policy(document) {
                return Err(REVIEW.into());
            }
            let list = brig
                .checked(&["policy".into(), "ls".into()], None)
                .map_err(|_| REVIEW)?;
            if !list.stderr.is_empty()
                || !project_policy::sole_binding(&list.stdout, &grant.name, project)
            {
                return Err(REVIEW.into());
            }
        }
        if attached && policy.is_none() {
            return Err(REVIEW.into());
        }
        check_routes(&self.plan["data"], attached)?;
        Ok((policy.is_some(), attached))
    }

    pub(super) fn verify_no_route(&self) -> Result<(), String> {
        check_routes(&self.plan["data"], false)
    }

    pub(super) fn keychain_available(&self) -> bool {
        verify_keychain_resolution(&self.plan).is_ok()
    }

    pub(super) fn require_stopped(&self) -> Result<(), String> {
        if self.running {
            return Err("Stop the project environment to finish the GitHub access change.".into());
        }
        Ok(())
    }

    fn is_attached(&self, name: &str) -> Result<bool, String> {
        let policies = self.plan["data"]["policies"].as_array().ok_or(REVIEW)?;
        Ok(policies.iter().any(|policy| policy == name))
    }
}

fn check_routes(data: &Value, attached: bool) -> Result<(), String> {
    if attached && data["broker"]["routes"] == serde_json::json!(["github"]) {
        return Ok(());
    }
    if !attached && data["broker"].is_null() {
        return Ok(());
    }
    Err(REVIEW.into())
}

pub(crate) fn verify_plan(brig: &Brig, project: &Project, plan: &Value) -> Result<(), String> {
    let Some(grant) = &project.github_access else {
        return check_routes(&plan["data"], false);
    };
    if grant.phase != Phase::Saved {
        return Err(
            "Finish the GitHub access change in Project settings before preparing the environment."
                .into(),
        );
    }
    let scope = Scope {
        running: false,
        sandbox: String::new(),
        plan: plan.clone(),
    };
    if scope.observe(brig, project, grant)? != (true, true) {
        return Err(REVIEW.into());
    }
    verify_keychain_resolution(plan)
}

pub(super) fn verify_keychain_resolution(plan: &Value) -> Result<(), String> {
    let broker = &plan["data"]["broker"];
    if !broker["storeError"].is_null()
        || !broker["refused"].is_null()
        || broker["missing"]
            .as_array()
            .is_some_and(|missing| !missing.is_empty())
    {
        return Err("Brig could not resolve the GitHub token from the Mac Keychain. Review the grant and retry.".into());
    }
    Ok(())
}

pub(super) fn create(brig: &Brig, grant: &Grant) -> Result<(), String> {
    let editor = PolicyEditor::create_document(grant.source().to_string().as_bytes())
        .map_err(|_| "Could not prepare the GitHub fetch policy.")?;
    let mut editing = brig.clone();
    for key in ["EDITOR", "VISUAL"] {
        editing
            .environment
            .insert(key.into(), editor.executable.to_string_lossy().into_owned());
    }
    editing.environment.insert(
        "BRIG_DESKTOP_POLICY_SOURCE".into(),
        editor.source.to_string_lossy().into_owned(),
    );
    editing
        .checked(
            &["policy".into(), "create".into(), grant.name.clone()],
            None,
        )
        .map_err(|_| "Brig could not create the GitHub fetch policy.")?;
    if !brig
        .policy(&grant.name)
        .map_err(|_| REVIEW)?
        .is_some_and(|document| grant.matches_policy(&document))
    {
        return Err(REVIEW.into());
    }
    Ok(())
}

pub(super) fn bind(
    brig: &Brig,
    project: &Project,
    grant: &Grant,
    action: &str,
) -> Result<(), String> {
    brig.checked(
        &[
            "policy".into(),
            action.into(),
            grant.name.clone(),
            project.profile.clone(),
            "-n".into(),
            project.label.clone(),
        ],
        None,
    )
    .map_err(|_| "Brig could not confirm the GitHub access policy change.")?;
    Ok(())
}

pub(super) fn remove(brig: &Brig, grant: &Grant) -> Result<(), String> {
    brig.checked(&["policy".into(), "rm".into(), grant.name.clone()], None)
        .map_err(|_| REVIEW)?;
    Ok(())
}
