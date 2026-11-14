// Included only inside environment_history_tests in disposable source copies.
// The driver keeps the same generated native files and inodes for both builds.
fn frozen_export(directory: &Path, name: &str, value: &impl Serialize) {
    fs::create_dir_all(directory).unwrap();
    fs::write(directory.join(name), serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn frozen_seed(watch: &Watch, name: &str) {
    if name == "codex-family" {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures/codex-subagents/v0.167.0");
        let directory = watch.home.join(".codex/sessions");
        fs::create_dir_all(&directory).unwrap();
        for name in ["primary.jsonl", "child-alpha.jsonl", "child-beta.jsonl",
            "grandchild-gamma.jsonl", "normal-fork.jsonl",
            "p02/history-primary.jsonl", "p02/history-child.jsonl"] {
            let file = Path::new(name).file_name().unwrap().to_str().unwrap();
            fs::copy(source.join(name), directory.join(format!("rollout-{file}"))).unwrap();
        }
        return;
    }
    if name == "claude-main" {
        write_claude(watch);
        return;
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/claude-code").join(name);
    let root = claude_file(watch, CHAT);
    fs::copy(source.join("root.jsonl"), &root).unwrap();
    let directory = root.with_extension("").join("subagents");
    for agent in ["alpha", "beta", "nested"] {
        let target = if agent == "nested" { directory.join("workflows/run-1") } else { directory.clone() };
        fs::create_dir_all(&target).unwrap();
        for extension in ["jsonl", "meta.json"] {
            fs::copy(source.join(format!("agent-{agent}.{extension}")),
                target.join(format!("agent-{agent}.{extension}"))).unwrap();
        }
    }
}

fn frozen_snapshot(watch: &Watch, root: &str, dashboard: Dashboard) -> Value {
    let ledger = frozen_read(watch);
    let details = frozen_details(watch, root, &ledger);
    let update = DashboardUpdate { dashboard: dashboard.clone(), sequence: 1 };
    let resumes = dashboard.conversations.iter().map(|row| {
        (row.id.clone(), resume_dashboard(watch, &row.id).is_ok())
    }).collect::<std::collections::BTreeMap<_,_>>();
    json!({"dashboard":dashboard,"details":details,"update":update,
        "event":(PROJECT,&update),"resumeEligibility":resumes,"ledger":ledger})
}

#[test]
#[ignore = "driver requires a marked generated fixture and a separate export directory"]
fn sqlite_frozen_full_contract_export() {
    let base = PathBuf::from(std::env::var_os("TUGBOAT_SQLITE_FROZEN_ROOT").unwrap());
    let output = PathBuf::from(std::env::var_os("TUGBOAT_SQLITE_FROZEN_OUTPUT").unwrap());
    assert!(fs::canonicalize(&base).unwrap().starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
    assert_eq!(fs::read(base.join(".frozen-contract-fixture")).unwrap(), b"generated-full-sqlite-contract-v1");
    let baseline = std::env::var("TUGBOAT_SQLITE_FROZEN_BACKEND").unwrap() == "json";
    for name in ["codex-family", "claude-main", "priced-subagents", "subagents"] {
        let case = base.join(name);
        if baseline { fs::create_dir(&case).unwrap(); }
        let fixture = Fixture(case);
        let kind = if name == "codex-family" { AgentKind::Codex } else { AgentKind::ClaudeCode };
        let mut watch = fixture.watch(kind);
        let root = if name == "codex-family" { "00000000-0000-4000-8000-000000000001" } else { CHAT };
        if name == "codex-family" { watch.project = "/work/fixture-project".into(); }
        if baseline { frozen_seed(&watch, name); }
        let directory = output.join(name);
        frozen_prepare(&mut watch, &base, name);
        let initial = synchronize(&watch).unwrap().0;
        let (rows, requests, tokens, cost, unpriced) = match name {
            "codex-family" => (7, 21, 320_782, 0.0914808, 0),
            "claude-main" => (1, 5, 82_948, 0.055747, 0),
            "priced-subagents" => (4, 4, 100_976, 0.0599405375, 0),
            _ => (4, 4, 1_350, 0.0, 4),
        };
        assert_eq!(initial.conversations.len(), rows, "{name} rows");
        assert_eq!(initial.usage.requests, requests, "{name} requests");
        assert_eq!(initial.usage.total, tokens, "{name} tokens");
        assert!((initial.usage.cost_usd - cost).abs() < 1e-12, "{name} cost");
        assert_eq!(initial.usage.unpriced_requests, unpriced, "{name} unknown prices");
        if baseline {
            fs::create_dir_all(base.join("inputs").join(name)).unwrap();
            fs::copy(&watch.ledger, base.join("inputs").join(name).join("ledger.json")).unwrap();
        }
        let before = frozen_read(&watch);
        let mut snapshots = vec![frozen_snapshot(&watch, root, initial.clone())];
        let history = History::default();
        history.record_published(PROJECT, &initial).unwrap();
        let quiet = synchronize(&watch).unwrap();
        assert!(!quiet.1);
        assert!(!history.should_publish(PROJECT, &quiet.0).unwrap());
        let blocked = change_state(&watch, root, SessionState::Archived, |_| true).is_err();
        assert!(blocked);
        assert_eq!(serde_json::to_value(frozen_read(&watch)).unwrap(), serde_json::to_value(&before).unwrap());
        for state in [SessionState::Archived, SessionState::Deleted, SessionState::Active] {
            change_state(&watch, root, state, |_| false).unwrap();
            snapshots.push(frozen_snapshot(&watch, root, synchronize(&watch).unwrap().0));
        }
        frozen_export(&directory, "trace.json", &json!({"initialQuietChanged":quiet.1,
            "quietEventSuppressed":true,"runningFamilyArchiveRejected":blocked,"snapshots":snapshots}));
        frozen_finish(&watch);
        // The other backend must read these exact native files and inodes.
        std::mem::forget(fixture);
    }
}

// FROZEN_BACKEND_ADAPTER
