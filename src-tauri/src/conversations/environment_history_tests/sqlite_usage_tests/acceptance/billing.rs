use super::*;

fn metadata(five: u64, hour: u64) -> Value {
    json!({"cache_creation":{"ephemeral_5m_input_tokens":five,"ephemeral_1h_input_tokens":hour},"speed":"standard","service_tier":"standard","inference_geo":"global"})
}

fn wrong_provider_rate() -> usage::RateSnapshot {
    serde_json::from_value(json!({"date":"2025-01-02","source":usage::PRICING_SOURCE,"input":2.0,"cached":0.1,"write30m":2.5,"write12h":5.0,"output":10.0,"tier":"standard","tierAssumed":true,"longContext":false})).unwrap()
}

#[test]
fn billing_metadata_tariffs_and_ordered_evidence_match_after_sql_reopen() {
    let mut evidence = Vec::new();
    for (index, name) in [
        "missing",
        "nullable",
        "malformed",
        "split-5m-1h",
        "fast-us",
        "batch",
        "priority",
        "fast-batch",
        "haiku-100000",
        "haiku-100001",
        "zero",
        "partial-priced",
        "unsupported-model",
        "unknown-cache",
        "removed-historical-model",
        "wrong-provider",
        "ordered-corrections",
    ]
    .into_iter()
    .enumerate()
    {
        let fixture = Fixture::new(&format!("sqlite-acceptance-billing-{name}"));
        let legacy = fixture.watch(AgentKind::ClaudeCode);
        let path = claude_file(&legacy, CHAT);
        let mut counts = json!({"input_tokens":100,"cache_read_input_tokens":40,"cache_creation_input_tokens":20,"output_tokens":10});
        let mut observed = metadata(12, 8);
        match name {
            "missing" => observed = json!({}),
            "nullable" => {
                observed = json!({"cache_creation":null,"speed":null,"service_tier":null,"inference_geo":null})
            }
            "malformed" => {
                observed = json!({"cache_creation":"invalid","speed":17,"service_tier":[],"inference_geo":false})
            }
            "fast-us" => {
                observed["speed"] = json!("fast");
                observed["inference_geo"] = json!("us");
            }
            "batch" => observed["service_tier"] = json!("batch"),
            "priority" => observed["service_tier"] = json!("priority"),
            "fast-batch" => {
                observed["speed"] = json!("fast");
                observed["service_tier"] = json!("batch");
            }
            "haiku-100000" => counts["input_tokens"] = json!(99_940),
            "haiku-100001" => counts["input_tokens"] = json!(99_941),
            "zero" => {
                counts = json!({"input_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":0});
                observed = metadata(0, 0);
            }
            _ => {}
        }
        for (key, value) in observed.as_object().unwrap() {
            counts[key] = value.clone();
        }
        let mut response =
            claude_response("msg_acceptance_price", counts, name != "partial-priced");
        response["message"]["model"] = json!(if name.starts_with("haiku") {
            "claude-haiku-5-5"
        } else {
            "claude-opus-5-5"
        });
        if name == "unsupported-model" {
            response["message"]["model"] = json!("claude-fixture");
        }
        fs::write(&path, format!("{response}\n")).unwrap();
        synchronize(&legacy).unwrap();
        let mut raw = legacy.selected_ledger().unwrap();
        let request = &mut raw
            .sessions
            .get_mut(CHAT)
            .unwrap()
            .turns
            .get_mut("msg_acceptance_price")
            .unwrap()
            .requests[0];
        if name == "unknown-cache" {
            request.tokens.cache_known = false;
            request.tokens.cached_known = Some(false);
            request.tokens.cache_write_known = Some(false);
            request.cost_usd = None;
        }
        if name == "removed-historical-model" {
            request.model = Some("claude-previously-verified-model".into());
            let rate = request.rates.as_mut().unwrap();
            rate.date = "2026-10-01".into();
            rate.anthropic.as_mut().unwrap().model = request.model.clone().unwrap();
        }
        if name == "wrong-provider" {
            request.rates = Some(wrong_provider_rate());
            request.cost_usd = Some(7.25);
        }
        if name == "ordered-corrections" {
            request.rate_correction = Some(serde_json::from_value(json!({"previousRates":wrong_provider_rate(),"previousCostUsd":0.0,"reason":"historical rate reason"})).unwrap());
            request.amount_correction = Some(usage::AmountCorrection {
                previous_tokens: request.tokens.clone(),
                previous_cost_usd: None,
                previous_partial: true,
                previous_unpriced_reason: Some("historical unknown amount".into()),
                reason: "historical amount reason".into(),
            });
            request.anthropic_corrections = vec![
                usage::anthropic::evidence(request, "first historical correction"),
                usage::anthropic::evidence(request, "second historical correction"),
            ];
        }
        let (sql, original) = imported(&fixture, &legacy, &raw);
        let original_bytes = fs::read(&original).unwrap();
        let backup = storage::v1_backup_path(&original);
        let backup_bytes = fs::read(&backup).unwrap();
        let target = sql.target.as_ref().unwrap();
        let store = sql.store.as_ref().unwrap();
        let environment = target.usage_environment().unwrap();
        assert_eq!(
            value(
                &store
                    .with(|store| store.load_environment(&environment))
                    .unwrap()
            ),
            value(&raw),
            "{name} raw parity"
        );
        storage::write(&legacy.ledger, &raw).unwrap();
        let legacy_records = RecordsDirectory::fixture(&fixture.0);
        fs::remove_dir_all(&legacy.home).unwrap();
        let expected = environment_history::load_saved_ledger(
            AgentKind::ClaudeCode,
            &legacy.ledger,
            &legacy_records,
        )
        .unwrap();
        let expected_dashboard = environment_history::saved_with_records(
            legacy.target.as_ref(),
            &legacy.ledger,
            Some(&legacy_records),
        )
        .unwrap();
        let expected_details = usage_details::from_ledger(&expected, CHAT).unwrap();
        if index % 2 == 0 {
            sql_backend::dashboard(target, store, true).unwrap();
        }
        if index % 2 == 1 {
            sql_backend::details(target, store, CHAT, |_| Ok(None)).unwrap();
        }
        assert_eq!(
            value(
                &store
                    .with(|store| store.load_environment(&environment))
                    .unwrap()
            ),
            value(&expected),
            "{name} prepared parity"
        );
        assert_eq!(
            value(&sql_backend::dashboard(target, store, true).unwrap()),
            value(&expected_dashboard),
            "{name} dashboard"
        );
        assert_eq!(
            value(&sql_backend::details(target, store, CHAT, |_| Ok(None)).unwrap()),
            value(&expected_details),
            "{name} details"
        );
        let request = &expected.sessions[CHAT].turns["msg_acceptance_price"].requests[0];
        assert!(request.tokens.write_12h.is_none());
        if [
            "malformed",
            "priority",
            "fast-batch",
            "unsupported-model",
            "unknown-cache",
        ]
        .contains(&name)
        {
            assert!(request.cost_usd.is_none(), "{name}");
        }
        if name == "malformed" {
            assert!(!request
                .anthropic
                .as_ref()
                .unwrap()
                .invalid_fields
                .is_empty());
        }
        if name == "zero" {
            assert_eq!(request.cost_usd, Some(0.0));
        }
        if name == "partial-priced" {
            assert!(request.partial && request.cost_usd.is_some());
        }
        if name == "haiku-100000" {
            assert_eq!(request.rates.as_ref().unwrap().input, 0.1);
        }
        if name == "haiku-100001" {
            assert_eq!(request.rates.as_ref().unwrap().input, 0.5);
        }
        if name == "removed-historical-model" {
            assert_eq!(request.rates.as_ref().unwrap().date, "2026-10-01");
        }
        if name == "wrong-provider" {
            assert!(request.provider_correction.is_some());
        }
        if name == "ordered-corrections" {
            assert_eq!(
                request
                    .anthropic_corrections
                    .iter()
                    .map(|value| value.reason.as_str())
                    .collect::<Vec<_>>(),
                [
                    "first historical correction",
                    "second historical correction"
                ]
            );
        }
        let changes = store
            .with(|store| Ok(store.connection.total_changes()))
            .unwrap();
        sql_backend::dashboard(target, store, true).unwrap();
        assert_eq!(
            store
                .with(|store| Ok(store.connection.total_changes()))
                .unwrap(),
            changes
        );
        store.close().unwrap();
        let reopened =
            SharedStore::new(Store::open(sql.records_directory.as_ref().unwrap()).unwrap());
        assert_eq!(
            value(
                &reopened
                    .with(|store| store.load_environment(&environment))
                    .unwrap()
            ),
            value(&expected),
            "{name} reopen"
        );
        assert_eq!(fs::read(&original).unwrap(), original_bytes);
        assert_eq!(fs::read(&backup).unwrap(), backup_bytes);
        evidence.push(json!({"case":name,"request":request,"summary":expected_dashboard.usage,"coverage":expected_dashboard.coverage,"userPromptCount":expected_details.user_prompt_count,"ledgerSha256":crate::hash::hex(Sha256::digest(serde_json::to_vec(&expected).unwrap())),"dashboardSha256":crate::hash::hex(Sha256::digest(serde_json::to_vec(&expected_dashboard).unwrap())),"detailsSha256":crate::hash::hex(Sha256::digest(serde_json::to_vec(&expected_details).unwrap()))}));
        reopened.close().unwrap();
    }
    emit("billing-matrix", &evidence);
}

