use super::*;
use serde_json::json;

fn request(model: Option<&str>, metadata: Value) -> RequestUsage {
    let mut request = super::super::request(
        "anthropic:msg_test".into(),
        "2026-11-13T00:00:00Z".into(),
        model.map(str::to_owned),
        None,
        Tokens {
            input: 160,
            cached: 40,
            cache_write: 20,
            output: 10,
            cache_known: true,
            cached_known: Some(true),
            cache_write_known: Some(true),
            ..Default::default()
        },
        false,
    );
    request.rates = None;
    request.cost_usd = None;
    request.source = Some(super::super::RequestSource {
        guest_namespace: "fixture".into(),
        model_provider: "anthropic".into(),
        origin_thread_id: "root".into(),
        origin_turn_id: Some("msg_test".into()),
        root_turn_id: None,
        response_id: "msg_test".into(),
    });
    request.anthropic = Some(parse(&metadata, request.tokens.cache_write));
    refresh(&mut request);
    request
}

fn close(actual: Option<f64>, expected: f64) {
    assert!(
        (actual.unwrap() - expected).abs() < 1e-12,
        "{actual:?} != {expected}"
    );
}

fn recorded(five: u64, hour: u64) -> Value {
    json!({"speed":"standard", "service_tier":"standard", "inference_geo":"global",
        "cache_creation":{"ephemeral_5m_input_tokens":five,"ephemeral_1h_input_tokens":hour}})
}

#[test]
fn distinct_input_cache_and_inclusive_output_use_provider_units() {
    let result = request(Some("claude-opus-5-5"), recorded(12, 8));
    close(
        result.cost_usd,
        (100.0 * 4.0 + 40.0 * 0.2 + 12.0 * 5.0 + 8.0 * 8.0 + 10.0 * 20.0) / 1e6,
    );
    let rate = result.rates.unwrap();
    assert_eq!(rate.date, PRICING_DATE);
    assert_eq!(rate.source, PRICING_SOURCE);
    assert_eq!(rate.write_12h, 0.0);
    assert_eq!(rate.write_30m, 0.0);
    assert!(rate.anthropic.unwrap().assumptions.is_empty());
    assert!(result.tokens.write_12h.is_none());
    let sonnet = request(Some("claude-sonnet-5-5"), recorded(12, 8));
    close(sonnet.cost_usd, 0.000366);
    let haiku = request(Some("claude-haiku-5-5"), recorded(12, 8));
    close(haiku.cost_usd, 0.0000185);
    for (model, expected) in [
        ("claude-fable-5-2", 0.001456),
        ("claude-mythos-5-2", 0.001456),
        ("claude-fable-5-1", 0.001820),
        ("claude-mythos-5-1", 0.001820),
        ("claude-fable-5", 0.001850),
        ("claude-mythos-5", 0.001850),
        ("claude-opus-4-5", 0.000925),
        ("claude-opus-4-5-20251101", 0.000925),
    ] {
        close(request(Some(model), recorded(12, 8)).cost_usd, expected);
    }
    for model in [
        "claude-opus-5",
        "claude-opus-4-8",
        "claude-opus-4-7",
        "claude-opus-4-6",
        "claude-sonnet-5",
        "claude-sonnet-4-6",
        "claude-sonnet-4-5",
        "claude-sonnet-4-5-20250929",
        "claude-haiku-4-5",
        "claude-haiku-4-5-20251001",
    ] {
        assert!(
            request(Some(model), json!({})).cost_usd.is_some(),
            "{model}"
        );
    }
}

#[test]
fn absent_metadata_has_persisted_assumptions_but_invalid_presence_is_unpriced() {
    let assumed = request(Some("claude-opus-5-5"), json!({}));
    close(assumed.cost_usd, 0.000708);
    assert_eq!(
        assumed
            .rates
            .as_ref()
            .unwrap()
            .anthropic
            .as_ref()
            .unwrap()
            .assumptions
            .len(),
        4
    );
    for invalid in [
        json!({"speed":"turbo"}),
        json!({"speed":1}),
        json!({"service_tier":"auto"}),
        json!({"inference_geo":"eu"}),
        json!({"inference_geo":false}),
        json!({"cache_creation":{}}),
        json!({"cache_creation":{"ephemeral_5m_input_tokens":20}}),
        json!({"cache_creation":{"ephemeral_5m_input_tokens":19,"ephemeral_1h_input_tokens":0}}),
        json!({"cache_creation":{"ephemeral_5m_input_tokens":-1,"ephemeral_1h_input_tokens":21}}),
        json!({"cache_creation":{"ephemeral_5m_input_tokens":"20","ephemeral_1h_input_tokens":0}}),
        json!({"cache_creation":{"ephemeral_5m_input_tokens":u64::MAX,"ephemeral_1h_input_tokens":1}}),
    ] {
        let invalid = request(Some("claude-opus-5-5"), invalid);
        assert!(invalid.cost_usd.is_none());
        assert_eq!(invalid.tokens.input, 160);
        assert!(!invalid
            .anthropic
            .as_ref()
            .unwrap()
            .invalid_fields
            .is_empty());
        let mut restored: RequestUsage =
            serde_json::from_value(serde_json::to_value(&invalid).unwrap()).unwrap();
        refresh(&mut restored);
        assert!(restored.cost_usd.is_none());
        assert_eq!(restored.unpriced_reason, invalid.unpriced_reason);
    }
}

