use super::*;
use crate::{
    platform::{command, host},
    projects::project_preview::tests::live::{register, Proof, PROOF_POLICY},
};
use std::{sync::atomic::AtomicBool, time::Duration};

fn boot_old(proof: &Proof, project: &Project, reference: &str, label: &str, home: Option<&Path>) {
    let policy = ["policy", "attach", PROOF_POLICY, "codex", "-n", label].map(str::to_owned);
    proof
        .brig
        .checked(&policy, Some(&project.path))
        .expect("Attach the private proof policy to the former project.");
    let mut arguments = [
        "--json",
        "--verbose",
        "run",
        "-d",
        "--network",
        "isolated",
        "--image",
        &project.image_ref,
    ]
    .map(str::to_owned)
    .to_vec();
    if let Some(home) = home {
        arguments.extend(["--home".into(), home.to_string_lossy().into_owned()]);
    }
    arguments.extend([
        reference.into(),
        project.path.to_string_lossy().into_owned(),
    ]);
    let output = command::run(
        &proof.brig.executable,
        &arguments,
        Some(&project.path),
        &proof.brig.environment,
        Duration::from_secs(180),
        &AtomicBool::new(false),
    )
    .unwrap();
    let run = host::envelope(&output, "Run")
        .expect("Boot the former disposable guest from the signed image.");
    assert_eq!(run["data"]["stage"], "detached");
    assert_eq!(run["data"]["ref"], reference);
    assert!(output.stderr.contains("image and boot assets verified"));
}

