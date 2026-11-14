use super::{RateSnapshot, RequestUsage, Tokens};
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod metadata;
#[cfg(test)]
mod tests;

pub const PRICING_DATE: &str = "2026-11-13";
pub const PRICING_SOURCE: &str = "https://platform.claude.com/docs/en/about-claude/pricing";
pub use metadata::{merge, parse, Metadata};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tariff {
    pub model: String,
    // USD per million tokens. These are not OpenAI's 30-minute/12-hour writes.
    pub write_5m: f64,
    pub write_1h: f64,
    pub speed: String,
    pub inference_geo: String,
    pub multiplier: f64,
    pub fast_multiplier: Option<f64>,
    pub us_multiplier: Option<f64>,
    pub batch_multiplier: f64,
    pub assumptions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Correction {
    pub previous_metadata: Option<Metadata>,
    pub previous_model: Option<String>,
    pub previous_rates: Option<RateSnapshot>,
    pub previous_cost_usd: Option<f64>,
    pub reason: String,
}

fn model_rates(model: &str, input: u64) -> Option<(f64, f64, f64)> {
    Some(match model {
        "claude-fable-5-2" | "claude-mythos-5-2" => (8.0, 0.20, 40.0),
        "claude-fable-5-1" | "claude-mythos-5-1" => (10.0, 0.25, 50.0),
        "claude-fable-5" | "claude-mythos-5" => (10.0, 1.0, 50.0),
        "claude-opus-5-5" => (4.0, 0.20, 20.0),
        "claude-sonnet-5-5" => (2.0, 0.10, 10.0),
        "claude-haiku-5-5" if input > 100_000 => (0.50, 0.05, 2.50),
        "claude-haiku-5-5" => (0.10, 0.01, 0.50),
        "claude-opus-5" | "claude-opus-4-8" | "claude-opus-4-7" | "claude-opus-4-6" => {
            (5.0, 0.50, 25.0)
        }
        "claude-sonnet-5" => (2.0, 0.20, 10.0),
        "claude-sonnet-4-6" => (3.0, 0.30, 15.0),
        "claude-opus-4-5" | "claude-opus-4-5-20251101" if input <= 200_000 => (5.0, 0.50, 25.0),
        "claude-sonnet-4-5" | "claude-sonnet-4-5-20250929" if input <= 200_000 => (3.0, 0.30, 15.0),
        "claude-haiku-4-5" | "claude-haiku-4-5-20251001" if input <= 200_000 => (1.0, 0.10, 5.0),
        _ => return None,
    })
}

fn catalogue(model: &str, input: u64) -> Option<RateSnapshot> {
    let (ordinary, cached, output) = model_rates(model, input)?;
    let legacy = matches!(
        model,
        "claude-opus-4-5"
            | "claude-opus-4-5-20251101"
            | "claude-sonnet-4-5"
            | "claude-sonnet-4-5-20250929"
            | "claude-haiku-4-5"
            | "claude-haiku-4-5-20251001"
    );
    Some(RateSnapshot {
        date: PRICING_DATE.into(),
        source: PRICING_SOURCE.into(),
        input: ordinary,
        cached,
        write_30m: 0.0,
        write_12h: 0.0,
        output,
        tier: "standard".into(),
        tier_assumed: true,
        long_context: model == "claude-haiku-5-5" && input > 100_000,
        anthropic: Some(Tariff {
            model: model.into(),
            write_5m: ordinary * 1.25,
            write_1h: ordinary * 2.0,
            speed: "standard".into(),
            inference_geo: "global".into(),
            multiplier: 1.0,
            fast_multiplier: matches!(
                model,
                "claude-fable-5-2"
                    | "claude-mythos-5-2"
                    | "claude-opus-5-5"
                    | "claude-opus-5"
                    | "claude-opus-4-8"
            )
            .then_some(2.0),
            us_multiplier: (!legacy).then_some(1.1),
            batch_multiplier: 0.5,
            assumptions: Vec::new(),
        }),
    })
}

pub(super) fn valid_tariff(record: &RequestUsage, rate: &RateSnapshot) -> bool {
    let Some(tariff) = &rate.anthropic else {
        return false;
    };
    record.model.as_deref() == Some(tariff.model.as_str())
        && tariff.model.starts_with("claude-")
        && tariff.model.len() <= 256
        && tariff
            .model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && rate.source == PRICING_SOURCE
        && chrono::NaiveDate::parse_from_str(&rate.date, "%Y-%m-%d").is_ok()
        && rate.write_30m == 0.0
        && rate.write_12h == 0.0
        && [
            rate.input,
            rate.cached,
            rate.output,
            tariff.write_5m,
            tariff.write_1h,
        ]
        .into_iter()
        .all(|amount| amount.is_finite() && amount >= 0.0)
        && [
            Some(tariff.multiplier),
            tariff.fast_multiplier,
            tariff.us_multiplier,
            Some(tariff.batch_multiplier),
        ]
        .into_iter()
        .flatten()
        .all(|amount| amount.is_finite() && amount > 0.0)
}

fn modifiers(metadata: &Metadata, tariff: &Tariff) -> Result<f64, String> {
    if !metadata.invalid_fields.is_empty() {
        return Err(format!(
            "Invalid recorded Anthropic billing metadata: {}",
            metadata.invalid_fields.join(", ")
        ));
    }
    let speed = metadata.speed.as_deref().unwrap_or("standard");
    let tier = metadata.service_tier.as_deref().unwrap_or("standard");
    let geo = metadata.inference_geo.as_deref().unwrap_or("global");
    let speed_multiplier = match speed {
        "standard" => 1.0,
        "fast" => tariff
            .fast_multiplier
            .ok_or("Fast-mode price is unavailable for this Anthropic model")?,
        _ => return Err("Recorded Anthropic speed is unsupported".into()),
    };
    let tier_multiplier = match tier {
        "standard" => 1.0,
        "batch" if speed == "standard" => tariff.batch_multiplier,
        "batch" => return Err("Anthropic fast mode does not support batch processing".into()),
        "priority" => {
            return Err("Anthropic Priority Tier has no verified public token tariff".into())
        }
        _ => return Err("Recorded Anthropic service tier is unsupported".into()),
    };
    let geo_multiplier = match geo {
        "global" => 1.0,
        "us" => tariff
            .us_multiplier
            .ok_or("Recorded inference geography is unsupported for this Anthropic model")?,
        _ => return Err("Recorded Anthropic inference geography is unsupported".into()),
    };
    Ok(speed_multiplier * tier_multiplier * geo_multiplier)
}

fn apply_metadata(
    rate: &mut RateSnapshot,
    metadata: &Metadata,
    cache_write: u64,
) -> Result<(), String> {
    let tariff = rate.anthropic.as_ref().expect("Anthropic tariff");
    let multiplier = modifiers(metadata, tariff)?;
    let ratio = multiplier / tariff.multiplier;
    rate.input *= ratio;
    rate.cached *= ratio;
    rate.output *= ratio;
    rate.tier = metadata
        .service_tier
        .clone()
        .unwrap_or_else(|| "standard".into());
    rate.tier_assumed = metadata.service_tier.is_none();
    let tariff = rate.anthropic.as_mut().expect("Anthropic tariff");
    tariff.write_5m *= ratio;
    tariff.write_1h *= ratio;
    tariff.multiplier = multiplier;
    tariff.speed = metadata.speed.clone().unwrap_or_else(|| "standard".into());
    tariff.inference_geo = metadata
        .inference_geo
        .clone()
        .unwrap_or_else(|| "global".into());
    tariff.assumptions = metadata.assumptions(cache_write);
    Ok(())
}

fn price(tokens: &Tokens, rate: &RateSnapshot, metadata: &Metadata) -> Option<f64> {
    let tariff = rate.anthropic.as_ref()?;
    let ordinary = tokens
        .input
        .checked_sub(tokens.cached.checked_add(tokens.cache_write)?)?;
    let (write_5m, write_1h) = metadata.writes(tokens.cache_write)?;
    let cost = (ordinary as f64 * rate.input
        + tokens.cached as f64 * rate.cached
        + write_5m as f64 * tariff.write_5m
        + write_1h as f64 * tariff.write_1h
        + tokens.output as f64 * rate.output)
        / 1_000_000.0;
    (cost.is_finite() && cost >= 0.0).then_some(cost)
}

// Uses an existing dated tariff. Catalogue changes alone never replace it.
pub fn refresh(record: &mut RequestUsage) {
    use super::pricing_state::PriceQuote;
    if !record
        .source
        .as_ref()
        .is_some_and(|source| source.model_provider == "anthropic")
    {
        return;
    }
    let recovery = record.anthropic.is_none().then(|| evidence(record, "Backfilled recorded Anthropic token API equivalent using the verified catalogue; missing billing metadata is explicitly assumed."));
    super::correct_provider_price(record);
    if !record.tokens.cached_is_known() || !record.tokens.writes_are_known() {
        record.record_quote(PriceQuote::Unknown(Some("Anthropic cache usage is not recorded".into())));
        return;
    }
    let metadata = record.anthropic.get_or_insert_with(Metadata::default);
    if record.rates.is_none() {
        record.rates = record
            .model
            .as_deref()
            .and_then(|model| catalogue(model, record.tokens.input));
    }
    record.service_tier = metadata.service_tier.clone();
    record.tier_source = Some(
        if metadata.service_tier.is_some() {
            "anthropic-usage"
        } else {
            "assumed"
        }
        .into(),
    );
    let Some(rate) = record.rates.as_mut() else {
        record.record_quote(PriceQuote::Unknown(Some("Anthropic model or long-context price is unavailable".into())));
        return;
    };
    let result = apply_metadata(rate, metadata, record.tokens.cache_write).and_then(|_| {
        price(&record.tokens, rate, metadata)
            .ok_or_else(|| "Anthropic cache-write counts do not reconcile".into())
    });
    record.record_quote(PriceQuote::from(result));
    if let Some(correction) = recovery.filter(|_| record.cost_usd.is_some()) {
        record.anthropic_corrections.push(correction);
    }
}

pub fn evidence(record: &RequestUsage, reason: &str) -> Correction {
    Correction {
        previous_metadata: record.anthropic.clone(),
        previous_model: record.model.clone(),
        previous_rates: record.rates.clone(),
        previous_cost_usd: record.cost_usd,
        reason: reason.into(),
    }
}