#[test]
fn live_claude_recovery_conflict_and_output_growth_keep_dated_evidence_after_replay() {
    let fixture = Fixture::new("sqlite-acceptance-live-corrections");
    let legacy = fixture.watch(AgentKind::ClaudeCode);
    let source = claude_file(&legacy, CHAT);
    let mut initial = claude_response("msg_live_acceptance", claude_counts(1), false);
    initial["message"].as_object_mut().unwrap().remove("model");
    fs::write(&source, format!("{initial}\n")).unwrap();
    synchronize(&legacy).unwrap();
    let (sql, _original) = imported(&fixture, &legacy, &legacy.selected_ledger().unwrap());
    synchronize(&sql).unwrap();
    let mut invalid = claude_response("msg_live_acceptance", claude_counts(3), false);
    invalid["message"]["model"] = json!("claude-opus-5-5");
    invalid["message"]["usage"]["cache_creation"] =
        json!({"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":0});
    fs::write(&source, format!("{initial}\n{invalid}\n")).unwrap();
    assert_eq!(
        value(&synchronize(&sql).unwrap().0),
        value(&synchronize(&legacy).unwrap().0)
    );
    let mut dated = legacy.selected_ledger().unwrap();
    let rate = dated
        .sessions
        .get_mut(CHAT)
        .unwrap()
        .turns
        .get_mut("msg_live_acceptance")
        .unwrap()
        .requests[0]
        .rates
        .as_mut()
        .unwrap();
    rate.date = "2026-10-01".into();
    rate.input *= 2.0;
    rate.cached *= 2.0;
    rate.output *= 2.0;
    rate.anthropic.as_mut().unwrap().write_5m *= 2.0;
    rate.anthropic.as_mut().unwrap().write_1h *= 2.0;
    write_ledger(&legacy.ledger, &mut dated).unwrap();
    sql.store
        .as_ref()
        .unwrap()
        .with(|store| store.save_environment(&environment(&sql), &dated))
        .unwrap();
    let mut recovered = claude_response("msg_live_acceptance", claude_counts(8), true);
    recovered["message"]["model"] = json!("claude-opus-5-5");
    recovered["message"]["usage"]["cache_creation"] =
        json!({"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":1});
    recovered["message"]["usage"]["speed"] = json!("fast");
    recovered["message"]["usage"]["service_tier"] = json!("standard");
    recovered["message"]["usage"]["inference_geo"] = json!("us");
    fs::write(&source, format!("{initial}\n{invalid}\n{recovered}\n")).unwrap();
    let expected = synchronize(&legacy).unwrap().0;
    let actual = synchronize(&sql).unwrap().0;
    assert_eq!(value(&actual), value(&expected));
    let mut conflicting = recovered.clone();
    conflicting["message"]["usage"]["speed"] = json!("standard");
    conflicting["message"]["usage"]["service_tier"] = json!("batch");
    conflicting["message"]["usage"]["output_tokens"] = json!(12);
    fs::write(
        &source,
        format!("{initial}\n{invalid}\n{recovered}\n{conflicting}\n"),
    )
    .unwrap();
    let expected = synchronize(&legacy).unwrap().0;
    let actual = synchronize(&sql).unwrap().0;
    assert_eq!(value(&actual), value(&expected));
    assert_eq!(actual.usage.requests, 1);
    let facts = sql.load().unwrap();
    let request = &facts.sessions[CHAT].turns["msg_live_acceptance"].requests[0];
    assert_eq!(request.tokens.output, 12);
    assert_eq!(request.rates.as_ref().unwrap().date, "2026-10-01");
    assert!(request.anthropic_corrections.len() >= 2);
    assert_eq!(
        request.anthropic.as_ref().unwrap().service_tier.as_deref(),
        Some("standard")
    );
    assert!(facts.sessions[CHAT].warning.is_some());
    for _ in 0..3 {
        let result = synchronize(&sql).unwrap();
        assert!(!result.1);
        assert_eq!(value(&result.0), value(&actual));
    }
    assert_eq!(value(&sql.load().unwrap()), value(&facts));
    emit(
        "live-claude-corrections",
        &json!({"ledger":facts,"dashboard":actual,"ledgerSha256":crate::hash::hex(Sha256::digest(serde_json::to_vec(&facts).unwrap()))}),
    );
}

