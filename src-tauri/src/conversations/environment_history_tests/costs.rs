use super::*;

fn priced_response(id: &str, output: u64, complete: bool) -> Value {
    let mut value = claude_response(id, claude_counts(output), complete);
    value["message"]["model"] = json!("claude-opus-5-5");
    value
}

fn stored_request(watch: &Watch, id: &str) -> usage::RequestUsage {
    load_ledger(&watch.ledger).unwrap().sessions[CHAT].turns[id].requests[0].clone()
}

fn write(path: &Path, values: &[Value]) {
    fs::write(
        path,
        values
            .iter()
            .map(|value| format!("{value}\n"))
            .collect::<String>(),
    )
    .unwrap();
}

#[test]
fn late_model_and_billing_metadata_recover_using_saved_rates_and_keep_corrections() {
    let fixture = Fixture::new("claude-price-recovery");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    let path = claude_file(&watch, CHAT);
    let mut initial = priced_response("msg_recover_price", 1, false);
    initial["message"].as_object_mut().unwrap().remove("model");
    write(&path, &[initial.clone()]);
    let unpriced = synchronize(&watch).unwrap().0;
    assert_eq!(unpriced.usage.unpriced_requests, 1);
    let mut invalid = priced_response("msg_recover_price", 3, false);
    invalid["message"]["usage"]["cache_creation"] =
        json!({"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":0});
    write(&path, &[initial.clone(), invalid.clone()]);
    assert_eq!(synchronize(&watch).unwrap().0.usage.unpriced_requests, 1);
    let mut ledger = load_ledger(&watch.ledger).unwrap();
    let rate = ledger
        .sessions
        .get_mut(CHAT)
        .unwrap()
        .turns
        .get_mut("msg_recover_price")
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
    write_ledger(&watch.ledger, &mut ledger).unwrap();
    let mut recovered = priced_response("msg_recover_price", 8, true);
    recovered["message"]["usage"]["cache_creation"] =
        json!({"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":1});
    recovered["message"]["usage"]["speed"] = json!("fast");
    recovered["message"]["usage"]["service_tier"] = json!("standard");
    recovered["message"]["usage"]["inference_geo"] = json!("us");
    write(&path, &[initial.clone(), invalid, recovered]);
    let priced = synchronize(&watch).unwrap().0;
    assert_eq!(priced.usage.requests, 1);
    assert_eq!(priced.usage.unpriced_requests, 0);
    assert!(
        (priced.usage.cost_usd - (2.0 * 4.0 + 10.0 * 0.2 + 8.0 + 8.0 * 20.0) * 2.0 * 2.2 / 1e6)
            .abs()
            < 1e-12
    );
    let request = stored_request(&watch, "msg_recover_price");
    assert_eq!(request.rates.as_ref().unwrap().date, "2026-10-01");
    assert!(!request.partial);
    assert!(request
        .anthropic
        .as_ref()
        .unwrap()
        .invalid_fields
        .is_empty());
    assert!(request.anthropic_corrections.len() >= 2);
    assert!(request
        .anthropic_corrections
        .iter()
        .any(|correction| correction
            .previous_metadata
            .as_ref()
            .is_some_and(|metadata| metadata
                .invalid_fields
                .iter()
                .any(|field| field == "cache_creation"))));
    write(&path, &[initial]);
    assert_eq!(synchronize(&watch).unwrap().0.usage, priced.usage);
    assert_eq!(
        stored_request(&watch, "msg_recover_price")
            .anthropic_corrections
            .len(),
        request.anthropic_corrections.len()
    );
    fs::remove_file(path).unwrap();
    assert_eq!(
        environment_history::saved(watch.target.as_ref(), &watch.ledger)
            .unwrap()
            .usage,
        priced.usage
    );
}