#[test]
fn modifiers_follow_anthropic_rules_and_unsupported_combinations_are_unpriced() {
    let base = request(Some("claude-opus-5-5"), recorded(12, 8))
        .cost_usd
        .unwrap();
    for (speed, tier, geo, multiplier) in [
        ("fast", "standard", "global", 2.0),
        ("fast", "standard", "us", 2.2),
        ("standard", "batch", "global", 0.5),
        ("standard", "batch", "us", 0.55),
        ("standard", "standard", "us", 1.1),
    ] {
        let mut metadata = recorded(12, 8);
        metadata["speed"] = json!(speed);
        metadata["service_tier"] = json!(tier);
        metadata["inference_geo"] = json!(geo);
        close(
            request(Some("claude-opus-5-5"), metadata).cost_usd,
            base * multiplier,
        );
    }
    for (model, metadata) in [
        (
            "claude-opus-5-5",
            json!({"speed":"fast","service_tier":"batch"}),
        ),
        (
            "claude-opus-5-5",
            json!({"speed":"fast","service_tier":"priority"}),
        ),
        ("claude-opus-5-5", json!({"service_tier":"priority"})),
        ("claude-sonnet-5-5", json!({"speed":"fast"})),
        ("claude-opus-4-6", json!({"speed":"fast"})),
        ("claude-haiku-4-5", json!({"inference_geo":"us"})),
        ("claude-opus-4-5", json!({"speed":"fast"})),
        ("claude-fable-5-1", json!({"speed":"fast"})),
    ] {
        assert!(request(Some(model), metadata).cost_usd.is_none(), "{model}");
    }
    for model in ["claude-fable-5-2", "claude-mythos-5-2"] {
        let base = request(Some(model), recorded(12, 8)).cost_usd.unwrap();
        let mut fast = recorded(12, 8);
        fast["speed"] = json!("fast");
        close(request(Some(model), fast).cost_usd, base * 2.0);
    }
}

#[test]
fn nullable_sdk_metadata_remains_explicitly_assumed() {
    let nullable =
        json!({"speed":null,"service_tier":null,"inference_geo":null,"cache_creation":null});
    let result = request(Some("claude-opus-5-5"), nullable.clone());
    close(result.cost_usd, 0.000708);
    assert!(result.anthropic.as_ref().unwrap().invalid_fields.is_empty());
    assert_eq!(
        result
            .rates
            .as_ref()
            .unwrap()
            .anthropic
            .as_ref()
            .unwrap()
            .assumptions
            .len(),
        4
    );
    let mut zero = request(Some("claude-opus-5-5"), nullable);
    zero.tokens = Tokens {
        cache_known: true,
        cached_known: Some(true),
        cache_write_known: Some(true),
        ..Default::default()
    };
    refresh(&mut zero);
    assert_eq!(zero.cost_usd, Some(0.0));
    assert_eq!(
        zero.rates
            .as_ref()
            .unwrap()
            .anthropic
            .as_ref()
            .unwrap()
            .assumptions
            .len(),
        3
    );
    let recorded_tier = request(Some("claude-opus-5-5"), json!({"service_tier":"standard"}));
    assert!(!recorded_tier.rates.as_ref().unwrap().tier_assumed);
    assert!(recorded_tier
        .rates
        .as_ref()
        .unwrap()
        .anthropic
        .as_ref()
        .unwrap()
        .assumptions
        .iter()
        .any(|assumption| assumption == "Standard speed assumed."));
}

#[test]
fn stored_tariffs_do_not_require_an_entry_in_the_current_catalogue() {
    let mut saved = request(Some("claude-opus-5-5"), recorded(12, 8));
    saved.model = Some("claude-previously-verified-model".into());
    let rate = saved.rates.as_mut().unwrap();
    rate.date = "2026-10-01".into();
    rate.anthropic.as_mut().unwrap().model = saved.model.clone().unwrap();
    let previous = saved.cost_usd;
    assert!(!super::super::correct_provider_price(&mut saved));
    refresh(&mut saved);
    assert_eq!(saved.cost_usd, previous);
    assert_eq!(saved.rates.as_ref().unwrap().date, "2026-10-01");
    assert!(request(saved.model.as_deref(), recorded(12, 8))
        .cost_usd
        .is_none());
}

