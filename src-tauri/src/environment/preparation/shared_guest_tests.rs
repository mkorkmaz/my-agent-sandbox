use super::{prepare, Settings};
use crate::{
    agents::{AgentCommand, AgentKind},
    environment::{self, shared_runtime},
    platform::{brig::Brig, command},
    projects::Registry,
    storage,
};
use std::{
    fs::{self, DirBuilder},
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Duration,
};

struct Guest {
    brig: Brig,
    reference: String,
}

impl Drop for Guest {
    fn drop(&mut self) {
        let _ = self
            .brig
            .run(&["stop".into(), self.reference.clone()], None);
        let _ = self.brig.run(&["rm".into(), self.reference.clone()], None);
    }
}

impl Guest {
    fn run(&self, project: &crate::projects::Project, program: &str, args: &[&str]) -> String {
        self.brig
            .checked(&shared_runtime::exec(project, program, args), None)
            .unwrap()
            .stdout
            .trim()
            .to_owned()
    }

    fn boot(&self, checked: &environment::CheckedProject) {
        let mut args = environment::run_arguments(checked, &["--version".into()]).unwrap();
        args.insert(0, "--verbose".into());
        let output = command::run(
            &self.brig.executable,
            &args,
            Some(&checked.project.path),
            &self.brig.environment,
            Duration::from_secs(720),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(output.exit_code, Some(0), "{}", output.stderr);
        assert!(
            output.stderr.contains("image and boot assets verified"),
            "{}",
            output.stderr
        );
    }
}

fn private(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    DirBuilder::new().mode(0o700).create(&path).unwrap();
    path
}

#[test]
#[ignore = "Requires Apple silicon HVI and access to the public signed runtime assets"]
fn production_shared_guest_boot_and_reopen() {
    let root = PathBuf::from(
        std::env::var_os("TUGBOAT_SHARED_GUEST_TEST_ROOT")
            .expect("Set TUGBOAT_SHARED_GUEST_TEST_ROOT to a new private disposable directory"),
    );
    assert_eq!(fs::canonicalize(&root).unwrap(), root);
    assert!(root.starts_with("/private/tmp"));
    let metadata = fs::symlink_metadata(&root).unwrap();
    assert!(
        metadata.is_dir()
            && metadata.mode() & 0o077 == 0
            && metadata.uid() == unsafe { libc::geteuid() }
    );
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    let settings = Settings::default();
    let sample = private(&root, "sample project");
    let records = private(&root, "records");
    let file = records.join("projects.json");
    let mut registry = Registry::default();
    let project = registry.register(&sample).unwrap();
    storage::write(&file, &registry).unwrap();
    let mut brig = Brig::new(&settings).unwrap();
    for (name, directory) in [
        ("HOME", "h"),
        ("BRIG_STATE_DIR", "state"),
        ("BRIG_POLICY_DIR", "policy"),
        ("XDG_CONFIG_HOME", "config"),
        ("TMPDIR", "tmp"),
    ] {
        brig.environment.insert(
            name.into(),
            private(&root, directory).to_string_lossy().into_owned(),
        );
    }
    let guest = Guest {
        brig: brig.for_project(&project, &root.join("profiles")).unwrap(),
        reference: project.reference(),
    };
    let prepared = prepare(&guest.brig, &file, &settings, &project.id).unwrap();
    let policy = prepared.project.policy.as_ref().unwrap();
    let definition = guest.brig.policy(&policy.name).unwrap().unwrap();
    assert!(super::matches_template(
        &definition,
        &super::allowed_hosts(&settings)
    ));
    let checked = environment::check_environment(
        guest.brig.clone(),
        prepared.project,
        settings.clone(),
        prepared.fingerprint.as_deref(),
        std::slice::from_ref(&records),
    )
    .unwrap();
    shared_runtime::install_codex(&checked).unwrap();
    guest.boot(&checked);
    assert!(environment::is_running(&guest.brig, &checked.project, &checked.paths).unwrap());
    shared_runtime::verify_versions(&checked).unwrap();
    let opened = Registry::load(&file).unwrap();
    let [codex, claude] = opened.agent_views(&project.id).unwrap();
    assert_eq!(codex.environment_id, claude.environment_id);
    assert_eq!(codex.reference(), claude.reference());
    let codex = environment::check_environment(
        guest.brig.clone(),
        codex,
        settings.clone(),
        Some(&checked.fingerprint),
        &[],
    )
    .unwrap();
    let claude = environment::check_environment(
        guest.brig.clone(),
        claude,
        settings,
        Some(&checked.fingerprint),
        &[],
    )
    .unwrap();
    assert_eq!(codex.paths, claude.paths);
    let boot = guest.run(
        &codex.project,
        "/usr/bin/cat",
        &["/proc/sys/kernel/random/boot_id"],
    );
    for (kind, checked) in [(AgentKind::Codex, &codex), (AgentKind::ClaudeCode, &claude)] {
        let args = shared_runtime::interactive(
            &checked.project,
            kind.command(AgentCommand::Version).unwrap(),
        );
        let version = guest
            .brig
            .checked(&args, Some(&checked.project.path))
            .unwrap();
        kind.validate_version(&version.stdout).unwrap();
        assert_eq!(
            guest.run(
                &checked.project,
                "/usr/bin/cat",
                &["/proc/sys/kernel/random/boot_id"]
            ),
            boot
        );
    }
    assert_eq!(guest.run(&codex.project, "/usr/bin/bash", &["-ec", r#"printf project > "$BRIG_PROJECT/native-marker"; printf home > "$HOME/native-marker""#]), "");
    assert_eq!(
        guest.run(
            &claude.project,
            "/usr/bin/bash",
            &[
                "-ec",
                r#"cat "$BRIG_PROJECT/native-marker"; cat "$HOME/native-marker""#
            ]
        ),
        "projecthome"
    );
    let list = guest
        .brig
        .json(&["--json".into(), "ls".into()], "SandboxList", None)
        .unwrap();
    assert_eq!(list["data"]["sandboxes"].as_array().unwrap().len(), 1);
    crate::projects::operations::shutdown::stop_registered(
        &brig,
        &file,
        &mut Registry::load(&file).unwrap(),
    )
    .unwrap();
    assert!(!environment::listed_as_running(&guest.brig, &checked.project).unwrap());

    let bwrap = checked
        .paths
        .host_home
        .join(".tugboat-codex-0.171.0/codex-resources/bwrap");
    fs::write(&bwrap, b"synthetic-corruption").unwrap();
    let archive = root.join(
        "state/tugboat-codex-0.171.0/verified/codex-package-aarch64-unknown-linux-musl.tar.gz",
    );
    fs::write(archive, b"synthetic-corruption").unwrap();
    assert!(shared_runtime::verify_codex(&checked).is_err());
    shared_runtime::install_codex(&checked).unwrap();
    shared_runtime::verify_codex(&checked).unwrap();

    guest.boot(&checked);
    shared_runtime::verify_versions(&checked).unwrap();
    assert_ne!(
        guest.run(
            &claude.project,
            "/usr/bin/cat",
            &["/proc/sys/kernel/random/boot_id"]
        ),
        boot
    );
    assert_eq!(
        guest.run(
            &claude.project,
            "/usr/bin/bash",
            &[
                "-ec",
                r#"cat "$BRIG_PROJECT/native-marker"; cat "$HOME/native-marker""#
            ]
        ),
        "projecthome"
    );
    guest
        .brig
        .checked(&["stop".into(), guest.reference.clone()], None)
        .unwrap();
    guest
        .brig
        .checked(&["rm".into(), guest.reference.clone()], None)
        .unwrap();
    environment::project_home::remove(
        &guest.brig,
        &checked.project,
        checked.paths.host_home.to_str().unwrap(),
    )
    .unwrap();
    assert!(!checked.paths.host_home.exists());
    assert_eq!(
        fs::read_to_string(sample.join("native-marker")).unwrap(),
        "project"
    );
    println!("Verified both CLI versions, one shared guest, stable uptime, repair, reboot and removal: {}", guest.reference);
}