#[test]
#[ignore = "requires a separate private Hull cache, macOS HVI, and verified Brig v0.5.2"]
fn resets_a_real_stopped_former_project_and_preserves_the_disposable_external_guest() {
    let root = PathBuf::from(
        std::env::var_os("TUGBOAT_RESET_PROOF_ROOT")
            .expect("Set an unused private Brig proof directory with a signed image cache."),
    );
    let records = root.join("reset-records");
    let proof = Proof::new(&root, records.join("projects.json"));
    let resume = std::env::var("TUGBOAT_RESET_PROOF_RESUME").as_deref() == Ok("1");
    let records = if resume {
        assert!(records.join(CHECKPOINT).is_file());
        RecordsDirectory::fixture_existing(&records, &"a".repeat(32))
    } else {
        assert_eq!(proof.inventory()["data"]["sandboxes"], json!([]));
        fs::create_dir(&records).unwrap();
        RecordsDirectory::fixture(&records)
    };
    let current = &proof.file;
    let mut registry = Registry::default();
    let first = register(&mut registry, &root, "a");
    let sibling = register(&mut registry, &root, "b");
    let first_old = "codex@retired-codex";
    let sibling_old = "codex@retired-peer";
    let first_sandbox = "brig-codex-retired-codex";
    let sibling_sandbox = "brig-codex-retired-peer";
    let retired_home = root.join("state/homes").join(first_sandbox);
    let external = root.join("named-external");
    let external_home = root.join("named-external-retired-peer");
    println!(
        "Former owned target {first_old} uses {}.",
        retired_home.display()
    );
    println!(
        "Former external target {sibling_old} uses {}.",
        external_home.display()
    );
    println!(
        "Fresh target {} uses {}.",
        first.reference(),
        proof.brig.shared_project_home(&first).unwrap().display()
    );
    println!(
        "Private transition records are in {}.",
        records.path().display()
    );
    let ledger = records
        .path()
        .join("conversation-ledgers")
        .join(format!("{}.json", first.id));
    if !resume {
        let policy_path = root.join("policies").join(format!("{PROOF_POLICY}.yaml"));
        if !policy_path.exists() {
            let policy = json!({"apiVersion":"brig.sh/v1alpha1","kind":"Policy","metadata":{"name":PROOF_POLICY},"spec":{"egress":{"default":"deny"}}});
            storage::write(&policy_path, &policy).unwrap();
        }
        boot_old(&proof, &first, first_old, "retired-codex", None);
        boot_old(
            &proof,
            &sibling,
            sibling_old,
            "retired-peer",
            Some(&external),
        );
        let list = proof.inventory();
        let rows = list["data"]["sandboxes"].as_array().unwrap();
        for (reference, sandbox, home) in [
            (first_old, first_sandbox, &retired_home),
            (sibling_old, sibling_sandbox, &external_home),
        ] {
            assert!(rows.iter().any(|row| row["ref"] == reference
                && row["sandbox"] == sandbox
                && row["workspace"].as_str() == home.to_str()
                && row["state"] == "running"));
        }
        fs::write(
            retired_home.join("retired-chat-fixture"),
            b"synthetic old transcript",
        )
        .unwrap();
        fs::write(
            external_home.join("external-canary"),
            b"external guest state",
        )
        .unwrap();
        fs::write(first.path.join("worktree-canary"), b"project worktree").unwrap();
        fs::write(sibling.path.join("sibling-canary"), b"unrelated worktree").unwrap();
        fs::create_dir(records.path().join("chatgpt")).unwrap();
        fs::write(
            records.path().join("chatgpt/account-canary"),
            b"provider stays on the Mac",
        )
        .unwrap();
        let old_a = former(&registry, &first, &[AgentKind::Codex]);
        let mut old_b = former(&registry, &sibling, &[AgentKind::Codex]);
        old_b["environments"][0]["label"] = "retired-peer".into();
        old_b["environments"][0]["ownsSession"] = false.into();
        let old_registry = json!({"schemaVersion":2,"projects":[old_a,old_b]});
        storage::write(current, &old_registry).unwrap();
        storage::write(&storage::v1_backup_path(current), &old_registry).unwrap();
        let info = transition::plan(&fs::read(current).unwrap(), &proof.settings)
            .unwrap()
            .unwrap();
        let old_project = &info
            .retired
            .iter()
            .find(|project| project.project_id == first.id)
            .unwrap()
            .environments[0];
        assert_eq!(ledger, ledger_path(old_project, records.path()));
        storage::write(&ledger, &json!({"formerFixture":true})).unwrap();
        storage::write(
            &storage::v1_backup_path(&ledger),
            &json!({"formerFixture":true}),
        )
        .unwrap();
        let db = records.path().join("usage.sqlite3");
        let connection = Connection::open(&db).unwrap();
        fs::set_permissions(&db, fs::Permissions::from_mode(0o600)).unwrap();
        connection.execute_batch(V1).unwrap();
        connection
            .execute("INSERT INTO store_identity VALUES (1,?1)", [records.id()])
            .unwrap();
        connection.execute("INSERT INTO usage_environments(project_id,environment_id,agent_kind) VALUES (?1,?2,'codex')", params![first.id,old_project.environment_id.to_string()]).unwrap();
        drop(connection);
        let stop = ["stop", first_old].map(str::to_owned);
        proof.brig.checked(&stop, Some(&first.path)).unwrap();
    }
    activate(
        &records,
        &proof.settings,
        || Ok(proof.brig.clone()),
        || Ok(()),
    )
    .unwrap();
    activate(
        &records,
        &proof.settings,
        || panic!("No old Brig runtime after a completed reset."),
        || Ok(()),
    )
    .unwrap();
    assert!(!ledger.exists());
    assert!(!storage::v1_backup_path(&ledger).exists());
    assert!(!storage::v1_backup_path(current).exists());
    assert!(!records.path().join(CHECKPOINT).exists());
    assert!(!retired_home.exists());
    let list = proof.inventory();
    let rows = list["data"]["sandboxes"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["ref"], sibling_old);
    assert_eq!(rows[0]["state"], "running");
    assert_eq!(
        fs::read(external_home.join("external-canary")).unwrap(),
        b"external guest state"
    );
    assert_eq!(
        fs::read(first.path.join("worktree-canary")).unwrap(),
        b"project worktree"
    );
    assert_eq!(
        fs::read(sibling.path.join("sibling-canary")).unwrap(),
        b"unrelated worktree"
    );
    assert_eq!(
        fs::read(records.path().join("chatgpt/account-canary")).unwrap(),
        b"provider stays on the Mac"
    );
    let store = Store::open(&records).unwrap();
    let fresh = Registry::load(current).unwrap();
    let [codex, claude] = fresh.agent_views(&first.id).unwrap();
    assert_eq!(codex.reference(), claude.reference());
    for project in [&codex, &claude] {
        let env = Environment::new(
            project.id.clone(),
            project.environment_id.clone(),
            project.agent_kind,
        )
        .unwrap();
        assert!(store.load_environment(&env).unwrap().warnings.is_empty());
    }
    assert_eq!(
        store
            .connection
            .query_row("SELECT COUNT(*) FROM usage_environments", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    store.close().unwrap();
    crate::environment::managed_profile::install(&root.join("profiles")).unwrap();
    proof.attach_policy(&codex);
    proof.boot(&codex);
    assert!(!proof.boot_id(&codex).is_empty());
    assert_eq!(
        proof.brig.shared_project_home(&codex).unwrap(),
        proof.brig.shared_project_home(&claude).unwrap()
    );
    println!("Activated one fresh shared guest after removing the old guest and history. The external sibling and provider fixture remain.");
}
