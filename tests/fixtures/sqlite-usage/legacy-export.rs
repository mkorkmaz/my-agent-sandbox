// Included only in a disposable pre-migration source snapshot by the baseline runner.
// The helpers in environment_history_tests create native transcripts below a temp folder.
fn sqlite_export_file(directory: &Path, name: &str, value: &impl Serialize) {
    fs::create_dir_all(directory).unwrap();
    fs::write(
        directory.join(name),
        serde_json::to_vec_pretty(value).unwrap(),
    )
    .unwrap();
}

fn sqlite_export_family(watch: &Watch, name: &str) {
    let root = claude_file(watch, CHAT);
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/claude-code")
        .join(name);
    fs::copy(fixtures.join("root.jsonl"), &root).unwrap();
    let directory = root.with_extension("").join("subagents");
    for agent in ["alpha", "beta", "nested"] {
        let destination = if agent == "nested" {
            directory.join("workflows/run-1")
        } else {
            directory.clone()
        };
        fs::create_dir_all(&destination).unwrap();
        for extension in ["jsonl", "meta.json"] {
            fs::copy(
                fixtures.join(format!("agent-{agent}.{extension}")),
                destination.join(format!("agent-{agent}.{extension}")),
            )
            .unwrap();
        }
    }
}

#[test]
fn sqlite_baseline_contract_exports() {
    let Some(output) = std::env::var_os("TUGBOAT_SQLITE_BASELINE_EXPORT") else {
        return;
    };
    let output = PathBuf::from(output);
    for (name, kind) in [
        ("codex", AgentKind::Codex),
        ("claude-main", AgentKind::ClaudeCode),
        ("claude-priced-family", AgentKind::ClaudeCode),
        ("claude-unsupported-family", AgentKind::ClaudeCode),
    ] {
        let fixture = Fixture::new(&format!("sqlite-baseline-{name}"));
        let watch = fixture.watch(kind);
        match name {
            "codex" => {
                write_codex(&watch);
            }
            "claude-main" => {
                write_claude(&watch);
            }
            "claude-priced-family" => sqlite_export_family(&watch, "priced-subagents"),
            _ => sqlite_export_family(&watch, "subagents"),
        }
        let (dashboard, changed) = synchronize(&watch).unwrap();
        assert!(changed);
        let ledger = load_ledger(&watch.ledger).unwrap();
        let details = usage_details::from_ledger(&ledger, CHAT).unwrap();
        let directory = output.join(name);
        sqlite_export_file(&directory, "ledger.json", &ledger);
        sqlite_export_file(&directory, "dashboard.json", &dashboard);
        sqlite_export_file(&directory, "details.json", &details);
        sqlite_export_file(
            &directory,
            "dashboard-update.json",
            &DashboardUpdate {
                dashboard: dashboard.clone(),
                sequence: 1,
            },
        );
        let repeated = synchronize(&watch).unwrap();
        assert!(!repeated.1);
        assert!(dashboard == repeated.0);
        sqlite_export_file(
            &directory,
            "saved-dashboard.json",
            &environment_history::saved(watch.target.as_ref(), &watch.ledger).unwrap(),
        );
        sqlite_export_file(
            &directory,
            "fixture.json",
            &json!({"projectId": PROJECT, "environmentId": projects::EnvironmentId::for_agent(PROJECT, kind), "agentKind":kind,"guestPath":watch.home,"pricingDate":usage::PRICING_DATE,"anthropicPricingDate":usage::anthropic::PRICING_DATE}),
        );
        if name == "claude-main" {
            let mut offline: Value = serde_json::to_value(&ledger).unwrap();
            let record = &mut offline["sessions"][CHAT];
            for turn in record["turns"].as_object_mut().unwrap().values_mut() {
                for request in turn["requests"].as_array_mut().unwrap() {
                    request["anthropic"] = Value::Null;
                    request["rates"] = Value::Null;
                    request["costUsd"] = Value::Null;
                    request["anthropicCorrections"] = json!([]);
                    request["unpricedReason"] = json!("Legacy fixture price is unavailable");
                }
            }
            let historical = record.clone();
            offline["variants"] = json!({CHAT:{"historical-guest":historical}});
            let records = crate::data_location::RecordsDirectory::fixture(&fixture.0);
            fs::remove_dir_all(&watch.home).unwrap();
            storage::write(&watch.ledger, &offline).unwrap();
            let backup = storage::v1_backup_path(&watch.ledger);
            fs::write(&backup, b"immutable historical v1 fixture backup\n").unwrap();
            sqlite_export_file(&directory, "offline-raw-ledger.json", &offline);
            let prepared =
                environment_history::load_saved_ledger(kind, &watch.ledger, &records).unwrap();
            sqlite_export_file(&directory, "offline-prepared-ledger.json", &prepared);
            sqlite_export_file(
                &directory,
                "offline-details.json",
                &usage_details::from_ledger(&prepared, CHAT).unwrap(),
            );
            sqlite_export_file(
                &directory,
                "offline-dashboard.json",
                &environment_history::saved_with_records(
                    watch.target.as_ref(),
                    &watch.ledger,
                    Some(&records),
                )
                .unwrap(),
            );
            assert_eq!(
                fs::read(&backup).unwrap(),
                b"immutable historical v1 fixture backup\n"
            );
        }
    }
}
