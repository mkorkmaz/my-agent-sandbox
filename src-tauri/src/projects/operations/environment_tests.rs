use super::*;
use crate::agents::AgentKind;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "tugboat-environment-removal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("project")).unwrap();
        let fixture = Self(root);
        let mut registry = Registry::default();
        let codex = registry
            .register(&fixture.0.join("project"), "codex")
            .unwrap();
        registry
            .ensure_environment(
                &codex.id,
                AgentKind::ClaudeCode,
                &settings::Settings::default(),
            )
            .unwrap();
        for environment in registry.environments(&codex.id).unwrap().iter() {
            let mut project = registry
                .environment(&codex.id, &environment.environment_id)
                .unwrap();
            let name = format!("{}-policy", project.profile);
            let document = serde_json::json!({"apiVersion":"brig.sh/v1alpha1","kind":"Policy","metadata":{"name":name},"spec":{"egress":{"default":"deny"}}});
            project.policy = Some(projects::Policy {
                name: name.clone(),
                owned: true,
                fingerprint: Some(brig::fingerprint(&document).unwrap()),
            });
            project.owns_binding = true;
            project.stage = crate::projects::ProjectStage::EnvironmentReady;
            registry.update(project.clone()).unwrap();
            fs::write(
                fixture.0.join(format!("policy-{name}.json")),
                document.to_string(),
            )
            .unwrap();
            fs::write(
                fixture.0.join(format!("environment-{}", project.profile)),
                project.reference(),
            )
            .unwrap();
            fs::write(
                fixture.0.join(format!("binding-{name}")),
                project.reference(),
            )
            .unwrap();
            fs::write(fixture.0.join(format!("info-{}.json", project.profile)), serde_json::json!({"apiVersion":"brig.sh/v1alpha1","kind":"Info","data":{"profile":project.profile,"session":project.label,"project":{"host":project.path},"workspace":fixture.0.join(format!("home-{}",project.profile))}}).to_string()).unwrap();
            fs::create_dir(fixture.0.join(format!("home-{}", project.profile))).unwrap();
            fs::write(
                project
                    .ledger_file(&fixture.0)
                    .unwrap()
                    .with_extension("proof"),
                "history",
            )
            .unwrap_or_else(|_| {
                let path = project
                    .ledger_file(&fixture.0)
                    .unwrap()
                    .with_extension("proof");
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, "history").unwrap();
            });
        }
        storage::write(&fixture.file(), &registry).unwrap();
        fs::write(fixture.0.join("brig"), r#"#!/bin/sh
root=${0%/*}
printf '%s\n' "$*" >> "$root/calls"
case "$1:$2" in
--json:ls)
    printf '{"apiVersion":"brig.sh/v1alpha1","kind":"SandboxList","data":{"sandboxes":['
    comma=''
    for file in "$root"/environment-*; do
        test -f "$file" || continue
        reference=$(cat "$file"); profile=${reference%@*}; state=running
        test ! -f "$root/stopped-$profile" || state=stopped
        printf '%s{"ref":"%s","state":"%s"}' "$comma" "$reference" "$state"; comma=,
    done
    printf ']}}\n';;
--json:info) profile=${3%@*}; cat "$root/info-$profile.json";;
stop:*) profile=${2%@*}; touch "$root/stopped-$profile";;
rm:*) profile=${2%@*}
    if test "$3" = --dry-run; then printf 'Remove this fixture environment\n'; exit 0; fi
    rm -f "$root/environment-$profile"
    test ! -f "$root/fail-rm" || exit 9;;
sh:*) printf '%s\n' "$*" >> "$root/logouts";;
policy:check) profile=$3; test ! -f "$root/binding-$profile-policy" || printf '%s-policy\n' "$profile";;
policy:detach) rm -f "$root/binding-$3";;
policy:show) if test -f "$root/policy-$3.json"; then cat "$root/policy-$3.json"; else printf 'unknown policy' >&2; exit 1; fi;;
policy:ls) for file in "$root"/policy-*.json; do test -f "$file" || continue; name=${file##*/policy-}; name=${name%.json}; printf '%s\n' "$name"; done;;
policy:rm) rm -f "$root/policy-$3.json";;
*) exit 8;;
esac
"#).unwrap();
        fs::set_permissions(fixture.0.join("brig"), fs::Permissions::from_mode(0o700)).unwrap();
        fixture
    }
    fn file(&self) -> PathBuf {
        self.0.join("projects.json")
    }
    fn registry(&self) -> Registry {
        Registry::load(&self.file()).unwrap()
    }
    fn project(&self, kind: AgentKind) -> Project {
        let registry = self.registry();
        let record = &registry.projects[0];
        registry
            .environment(
                &record.id,
                &record.environments.by_kind(kind).unwrap().environment_id,
            )
            .unwrap()
    }
    fn brig(&self) -> Brig {
        Brig {
            executable: self.0.join("brig"),
            environment: crate::platform::command::environment(),
        }
    }
    fn remove(&self, project: &Project, logout: bool) -> Result<String, String> {
        let brig = self.brig();
        let current = preview(&brig, project)?;
        continue_removal_with(
            &brig,
            &self.file(),
            &project.id,
            &project.environment_id,
            &current.fingerprint,
            &project.reference(),
            logout,
            guest_logout,
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn removing_claude_preserves_codex_registration_policy_home_and_history() {
    let fixture = Fixture::new();
    let codex = fixture.project(AgentKind::Codex);
    let claude = fixture.project(AgentKind::ClaudeCode);
    let original = serde_json::to_value(&codex).unwrap();
    fixture.remove(&claude, false).unwrap();
    assert_eq!(
        serde_json::to_value(fixture.registry().project(&codex.id).unwrap()).unwrap(),
        original
    );
    assert!(fixture
        .0
        .join(format!("environment-{}", codex.profile))
        .exists());
    assert!(fixture
        .0
        .join(format!("policy-{}-policy.json", codex.profile))
        .exists());
    assert!(fixture.0.join(format!("home-{}", codex.profile)).exists());
    assert!(claude
        .ledger_file(&fixture.0)
        .unwrap()
        .with_extension("proof")
        .exists());
    assert!(fixture
        .registry()
        .environment(&codex.id, &claude.environment_id)
        .is_err());
    let calls = fs::read_to_string(fixture.0.join("calls")).unwrap();
    assert!(
        !calls.lines().any(|line| line.contains(&codex.reference())),
        "{calls}"
    );
    assert!(!fixture.0.join("logouts").exists());
}

#[test]
fn interrupted_claude_removal_recovers_only_its_environment_and_uses_claude_logout() {
    let fixture = Fixture::new();
    let codex = fixture.project(AgentKind::Codex);
    let claude = fixture.project(AgentKind::ClaudeCode);
    fs::write(fixture.0.join("fail-rm"), "once").unwrap();
    assert!(fixture.remove(&claude, true).is_err());
    let registry = fixture.registry();
    let pending = registry
        .environment(&claude.id, &claude.environment_id)
        .unwrap();
    assert_eq!(pending.stage, "removing");
    assert!(pending.removal.as_ref().unwrap().logged_out);
    assert_eq!(
        serde_json::to_value(registry.project(&codex.id).unwrap()).unwrap(),
        serde_json::to_value(&codex).unwrap()
    );
    fs::remove_file(fixture.0.join("fail-rm")).unwrap();
    fixture.remove(&pending, true).unwrap();
    assert_eq!(
        fixture
            .registry()
            .environments(&codex.id)
            .unwrap()
            .iter()
            .count(),
        1
    );
    let logouts = fs::read_to_string(fixture.0.join("logouts")).unwrap();
    assert_eq!(
        logouts.trim(),
        format!("sh {} claude auth logout", claude.reference())
    );
    let calls = fs::read_to_string(fixture.0.join("calls")).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("stop "))
            .count(),
        1
    );
}

