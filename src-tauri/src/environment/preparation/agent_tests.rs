use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    root: PathBuf,
    brig: Brig,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(format!(
            "/private/tmp/t4-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        for name in [
            "h",
            "s",
            "c",
            "y",
            "records",
            "project with spaces",
            "external-home",
        ] {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        let mut brig = Brig::new(&Settings::default()).unwrap();
        for (name, directory) in [
            ("HOME", "h"),
            ("BRIG_STATE_DIR", "s"),
            ("XDG_CONFIG_HOME", "c"),
            ("BRIG_POLICY_DIR", "y"),
        ] {
            brig.environment
                .insert(name.into(), root.join(directory).to_string_lossy().into());
        }
        let inventory = root.join("runtime");
        fs::write(&inventory, "#!/bin/sh\nroot=${0%/*}\nprintf '%s\\n' \"$*\" >> \"$root/runtime-calls\"\ncase \"$1\" in\nps) printf 'NAME\\tSTATUS\\n';;\ninspect) exit 1;;\n*) exit 91;;\nesac\n").unwrap();
        fs::set_permissions(&inventory, fs::Permissions::from_mode(0o700)).unwrap();
        brig.environment.insert(
            "BRIG_RUNTIME_BIN".into(),
            inventory.to_string_lossy().into(),
        );
        let fixture = Self { root, brig };
        let mut registry = Registry::default();
        let codex = registry
            .register(&fixture.root.join("project with spaces"), "codex")
            .unwrap();
        registry
            .ensure_environment(&codex.id, AgentKind::ClaudeCode, &Settings::default())
            .unwrap();
        storage::write(&fixture.file(), &registry).unwrap();
        fixture
    }

    fn file(&self) -> PathBuf {
        self.root.join("records/projects.json")
    }

    fn records(&self) -> Vec<PathBuf> {
        vec![self.root.join("records")]
    }

    fn project(&self, kind: AgentKind) -> Project {
        let registry = Registry::load(&self.file()).unwrap();
        let record = &registry.projects[0];
        let environment = record.environments.by_kind(kind).unwrap();
        registry
            .environment(&record.id, &environment.environment_id)
            .unwrap()
    }

    fn scoped(&self, project: &Project) -> Brig {
        self.brig
            .for_project(project, &self.root.join("records/profiles"))
            .unwrap()
    }

    fn prepare(&self, project: &Project) -> Preparation {
        prepare_selected(
            &self.scoped(project),
            &self.file(),
            &Settings::default(),
            &project.id,
            &project.environment_id,
            None,
            &self.records(),
        )
        .unwrap()
    }

    fn fake_external(&self) -> Brig {
        let project = self.project(AgentKind::ClaudeCode);
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/brig/v0.5.2/ready.json"
        ))
        .unwrap();
        let mut plan: Value = serde_json::from_str(fixture["stdout"].as_str().unwrap()).unwrap();
        plan["data"]["profile"] = "custom-claude".into();
        plan["data"]["session"] = "existing".into();
        plan["data"]["project"]["host"] = project.path.to_string_lossy().into_owned().into();
        plan["data"]["project"]["guest"] = "/work/project with spaces".into();
        plan["data"]["home"]["host"] = self
            .root
            .join("external-home")
            .to_string_lossy()
            .into_owned()
            .into();
        plan["data"]["verify"]["mode"] = "require".into();
        plan["data"]["policies"] = serde_json::json!(["external-policy"]);
        plan["data"]["credentials"] = serde_json::json!([]);
        plan["data"]["limits"] = serde_json::json!({"cpus":4,"mem":4096});
        storage::write(&self.root.join("plan.json"), &plan).unwrap();
        storage::write(&self.root.join("inventory.json"), &serde_json::json!({"apiVersion":"brig.sh/v1alpha1","kind":"SandboxList","data":{"sandboxes":[{"agent":"custom-claude","ref":"custom-claude@existing","sandbox":"brig-custom-claude-existing","state":"stopped"}]}})).unwrap();
        storage::write(&self.root.join("info.json"), &serde_json::json!({"apiVersion":"brig.sh/v1alpha1","kind":"Info","data":{"profile":"custom-claude","session":"existing","project":{"host":project.path},"workspace":self.root.join("external-home")}})).unwrap();
        storage::write(&self.root.join("policy.json"), &serde_json::json!({"apiVersion":"brig.sh/v1alpha1","kind":"Policy","metadata":{"name":"external-policy"},"spec":{"egress":{"default":"deny","allow":[]}}})).unwrap();
        let executable = self.root.join("fake-brig");
        fs::write(&executable, r#"#!/bin/sh
root=${0%/*}
printf '%s|%s\n' "${BRIG_PROFILE_DIR:-}" "$*" >> "$root/calls"
case "$1:$2" in
agent:show) if test "$3" != tugboat-claude && test -n "${BRIG_PROFILE_DIR:-}"; then exit 85; fi; printf '{"name":"%s","kind":"agent","binary":"claude"}\n' "$3";;
--json:ls) if test -n "${BRIG_PROFILE_DIR:-}" && test -f "$root/managed-inventory.json"; then cat "$root/managed-inventory.json"; else cat "$root/inventory.json"; fi;;
--json:info) if test "$3" = tugboat-claude@previous && test -n "${BRIG_PROFILE_DIR:-}"; then cat "$root/managed-info.json"; exit; fi; test -z "${BRIG_PROFILE_DIR:-}" || exit 86; test "$3" = custom-claude@existing || exit 89; cat "$root/info.json";;
--json:plan) if test -n "${BRIG_PROFILE_DIR:-}" && test -f "$root/managed-plan.json"; then cat "$root/managed-plan.json"; exit; fi; test -z "${BRIG_PROFILE_DIR:-}" || exit 87; cat "$root/plan.json";;
policy:show) cat "$root/policy.json";;
policy:check) printf 'external-policy\n';;
*) exit 88;;
esac
"#).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        Brig {
            executable,
            environment: self.brig.environment.clone(),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn real_brig_managed_claude_preparation_keeps_codex_policy_home_and_retry_state_separate() {
    let fixture = Fixture::new();
    let codex = fixture.prepare(&fixture.project(AgentKind::Codex));
    let codex = crate::projects::project_policy::change(
        &fixture.brig,
        &fixture.file(),
        &Settings::default(),
        &codex.project.id,
        &codex.project.environment_id,
        codex
            .project
            .policy
            .as_ref()
            .unwrap()
            .fingerprint
            .as_deref()
            .unwrap(),
        crate::projects::project_policy::Action::Save,
        vec!["private.example".into()],
        &fixture.records(),
    )
    .unwrap();
    let codex_policy = fixture
        .root
        .join("y")
        .join(format!("{}.yaml", codex.policy.as_ref().unwrap().name));
    let original_policy = fs::read(&codex_policy).unwrap();
    let original_codex = serde_json::to_value(&codex).unwrap();
    let claude = fixture.project(AgentKind::ClaudeCode);
    let scoped = fixture.scoped(&claude);
    assert!(!fixture.brig.environment.contains_key("BRIG_PROFILE_DIR"));
    assert!(!fixture
        .scoped(&codex)
        .environment
        .contains_key("BRIG_PROFILE_DIR"));
    let managed = scoped.agent_profile("tugboat-claude").unwrap();
    let built_in = fixture.brig.agent_profile("claude-code").unwrap();
    for field in [
        "binary",
        "image",
        "guestHome",
        "volumes",
        "headless",
        "mem",
        "cpus",
        "hypervisor",
        "network",
        "genericBoot",
        "staleCredentialFiles",
        "onboarding",
        "hostConfigDir",
        "projectPaths",
    ] {
        assert_eq!(managed[field], built_in[field], "{field}");
    }
    assert!(managed["secrets"].as_array().is_none_or(Vec::is_empty));
    assert!(managed["files"].as_array().is_none_or(Vec::is_empty));
    assert_eq!(
        managed["env"],
        serde_json::json!([{"name":"IS_SANDBOX","value":"1"},{"name":"CLAUDE_CONFIG_DIR","value":"/brig/claude"}])
    );
    let prepared = fixture.prepare(&claude);
    let plan = prepared.plan.as_ref().unwrap();
    let codex_plan = fixture.brig.plan(&codex, &Settings::default()).unwrap();
    assert_ne!(
        plan["data"]["home"]["host"],
        codex_plan["data"]["home"]["host"]
    );
    assert_ne!(prepared.project.reference(), codex.reference());
    assert_ne!(
        prepared.project.policy.as_ref().unwrap().name,
        codex.policy.as_ref().unwrap().name
    );
    assert_eq!(plan["data"]["credentials"], serde_json::json!([]));
    assert_eq!(plan["data"]["ports"], serde_json::json!([]));
    assert_eq!(plan["data"]["skills"], serde_json::json!([]));
    assert_eq!(plan["data"]["egress"]["default"], "deny");
    let hosts = plan["data"]["egress"]["allow"].as_array().unwrap();
    assert!(hosts.iter().any(|rule| rule["host"] == "api.anthropic.com"));
    assert!(hosts
        .iter()
        .any(|rule| rule["host"] == "platform.claude.com"));
    assert!(!hosts.iter().any(|rule| rule["host"] == "private.example"));
    let sandbox = plan["data"]["sandbox"].as_str().unwrap();
    crate::platform::hull::check_instance_socket_paths(
        Path::new(scoped.environment.get("HOME").unwrap()),
        sandbox,
    )
    .unwrap();
    let wrong_fingerprint = crate::environment::plan_fingerprint(
        &codex_plan,
        &codex,
        &fixture.brig.agent_profile(&codex.profile).unwrap(),
    )
    .unwrap();
    let configured = prepared.project.settings(Settings::default()).unwrap();
    let rejected = crate::environment::check_environment(
        scoped.clone(),
        prepared.project.clone(),
        configured,
        Some(&wrong_fingerprint),
        &fixture.records(),
    );
    assert!(rejected.err().unwrap().contains("execution plan changed"));
    let mut interrupted = prepared.project.clone();
    interrupted.stage = crate::projects::ProjectStage::CreatingPolicy;
    interrupted.policy.as_mut().unwrap().fingerprint = None;
    save(&fixture.file(), &interrupted).unwrap();
    for _ in 0..2 {
        let retried = fixture.prepare(&interrupted);
        assert_eq!(
            retried.project.policy.as_ref().unwrap().name,
            prepared.project.policy.as_ref().unwrap().name
        );
        assert_eq!(retried.fingerprint, prepared.fingerprint);
    }
    let bindings = scoped
        .checked(&["policy".into(), "ls".into()], None)
        .unwrap();
    assert_eq!(
        bindings
            .stdout
            .matches(&format!(
                "bound to: {} -n {}",
                prepared.project.profile, prepared.project.label
            ))
            .count(),
        1
    );
    let modified = crate::projects::project_policy::change(
        &scoped,
        &fixture.file(),
        &Settings::default(),
        &prepared.project.id,
        &prepared.project.environment_id,
        prepared
            .project
            .policy
            .as_ref()
            .unwrap()
            .fingerprint
            .as_deref()
            .unwrap(),
        crate::projects::project_policy::Action::Save,
        vec!["claude.example".into()],
        &fixture.records(),
    )
    .unwrap();
    let changed_plan = scoped.plan(&modified, &Settings::default()).unwrap();
    assert_eq!(
        changed_plan["data"]["egress"]["allow"],
        serde_json::json!([{"host":"claude.example"}])
    );
    let mut offline = modified;
    offline.network = settings::Network::Offline;
    save(&fixture.file(), &offline).unwrap();
    assert_eq!(
        fixture.prepare(&offline).plan.unwrap()["data"]["posture"],
        "offline"
    );
    assert_eq!(fs::read(codex_policy).unwrap(), original_policy);
    assert_eq!(
        serde_json::to_value(fixture.project(AgentKind::Codex)).unwrap(),
        original_codex
    );
    assert!(!fs::read_to_string(fixture.root.join("runtime-calls"))
        .unwrap()
        .lines()
        .any(|line| line.starts_with("run")));
}

#[test]
fn existing_claude_reference_uses_its_own_profile_and_home_without_the_managed_override() {
    let fixture = Fixture::new();
    let claude = fixture.project(AgentKind::ClaudeCode);
    let original_codex = serde_json::to_value(fixture.project(AgentKind::Codex)).unwrap();
    let brig = fixture
        .fake_external()
        .for_project(&claude, &fixture.root.join("records/profiles"))
        .unwrap();
    let prepared = prepare_selected(
        &brig,
        &fixture.file(),
        &Settings::default(),
        &claude.id,
        &claude.environment_id,
        Some("custom-claude@existing"),
        &fixture.records(),
    )
    .unwrap();
    assert_eq!(prepared.project.reference(), "custom-claude@existing");
    assert!(!prepared.project.owns_session);
    assert!(!prepared.project.policy.as_ref().unwrap().owned);
    assert_eq!(prepared.project.environment_id, claude.environment_id);
    assert_eq!(
        prepared.plan.unwrap()["data"]["home"]["host"],
        fixture
            .root
            .join("external-home")
            .to_string_lossy()
            .as_ref()
    );
    let calls = fs::read_to_string(fixture.root.join("calls")).unwrap();
    assert!(calls
        .lines()
        .any(|line| line == "|--json info custom-claude@existing"));
    assert!(!calls.contains("policy create") && !calls.contains("policy attach"));
    assert_eq!(
        serde_json::to_value(fixture.project(AgentKind::Codex)).unwrap(),
        original_codex
    );
    let unscoped = fixture
        .brig
        .for_project(&prepared.project, &fixture.root.join("not-created"))
        .unwrap();
    assert!(!unscoped.environment.contains_key("BRIG_PROFILE_DIR"));
    assert!(!fixture.root.join("not-created").exists());
}

#[test]
fn chooser_discovers_both_profile_directories_and_reopens_an_adopted_managed_profile() {
    let fixture = Fixture::new();
    let project = fixture.project(AgentKind::ClaudeCode);
    let brig = fixture
        .fake_external()
        .for_project(&project, &fixture.root.join("records/profiles"))
        .unwrap();
    let managed = "tugboat-claude@previous";
    let external = "custom-claude@existing";
    let mut inventory: Value = storage::read(&fixture.root.join("inventory.json"))
        .unwrap()
        .unwrap();
    inventory["data"]["sandboxes"] = serde_json::json!([{"agent":"tugboat-claude","ref":managed,"sandbox":"brig-tugboat-claude-previous","state":"stopped"}]);
    storage::write(&fixture.root.join("managed-inventory.json"), &inventory).unwrap();
    for kind in ["info", "plan"] {
        let mut data: Value = storage::read(&fixture.root.join(format!("{kind}.json")))
            .unwrap()
            .unwrap();
        data["data"]["profile"] = "tugboat-claude".into();
        data["data"]["session"] = "previous".into();
        storage::write(&fixture.root.join(format!("managed-{kind}.json")), &data).unwrap();
    }
    let prepare = |brig: &Brig, chosen| {
        prepare_selected(
            brig,
            &fixture.file(),
            &Settings::default(),
            &project.id,
            &project.environment_id,
            chosen,
            &fixture.records(),
        )
        .unwrap()
    };
    let choice = prepare(&brig, None);
    assert_eq!(choice.candidates, [external, managed]);
    assert!(choice.plan.is_none());
    let adopted = prepare(&brig, Some(managed));
    assert_eq!(adopted.plan.unwrap()["data"]["session"], "previous");
    assert!(!adopted.project.owns_session);
    assert!(!crate::environment::managed_profile::owns(&adopted.project));
    let reopened = fixture.scoped(&fixture.project(AgentKind::ClaudeCode));
    assert_eq!(
        prepare(
            &Brig {
                executable: brig.executable,
                ..reopened
            },
            None
        )
        .project
        .reference(),
        managed
    );
}

#[test]
fn indexed_session_discovery_skips_unrelated_paths_and_adopts_a_confirmed_match() {
    let fixture = Fixture::new();
    let project = fixture.project(AgentKind::ClaudeCode);
    let mut brig = fixture
        .fake_external()
        .for_project(&project, &fixture.root.join("records/profiles"))
        .unwrap();
    brig.environment.remove("BRIG_STATE_DIR");
    let selected_alias = Path::new("/tmp").join(project.path.strip_prefix("/private/tmp").unwrap());
    storage::write(
        &fixture.root.join("inventory.json"),
        &serde_json::json!({"apiVersion":"brig.sh/v1alpha1","kind":"SandboxList","data":{"sandboxes":[
            {"agent":"custom-claude","ref":"custom-claude@unavailable","sandbox":"brig-custom-claude-unavailable","state":"stopped"},
            {"agent":"custom-claude","ref":"custom-claude@existing","sandbox":"brig-custom-claude-existing","state":"stopped"}
        ]}}),
    )
    .unwrap();
    storage::write(
        &fixture.root.join("h/.brig/sessions.json"),
        &serde_json::json!({
            "custom-claude@unavailable":{"sandbox":"brig-custom-claude-unavailable","project":"/Volumes/Unavailable/project with spaces"},
            "custom-claude@existing":{"sandbox":"brig-custom-claude-existing","project":selected_alias},
            "custom-claude@absent":{"sandbox":"brig-custom-claude-absent","project":project.path}
        }),
    )
    .unwrap();

    let prepared = prepare_selected(
        &brig,
        &fixture.file(),
        &Settings::default(),
        &project.id,
        &project.environment_id,
        None,
        &fixture.records(),
    )
    .unwrap();
    assert_eq!(prepared.project.reference(), "custom-claude@existing");
    let calls = fs::read_to_string(fixture.root.join("calls")).unwrap();
    let info_calls: Vec<_> = calls
        .lines()
        .filter(|line| line.contains("--json info"))
        .collect();
    assert_eq!(info_calls, ["|--json info custom-claude@existing"]);
}

#[test]
fn indexed_session_discovery_checks_bundled_brig_with_a_stub_runtime() {
    let fixture = Fixture::new();
    let project = fixture.project(AgentKind::Codex);
    let runtime = fixture.root.join("runtime");
    fs::write(
        &runtime,
        "#!/bin/sh\ncase \"$1\" in\nps) printf 'NAME\\tSTATUS\\nbrig-codex-existing\\tstopped\\nbrig-codex-unavailable\\tstopped\\n';;\ninspect) exit 1;;\n*) exit 91;;\nesac\n",
    )
    .unwrap();
    storage::write(
        &fixture.root.join("s/sessions.json"),
        &serde_json::json!({
            "codex@unavailable":{"sandbox":"brig-codex-unavailable", "home":fixture.root.join("external-home"), "project":"/Volumes/Unavailable/project with spaces"},
            "codex@existing":{"sandbox":"brig-codex-existing", "home":fixture.root.join("external-home"), "project":project.path}
        }),
    )
    .unwrap();
    let mut brig = fixture.brig.clone();
    brig.environment.insert(
        "TEST_BUNDLED_BRIG".into(),
        brig.executable.to_string_lossy().into_owned(),
    );
    brig.executable = fixture.root.join("trace-brig");
    fs::write(
        &brig.executable,
        "#!/bin/sh\nif test \"$1:$2:$3\" = --json:info:codex@unavailable; then exit 89; fi\nexec \"$TEST_BUNDLED_BRIG\" \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&brig.executable, fs::Permissions::from_mode(0o700)).unwrap();

    assert_eq!(candidates(&brig, &project).unwrap(), ["codex@existing"]);
}

#[test]
fn indexed_session_discovery_requires_matching_identity_and_live_project_confirmation() {
    let fixture = Fixture::new();
    let brig = fixture.fake_external();
    let project = fixture.project(AgentKind::ClaudeCode);
    let index = fixture.root.join("s/sessions.json");
    storage::write(
        &fixture.root.join("h/.brig/sessions.json"),
        &serde_json::json!({
            "custom-claude@existing":{"sandbox":"brig-custom-claude-existing","project":"/Volumes/Unavailable/project"}
        }),
    )
    .unwrap();
    storage::write(
        &index,
        &serde_json::json!({
            "custom-claude@existing":{"sandbox":"brig-custom-claude-replaced","project":"/Volumes/Unavailable/project"}
        }),
    )
    .unwrap();
    assert_eq!(
        candidates(&brig, &project).unwrap(),
        ["custom-claude@existing"]
    );

    storage::write(
        &index,
        &serde_json::json!({
            "custom-claude@existing":{"sandbox":"brig-custom-claude-existing","project":project.path}
        }),
    )
    .unwrap();
    let info_file = fixture.root.join("info.json");
    let mut info: Value = storage::read(&info_file).unwrap().unwrap();
    info["data"]["project"]["host"] = fixture
        .root
        .join("external-home")
        .to_string_lossy()
        .into_owned()
        .into();
    storage::write(&info_file, &info).unwrap();
    assert!(candidates(&brig, &project).unwrap().is_empty());
    let calls = fs::read_to_string(fixture.root.join("calls")).unwrap();
    assert_eq!(
        calls.matches("--json info custom-claude@existing").count(),
        2
    );
}

#[test]
fn real_brig_literal_credentials_are_rejected_even_when_the_plan_credential_list_is_empty() {
    let fixture = Fixture::new();
    let mut definition = fixture.brig.agent_profile("claude-code").unwrap();
    definition["name"] = "literal-claude".into();
    definition["secrets"] = serde_json::json!([]);
    definition["files"] = serde_json::json!([]);
    definition["deny"] = serde_json::json!([]);
    definition["env"] = serde_json::json!([
        {"name":"IS_SANDBOX","value":"1"},
        {"name":"CLAUDE_CONFIG_DIR","value":"/brig/claude"},
        {"name":"ANTHROPIC_API_KEY","value":"synthetic-private-value"}
    ]);
    let source = fixture.root.join("literal-claude.yaml");
    storage::write(&source, &definition).unwrap();
    fixture
        .brig
        .checked(
            &[
                "agent".into(),
                "import".into(),
                source.to_string_lossy().into(),
            ],
            None,
        )
        .unwrap();
    let mut project = fixture.project(AgentKind::ClaudeCode);
    project.profile = "literal-claude".into();
    save(&fixture.file(), &project).unwrap();
    let plan = fixture.brig.plan(&project, &Settings::default()).unwrap();
    assert_eq!(plan["data"]["credentials"], serde_json::json!([]));
    let original = fs::read(fixture.file()).unwrap();
    let error = prepare_selected(
        &fixture.brig,
        &fixture.file(),
        &Settings::default(),
        &project.id,
        &project.environment_id,
        None,
        &fixture.records(),
    )
    .err()
    .unwrap();
    assert!(error.contains("literal host credential"));
    assert!(!error.contains("synthetic-private-value"));
    assert_eq!(fs::read(fixture.file()).unwrap(), original);
    assert_eq!(fs::read_dir(fixture.root.join("y")).unwrap().count(), 0);
}

#[test]
fn unsafe_external_claude_credentials_and_private_home_shares_fail_before_adoption_or_policy_writes(
) {
    for failure in ["unknown", "resolved", "private-home"] {
        let fixture = Fixture::new();
        let claude = fixture.project(AgentKind::ClaudeCode);
        let brig = fixture
            .fake_external()
            .for_project(&claude, &fixture.root.join("records/profiles"))
            .unwrap();
        let mut plan: Value = storage::read(&fixture.root.join("plan.json"))
            .unwrap()
            .unwrap();
        let mut records = fixture.records();
        if failure == "private-home" {
            records.push(fixture.root.join("external-home/pending-records"));
        }
        if failure != "private-home" {
            plan["data"]["credentials"] =
                serde_json::json!([{"name":"claude-credentials","state":failure}]);
            storage::write(&fixture.root.join("plan.json"), &plan).unwrap();
        }
        let original = fs::read(fixture.file()).unwrap();
        let error = prepare_selected(
            &brig,
            &fixture.file(),
            &Settings::default(),
            &claude.id,
            &claude.environment_id,
            None,
            &records,
        )
        .err()
        .unwrap();
        assert!(
            error.contains("credential") || error.contains("records folders"),
            "{error}"
        );
        assert_eq!(fs::read(fixture.file()).unwrap(), original);
        let calls = fs::read_to_string(fixture.root.join("calls")).unwrap();
        assert!(!calls.contains("policy create") && !calls.contains("policy attach"));
    }
}

#[test]
fn codex_candidates_keep_the_saved_profile_when_inventory_metadata_disagrees() {
    let fixture = Fixture::new();
    let brig = fixture.fake_external();
    let mut inventory: Value = storage::read(&fixture.root.join("inventory.json"))
        .unwrap()
        .unwrap();
    inventory["data"]["sandboxes"][0]["agent"] = "codex".into();
    storage::write(&fixture.root.join("inventory.json"), &inventory).unwrap();
    assert!(candidates(&brig, &fixture.project(AgentKind::Codex))
        .unwrap()
        .is_empty());
}

#[test]
fn claude_candidates_skip_unclassified_hull_sandboxes_and_keep_custom_profiles() {
    let fixture = Fixture::new();
    let brig = fixture.fake_external();
    let mut inventory: Value = storage::read(&fixture.root.join("inventory.json"))
        .unwrap()
        .unwrap();
    let sandboxes = inventory["data"]["sandboxes"].as_array_mut().unwrap();
    sandboxes.insert(
        0,
        serde_json::json!({
            "sandbox":"brig-codex-proof-existing",
            "state":"running",
            "workspace":fixture.root.join("unclassified-home")
        }),
    );
    sandboxes.insert(
        1,
        serde_json::json!({
            "agent":null,
            "sandbox":"brig-other-unclassified",
            "state":"stopped",
            "workspace":fixture.root.join("other-home")
        }),
    );
    storage::write(&fixture.root.join("inventory.json"), &inventory).unwrap();
    assert_eq!(
        candidates(&brig, &fixture.project(AgentKind::ClaudeCode)).unwrap(),
        vec!["custom-claude@existing"]
    );
}

#[test]
fn identified_agent_candidates_still_require_their_reference() {
    let fixture = Fixture::new();
    let brig = fixture.fake_external();
    for kind in [AgentKind::Codex, AgentKind::ClaudeCode] {
        let project = fixture.project(kind);
        let inventory = serde_json::json!({
            "apiVersion":"brig.sh/v1alpha1",
            "kind":"SandboxList",
            "data":{"sandboxes":[{"agent":project.profile,"state":"running"}]}
        });
        storage::write(&fixture.root.join("inventory.json"), &inventory).unwrap();
        assert_eq!(
            candidates(&brig, &project).unwrap_err(),
            "The Brig session identity is missing."
        );
    }
}

#[test]
fn each_agent_checks_offline_resources_signature_ports_skills_and_credential_states() {
    let fixture = Fixture::new();
    let brig = fixture.fake_external();
    let base: Value = storage::read(&fixture.root.join("plan.json"))
        .unwrap()
        .unwrap();
    for kind in [AgentKind::Codex, AgentKind::ClaudeCode] {
        let mut project = fixture.project(kind);
        project.policy = Some(Policy {
            name: "external-policy".into(),
            owned: false,
            fingerprint: None,
        });
        let mut plan = base.clone();
        plan["data"]["profile"] = project.profile.clone().into();
        plan["data"]["session"] = project.label.clone().into();
        assert!(brig::validate_plan(&plan, &project, &Settings::default()).is_ok());
        for (field, invalid) in [
            ("posture", serde_json::json!("offline")),
            ("limits", serde_json::json!({"cpus":1,"mem":1024})),
            ("verify", serde_json::json!({"mode":"off"})),
            ("ports", serde_json::json!([{"host":8080,"guest":80}])),
            ("skills", serde_json::json!(["/host/skill"])),
            (
                "credentials",
                serde_json::json!([{"name":"host-token","state":"unknown"}]),
            ),
            (
                "credentials",
                serde_json::json!([{"name":"host-token","state":"resolved"}]),
            ),
        ] {
            let mut changed = plan.clone();
            changed["data"][field] = invalid;
            assert!(
                brig::validate_plan(&changed, &project, &Settings::default()).is_err(),
                "{kind:?}: {field}"
            );
        }
        let mut offline_settings = Settings::default();
        offline_settings.network = settings::Network::Offline;
        plan["data"]["posture"] = "offline".into();
        assert!(brig::validate_plan(&plan, &project, &offline_settings).is_ok());
    }
    drop(brig);
}

#[test]
fn recheck_preparation_does_not_clear_a_pending_attempt_stage() {
    let fixture = Fixture::new();
    let prepared = fixture.prepare(&fixture.project(AgentKind::Codex));
    let mut project = prepared.project;
    project
        .begin_environment(crate::projects::EnvironmentAttempt::pending(
            "in-flight".into(),
            52,
            prepared.fingerprint.unwrap(),
        ))
        .unwrap();
    save(&fixture.file(), &project).unwrap();
    let reopened = fixture.prepare(&project);
    assert!(reopened.project.environment_attempt.is_some());
    assert_eq!(
        reopened.project.stage,
        crate::projects::ProjectStage::EnvironmentStarting
    );
}

#[test]
fn recheck_setup_preserves_an_unfinished_stop() {
    let fixture = Fixture::new();
    let mut project = fixture.prepare(&fixture.project(AgentKind::Codex)).project;
    project.stop_environment().unwrap();
    save(&fixture.file(), &project).unwrap();
    let reopened = fixture.prepare(&project);
    assert_eq!(
        reopened.project.stage,
        crate::projects::ProjectStage::Stopping
    );
}

#[test]
fn recheck_a_second_admission_cannot_replace_a_live_attempt_before_registration() {
    let fixture = Fixture::new();
    let prepared = fixture.prepare(&fixture.project(AgentKind::Codex));
    let mut project = prepared.project;
    let plan = prepared.fingerprint.unwrap();
    project
        .begin_environment(crate::projects::EnvironmentAttempt::pending(
            "one".into(),
            21,
            plan.clone(),
        ))
        .unwrap();
    let second = project.begin_environment(crate::projects::EnvironmentAttempt::pending(
        "two".into(),
        22,
        plan,
    ));
    assert!(second.is_err());
    assert!(!project.recover_preparation(true).unwrap());
    assert_eq!(project.environment_attempt.as_ref().unwrap().token, "one");
    assert!(project.recover_preparation(false).unwrap());
    assert_eq!(project.stage, crate::projects::ProjectStage::Ready);
    assert!(project.environment_attempt.as_ref().unwrap().completed);
    assert!(project
        .begin_environment(crate::projects::EnvironmentAttempt::pending(
            "retry".into(),
            23,
            "new-plan".into(),
        ))
        .is_ok());
}

#[test]
fn recheck_an_already_attached_policy_keeps_a_valid_verification_receipt() {
    let fixture = Fixture::new();
    let prepared = fixture.prepare(&fixture.project(AgentKind::Codex));
    let mut project = prepared.project;
    let mut receipt = crate::projects::EnvironmentAttempt::pending(
        "done".into(),
        12,
        prepared.fingerprint.unwrap(),
    );
    receipt.finish(Ok(())).unwrap();
    project.environment_attempt = Some(receipt);
    project.environment_verified = Some(true);
    save(&fixture.file(), &project).unwrap();
    let before = fs::read(fixture.file()).unwrap();
    let expected = project
        .policy
        .as_ref()
        .unwrap()
        .fingerprint
        .as_deref()
        .unwrap();
    let _result = crate::projects::project_policy::change(
        &fixture.scoped(&project),
        &fixture.file(),
        &Settings::default(),
        &project.id,
        &project.environment_id,
        expected,
        crate::projects::project_policy::Action::Attach,
        vec![],
        &fixture.records(),
    );
    let current = fixture.project(AgentKind::Codex);
    assert_eq!(current.environment_verified, Some(true));
    assert!(current.environment_attempt.is_some());
    assert_eq!(fs::read(fixture.file()).unwrap(), before);
}
