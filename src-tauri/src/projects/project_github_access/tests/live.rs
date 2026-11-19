use super::*;
use crate::projects::project_policy;
use crate::projects::project_preview::tests::live::{register, Proof, PROOF_POLICY};

struct OwnedHostPolicy<'a> {
    proof: &'a Proof,
    project: &'a Project,
    name: String,
}

impl OwnedHostPolicy<'_> {
    fn bind(&self, action: &str, name: &str) -> Result<(), String> {
        let args = [
            "policy",
            action,
            name,
            &self.project.profile,
            "-n",
            &self.project.label,
        ]
        .map(str::to_owned);
        self.proof
            .brig
            .checked(&args, Some(&self.project.path))
            .map(drop)
    }
}

impl Drop for OwnedHostPolicy<'_> {
    fn drop(&mut self) {
        let detached = self.bind("detach", &self.name).is_ok();
        let restored = self.bind("attach", PROOF_POLICY).is_ok();
        let removed = self
            .proof
            .brig
            .checked(&["policy".into(), "rm".into(), self.name.clone()], None)
            .is_ok();
        if !(detached && restored && removed) {
            eprintln!("Review the disposable host policy cleanup.");
        }
    }
}

struct GrantCleanup<'a> {
    proof: &'a Proof,
    keychain: &'a Keychain,
    project: &'a Project,
}
impl GrantCleanup<'_> {
    fn change(&self, action: Action, input: Option<(String, Vec<u8>)>) -> Result<View, String> {
        let mut registry = Registry::load(&self.proof.file)?;
        let project = registry.environment(&self.project.id, &self.project.environment_id)?;
        let _operation =
            environment::EnvironmentOperation::begin(&project.id, &project.environment_id)?;
        apply(
            &mut registry,
            &self.proof.file,
            &project,
            &self.proof.settings,
            &self.proof.brig,
            self.keychain,
            action,
            input,
        )
    }
}
impl Drop for GrantCleanup<'_> {
    fn drop(&mut self) {
        if self.change(Action::Revoke, None).is_err() {
            eprintln!(
                "Synthetic GitHub grant cleanup requires review in the disposable proof records."
            );
        }
    }
}

