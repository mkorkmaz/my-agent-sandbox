use super::{replay, root, state::NamedHome};
use crate::{
    desktop::settings::Settings,
    environment::{self, managed_profile, shared_runtime, CheckedProject},
    platform::{
        brig::{self, Brig},
        command::{self, CommandOutput},
    },
    projects::{self, Project},
    storage::data_location,
};
use serde::Deserialize;
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Duration,
};

const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
const UPDATE_TIMEOUT: Duration = Duration::from_secs(720);
const SIGNER: &str = "https://github.com/brig-sh/community-images/.github/workflows/build-images.yml@refs/heads/main";
const ISSUER: &str = "https://token.actions.githubusercontent.com";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Instance {
    pub(super) id: String,
    image_digest: String,
    pub(super) status: String,
    backend: String,
    bundle_dir: PathBuf,
    #[serde(rename = "cmdLine")]
    command_line: Vec<String>,
}

#[derive(Clone)]
pub(super) struct Runtime<'a> {
    pub(super) brig: Brig,
    pub(super) settings: Settings,
    pub(super) project: &'a Project,
    pub(super) protected: Vec<PathBuf>,
    pub(super) hull: PathBuf,
    pub(super) cosign: PathBuf,
    pub(super) debugfs: PathBuf,
    pub(super) e2fsck: PathBuf,
}

fn complete(output: CommandOutput, message: &str) -> Result<CommandOutput, String> {
    if output.timed_out {
        return Err("The image operation timed out.".into());
    }
    if output
        .stderr
        .to_ascii_lowercase()
        .contains("no space left on device")
    {
        return Err("The image operation needs more free disk space.".into());
    }
    if output.cancelled || output.output_exceeded || output.exit_code != Some(0) {
        return Err(message.into());
    }
    Ok(output)
}

impl Runtime<'_> {
    fn tool(
        &self,
        executable: &Path,
        arguments: &[String],
        timeout: Duration,
        brig: &Brig,
    ) -> Result<CommandOutput, String> {
        command::run(
            executable,
            arguments,
            Some(&self.project.path),
            &brig.environment,
            timeout,
            &AtomicBool::new(false),
        )
        .map_err(|_| "Could not start the bundled image tool.".into())
    }

    pub(super) fn plan(&self, image: &str) -> Result<(CheckedProject, NamedHome), String> {
        let mut project = self.project.clone();
        project.image_ref = image.into();
        project.image_update = None;
        project.stage = projects::ProjectStage::Ready;
        let mut brig = self.brig.clone();
        brig.environment.insert("BRIG_IMAGE".into(), image.into());
        let definition = brig.agent_profile(&project.profile)?;
        managed_profile::validate(&project, &definition)?;
        let policy = project
            .policy
            .as_ref()
            .ok_or("The project network policy is missing.")?;
        let document = brig
            .policy(&policy.name)?
            .ok_or("The project network policy was removed.")?;
        if project.policy_detached
            || policy.fingerprint.as_deref() != Some(brig::fingerprint(&document)?.as_str())
        {
            return Err(
                "Review the current project network policy before updating the image.".into(),
            );
        }
        let plan = brig.plan(&project, &self.settings)?;
        let paths = brig::validate_plan(&plan, &project, &self.settings)?;
        projects::project_github_access::verify_plan(&brig, &project, &plan)?;
        data_location::require_private_plan_records(&self.protected, &plan)?;
        let home = NamedHome::read(&paths.host_home)?;
        let fingerprint = environment::plan_fingerprint(&plan, &project, &definition)?;
        Ok((
            CheckedProject {
                project,
                brig,
                settings: self.settings.clone(),
                plan,
                paths,
                fingerprint,
            },
            home,
        ))
    }

    pub(super) fn instance(&self, expected: &CheckedProject) -> Result<Instance, String> {
        let sandbox = self.sandbox();
        if expected.plan["data"]["sandbox"] != sandbox {
            return Err("Brig reported an invalid Hull instance name.".into());
        }
        let output = complete(self.tool(&self.hull, &["inspect".into(), sandbox.clone()], QUERY_TIMEOUT, &expected.brig)?,
            "Could not verify the current Hull instance. Prepare this project before checking its image.")?;
        let instance: Instance = serde_json::from_str(&output.stdout)
            .map_err(|_| "The current Hull image could not be verified.")?;
        let image = expected
            .project
            .image_ref
            .split_once('@')
            .ok_or("The project image is invalid.")?
            .1;
        if instance.id != sandbox
            || instance.backend != "hvi"
            || instance.image_digest != image
            || !matches!(instance.status.as_str(), "running" | "stopped")
        {
            return Err("Hull reports a different image or instance state.".into());
        }
        Ok(instance)
    }

    fn sandbox(&self) -> String {
        format!("brig-{}-{}", self.project.profile, self.project.label)
    }

    fn store(&self) -> Result<PathBuf, String> {
        Ok(self.brig.runtime_home()?.join(".hull/store"))
    }

    pub(super) fn backup_path(&self) -> Result<PathBuf, String> {
        Ok(root::backup_path(&self.store()?, &self.sandbox()))
    }

    pub(super) fn upper(&self, instance: &Instance) -> Result<PathBuf, String> {
        stopped_upper(&self.store()?, instance)
    }

    pub(super) fn stage(&self, image: &str) -> Result<(), String> {
        let verify = [
            "verify",
            "--certificate-identity",
            SIGNER,
            "--certificate-oidc-issuer",
            ISSUER,
            image,
        ];
        complete(
            self.tool(
                &self.cosign,
                &verify.map(String::from),
                UPDATE_TIMEOUT,
                &self.brig,
            )?,
            "The candidate image did not pass the signature check.",
        )?;
        let pull = ["pull", "--platform", "linux/arm64", image];
        complete(
            self.tool(
                &self.hull,
                &pull.map(String::from),
                UPDATE_TIMEOUT,
                &self.brig,
            )?,
            "Could not download the verified guest image.",
        )?;
        Ok(())
    }

    pub(super) fn flush(&self, checked: &CheckedProject) -> Result<(), String> {
        if !environment::is_running(&checked.brig, &checked.project, &checked.paths)? {
            return Err("The reviewed guest stopped before its files could be saved.".into());
        }
        checked
            .brig
            .checked(
                &shared_runtime::exec(&checked.project, "/usr/bin/sync", &[]),
                None,
            )
            .map_err(|_| "Could not flush the guest files before stopping it.".to_owned())?;
        Ok(())
    }

    pub(super) fn replay(&self, for_boot: bool) -> Result<replay::Disk, String> {
        replay::Disk::prepare(
            &self.backup_path()?,
            &self.e2fsck,
            &self.debugfs,
            &self.brig.environment,
            for_boot,
        )
    }

    pub(super) fn stop(&self) -> Result<(), String> {
        if !environment::listed_as_running(&self.brig, self.project)? {
            return Ok(());
        }
        let output = self
            .brig
            .run(&["stop".into(), self.project.reference()], None)?;
        complete(output, "Brig could not stop the project guest.")?;
        Ok(())
    }

    pub(super) fn boot(
        &self,
        checked: &CheckedProject,
        restore: bool,
    ) -> Result<shared_runtime::Versions, String> {
        crate::platform::hull::check_instance_socket_paths(
            self.brig.runtime_home()?,
            &self.sandbox(),
        )?;
        let mut brig = checked.brig.clone();
        if restore {
            let (_, digest) = checked
                .project
                .image_ref
                .split_once('@')
                .ok_or("The previous image is invalid.")?;
            brig.environment
                .insert("HULL_TUGBOAT_RESTORE_IMAGE".into(), digest.into());
            brig.environment
                .insert("HULL_TUGBOAT_RESTORE_INSTANCE".into(), self.sandbox());
        }
        let mut arguments = vec!["--verbose".into(), "run".into(), "-d".into()];
        arguments.extend(brig.project_arguments(&checked.project, &self.settings)?);
        let output = complete(
            self.tool(&brig.executable, &arguments, UPDATE_TIMEOUT, &brig)?,
            "Could not start the verified project image.",
        )?;
        if !output.stderr.contains("image and boot assets verified") {
            return Err("Brig did not confirm the image and boot asset signatures.".into());
        }
        if restore
            && !output
                .stderr
                .contains("restored reviewed Tugboat writable system disk")
        {
            return Err("Hull did not confirm the previous writable system disk. The backup is ready for recovery.".into());
        }
        if self.instance(checked)?.status != "running"
            || !environment::is_running(&checked.brig, &checked.project, &checked.paths)?
        {
            return Err("Hull is not running the reviewed signed image.".into());
        }
        shared_runtime::verify_versions(checked)
    }
}

