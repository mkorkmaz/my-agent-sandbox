use super::*;
use crate::{
    desktop::settings::Settings,
    environment::{preparation, shared_runtime},
    platform::{brig::Brig, command},
    storage,
};
use std::{
    fs::{self, DirBuilder},
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Duration,
};

fn proof_inputs() -> (PathBuf, PathBuf) {
    let root = PathBuf::from(
        std::env::var_os("TUGBOAT_IMAGE_TEST_ROOT")
            .expect("Set TUGBOAT_IMAGE_TEST_ROOT to a new private image test root."),
    );
    let metadata = fs::symlink_metadata(&root).unwrap();
    assert!(
        root.parent() == Some(Path::new("/private/tmp"))
            && fs::canonicalize(&root).unwrap() == root
    );
    assert!(
        metadata.is_dir()
            && metadata.mode() & 0o077 == 0
            && metadata.uid() == unsafe { libc::geteuid() }
    );
    let hull = PathBuf::from(
        std::env::var_os("TUGBOAT_IMAGE_TEST_HULL")
            .expect("Set TUGBOAT_IMAGE_TEST_HULL to the isolated Hull recovery build."),
    );
    assert!(fs::canonicalize(&hull).unwrap() == hull && hull.starts_with("/private/tmp"));
    assert!(tools::executable(&hull));
    (root, hull)
}

fn proof_settings() -> Settings {
    let mut settings = Settings::default();
    if std::env::var_os("TUGBOAT_IMAGE_TEST_TRANSCRIPT_MANIFEST").is_some() {
        super::proof_policy::configure(&mut settings);
    }
    settings
}

fn proof_brig(hull: &Path, settings: &Settings, directory: impl Fn(&str) -> String) -> Brig {
    let mut brig = Brig::new(settings).unwrap();
    brig.environment.extend([
        ("HOME".into(), directory("h")),
        ("BRIG_STATE_DIR".into(), directory("state")),
        ("BRIG_PROFILE_DIR".into(), directory("profiles")),
        ("BRIG_POLICY_DIR".into(), directory("policies")),
        ("XDG_CONFIG_HOME".into(), directory("config")),
        (
            "BRIG_RUNTIME_BIN".into(),
            hull.to_string_lossy().into_owned(),
        ),
        ("TMPDIR".into(), directory("tmp")),
    ]);
    brig
}

struct Fixture {
    root: PathBuf,
    file: PathBuf,
    brig: Brig,
    settings: Settings,
    projects: Vec<Project>,
}

impl Fixture {
    fn new() -> Self {
        let (root, hull) = proof_inputs();
        let names = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(names.iter().all(|name| name == "h" || name == "cache"));
        let directory = |name: &str| {
            let path = root.join(name);
            DirBuilder::new()
                .mode(0o700)
                .recursive(true)
                .create(&path)
                .unwrap();
            assert_eq!(fs::canonicalize(&path).unwrap(), path);
            path.to_string_lossy().into_owned()
        };
        let settings = proof_settings();
        let brig = proof_brig(&hull, &settings, |name| directory(name));
        let cached = root.join("cache/verified");
        if cached.is_dir() {
            let target = PathBuf::from(directory("state/tugboat-codex-0.167.0/verified"));
            for file in [
                "codex-package-aarch64-unknown-linux-musl.tar.gz",
                "codex-package-aarch64-unknown-linux-musl.tar.gz.sigstore",
            ] {
                fs::copy(cached.join(file), target.join(file)).unwrap();
            }
        }
        let file = PathBuf::from(directory("records")).join("projects.json");
        let mut registry = Registry::default();
        let projects = ["project-a/sample project", "project-b"]
            .into_iter()
            .map(|name| registry.register(Path::new(&directory(name))).unwrap())
            .collect();
        storage::write(&file, &registry).unwrap();
        Self {
            root,
            file,
            brig,
            settings,
            projects,
        }
    }

    fn project(&self, index: usize) -> Project {
        Registry::load(&self.file)
            .unwrap()
            .project(&self.projects[index].id)
            .unwrap()
    }