#[test]
#[ignore = "requires the managed disposable preview proof guests, a native Mac Keychain and cached signed image"]
fn saves_and_revokes_a_synthetic_git_grant_through_the_native_service() {
    let root = PathBuf::from(
        std::env::var_os("TUGBOAT_GITHUB_ACCESS_PROOF_ROOT")
            .expect("Set the private proof directory."),
    );
    let records = Fixture::new();
    let mut proof = Proof::new(&root, records.0.join("projects.json"));
    proof.brig.enable_proof_keychain(&root).unwrap();
    let mut registry = Registry::default();
    let mut first = register(&mut registry, &root, "a");
    let sibling = register(&mut registry, &root, "b");
    let inventory = proof.inventory();
    let rows = inventory["data"]["sandboxes"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    for project in [&first, &sibling] {
        let home = proof.brig.shared_project_home(project).unwrap();
        assert!(rows.iter().any(|row| row["state"] == "running"
            && row["ref"] == project.reference()
            && row["workspace"].as_str() == home.to_str()));
        let info = super::super::super::operations::verify_target(&proof.brig, project).unwrap();
        assert_eq!(info["data"]["workspace"].as_str(), home.to_str());
        proof
            .brig
            .verify_running_image(
                project,
                rows.iter()
                    .find(|row| row["ref"] == project.reference())
                    .unwrap()["sandbox"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
        println!(
            "Verified disposable GitHub grant target {} at {}.",
            project.reference(),
            home.display()
        );
    }
    let host_policy = proof.brig.policy(PROOF_POLICY).unwrap().unwrap();
    let initial_hosts = ["api.anthropic.com", "api.openai.com"].map(str::to_owned);
    let name = format!("github-host-proof-{}", first.label);
    assert!(proof.brig.policy(&name).unwrap().is_none());
    let owned_document = json!({"apiVersion":"brig.sh/v1alpha1","kind":"Policy","metadata":{"name":name},"spec":{"egress":{"default":"deny","allow":initial_hosts.iter().map(|host| json!({"host":host})).collect::<Vec<_>>()}}});
    storage::write(
        &root.join("policies").join(format!("{name}.yaml")),
        &owned_document,
    )
    .unwrap();
    first.policy = Some(crate::projects::Policy {
        name: name.clone(),
        owned: true,
        fingerprint: Some(
            crate::platform::brig::fingerprint(&proof.brig.policy(&name).unwrap().unwrap())
                .unwrap(),
        ),
    });
    first.owns_binding = true;
    registry.update(first.clone()).unwrap();
    storage::write(&proof.file, &registry).unwrap();
    let sibling_boot = proof.boot_id(&sibling);
    proof.stop(&first);
    let owned = OwnedHostPolicy {
        proof: &proof,
        project: &first,
        name,
    };
    owned.bind("detach", PROOF_POLICY).unwrap();
    owned.bind("attach", &owned.name).unwrap();
    let keychain = Keychain::host().unwrap();
    let changes = GrantCleanup {
        proof: &proof,
        keychain: &keychain,
        project: &first,
    };
    assert_eq!(
        changes.change(Action::Read, None).unwrap().status,
        Status::None
    );
    let input = |value: String| Some(("demo/private".into(), keychain::token(value).unwrap()));
    assert_eq!(
        changes
            .change(Action::Save, input(TOKEN.into()))
            .unwrap()
            .status,
        Status::Saved
    );
    let read = changes.change(Action::Read, None).unwrap();
    assert_eq!(
        (read.status, read.repository.as_deref()),
        (Status::Saved, Some("demo/private"))
    );
    assert_eq!(
        changes
            .change(Action::Save, input(format!("{TOKEN}replacement")))
            .unwrap()
            .status,
        Status::Saved
    );
    let registry = Registry::load(&proof.file).unwrap();
    let [codex, claude] = registry.agent_views(&first.id).unwrap();
    assert_eq!(codex.github_access, claude.github_access);
    let grant = codex.github_access.unwrap();
    assert!(keychain.contains(&grant.name).unwrap());
    assert!(!fs::read_to_string(&proof.file).unwrap().contains(TOKEN));
    let route_before = proof.brig.policy(&grant.name).unwrap().unwrap();
    let edited_hosts = ["api.anthropic.com", "api.openai.com", "claude.ai"]
        .map(str::to_owned)
        .to_vec();
    let edited = project_policy::change(
        &proof.brig,
        &proof.file,
        &proof.settings,
        &first.id,
        &first.environment_id,
        first
            .policy
            .as_ref()
            .unwrap()
            .fingerprint
            .as_deref()
            .unwrap(),
        project_policy::Action::Save,
        edited_hosts,
        &[],
    )
    .unwrap();
    let host_policy_after_edit = proof.brig.policy(&owned.name).unwrap().unwrap();
    assert!(host_policy_after_edit["spec"]["egress"]["allow"]
        .as_array()
        .unwrap()
        .iter()
        .any(|rule| rule["host"] == "claude.ai"));
    assert_eq!(
        proof.brig.policy(&grant.name).unwrap(),
        Some(route_before.clone())
    );
    assert_eq!(edited.github_access.as_ref(), Some(&grant));
    assert_eq!(
        changes.change(Action::Read, None).unwrap().status,
        Status::Saved
    );
    let grant_file = root.join("policies").join(format!("{}.yaml", grant.name));
    let source = fs::read(&grant_file).unwrap();
    let mut changed = grant.source();
    changed["metadata"]["description"] = "External proof edit".into();
    storage::write(&grant_file, &changed).unwrap();
    let refused = changes.change(Action::Read, None);
    storage::write_bytes_existing(&grant_file, &source).unwrap();
    assert!(refused.unwrap_err().contains("changed outside Tugboat"));
    proof.boot(&first);
    let first_boot = proof.boot_id(&first);
    assert!(changes.change(Action::Read, None).unwrap().running);
    let result = changes.change(Action::Revoke, None).unwrap();
    assert_eq!(result.status, Status::None);
    assert!(result.running);
    assert!(!keychain.contains(&grant.name).unwrap());
    assert!(proof.brig.policy(&grant.name).unwrap().is_none());
    assert_eq!(
        proof.brig.policy(&owned.name).unwrap(),
        Some(host_policy_after_edit)
    );
    assert_eq!(proof.brig.policy(PROOF_POLICY).unwrap(), Some(host_policy));
    assert_eq!(proof.boot_id(&first), first_boot);
    assert_eq!(proof.boot_id(&sibling), sibling_boot);
    assert_eq!(
        changes.change(Action::Read, None).unwrap().status,
        Status::None
    );
    println!("Saved, replaced, and revoked one synthetic Mac Keychain grant without restarting either guest during revocation.");
}
