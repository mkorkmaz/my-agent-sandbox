use super::{Brig, PlanPaths, COMMAND_TIMEOUT};
use crate::{desktop::settings::Settings, platform::command, projects::Project};
use serde_json::Value;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;

impl Brig {
    pub(crate) fn runtime_home(&self) -> Result<&Path, String> {
        self.environment
            .get("TUGBOAT_HULL_HOME")
            .or_else(|| self.environment.get("HOME"))
            .map(Path::new)
            .filter(|path| {
                path.is_absolute() && !path.components().any(|part| part == Component::ParentDir)
            })
            .ok_or_else(|| "The Hull runtime home is unavailable.".into())
    }

    pub(crate) fn state_directory(&self) -> Result<PathBuf, String> {
        let state = self
            .environment
            .get("BRIG_STATE_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                self.environment
                    .get("HOME")
                    .map(|home| Path::new(home).join(".brig"))
            })
            .ok_or("The Brig state directory is unavailable.")?;
        if !state.is_absolute() || state.components().any(|part| part == Component::ParentDir) {
            return Err("The Brig state directory is invalid.".into());
        }
        Ok(state)
    }

    pub(crate) fn shared_project_home(&self, project: &Project) -> Result<PathBuf, String> {
        let base = self.shared_home_base(project)?;
        // Brig appends the session label to an explicit --home on named sessions.
        Ok(base.with_file_name(format!("{}-{}", project.environment_id, project.label)))
    }

    pub(super) fn shared_home_base(&self, project: &Project) -> Result<PathBuf, String> {
        project.validate()?;
        Ok(self
            .state_directory()?
            .join("tugboat-project-homes")
            .join(project.environment_id.to_string()))
    }

    pub(crate) fn project_arguments(
        &self,
        project: &Project,
        settings: &Settings,
    ) -> Result<Vec<String>, String> {
        let mut arguments = settings.arguments();
        arguments.extend([
            "--image".into(),
            project.image_ref.clone(),
            "--home".into(),
            self.shared_home_base(project)?
                .to_string_lossy()
                .into_owned(),
        ]);
        arguments.extend([
            project.reference(),
            project.path.to_string_lossy().into_owned(),
        ]);
        Ok(arguments)
    }

    pub(crate) fn verify_shared_plan(&self, project: &Project, plan: &Value) -> Result<(), String> {
        let data = &plan["data"];
        let expected_home = self.shared_project_home(project)?;
        let paths = PlanPaths::from_plan(plan)?;
        if paths.host_home != expected_home
            || data["home"]["guest"] != "/root"
            || data["home"]["mode"] != "read-write"
        {
            return Err("The Brig plan does not mount the project's persistent guest home.".into());
        }
        Ok(())
    }

    pub(crate) fn verify_running_image(
        &self,
        project: &Project,
        sandbox: &str,
    ) -> Result<(), String> {
        let runtime = self
            .environment
            .get("BRIG_RUNTIME_BIN")
            .ok_or("The bundled Hull executable is unavailable.")?;
        let output = command::run(
            Path::new(runtime),
            &["inspect".into(), sandbox.into()],
            None,
            &self.environment,
            COMMAND_TIMEOUT,
            &AtomicBool::new(false),
        )?;
        if output.exit_code != Some(0)
            || output.timed_out
            || output.cancelled
            || output.output_exceeded
        {
            return Err("Could not inspect the running Brig guest image.".into());
        }
        let image = project
            .image_ref
            .split_once('@')
            .map(|(_, digest)| digest)
            .ok_or("The project guest image digest is unavailable.")?;
        let state: Value = serde_json::from_str(&output.stdout)
            .map_err(|_| "Could not read the running Brig guest image.")?;
        if state["id"] != sandbox
            || state["status"] != "running"
            || state["backend"] != "hvi"
            || state["imageDigest"] != image
        {
            return Err("The running guest does not use this project's verified image. Stop it and prepare the project again.".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_keychain_home_leaves_hull_in_the_verified_dev_store() {
        let mut brig = Brig {
            executable: PathBuf::from("/unused"),
            environment: [("HOME".into(), "/Users/test/.tugboat-dev".into())].into(),
        };
        assert_eq!(
            brig.runtime_home().unwrap(),
            Path::new("/Users/test/.tugboat-dev")
        );
        brig.environment.insert("HOME".into(), "/Users/test".into());
        brig.environment.insert(
            "TUGBOAT_HULL_HOME".into(),
            "/Users/test/.tugboat-dev".into(),
        );
        assert_eq!(
            brig.runtime_home().unwrap(),
            Path::new("/Users/test/.tugboat-dev")
        );
        brig.environment
            .insert("TUGBOAT_HULL_HOME".into(), "../unverified".into());
        assert!(brig.runtime_home().is_err());
    }
}