#[test]
fn late_explicit_global_geography_keeps_the_saved_legacy_tariff() {
    let mut saved = request(Some("claude-haiku-4-5"), json!({}));
    saved.rates.as_mut().unwrap().date = "2026-10-01".into();
    let previous = saved.cost_usd;
    let mut metadata = saved.anthropic.clone().unwrap();
    assert!(!merge(
        &mut metadata,
        &parse(&json!({"inference_geo":"global"}), 20)
    ));
    saved.anthropic = Some(metadata);
    refresh(&mut saved);
    assert_eq!(saved.cost_usd, previous);
    assert_eq!(saved.rates.as_ref().unwrap().date, "2026-10-01");
}

#[test]
fn haiku_threshold_counts_all_input_and_changes_both_tariffs() {
    for (input, input_rate, output_rate) in [
        (99_999, 0.10, 0.50),
        (100_000, 0.10, 0.50),
        (100_001, 0.50, 2.50),
    ] {
        let mut result = request(Some("claude-haiku-5-5"), json!({}));
        result.rates = None;
        result.tokens.input = input;
        result.tokens.cached = input - 21; // Ordinary input remains one token.
        refresh(&mut result);
        let rate = result.rates.unwrap();
        assert_eq!(rate.input, input_rate);
        assert_eq!(rate.output, output_rate);
        assert_eq!(rate.long_context, input > 100_000);
    }
}

#[test]
fn unknown_ids_missing_model_and_legacy_long_context_retain_tokens_without_prices() {
    for model in [
        None,
        Some("claude-fixture"),
        Some("claude-opus-5-5-20261113"),
        Some("opus"),
        Some("anthropic.claude-opus-5-5"),
        Some("gpt-6.1-sol"),
    ] {
        let result = request(model, json!({}));
        assert!(result.cost_usd.is_none());
        assert!(result.rates.is_none());
        assert_eq!(result.tokens.input, 160);
    }
    let mut legacy = request(Some("claude-sonnet-4-5"), json!({}));
    legacy.rates = None;
    legacy.tokens.input = 200_001;
    refresh(&mut legacy);
    assert!(legacy.cost_usd.is_none());
}

#[test]
fn provider_correction_rejects_generic_prices_and_keeps_valid_anthropic_snapshots() {
    let mut valid = request(Some("claude-opus-5-5"), json!({}));
    assert!(!super::super::correct_provider_price(&mut valid));
    let mut generic = valid.clone();
    generic.rates.as_mut().unwrap().anthropic = None;
    let old_cost = generic.cost_usd;
    assert!(super::super::correct_provider_price(&mut generic));
    assert_eq!(
        generic
            .provider_correction
            .as_ref()
            .unwrap()
            .previous_cost_usd,
        old_cost
    );
    assert!(generic.cost_usd.is_none());
    refresh(&mut generic);
    assert!(generic.rates.as_ref().unwrap().anthropic.is_some());
    assert!(generic.provider_correction.is_some());
    let mut unknown = valid;
    unknown.source.as_mut().unwrap().model_provider = "other".into();
    assert!(super::super::correct_provider_price(&mut unknown));
    refresh(&mut unknown);
    assert!(unknown.cost_usd.is_none());
}

#[test]
fn saved_tariff_survives_output_growth_and_late_metadata() {
    let mut saved = request(Some("claude-opus-5-5"), json!({}));
    let rate = saved.rates.as_mut().unwrap();
    rate.date = "2026-10-01".into();
    rate.input *= 3.0;
    rate.cached *= 3.0;
    rate.output *= 3.0;
    rate.anthropic.as_mut().unwrap().write_5m *= 3.0;
    rate.anthropic.as_mut().unwrap().write_1h *= 3.0;
    saved.tokens.output = 20;
    let mut metadata = recorded(0, 20);
    metadata["speed"] = json!("fast");
    metadata["inference_geo"] = json!("us");
    saved.anthropic = Some(parse(&metadata, 20));
    refresh(&mut saved);
    close(
        saved.cost_usd,
        (400.0 + 8.0 + 160.0 + 400.0) * 3.0 * 2.2 / 1e6,
    );
    assert_eq!(saved.rates.as_ref().unwrap().date, "2026-10-01");
    let previous = saved.cost_usd;
    for _ in 0..3 {
        refresh(&mut saved);
        assert_eq!(saved.cost_usd, previous);
    }
}

#[test]
fn saved_unknown_cache_counts_cannot_be_backfilled_as_zero() {
    let mut saved = request(Some("claude-opus-5-5"), json!({}));
    saved.rates = None;
    saved.cost_usd = None;
    saved.anthropic = None;
    saved.tokens.cached_known = Some(false);
    refresh(&mut saved);
    assert!(saved.cost_usd.is_none());
    assert!(saved.rates.is_none());
    assert_eq!(
        saved.unpriced_reason.as_deref(),
        Some("Anthropic cache usage is not recorded")
    );
    assert_eq!(saved.tokens.input, 160);
}
