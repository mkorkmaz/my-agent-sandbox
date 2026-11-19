use super::*;
use serde_json::{json, Value};
use std::{
    fs,
    net::{Ipv4Addr, TcpListener},
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

pub(in crate::projects) mod live;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "tugboat-preview-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let binary = root.join("brig");
        fs::write(
            &binary,
            include_str!("../../../../tests/fixtures/brig/v0.5.2/project-preview.sh"),
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        Self(root.canonicalize().unwrap())
    }

    fn file(&self) -> PathBuf {
        self.0.join("projects.json")
    }

    fn brig(&self) -> Brig {
        Brig {
            executable: self.0.join("brig"),
            environment: [(
                "BRIG_STATE_DIR".into(),
                self.0.to_string_lossy().into_owned(),
            )]
            .into(),
        }
    }

    fn project(&self, registry: &mut Registry, name: &str) -> Project {
        let folder = self.0.join(name);
        fs::create_dir(&folder).unwrap();
        let project = registry.register(&folder).unwrap();
        self.write(&project, "plan-ports", "[]");
        self.write(&project, "ports", "[]");
        fs::write(project.path.join("fixture-ref"), project.reference()).unwrap();
        self.info(&project, &folder);
        project
    }

    fn home(&self, project: &Project) -> PathBuf {
        self.brig().shared_project_home(project).unwrap()
    }

    fn info(&self, project: &Project, mount: &Path) {
        self.write(project, "info", json!({
            "apiVersion": "brig.sh/v1alpha1", "kind": "Info", "data": {
                "profile": project.profile, "session": project.label, "project": { "host": mount }, "workspace": self.home(project)
            }
        }).to_string());
    }

    fn inventory(&self, projects: &[(&Project, &str)], available: bool) {
        let rows: Vec<Value> = projects.iter().map(|(project, state)| json!({
            "ref": project.reference(), "sandbox": format!("brig-{}", project.reference()), "state": state, "workspace": self.home(project)
        })).collect();
        fs::write(
            self.0.join("inventory"),
            json!({
                "apiVersion": "brig.sh/v1alpha1", "kind": "SandboxList", "data": {
                    "runtime": { "kind": "hull", "available": available }, "sandboxes": rows
                }
            })
            .to_string(),
        )
        .unwrap();
    }

    fn write(&self, project: &Project, name: &str, content: impl AsRef<[u8]>) {
        fs::write(
            self.0.join(format!("{}-{name}", project.reference())),
            content,
        )
        .unwrap();
    }

    fn ports(&self, project: &Project, mapping: Mapping, live: bool) {
        self.write(
            project,
            "plan-ports",
            json!([mapping.publication()]).to_string(),
        );
        self.write(project, "ports", json!([{
            "host": mapping.address().to_string(), "guest": mapping.guest_port, "protocol": "tcp", "live": live
        }]).to_string());
    }

    fn action(
        &self,
        selected: &Project,
        action: Action,
        mapping: Option<Mapping>,
    ) -> Result<View, String> {
        let mut registry = Registry::load(&self.file()).unwrap();
        let project = registry
            .environment(&selected.id, &selected.environment_id)
            .unwrap();
        change(
            &mut registry,
            &self.file(),
            &project,
            &settings::Settings::default(),
            &self.brig(),
            action,
            mapping,
        )
    }

    fn approval(&self, project: &Project) -> Option<Approval> {
        Registry::load(&self.file())
            .unwrap()
            .environment(&project.id, &project.environment_id)
            .unwrap()
            .preview
    }

    fn mutations(&self) -> String {
        fs::read_to_string(self.0.join("mutations")).unwrap_or_default()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn mapping(host_port: u16) -> Mapping {
    Mapping {
        host_port,
        guest_port: 38123,
    }
}

#[test]
fn interrupted_publication_and_closure_reconcile_the_selected_record_on_retry() {
    let fixture = Fixture::new();
    let mut registry = Registry::default();
    let first = fixture.project(&mut registry, "first");
    let second = fixture.project(&mut registry, "second");
    fixture.inventory(&[(&first, "stopped"), (&second, "stopped")], true);
    let mut second_preview = Approval::new(&second, mapping(49157));
    second_preview.phase = Phase::Published;
    fixture.ports(&second, second_preview.mapping, false);
    save(
        &mut registry,
        &fixture.file(),
        &second.id,
        Some(second_preview.clone()),
    )
    .unwrap();
    assert!(fixture
        .action(&first, Action::Publish, Some(second_preview.mapping))
        .unwrap_err()
        .contains("approved for another project"));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let preview = mapping(listener.local_addr().unwrap().port());
    assert!(fixture
        .action(&first, Action::Publish, Some(preview))
        .unwrap_err()
        .contains("already in use"));
    assert!(fixture.mutations().is_empty());
    drop(listener);

    fixture.write(&first, "interrupt-publish", "");
    assert!(fixture
        .action(&first, Action::Publish, Some(preview))
        .unwrap_err()
        .contains("could not confirm"));
    assert_eq!(fixture.approval(&first).unwrap().phase, Phase::Publishing);
    let view = fixture
        .action(&first, Action::Publish, Some(preview))
        .unwrap();
    assert_eq!(view.status, Status::GuestStopped);
    assert_eq!(fixture.approval(&first).unwrap().phase, Phase::Published);
    assert!(fixture
        .action(&first, Action::Publish, Some(mapping(49158)))
        .is_err());

    fixture.write(&first, "interrupt-unpublish", "");
    assert!(fixture
        .action(&first, Action::Close, None)
        .unwrap_err()
        .contains("could not confirm"));
    assert_eq!(fixture.approval(&first).unwrap().phase, Phase::Closing);
    assert_eq!(
        fixture.action(&first, Action::Read, None).unwrap().status,
        Status::Closed
    );
    assert!(fixture.approval(&first).is_none());
    fixture.inventory(&[(&first, "running"), (&second, "stopped")], true);
    assert_eq!(
        fixture.action(&first, Action::Read, None).unwrap().status,
        Status::Closed
    );
    assert_eq!(fixture.approval(&second), Some(second_preview));
    assert_eq!(
        fixture.action(&second, Action::Read, None).unwrap().status,
        Status::GuestStopped
    );
    assert_eq!(
        fixture.mutations(),
        format!(
            "publish {} {}\nunpublish {} {}\n",
            first.reference(),
            preview.publication(),
            first.reference(),
            preview.address()
        )
    );
}

#[test]
fn missing_approval_unexpected_ports_or_unavailable_guest_fail_without_mutation() {
    let fixture = Fixture::new();
    let mut registry = Registry::default();
    let project = fixture.project(&mut registry, "project");
    storage::write(&fixture.file(), &registry).unwrap();
    fixture.inventory(&[(&project, "running")], true);
    let preview = mapping(49157);
    fixture.ports(&project, preview, true);
    fixture.write(&project, "plan-ports", "[]");
    for action in [Action::Read, Action::Close, Action::Publish] {
        assert_eq!(
            fixture.action(&project, action, Some(preview)).unwrap_err(),
            state::REVIEW
        );
    }
    fixture.write(&project, "ports", "[]");
    fixture.info(&project, &fixture.0);
    assert!(fixture
        .action(&project, Action::Close, None)
        .unwrap_err()
        .contains("does not match the selected project"));
    fixture.inventory(&[], false);
    assert!(fixture
        .action(&project, Action::Read, None)
        .unwrap_err()
        .contains("Could not verify the project's Brig guest"));
    fixture.inventory(&[], true);
    assert!(fixture
        .action(&project, Action::Publish, Some(preview))
        .unwrap_err()
        .contains("Prepare the project environment"));
    assert!(fixture.approval(&project).is_none());
    assert!(fixture.mutations().is_empty());
}

#[test]
fn final_open_checks_exact_live_mapping_and_guest_response_even_after_a_ready_read() {
    let fixture = Fixture::new();
    let mut registry = Registry::default();
    let mut project = fixture.project(&mut registry, "project");
    let preview = mapping(49157);
    let mut approval = Approval::new(&project, preview);
    approval.phase = Phase::Published;
    project.preview = Some(approval);
    fixture.inventory(&[(&project, "running")], true);
    fixture.ports(&project, preview, true);
    let brig = fixture.brig();
    let settings = settings::Settings::default();
    let open = |ready, launch: fn(Mapping) -> Result<(), String>| {
        open_verified(&brig, &project, &settings, preview, |_| ready, launch)
    };
    let refuse_launch = |_| panic!("unverified preview opened the browser");
    assert!(open(false, refuse_launch)
        .unwrap_err()
        .contains("Start the guest web server"));
    open(true, |value| {
        assert_eq!(value.url(), "http://127.0.0.1:49157/");
        Ok(())
    })
    .unwrap();
    fixture.ports(&project, mapping(49158), true);
    assert_eq!(open(true, refuse_launch).unwrap_err(), state::REVIEW);
    fixture.write(&project, "plan-ports", "[]");
    fixture.write(&project, "ports", "[]");
    assert!(open(true, refuse_launch)
        .unwrap_err()
        .contains("approved port is unavailable"));
    assert!(fixture.mutations().is_empty());
}

#[test]
fn preparation_matches_the_approved_plan_and_rejects_other_mappings() {
    let fixture = Fixture::new();
    let mut project = fixture.project(&mut Registry::default(), "project");
    let preview = mapping(49157);
    let exact = json!([preview.publication()]);
    assert!(validate_plan_ports(&project, &json!([])).is_ok());
    assert!(validate_plan_ports(&project, &exact).is_err());
    let mut approval = Approval::new(&project, preview);
    project.preview = Some(approval.clone());
    assert!(validate_plan_ports(&project, &exact).is_err());
    approval.phase = Phase::Published;
    project.preview = Some(approval);
    assert!(validate_plan_ports(&project, &exact).is_ok());
    for ports in [
        json!([]),
        json!([preview.publication(), "127.0.0.1:2000:2000"]),
        json!(["0.0.0.0:49157:38123"]),
        json!(["127.0.0.1:49157:38123/udp"]),
        json!(["127.0.0.1:49157:38124"]),
    ] {
        assert!(validate_plan_ports(&project, &ports).is_err());
    }
}
