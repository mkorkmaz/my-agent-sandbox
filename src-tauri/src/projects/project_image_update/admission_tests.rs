use super::*;
use crate::{
    agents::AgentKind,
    desktop::settings::Settings,
    environment::managed_profile,
    platform::brig::{self, Brig},
    projects::Policy,
};
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::PathBuf,
};

struct Fixture {
    root: PathBuf,
    project: Project,
    sibling: Project,
}

impl Fixture {
    fn new(case: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "tugboat-image-admission-{}-{case}",
            std::process::id(),
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let root = root.canonicalize().unwrap();
        for directory in ["project", "sibling", "records", "tugboat-project-homes"] {
            fs::create_dir(root.join(directory)).unwrap();
        }
        let mut registry = Registry::default();
        let mut project = registry.register(&root.join("project")).unwrap();
        let sibling = registry.register(&root.join("sibling")).unwrap();
        let policy = json!({
            "apiVersion": "brig.sh/v1alpha1",
            "kind": "Policy",
            "metadata": {"name": "bd01-fixture"},
            "spec": {"egress": {"default": "deny"}},
        });
        project.stage = ProjectStage::Ready;
        project.policy = Some(Policy {
            name: "bd01-fixture".into(),
            owned: true,
            fingerprint: Some(brig::fingerprint(&policy).unwrap()),
        });
        registry.update(project.clone()).unwrap();
        let fixture = Self {
            root,
            project,
            sibling,
        };
        storage::write(&fixture.file(), &registry).unwrap();
        fixture.write_json("policy.json", &policy);
        fixture.write_json("profile.json", &managed_profile::expected_definition());
        for tool in ["brig", "hull", "cosign"] {
            let path = fixture.root.join(tool);
            fs::write(&path, TOOL).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let runtime = fixture.runtime(case);
        let home = runtime.brig.shared_project_home(&fixture.project).unwrap();
        fs::DirBuilder::new().mode(0o700).create(&home).unwrap();
        fs::write(home.join("saved-chat"), "previous chat").unwrap();
        fs::write(fixture.project.path.join("work"), "project files").unwrap();
        fs::write(
            fixture.root.join("rootfs-upper.ext4"),
            "previous guest files",
        )
        .unwrap();
        let sandbox = format!("brig-{}-{}", fixture.project.profile, fixture.project.label);
        let recorded: Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/brig/v0.5.2/ready.json"
        ))
        .unwrap();
        let mut plan: Value = serde_json::from_str(recorded["stdout"].as_str().unwrap()).unwrap();
        plan["data"]["home"]["host"] = json!(home);
        plan["data"]["project"]["host"] = json!(fixture.project.path);
        plan["data"]["project"]["guest"] = json!("/work/project");
        plan["data"]["session"] = json!(fixture.project.label);
        plan["data"]["sandbox"] = json!(sandbox);
        plan["data"]["image"]["ref"] = json!(DEFAULT_ENVIRONMENT_IMAGE);
        fixture.write_json("previous.json", &plan);
        plan["data"]["image"]["ref"] = json!(CANDIDATE_IMAGE);
        fixture.write_json("candidate.json", &plan);
        fixture.write_json(
            "info.json",
            &json!({
                "apiVersion": "brig.sh/v1alpha1",
                "kind": "Info",
                "data": {
                    "profile": fixture.project.profile,
                    "session": fixture.project.label,
                    "project": {"host": fixture.project.path},
                    "workspace": home,
                },
            }),
        );
        fixture.write_json(
            "instance.json",
            &json!({
                "id": sandbox,
                "imageDigest": DEFAULT_ENVIRONMENT_IMAGE.split_once('@').unwrap().1,
                "status": "running",
                "backend": "hvi",
                "bundleDir": fixture.root.join("bundle"),
                "cmdLine": [],
            }),
        );
        fixture
    }

    fn write_json(&self, name: &str, content: &Value) {
        fs::write(self.root.join(name), content.to_string()).unwrap();
    }

    fn file(&self) -> PathBuf {
        self.root.join("records/projects.json")
    }

    fn runtime(&self, failure: &str) -> Runtime<'_> {
        let root = self.root.to_str().unwrap();
        Runtime {
            project: &self.project,
            brig: Brig {
                executable: self.root.join("brig"),
                environment: [
                    ("FIXTURE_ROOT", root),
                    ("BRIG_STATE_DIR", root),
                    ("CANDIDATE_IMAGE", CANDIDATE_IMAGE),
                    ("FIXTURE_FAILURE", failure),
                ]
                .into_iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
            },
            settings: Settings::default(),
            protected: vec![self.root.join("records")],
            hull: self.root.join("hull"),
            cosign: self.root.join("cosign"),
            debugfs: self.root.join("must-not-run"),
            e2fsck: self.root.join("must-not-run"),
        }
    }

    fn review(&self, failure: &str) -> Result<Status, String> {
        let _operation =
            EnvironmentOperation::begin(&self.project.id, &self.project.environment_id)?;
        change(&self.file(), &self.runtime(failure), Action::Review)
    }

    fn snapshot(&self) -> [Vec<u8>; 5] {
        let home = self
            .runtime("")
            .brig
            .shared_project_home(&self.project)
            .unwrap();
        [
            self.file(),
            self.root.join("instance.json"),
            self.root.join("rootfs-upper.ext4"),
            self.project.path.join("work"),
            home.join("saved-chat"),
        ]
        .map(|path| fs::read(path).unwrap())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

const TOOL: &str = r#"#!/bin/sh
printf '%s:%s\n' "${0##*/}" "$*" >>"$FIXTURE_ROOT/calls"
case "${0##*/}:$1:$2" in
brig:agent:show) /bin/cat "$FIXTURE_ROOT/profile.json";;
brig:policy:show) /bin/cat "$FIXTURE_ROOT/policy.json";;
brig:--json:plan)
    if test "$BRIG_IMAGE" = "$CANDIDATE_IMAGE"; then
        /bin/cat "$FIXTURE_ROOT/candidate.json"
    else
        /bin/cat "$FIXTURE_ROOT/previous.json"
    fi;;
