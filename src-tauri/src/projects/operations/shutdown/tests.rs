use super::*;
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

struct Fixture(PathBuf);

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("tugboat-quit-{name}-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        fs::create_dir(root.join("rows")).unwrap();
        let program = root.join("brig");
        fs::write(&program, r#"#!/bin/sh
set -eu
root=${0%/*}
printf '%s\n' "$*" >> "$root/calls"
case "$1:$2" in
--json:ls)
    test -f "$BRIG_PROFILE_DIR/tugboat-project.yaml"
    printf '%s' '{"apiVersion":"brig.sh/v1alpha1","kind":"SandboxList","data":{"runtime":{"kind":"hull","available":true},"sandboxes":['
    separator=
    for row in "$root"/rows/*; do
        test -f "$row" || continue
        state=$(/bin/cat "$root/${row##*/}.state")
        printf '%s' "$separator"
        /usr/bin/sed "s/\"STATE\"/\"$state\"/" "$row"
        separator=,
    done
    printf '%s' ']}}';;
--json:info) /bin/cat "$root/$3.info";;
stop:*)
    test "$#" = 2
    test ! -f "$root/$2.noop" || exit 0
    printf stopped > "$root/$2.state"
    test ! -f "$root/$2.failure" || exit 8;;
*) exit 9;;
esac
"#).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        Self(root)
    }

    fn brig(&self) -> Brig {
        Brig {
            executable: self.0.join("brig"),
            environment: [("BRIG_STATE_DIR".into(), self.0.display().to_string())].into(),
        }
    }

    fn file(&self) -> PathBuf {
        self.0.join("projects.json")
    }

    fn register(&self, registry: &mut Registry, name: &str, state: Option<&str>) -> Project {
        let folder = self.0.join(name);
        fs::create_dir(&folder).unwrap();
        let mut project = registry.register(&folder).unwrap();
        project.stage = ProjectStage::EnvironmentReady;
        project.environment_verified = Some(true);
        project.approved_plan = Some("approved".into());
        registry.update(project.clone()).unwrap();
        let Some(state) = state else { return project };
        let reference = project.reference();
        let home = self.brig().shared_project_home(&project).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("chat.jsonl"), "saved conversation").unwrap();
        let sandbox = format!("brig-{}-{}", project.profile, project.label);
        let row = json!({"ref":reference,"sandbox":sandbox,"workspace":home,"state":"STATE"});
        fs::write(self.0.join("rows").join(&reference), row.to_string()).unwrap();
        fs::write(self.0.join(format!("{reference}.state")), state).unwrap();
        let info = json!({"apiVersion":"brig.sh/v1alpha1","kind":"Info","data":{"profile":project.profile,"session":project.label,"sandbox":sandbox,"project":{"host":folder},"workspace":home}});
        fs::write(self.0.join(format!("{reference}.info")), info.to_string()).unwrap();
        project
    }

    fn calls(&self, project: &Project) -> usize {
        let stop = format!("stop {}", project.reference());
        fs::read_to_string(self.0.join("calls"))
            .unwrap()
            .lines()
            .filter(|line| *line == stop)
            .count()
    }

    fn stop(&self) -> Result<(), String> {
        let file = self.file();
        stop_registered(&self.brig(), &file, &mut Registry::load(&file)?)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn quit_stops_registered_guests_and_retries_only_unconfirmed_stops() {
    let fixture = Fixture::new("retry");
    let mut registry = Registry::default();
    let pending = fixture.register(&mut registry, "Pending", Some("running"));
    let partial = fixture.register(&mut registry, "Partial", Some("running"));
    let done = fixture.register(&mut registry, "Done", Some("running"));
    let stopped = fixture.register(&mut registry, "Stopped", Some("stopped"));
    let absent = fixture.register(&mut registry, "Absent", None);
    let other = fixture.register(&mut Registry::default(), "Outside", Some("running"));
    storage::write(&fixture.file(), &registry).unwrap();
    let noop = fixture.0.join(format!("{}.noop", pending.reference()));
    let failure = fixture.0.join(format!("{}.failure", partial.reference()));
    fs::write(&noop, "").unwrap();
    fs::write(&failure, "").unwrap();

    let error = fixture.stop().unwrap_err();
    assert!(error.contains("Pending") && error.contains("still reports the guest as running"));
    assert!(error.contains("Partial"));
    let checkpoint = Registry::load(&fixture.file()).unwrap();
    for project in [&pending, &partial] {
        assert_eq!(
            checkpoint.project(&project.id).unwrap().stage,
            ProjectStage::Stopping
        );
    }
    for project in [&done, &stopped, &absent] {
        assert_eq!(
            checkpoint.project(&project.id).unwrap().stage,
            ProjectStage::Ready
        );
    }
    fs::remove_file(noop).unwrap();
    fs::remove_file(failure).unwrap();
    fixture.stop().unwrap();
    fixture.stop().unwrap();

    assert_eq!(
        (
            fixture.calls(&pending),
            fixture.calls(&partial),
            fixture.calls(&done)
        ),
        (2, 2, 1)
    );
    assert_eq!(
        (
            fixture.calls(&stopped),
            fixture.calls(&absent),
            fixture.calls(&other)
        ),
        (0, 0, 0)
    );
    let reopened = Registry::load(&fixture.file())
        .unwrap()
        .project(&pending.id)
        .unwrap();
    assert_eq!(reopened.stage, ProjectStage::Ready);
    assert_eq!(reopened.approved_plan.as_deref(), Some("approved"));
    assert_eq!(
        fs::read_to_string(
            fixture
                .brig()
                .shared_project_home(&done)
                .unwrap()
                .join("chat.jsonl")
        )
        .unwrap(),
        "saved conversation"
    );
}

#[test]
fn a_different_host_folder_or_guest_home_prevents_the_stop() {
    let fixture = Fixture::new("scope");
    let mut registry = Registry::default();
    let first = fixture.register(&mut registry, "First", Some("running"));
    let second = fixture.register(&mut registry, "Second", Some("running"));
    storage::write(&fixture.file(), &registry).unwrap();
    let info = fixture.0.join(format!("{}.info", first.reference()));
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&info).unwrap()).unwrap();
    value["data"]["project"]["host"] = json!(second.path);
    fs::write(info, value.to_string()).unwrap();
    let row = fixture.0.join("rows").join(second.reference());
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&row).unwrap()).unwrap();
    value["workspace"] = json!(fixture.brig().shared_project_home(&first).unwrap());
    fs::write(row, value.to_string()).unwrap();
    let error = fixture.stop().unwrap_err();
    assert!(error.contains("First") && error.contains("Second"));
    assert_eq!((fixture.calls(&first), fixture.calls(&second)), (0, 0));
}