#[test]
fn schema_two_unpriced_records_backfill_offline_once_and_preserve_existing_backup() {
    let fixture = Fixture::new("claude-offline-backfill");
    let records = crate::storage::data_location::RecordsDirectory::fixture(&fixture.0);
    let watch = fixture.watch(AgentKind::ClaudeCode);
    write_claude(&watch);
    let expected = synchronize(&watch).unwrap().0.usage;
    let mut legacy: Value = serde_json::from_slice(&fs::read(&watch.ledger).unwrap()).unwrap();
    for turn in legacy["sessions"][CHAT]["turns"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        for request in turn["requests"].as_array_mut().unwrap() {
            request.as_object_mut().unwrap().remove("anthropic");
            request
                .as_object_mut()
                .unwrap()
                .remove("anthropicCorrections");
            request["rates"] = Value::Null;
            request["costUsd"] = Value::Null;
            request["unpricedReason"] = json!("Anthropic API price is unavailable");
        }
    }
    fs::write(&watch.ledger, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let backup = storage::v1_backup_path(&watch.ledger);
    fs::write(&backup, "prior immutable backup").unwrap();
    fs::remove_dir_all(&watch.home).unwrap();
    let original = fs::read(&watch.ledger).unwrap();
    let unverified = environment_history::saved(watch.target.as_ref(), &watch.ledger).unwrap();
    assert_eq!(unverified.usage.unpriced_requests, 5);
    assert_eq!(fs::read(&watch.ledger).unwrap(), original);
    let offline = environment_history::saved_with_records(
        watch.target.as_ref(),
        &watch.ledger,
        Some(&records),
    )
    .unwrap();
    assert_eq!(offline.usage, expected);
    let once = fs::read(&watch.ledger).unwrap();
    assert_eq!(
        fs::read_to_string(&backup).unwrap(),
        "prior immutable backup"
    );
    let ledger = load_ledger(&watch.ledger).unwrap();
    assert_eq!(ledger.schema_version, 2);
    for request in ledger.sessions[CHAT]
        .turns
        .values()
        .flat_map(|turn| &turn.requests)
    {
        assert_eq!(request.anthropic_corrections.len(), 1);
        assert!(request.anthropic_corrections[0].previous_rates.is_none());
        assert_eq!(
            request.rates.as_ref().unwrap().date,
            usage::anthropic::PRICING_DATE
        );
    }
    assert_eq!(
        environment_history::saved_with_records(
            watch.target.as_ref(),
            &watch.ledger,
            Some(&records)
        )
        .unwrap()
        .usage,
        expected
    );
    assert_eq!(fs::read(&watch.ledger).unwrap(), once);
    assert_eq!(
        fs::read_to_string(&backup).unwrap(),
        "prior immutable backup"
    );
}

#[test]
fn removed_or_replaced_records_locations_cannot_create_locks_or_backfill_files() {
    let fixture = Fixture::new("claude-backfill-storage-guard");
    let location = fixture.0.join("records");
    fs::create_dir(&location).unwrap();
    let records = crate::storage::data_location::RecordsDirectory::fixture(&location);
    let mut watch = fixture.watch(AgentKind::ClaudeCode);
    watch.records_directory = Some(records.clone());
    watch.ledger = location.join("conversation-ledgers/claude.json");
    write_claude(&watch);
    let expected = synchronize(&watch).unwrap().0;
    let history = History::default();
    let key = watch.target.as_ref().unwrap().key();
    let moved = fixture.0.join("original-records");
    fs::rename(&location, &moved).unwrap();
    history.refresh(&key, &watch).unwrap();
    assert!(!location.exists());
    assert_eq!(history.merged(PROJECT).unwrap().usage.requests, 0);
    assert!(environment_history::saved_with_records(
        watch.target.as_ref(),
        &watch.ledger,
        Some(&records)
    )
    .is_err());
    assert!(!location.exists());
    assert!(
        environment_history::saved(watch.target.as_ref(), &watch.ledger)
            .unwrap()
            .conversations
            .is_empty()
    );
    assert!(!location.exists());
    fs::create_dir_all(location.join("conversation-ledgers")).unwrap();
    fs::write(
        location.join(".tugboat-data.json"),
        json!({"schemaVersion":1,"id":"b".repeat(32),"complete":true}).to_string(),
    )
    .unwrap();
    let original = fs::read(moved.join("conversation-ledgers/claude.json")).unwrap();
    fs::write(&watch.ledger, &original).unwrap();
    history.refresh(&key, &watch).unwrap();
    assert!(!watch.ledger.with_extension("json.lock").exists());
    assert_eq!(fs::read(&watch.ledger).unwrap(), original);
    assert_eq!(
        fs::read(moved.join("conversation-ledgers/claude.json")).unwrap(),
        original
    );
    let saved = environment_history::saved(watch.target.as_ref(), &watch.ledger).unwrap();
    assert_eq!(saved.usage, expected.usage);
    assert!(!watch.ledger.with_extension("json.lock").exists());
}

#[test]
fn malformed_billing_metadata_stays_unpriced_offline_until_valid_recorded_metadata_recovers() {
    let fixture = Fixture::new("claude-invalid-price-metadata");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    let path = claude_file(&watch, CHAT);
    let mut invalid = priced_response("msg_invalid_price", 7, true);
    invalid["message"]["usage"]["speed"] = json!({"invalid":"fast"});
    write(&path, &[invalid]);
    let first = synchronize(&watch).unwrap().0;
    assert_eq!(first.usage.requests, 1);
    assert_eq!(first.usage.unpriced_requests, 1);
    assert_eq!(first.usage.total, 20);
    fs::remove_file(&path).unwrap();
    let offline = environment_history::saved(watch.target.as_ref(), &watch.ledger).unwrap();
    assert_eq!(offline.usage, first.usage);
    let invalid = stored_request(&watch, "msg_invalid_price");
    assert_eq!(invalid.anthropic.unwrap().invalid_fields, ["speed"]);
    let mut valid = priced_response("msg_invalid_price", 7, true);
    valid["message"]["usage"]["speed"] = json!("standard");
    write(&path, &[valid]);
    let recovered = synchronize(&watch).unwrap().0;
    assert_eq!(recovered.usage.unpriced_requests, 0);
    assert_eq!(recovered.usage.total, first.usage.total);
    assert_eq!(
        stored_request(&watch, "msg_invalid_price")
            .anthropic_corrections
            .len(),
        1
    );
}

#[test]
fn partial_recorded_prices_zero_and_unknown_usage_keep_separate_knownness() {
    let fixture = Fixture::new("claude-mixed-cost-knownness");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    let path = claude_file(&watch, CHAT);
    let mut zero = priced_response("msg_zero_price", 0, true);
    zero["message"]["usage"] = json!({"input_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":0,"cache_creation":null});
    let unknown = claude_response("msg_unknown_price", claude_counts(2), true);
    let missing = priced_response("msg_unknown_amount", 2, true);
    let mut missing = missing;
    missing["message"]["usage"]["output_tokens"] = json!(-1);
    let partial = priced_response("msg_partial_price", 1, false);
    write(&path, &[zero, unknown, missing, partial]);
    let result = synchronize(&watch).unwrap().0;
    assert_eq!(result.usage.requests, 3);
    assert_eq!(result.usage.unpriced_requests, 1);
    assert_eq!(result.usage.unmeasured_turns, 1);
    assert_eq!(stored_request(&watch, "msg_zero_price").cost_usd, Some(0.0));
    let partial = stored_request(&watch, "msg_partial_price");
    assert!(partial.partial && partial.cost_usd.is_some());
    assert_eq!(result.usage.cost_usd, partial.cost_usd.unwrap());
    assert_eq!(result.coverage, usage::Coverage::Partial);
}

#[test]
fn conflicting_billing_metadata_retains_verified_fields_and_merges_output_growth_once() {
    let fixture = Fixture::new("claude-price-conflict-growth");
    let watch = fixture.watch(AgentKind::ClaudeCode);
    let path = claude_file(&watch, CHAT);
    let mut initial = priced_response("msg_price_conflict", 1, false);
    initial["message"]["usage"]["speed"] = json!("standard");
    initial["message"]["usage"]["service_tier"] = json!("standard");
    initial["message"]["usage"]["inference_geo"] = json!("global");
    initial["message"]["usage"]["cache_creation"] =
        json!({"ephemeral_5m_input_tokens":1,"ephemeral_1h_input_tokens":0});
    write(&path, &[initial.clone()]);
    synchronize(&watch).unwrap();
    let before = stored_request(&watch, "msg_price_conflict");
    let mut growth = initial.clone();
    growth["message"]["usage"]["output_tokens"] = json!(9);
    growth["message"]["stop_reason"] = json!("end_turn");
    growth["message"]["usage"]["speed"] = json!("fast");
    growth["message"]["usage"]["cache_creation"] =
        json!({"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":1});
    write(&path, &[initial, growth]);
    let updated = synchronize(&watch).unwrap().0;
    assert_eq!(updated.usage.requests, 1);
    assert_eq!(updated.usage.output, 9);
    let request = stored_request(&watch, "msg_price_conflict");
    assert_eq!(request.anthropic, before.anthropic);
    assert_eq!(
        request.anthropic_corrections.len(),
        before.anthropic_corrections.len()
    );
    assert_eq!(
        request.cost_usd,
        Some((2.0 * 4.0 + 10.0 * 0.2 + 5.0 + 9.0 * 20.0) / 1e6)
    );
    assert!(updated.conversations[0]
        .warning
        .as_deref()
        .unwrap()
        .contains("Conflicting"));
    for _ in 0..3 {
        assert_eq!(synchronize(&watch).unwrap().0.usage, updated.usage);
        assert_eq!(
            stored_request(&watch, "msg_price_conflict")
                .anthropic_corrections
                .len(),
            request.anthropic_corrections.len()
        );
    }
}