pub(super) fn scope(plan: &Value) -> Result<String, String> {
    let mut data = plan["data"]
        .as_object()
        .ok_or("Could not verify the image replacement scope.")?
        .clone();
    for key in ["image", "digest", "network"] {
        data.remove(key);
    }
    brig::fingerprint(&Value::Object(data))
}

fn stopped_upper(store: &Path, instance: &Instance) -> Result<PathBuf, String> {
    if instance.status != "stopped" {
        return Err("Stop the guest before reviewing its writable system files.".into());
    }
    let directory = store.join("instances").join(&instance.id);
    let upper = directory.join("rootfs-upper.ext4");
    let descriptor = format!(
        "path={},serial=disk1",
        upper
            .to_str()
            .ok_or("The writable system disk path is invalid.")?
    );
    if instance.bundle_dir != directory.join("bundle")
        || !instance
            .command_line
            .windows(2)
            .any(|words| words[0] == "--disk" && words[1] == descriptor)
    {
        return Err("Hull reports an unsupported writable system disk.".into());
    }
    Ok(upper)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_only_the_exact_writable_hull_disk_descriptor() {
        let store = Path::new("/private/tmp/tugboat proof");
        let id = "brig-tugboat-project-0123456789abcdef";
        let directory = store.join("instances").join(id);
        let disk = directory.join("rootfs-upper.ext4");
        let mut instance = Instance {
            id: id.into(),
            status: "stopped".into(),
            backend: "hvi".into(),
            image_digest: "synthetic-digest".into(),
            bundle_dir: directory.join("bundle"),
            command_line: vec![
                "--disk".into(),
                format!("path={},serial=disk1", disk.display()),
            ],
        };
        assert_eq!(stopped_upper(store, &instance).unwrap(), disk);
        instance.status = "running".into();
        assert!(stopped_upper(store, &instance).is_err());
        instance.status = "stopped".into();
        for descriptor in [
            disk.to_string_lossy().into_owned(),
            format!("path={},serial=disk0", disk.display()),
            format!(
                "path={},serial=disk1",
                directory.join("unrelated.ext4").display()
            ),
        ] {
            instance.command_line[1] = descriptor;
            assert!(stopped_upper(store, &instance).is_err());
        }
    }
}