#[test]
fn saved_pricing_sweeps_only_the_requested_environment_and_rejects_future_policy() {
    let fixture = Fixture::new("sqlite-acceptance-pricing-environment-scope");
    let legacy = fixture.watch(AgentKind::ClaudeCode);
    write_claude(&legacy);
    synchronize(&legacy).unwrap();
    let raw = unpriced(legacy.selected_ledger().unwrap());
    fs::remove_dir_all(&legacy.home).unwrap();
    let directory = fixture.0.join("records");
    fs::create_dir(&directory).unwrap();
    let records = RecordsDirectory::fixture(&directory);
    let other_project = "2".repeat(32);
    let other = crate::usage_store::Environment::new(
        other_project.clone(),
        projects::EnvironmentId::for_agent(&other_project, AgentKind::ClaudeCode),
        AgentKind::ClaudeCode,
    )
    .unwrap();
    let target = legacy.target.as_ref().unwrap();
    let first = target.usage_environment().unwrap();
    for environment in [&first, &other] {
        storage::write(
            &directory
                .join("conversation-ledgers")
                .join(&environment.project_id)
                .join(format!("{}.json", environment.environment_id)),
            &raw,
        )
        .unwrap();
    }
    importer::activate(&records).unwrap();
    let store = SharedStore::new(Store::open(&records).unwrap());
    let other_ids=store.with(|store|store.connection.prepare("SELECT r.record_id FROM usage_environments e JOIN session_groups g ON g.environment_key=e.environment_key JOIN session_records r ON r.session_key=g.session_key WHERE e.project_id=?1").map_err(crate::usage_store::error)?.query_map([&other_project],|row|row.get::<_,i64>(0)).map_err(crate::usage_store::error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(crate::usage_store::error)).unwrap();
    crate::usage_store::record_reads::begin();
    sql_backend::dashboard(target, &store, true).unwrap();
    let reads = crate::usage_store::record_reads::finish();
    assert!(reads.iter().all(|record| !other_ids.contains(record)));
    let untouched = store.with(|store| store.load_environment(&other)).unwrap();
    assert_eq!(value(&untouched), value(&raw));
    let versions=store.with(|store|store.connection.prepare("SELECT project_id,claude_pricing_version FROM usage_environments ORDER BY project_id").map_err(crate::usage_store::error)?.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?))).map_err(crate::usage_store::error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(crate::usage_store::error)).unwrap();
    assert_eq!(
        versions,
        vec![(PROJECT.into(), 1), (other_project.clone(), 0)]
    );
    let changes = store
        .with(|store| Ok(store.connection.total_changes()))
        .unwrap();
    sql_backend::dashboard(target, &store, true).unwrap();
    assert_eq!(
        store
            .with(|store| Ok(store.connection.total_changes()))
            .unwrap(),
        changes
    );
    store
        .with(|store| {
            store
                .connection
                .execute(
                    "UPDATE usage_environments SET claude_pricing_version=2 WHERE project_id=?1",
                    [&other_project],
                )
                .map(|_| ())
                .map_err(crate::usage_store::error)
        })
        .unwrap();
    let future = environment_history::Target {
        project_id: other_project,
        environment_id: other.environment_id.clone(),
        agent_kind: AgentKind::ClaudeCode,
    };
    assert!(sql_backend::dashboard(&future, &store, true).is_err());
    assert_eq!(
        value(&store.with(|store| store.load_environment(&other)).unwrap()),
        value(&raw)
    );
    emit(
        "pricing-environment-scope",
        &json!({"versionsAfterFirstOpen":versions,"rawOtherEnvironmentSha256":crate::hash::hex(Sha256::digest(serde_json::to_vec(&raw).unwrap())),"rawRecordsRead":reads,"otherRecordIdsExcluded":other_ids,"repeatChanges":0,"futureVersionRejected":true}),
    );
}