brig:--json:info) /bin/cat "$FIXTURE_ROOT/info.json";;
hull:inspect:*) /bin/cat "$FIXTURE_ROOT/instance.json";;
cosign:verify:--certificate-identity)
    test "$4" = --certificate-oidc-issuer && test "$6" = "$CANDIDATE_IMAGE" || exit 90
    test "$FIXTURE_FAILURE" != signature;;
hull:pull:--platform)
    test "$3" = linux/arm64 && test "$4" = "$CANDIDATE_IMAGE" || exit 90
    if test "$FIXTURE_FAILURE" = disk-full; then
        printf '%s\n' 'no space left on device' >&2
    fi
    exit 1;;
*) exit 90;;
esac
"#;

#[test]
fn active_project_operation_refuses_review_and_the_sibling_stays_available() {
    let fixture = Fixture::new("active-operation");
    let before = fixture.snapshot();
    let operation =
        EnvironmentOperation::begin(&fixture.project.id, &fixture.project.environment_id).unwrap();
    assert!(fixture
        .review("download")
        .err()
        .unwrap()
        .contains("already in progress"));
    let _sibling =
        EnvironmentOperation::begin(&fixture.sibling.id, &fixture.sibling.environment_id).unwrap();
    assert!(!fixture.root.join("calls").exists());
    assert_eq!(fixture.snapshot(), before);
    drop(operation);
}

#[test]
fn both_agents_and_each_project_terminal_block_review_until_idle() {
    let fixture = Fixture::new("active-terminal");
    let before = fixture.snapshot();
    for (purpose, agent_kind) in [
        ("codex", Some(AgentKind::Codex)),
        ("claude", Some(AgentKind::ClaudeCode)),
        ("shell", Some(AgentKind::Codex)),
        ("local", None),
    ] {
        let status = terminal::TerminalStatus {
            identifier: 1,
            project_id: fixture.project.id.clone(),
            environment_id: agent_kind.map(|_| fixture.project.environment_id.clone()),
            agent_kind,
            purpose,
            running: true,
            exit_code: None,
            conversation_id: terminal::is_chat(purpose).then(|| "native-chat-id".into()),
            busy: false,
        };
        let error = terminal::with_fixture_terminal_statuses(vec![status], || {
            fixture.review("download").err().unwrap()
        });
        assert_eq!(
            error,
            "Close the project's active chats and terminals before updating the guest image.",
            "{purpose}"
        );
        assert!(!fixture.root.join("calls").exists());
    }
    assert_eq!(fixture.snapshot(), before);
}

fn expect_preflight_failure(failure: &str, message: &str, pulls: bool) {
    let fixture = Fixture::new(failure);
    let before = fixture.snapshot();
    assert_eq!(fixture.review(failure).err().unwrap(), message);
    let calls = fs::read_to_string(fixture.root.join("calls")).unwrap();
    let calls: Vec<_> = calls.lines().collect();
    assert_eq!(calls.len(), if pulls { 10 } else { 9 }, "{calls:?}");
    assert_eq!(
        calls[6],
        format!("brig:--json info {}", fixture.project.reference())
    );
    assert_eq!(
        calls[7],
        format!(
            "hull:inspect brig-{}-{}",
            fixture.project.profile, fixture.project.label
        )
    );
    assert!(calls[8].starts_with("cosign:verify --certificate-identity "));
    assert!(calls[8].ends_with(CANDIDATE_IMAGE));
    if pulls {
        assert_eq!(
            calls[9],
            format!("hull:pull --platform linux/arm64 {CANDIDATE_IMAGE}")
        );
    }
    assert_eq!(fixture.snapshot(), before);
    let saved = Registry::load(&fixture.file())
        .unwrap()
        .environment(&fixture.project.id, &fixture.project.environment_id)
        .unwrap();
    assert_eq!(saved.stage, ProjectStage::Ready);
    assert_eq!(saved.image_ref, DEFAULT_ENVIRONMENT_IMAGE);
    assert!(saved.image_update.is_none());
}

#[test]
fn invalid_signature_prevents_pull_checkpoint_and_stop() {
    expect_preflight_failure(
        "signature",
        "The candidate image did not pass the signature check.",
        false,
    );
}

#[test]
fn download_failure_leaves_the_running_project_unchanged() {
    expect_preflight_failure(
        "download",
        "Could not download the verified guest image.",
        true,
    );
}

#[test]
fn full_disk_during_pull_leaves_the_running_project_unchanged() {
    expect_preflight_failure(
        "disk-full",
        "The image operation needs more free disk space.",
        true,
    );
}