    fn runtime<'a>(&self, project: &'a Project) -> Runtime<'a> {
        let brig = self
            .brig
            .for_project(project, &self.root.join("profiles"))
            .unwrap();
        Runtime {
            brig,
            project,
            settings: self.settings.clone(),
            protected: vec![self.root.join("records")],
            hull: PathBuf::from(&self.brig.environment["BRIG_RUNTIME_BIN"]),
            cosign: tools::tool("cosign").unwrap(),
            debugfs: tools::tool("debugfs").unwrap(),
            e2fsck: tools::tool("e2fsck").unwrap(),
        }
    }

    fn start(&self, index: usize) {
        let project = self.project(index);
        let runtime = self.runtime(&project);
        let prepared =
            preparation::prepare(&runtime.brig, &self.file, &self.settings, &project.id).unwrap();
        let checked = environment::check_environment(
            runtime.brig,
            prepared.project,
            self.settings.clone(),
            prepared.fingerprint.as_deref(),
            &[self.root.join("records")],
        )
        .unwrap();
        shared_runtime::install_codex(&checked).unwrap();
        let project = self.project(index);
        let runtime = self.runtime(&project);
        let (checked, _) = runtime.plan(DEFAULT_ENVIRONMENT_IMAGE).unwrap();
        runtime.boot(&checked, false).unwrap();
    }

    fn command(&self, project: &Project, script: &str) -> Result<String, String> {
        let runtime = self.runtime(project);
        let arguments = shared_runtime::exec(project, "/usr/bin/bash", &["-ec", script]);
        runtime
            .brig
            .checked(&arguments, None)
            .map(|output| output.stdout.trim().into())
    }

    fn review(&self) -> String {
        let project = self.project(0);
        let result = change(&self.file, &self.runtime(&project), Action::Review).unwrap();
        let Status::Review { changes, token, .. } = result else {
            panic!("The file review was unavailable");
        };
        assert!(changes
            .iter()
            .any(|entry| entry.path == "/var/tmp/tugboat-image-root-only" && entry.kind == "file"));
        println!("Reviewed {} exact guest system paths.", changes.len());
        token
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::env::var_os("TUGBOAT_IMAGE_TEST_KEEP").is_some() {
            return;
        }
        for project in &self.projects {
            let brig = self.brig.for_project(project, &self.root.join("profiles"));
            if let Ok(brig) = brig {
                let _ = brig.run(&["stop".into(), project.reference()], None);
                let _ = brig.run(&["rm".into(), project.reference()], None);
            }
        }
        let _ = command::run(
            Path::new(&self.brig.environment["BRIG_RUNTIME_BIN"]),
            &["store".into(), "detach".into()],
            None,
            &self.brig.environment,
            Duration::from_secs(30),
            &AtomicBool::new(false),
        );
    }
}

