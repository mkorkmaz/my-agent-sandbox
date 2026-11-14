use super::*;
use crate::projects::Policy;
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

struct Fixture {
    root: PathBuf,
    project: Project,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "tugboat-checked-paths-{name}-{}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("project")).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let mut project = Registry::default()
            .register(&root.join("project"), "codex")
            .unwrap();
        let policy = json!({"apiVersion":"brig.sh/v1alpha1","kind":"Policy","metadata":{"name":"bd01-fixture"},"spec":{"egress":{"default":"deny"}}});
        project.label = "bd01-fixture".into();
        project.stage = crate::projects::ProjectStage::Ready;
        project.policy = Some(Policy {
            name: "bd01-fixture".into(),
            owned: true,
            fingerprint: Some(brig::fingerprint(&policy).unwrap()),
        });
        fs::write(root.join("policy.json"), policy.to_string()).unwrap();
        let executable = root.join("brig");
        fs::write(&executable, r#"#!/bin/sh
case "$1:$2" in
agent:show) printf '%s' '{"name":"codex","kind":"agent","binary":"codex"}';;
policy:show) /bin/cat "$FIXTURE_ROOT/policy.json";;
--json:plan) /bin/cat "$FIXTURE_ROOT/plan.json";;
--json:ls) printf '%s' '{"apiVersion":"brig.sh/v1alpha1","kind":"SandboxList","data":{"sandboxes":[]}}';;
*) exit 91;;
esac
"#).unwrap();
        fs::set_permissions(executable, fs::Permissions::from_mode(0o700)).unwrap();
        Self { root, project }
    }

    fn plan(&self) -> Value {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/brig/v0.5.2/ready.json"
        ))
        .unwrap();
        let mut plan: Value = serde_json::from_str(fixture["stdout"].as_str().unwrap()).unwrap();
        plan["data"]["project"]["host"] = json!(self.project.path);
        plan["data"]["home"]["host"] = json!(self.root.join("uncreated-home"));
        plan
    }

    fn checked(&self, plan: &Value, expected: Option<&str>) -> Result<CheckedProject, String> {
        fs::write(self.root.join("plan.json"), plan.to_string()).unwrap();
        let brig = Brig {
            executable: self.root.join("brig"),
            environment: std::collections::BTreeMap::from([(
                "FIXTURE_ROOT".into(),
                self.root.to_string_lossy().into_owned(),
            )]),
        };
        let settings = Settings {
            verification: crate::desktop::settings::Verification::Warn,
            ..Settings::default()
        };
        check_environment(
            brig,
            self.project.clone(),
            settings,
            expected,
            &[self.root.join("records")],
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn malformed_shared_paths_fail_before_a_checked_project_is_created() {
    let fixture = Fixture::new("malformed");
    for (field, key, value) in [
        ("home", "host", Value::Null),
        ("home", "host", json!(17)),
        ("home", "host", json!("")),
        ("home", "host", json!("relative/home")),
        ("home", "host", json!("/guest/../home")),
        ("home", "host", json!("/guest/\0home")),
        ("project", "guest", Value::Null),
        ("project", "guest", json!([])),
        ("project", "guest", json!("")),
        ("project", "guest", json!("relative/project")),
        ("project", "guest", json!("/work/../project")),
        ("project", "guest", json!("/work/project\tname")),
    ] {
        let mut plan = fixture.plan();
        plan["data"][field][key] = value;
        assert!(fixture.checked(&plan, None).is_err(), "{plan}");
    }
    let mut missing_home = fixture.plan();
    missing_home["data"]["home"]
        .as_object_mut()
        .unwrap()
        .remove("host");
    assert_eq!(
        fixture.checked(&missing_home, None).err().unwrap(),
        "The plan host share is missing."
    );
    let mut missing_project = fixture.plan();
    missing_project["data"]["project"]
        .as_object_mut()
        .unwrap()
        .remove("guest");
    assert_eq!(
        fixture.checked(&missing_project, None).err().unwrap(),
        "The guest project path is missing."
    );
}

#[test]
fn checked_paths_keep_the_raw_plan_and_its_fingerprint_including_additive_fields() {
    let fixture = Fixture::new("wire");
    let mut plan = fixture.plan();
    plan["data"]["futureField"] = json!({"kept":"verbatim"});
    let bytes = serde_json::to_vec(&plan).unwrap();
    let profile = json!({"name":"codex","kind":"agent","binary":"codex"});
    let fingerprint = plan_fingerprint(&plan, &fixture.project, &profile).unwrap();
    let checked = fixture.checked(&plan, Some(&fingerprint)).unwrap();
    assert_eq!(serde_json::to_vec(&checked.plan).unwrap(), bytes);
    assert_eq!(checked.fingerprint, fingerprint);
    assert_eq!(checked.paths.host_home, fixture.root.join("uncreated-home"));
    assert_eq!(
        checked.paths.guest_project,
        PathBuf::from("/work/örnek proje")
    );
    assert!(!checked.paths.host_home.exists());
    for (field, key, value) in [
        ("home", "host", json!(fixture.root.join("changed-home"))),
        ("project", "guest", json!("/work/changed project")),
    ] {
        let mut changed = plan.clone();
        changed["data"][field][key] = value;
        assert_eq!(
            fixture.checked(&changed, Some(&fingerprint)).err().unwrap(),
            "The execution plan changed. Review it and start again."
        );
    }
}