#[test]
fn project_removal_preview_covers_both_environment_resources_and_rejects_wrong_identity() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let codex = fixture.project(AgentKind::Codex);
    let claude = fixture.project(AgentKind::ClaudeCode);
    let targets = removal_target::select(&registry, &codex.id, None).unwrap();
    let snapshots: Vec<_> = targets
        .iter()
        .map(|project| (project.clone(), preview(&fixture.brig(), project).unwrap()))
        .collect();
    let aggregate = removal_target::combine(&snapshots).unwrap();
    assert_eq!(aggregate.environments.len(), 2);
    assert_eq!(aggregate.reference, codex.name);
    for project in [&codex, &claude] {
        assert!(aggregate.description.contains(&project.profile));
        assert!(aggregate.description.contains(&project.label));
        assert!(aggregate
            .description
            .contains(&format!("home-{}", project.profile)));
    }
    assert_eq!(
        removal_target::select(&registry, &codex.id, Some(&claude.environment_id))
            .unwrap()
            .len(),
        1
    );
    assert!(removal_target::select(
        &registry,
        &codex.id,
        Some(&EnvironmentId::for_agent(
            &"b".repeat(32),
            AgentKind::ClaudeCode
        ))
    )
    .is_err());
}

#[test]
fn removing_codex_keeps_claude_accessible_and_recreation_has_a_fresh_codex_environment() {
    let fixture = Fixture::new();
    let codex = fixture.project(AgentKind::Codex);
    let claude = fixture.project(AgentKind::ClaudeCode);
    fixture.remove(&codex, false).unwrap();
    let mut registry = fixture.registry();
    assert_eq!(
        registry.project(&codex.id).unwrap().agent_kind,
        AgentKind::ClaudeCode
    );
    assert_eq!(registry.project_views().unwrap().len(), 1);
    assert!(registry.codex_projects().unwrap().is_empty());
    assert_eq!(
        registry
            .register(&claude.path, "codex")
            .unwrap()
            .environment_id,
        claude.environment_id
    );
    let rebuilt = registry
        .ensure_environment(&codex.id, AgentKind::Codex, &settings::Settings::default())
        .unwrap();
    assert_eq!(rebuilt.stage, "selected");
    assert!(rebuilt.policy.is_none());
    assert_eq!(
        registry
            .environment(&claude.id, &claude.environment_id)
            .unwrap()
            .reference(),
        claude.reference()
    );
}