#[test]
#[ignore = "Requires Apple silicon, the isolated patched Hull executable and a private boot store"]
fn updates_and_restores_the_real_disposable_writable_guest_disk() {
    let fixture = Fixture::new();
    fixture.start(0);
    fixture.start(1);
    let a = fixture.project(0);
    let b = fixture.project(1);
    println!("Initial guests {} and {}", a.reference(), b.reference());
    fixture.command(&a, r#"printf root-marker > /var/tmp/tugboat-image-root-only; printf project-marker > "$BRIG_PROJECT/tugboat-image-project"; printf home-marker > "$HOME/tugboat-image-home""#).unwrap();
    let b_boot = fixture
        .command(&b, "cat /proc/sys/kernel/random/boot_id")
        .unwrap();
    let retained = if std::env::var_os("TUGBOAT_IMAGE_TEST_TRANSCRIPT_MANIFEST").is_some() {
        let (checked, _) = fixture.runtime(&a).plan(DEFAULT_ENVIRONMENT_IMAGE).unwrap();
        Some(super::post_update::before(&fixture.root, &fixture.file, &checked).unwrap())
    } else {
        None
    };

    // This is the interruption boundary between the durable review and the user's choice.
    let token = fixture.review();
    let a = fixture.project(0);
    assert!(a.require_active().is_err());
    let runtime = fixture.runtime(&a);
    let resumed = change(&fixture.file, &runtime, Action::Read).unwrap();
    assert!(matches!(resumed, Status::Review { token: recovered, .. } if token == recovered));
    let kept = change(&fixture.file, &runtime, Action::KeepCurrent { token }).unwrap();
    assert!(matches!(kept, Status::Restored { .. }));
    assert_eq!(
        fixture
            .command(&fixture.project(0), "cat /var/tmp/tugboat-image-root-only")
            .unwrap(),
        "root-marker"
    );

    let token = fixture.review();
    let a = fixture.project(0);
    let runtime = fixture.runtime(&a);
    let restored = replace(&fixture.file, &runtime, &token, true, |runtime, review| {
        let (candidate, _) = boot_target(runtime, review, CANDIDATE_IMAGE, false)?;
        fixture.command(
            &candidate.project,
            "printf altered-test-marker >> /usr/local/bin/claude",
        )?;
        assert!(shared_runtime::verify_claude(&candidate)
            .unwrap_err()
            .contains("changed"));
        Err("The candidate Claude Code executable failed verification.".into())
    })
    .unwrap();
    assert!(matches!(
        restored,
        Status::Restored {
            image: DEFAULT_ENVIRONMENT_IMAGE,
            ..
        }
    ));
    assert_eq!(
        fixture
            .command(&fixture.project(0), "cat /var/tmp/tugboat-image-root-only")
            .unwrap(),
        "root-marker"
    );
    println!("The previous image and its outside-home root marker survived candidate verification failure.");

    let token = fixture.review();
    let a = fixture.project(0);
    let update = change(&fixture.file, &fixture.runtime(&a), Action::Apply { token }).unwrap();
    assert!(
        matches!(
            &update,
            Status::Updated {
                candidate: CANDIDATE_IMAGE,
                ..
            }
        ),
        "The candidate operation returned {}",
        serde_json::to_string(&update).unwrap()
    );
    let a = fixture.project(0);
    let a_runtime = fixture.runtime(&a);

    assert!(matches!(
        change(&fixture.file, &a_runtime, Action::Read).unwrap(),
        Status::Current { .. }
    ));
    assert!(a.image_update.is_none());
    assert_eq!(fixture.command(&a, r#"test ! -e /var/tmp/tugboat-image-root-only; cat "$BRIG_PROJECT/tugboat-image-project" "$HOME/tugboat-image-home""#).unwrap(), "project-markerhome-marker");
    assert_eq!(
        fixture
            .command(&b, "cat /proc/sys/kernel/random/boot_id")
            .unwrap(),
        b_boot
    );
    let b_runtime = fixture.runtime(&b);
    b_runtime.stop().unwrap();
    let (b_checked, _) = b_runtime.plan(DEFAULT_ENVIRONMENT_IMAGE).unwrap();
    let versions = b_runtime.boot(&b_checked, false).unwrap();
    assert_eq!(
        versions,
        pinned_versions(DEFAULT_ENVIRONMENT_IMAGE).unwrap()
    );
    assert_ne!(
        fixture
            .command(&b, "cat /proc/sys/kernel/random/boot_id")
            .unwrap(),
        b_boot
    );
    assert!(fixture.project(1).image_ref == DEFAULT_ENVIRONMENT_IMAGE);
    println!(
        "Updated {} and restarted sibling {} on its original signed digest.",
        a.reference(),
        b.reference()
    );
    if let Some(before) = retained.as_ref() {
        let checked = environment::check_environment(
            a_runtime.brig,
            a.clone(),
            a_runtime.settings,
            a.approved_plan.as_deref(),
            &a_runtime.protected,
        )
        .unwrap();
        super::post_update::after(&fixture.root, &fixture.file, &checked, before).unwrap();
    }
}

#[test]
#[ignore = "Requires the preserved signed candidate and authorized Dev ChatGPT access"]
fn resumes_authorized_chats_on_the_retained_signed_candidate() {
    assert!(std::env::var_os("TUGBOAT_IMAGE_TEST_TRANSCRIPT_MANIFEST").is_some());
    assert!(std::env::var_os("TUGBOAT_IMAGE_TEST_CHATGPT_RECORDS").is_some());
    let (root, hull) = proof_inputs();
    let file = root.join("records/projects.json");
    assert_eq!(fs::canonicalize(&file).unwrap(), file);
    let project_path = root.join("project-a/sample project");
    let registry = Registry::load(&file).unwrap();
    let record = registry
        .projects
        .iter()
        .find(|record| record.path == project_path)
        .expect("Select the existing disposable A project.");
    let project = registry.project(&record.id).unwrap();
    assert_eq!(project.image_ref, CANDIDATE_IMAGE);
    let expected = project
        .approved_plan
        .clone()
        .expect("The candidate has no saved plan approval.");
    let settings = proof_settings();
    let brig = proof_brig(&hull, &settings, |name| {
        let path = root.join(name);
        assert_eq!(fs::canonicalize(&path).unwrap(), path);
        assert!(path.is_dir());
        path.to_string_lossy().into_owned()
    })
    .for_project(&project, &root.join("profiles"))
    .unwrap();
    let checked = environment::check_environment(
        brig,
        project,
        settings,
        Some(&expected),
        &[root.join("records")],
    )
    .unwrap();
    assert_eq!(
        checked.project.stage,
        projects::ProjectStage::EnvironmentReady
    );
    super::post_update::resume_existing(&root, &file, &checked).unwrap();
}
