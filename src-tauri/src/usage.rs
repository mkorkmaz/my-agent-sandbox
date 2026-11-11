use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

pub const PRICING_DATE: &str = "2026-11-10";
pub const PRICING_SOURCE: &str = "https://developers.openai.com/api/docs/pricing";
pub const PARSER_VERSION: u32 = 3;

mod cursor;
#[cfg(test)]
pub use cursor::sync_file;
pub use cursor::{sync_file_bound, Cursor};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    pub input: u64,
    pub cached: u64,
    pub cache_write: u64,
    pub output: u64,
    pub reasoning: u64,
    pub write_12h: Option<u64>,
    pub cache_known: bool,
    #[serde(default)]
    pub cached_known: Option<bool>,
    #[serde(default)]
    pub cache_write_known: Option<bool>,
}

impl Tokens {
    fn cached_is_known(&self) -> bool {
        self.cached_known.unwrap_or(self.cache_known)
    }
    fn writes_are_known(&self) -> bool {
        self.cache_write_known.unwrap_or(self.cache_known)
    }

    fn recover_cache_knowledge(&mut self, source: &Self) {
        if self.input != source.input
            || self.output != source.output
            || self.cached != source.cached
            || self.cache_write != source.cache_write
        {
            return;
        }
        if self.cached_known.is_none() {
            self.cached_known = Some(source.cached_is_known());
        }
        if self.cache_write_known.is_none() {
            self.cache_write_known = Some(source.writes_are_known());
        }
        self.cache_known = self.cached_is_known() && self.writes_are_known();
    }

    fn parse(value: &Value) -> Result<Self, String> {
        let input = value["input_tokens"]
            .as_u64()
            .ok_or("Missing input token count")?;
        let output = value["output_tokens"]
            .as_u64()
            .ok_or("Missing output token count")?;
        let reasoning = value["reasoning_output_tokens"].as_u64().unwrap_or(0);
        let total = value["total_tokens"]
            .as_u64()
            .ok_or("Missing total token count")?;
        if input > 1_000_000_000_000 || output > 1_000_000_000_000 {
            return Err("Token count exceeds the supported range".into());
        }
        let output = match total.checked_sub(input) {
            Some(reported)
                if reported == output || output.checked_add(reasoning) == Some(reported) =>
            {
                reported
            }
            _ => return Err("Token totals do not reconcile".into()),
        };
        let cached = value["cached_input_tokens"].as_u64();
        let writes = value["cache_write_input_tokens"].as_u64();
        let write_12h = value["cache_write_input_tokens_details"]["ttl_12h_tokens"]
            .as_u64()
            .or_else(|| value["cache_write_input_tokens_12h"].as_u64());
        let result = Self {
            input,
            output,
            reasoning,
            cached: cached.unwrap_or(0),
            cache_write: writes.unwrap_or(0),
            write_12h: if writes == Some(0) {
                Some(0)
            } else {
                write_12h
            },
            cache_known: cached.is_some() && writes.is_some(),
            cached_known: Some(cached.is_some()),
            cache_write_known: Some(writes.is_some()),
        };
        if result
            .cached
            .checked_add(result.cache_write)
            .is_none_or(|sum| sum > input)
            || reasoning > output
            || write_12h.is_some_and(|count| count > result.cache_write)
        {
            return Err("Invalid token classes".into());
        }
        Ok(result)
    }

    fn difference(&self, previous: &Self) -> Option<Self> {
        Some(Self {
            input: self.input.checked_sub(previous.input)?,
            cached: self.cached.checked_sub(previous.cached)?,
            cache_write: self.cache_write.checked_sub(previous.cache_write)?,
            output: self.output.checked_sub(previous.output)?,
            reasoning: self.reasoning.checked_sub(previous.reasoning)?,
            write_12h: self
                .write_12h
                .zip(previous.write_12h)
                .and_then(|(current, previous)| current.checked_sub(previous)),
            cache_known: self.cached_is_known()
                && previous.cached_is_known()
                && self.writes_are_known()
                && previous.writes_are_known(),
            cached_known: Some(self.cached_is_known() && previous.cached_is_known()),
            cache_write_known: Some(self.writes_are_known() && previous.writes_are_known()),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateSnapshot {
    pub date: String,
    pub source: String,
    pub input: f64,
    pub cached: f64,
    pub write_30m: f64,
    pub write_12h: f64,
    pub output: f64,
    pub tier: String,
    pub tier_assumed: bool,
    pub long_context: bool,
}

fn rates(model: &str, tier: Option<&str>, input: u64) -> Option<RateSnapshot> {
    let base = match model {
        "gpt-6.1-sol" => (2.0, 0.1, 10.0),
        "gpt-6.1-terra" => (1.0, 0.05, 5.0),
        "gpt-6-astra" => (10.0, 1.0, 50.0),
        "gpt-6-luna" => (0.1, 0.01, 0.5),
        _ => return None,
    };
    let multiplier = tier_multiplier(model, tier.unwrap_or("standard"))?;
    // The verified Sol model page specifies the 272K threshold. Do not invent thresholds for other models.
    if input > 272_000 && model != "gpt-6.1-sol" {
        return None;
    }
    let long = input > 272_000;
    let input_multiplier = multiplier * if long { 2.0 } else { 1.0 };
    Some(RateSnapshot {
        date: PRICING_DATE.into(),
        source: PRICING_SOURCE.into(),
        input: base.0 * input_multiplier,
        cached: base.1 * input_multiplier,
        write_30m: base.0 * 1.25 * input_multiplier,
        write_12h: base.0 * 2.5 * input_multiplier,
        output: base.2 * multiplier * if long { 1.5 } else { 1.0 },
        tier: tier.unwrap_or("standard").into(),
        tier_assumed: tier.is_none() || tier == Some("auto"),
        long_context: long,
    })
}

fn tier_multiplier(model: &str, tier: &str) -> Option<f64> {
    Some(match tier {
        "standard" | "default" | "auto" => 1.0,
        "priority" | "fast" => 2.0,
        "flex" | "batch" => 0.5,
        "ultrafast" if matches!(model, "gpt-6-astra" | "gpt-6.1-sol") => 6.0,
        _ => return None,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestUsage {
    pub key: String,
    pub at: String,
    pub model: Option<String>,
    pub tokens: Tokens,
    pub rates: Option<RateSnapshot>,
    pub cost_usd: Option<f64>,
    pub partial: bool,
    pub unpriced_reason: Option<String>,
    #[serde(default)]
    pub service_tier: Option<String>,
    #[serde(default)]
    pub tier_source: Option<String>,
    #[serde(default)]
    pub rate_correction: Option<RateCorrection>,
    #[serde(default)]
    pub source: Option<RequestSource>,
    #[serde(default)]
    pub amount_correction: Option<AmountCorrection>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestSource {
    pub guest_namespace: String,
    pub model_provider: String,
    pub origin_thread_id: String,
    pub origin_turn_id: Option<String>,
    pub root_turn_id: Option<String>,
    pub response_id: String,
}

impl RequestSource {
    pub fn physical_key(&self) -> String {
        serde_json::to_string(&[
            &self.guest_namespace,
            &self.model_provider,
            &self.origin_thread_id,
            &self.response_id,
        ])
        .expect("string tuple serialization")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AmountCorrection {
    pub previous_tokens: Tokens,
    pub previous_cost_usd: Option<f64>,
    pub previous_partial: bool,
    pub previous_unpriced_reason: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateCorrection {
    previous_rates: RateSnapshot,
    previous_cost_usd: Option<f64>,
    reason: String,
}

fn price(tokens: &Tokens, rate: &RateSnapshot) -> Option<f64> {
    let regular = tokens
        .input
        .checked_sub(tokens.cached.checked_add(tokens.cache_write)?)?;
    let write_12h = tokens.write_12h.unwrap_or(0);
    let write_30m = tokens.cache_write.checked_sub(write_12h)?;
    let cost = (regular as f64 * rate.input
        + tokens.cached as f64 * rate.cached
        + write_30m as f64 * rate.write_30m
        + write_12h as f64 * rate.write_12h
        + tokens.output as f64 * rate.output)
        / 1_000_000.0;
    (cost.is_finite() && cost >= 0.0).then_some(cost)
}

fn request(
    key: String,
    at: String,
    model: Option<String>,
    tier: Option<&str>,
    tokens: Tokens,
    partial: bool,
) -> RequestUsage {
    let rates = model
        .as_deref()
        .and_then(|model| rates(model, tier, tokens.input));
    let mut reason = None;
    if rates.is_none() {
        reason = Some("Model, service tier or long-context price is unavailable".into());
    }
    if !tokens.cached_is_known() || !tokens.writes_are_known() {
        reason = Some("Cache usage is not recorded".into());
    }
    if tokens.cache_write > 0 && tokens.write_12h.is_none() {
        reason = Some("Cache-write lifetime is not recorded".into());
    }
    if partial {
        reason = Some("Usage counter reset or aggregated requests; coverage is partial".into());
    }
    let cost_usd = if reason.is_none() {
        rates.as_ref().and_then(|rate| price(&tokens, rate))
    } else {
        None
    };
    RequestUsage {
        key,
        at,
        model,
        tokens,
        rates,
        cost_usd,
        partial,
        unpriced_reason: reason,
        service_tier: tier.map(str::to_owned),
        tier_source: Some(
            if tier.is_some() {
                "turn-context"
            } else {
                "assumed"
            }
            .into(),
        ),
        rate_correction: None,
        source: None,
        amount_correction: None,
    }
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnUsage {
    pub id: String,
    pub status: String,
    pub requests: Vec<RequestUsage>,
    #[serde(default)]
    pub origin_thread_id: Option<String>,
    #[serde(default)]
    pub root_turn_id: Option<String>,
    #[serde(default)]
    pub parent_turn_id: Option<String>,
}

#[derive(Default, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub input: u64,
    pub cached: u64,
    pub cache_write: u64,
    pub output: u64,
    pub reasoning: u64,
    pub total: u64,
    pub cost_usd: f64,
    pub unpriced_requests: u64,
    pub requests: u64,
    pub completed_turns: u64,
    pub assumed_tier_requests: u64,
    pub missing_cache_requests: u64,
    #[serde(default)]
    pub missing_cached_requests: u64,
    #[serde(default)]
    pub missing_cache_write_requests: u64,
    #[serde(default)]
    pub unmeasured_turns: u64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Coverage {
    Complete,
    Partial,
    #[default]
    Unknown,
}

impl Summary {
    pub fn add(&mut self, other: &Self) {
        self.input += other.input;
        self.cached += other.cached;
        self.cache_write += other.cache_write;
        self.output += other.output;
        self.reasoning += other.reasoning;
        self.total += other.total;
        self.cost_usd += other.cost_usd;
        self.unpriced_requests += other.unpriced_requests;
        self.requests += other.requests;
        self.completed_turns += other.completed_turns;
        self.assumed_tier_requests += other.assumed_tier_requests;
        self.missing_cache_requests += other.missing_cache_requests;
        self.missing_cached_requests += other.missing_cached_requests;
        self.missing_cache_write_requests += other.missing_cache_write_requests;
        self.unmeasured_turns += other.unmeasured_turns;
    }
}
pub fn summarize(turns: &BTreeMap<String, TurnUsage>) -> Summary {
    let mut result = Summary::default();
    let mut seen = HashSet::new();
    for turn in turns.values() {
        if turn.status == "complete" {
            result.completed_turns += 1;
        }
        if turn.requests.is_empty() {
            result.unmeasured_turns += 1;
        }
        for request in &turn.requests {
            if request
                .source
                .as_ref()
                .is_some_and(|source| !seen.insert(source.physical_key()))
            {
                continue;
            }
            let tokens = &request.tokens;
            result.input += tokens.input;
            if tokens.cached_is_known() {
                result.cached += tokens.cached;
            }
            if tokens.writes_are_known() {
                result.cache_write += tokens.cache_write;
            }
            result.output += tokens.output;
            result.reasoning += tokens.reasoning;
            result.total += tokens.input + tokens.output;
            result.requests += 1;
            if !tokens.cached_is_known() || !tokens.writes_are_known() {
                result.missing_cache_requests += 1;
            }
            if !tokens.cached_is_known() {
                result.missing_cached_requests += 1;
            }
            if !tokens.writes_are_known() {
                result.missing_cache_write_requests += 1;
            }
            result.cost_usd += request.cost_usd.unwrap_or(0.0);
            if request.cost_usd.is_none() {
                result.unpriced_requests += 1;
            }
            if request.rates.as_ref().is_some_and(|rate| rate.tier_assumed) {
                result.assumed_tier_requests += 1;
            }
        }
    }
    result
}

pub fn own_turns(turns: &BTreeMap<String, TurnUsage>, owner: &str) -> BTreeMap<String, TurnUsage> {
    let mut result = BTreeMap::new();
    let mut exact: BTreeMap<String, (String, RequestUsage)> = BTreeMap::new();
    for turn in turns.values() {
        let own = turn
            .origin_thread_id
            .as_deref()
            .is_none_or(|origin| origin == owner);
        if own {
            let mut metadata = turn.clone();
            metadata.requests.clear();
            result.insert(turn.id.clone(), metadata);
        }
        for request in &turn.requests {
            let Some(source) = request.source.as_ref() else {
                if own {
                    result
                        .get_mut(&turn.id)
                        .unwrap()
                        .requests
                        .push(request.clone());
                }
                continue;
            };
            if source.origin_thread_id != owner {
                continue;
            }
            let id = source
                .origin_turn_id
                .clone()
                .unwrap_or_else(|| turn.id.clone());
            let key = source.physical_key();
            let replace = exact.get(&key).is_none_or(|(_, previous)| {
                previous.key.starts_with("source:") && !request.key.starts_with("source:")
            });
            if replace {
                exact.insert(key, (id, request.clone()));
            }
        }
    }
    for (_, (id, request)) in exact {
        let turn = result.entry(id.clone()).or_insert_with(|| TurnUsage {
            id,
            status: "in-progress".into(),
            origin_thread_id: Some(owner.into()),
            root_turn_id: request
                .source
                .as_ref()
                .and_then(|source| source.root_turn_id.clone()),
            ..Default::default()
        });
        turn.requests.push(request);
    }
    // A stored bucket containing only copied/aliased requests is not an empty own turn.
    result.retain(|id, turn| {
        !turn.requests.is_empty()
            || turns
                .get(id)
                .is_some_and(|stored| stored.requests.is_empty())
    });
    result
}

fn correct_legacy_tier(request: &mut RequestUsage, tier: &str, source: &str) {
    if request.tier_source.is_some() {
        return;
    }
    request.service_tier = Some(tier.into());
    request.tier_source = Some(source.into());
    let Some(previous) = request.rates.clone() else {
        return;
    };
    if previous.tier == tier && !previous.tier_assumed {
        return;
    }
    let model = request.model.as_deref().unwrap_or_default();
    let ratio = tier_multiplier(model, tier)
        .zip(tier_multiplier(model, &previous.tier))
        .map(|(next, old)| next / old);
    request.rate_correction = Some(RateCorrection {
        previous_rates: previous.clone(),
        previous_cost_usd: request.cost_usd,
        reason: format!(
            "Recovered recorded service tier from {source}; original dated rates retained."
        ),
    });
    let Some(ratio) = ratio else {
        request.rates = None;
        request.cost_usd = None;
        request.unpriced_reason = Some("The recorded service tier price is unavailable".into());
        return;
    };
    let mut corrected = previous;
    corrected.input *= ratio;
    corrected.cached *= ratio;
    corrected.write_30m *= ratio;
    corrected.write_12h *= ratio;
    corrected.output *= ratio;
    corrected.tier = tier.into();
    corrected.tier_assumed = tier == "auto";
    if request.cost_usd.is_some() {
        request.cost_usd = price(&request.tokens, &corrected);
    }
    request.rates = Some(corrected);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn usage(input: u64, cached: u64, output: u64) -> Value {
        json!({"input_tokens":input,"cached_input_tokens":cached,"cache_write_input_tokens":0,"output_tokens":output,"reasoning_output_tokens":0,"total_tokens":input+output})
    }
    fn event(at: &str, total: Value, last: Value) -> Value {
        json!({"timestamp":at,"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":total,"last_token_usage":last}}})
    }
    #[test]
    fn cache_reads_and_writes_keep_independent_known_and_missing_counts() {
        let mut raw = usage(300, 75, 30);
        raw.as_object_mut()
            .unwrap()
            .remove("cache_write_input_tokens");
        let tokens = Tokens::parse(&raw).unwrap();
        assert!(tokens.cached_is_known());
        assert!(!tokens.writes_are_known());
        let record = request(
            "k".into(),
            "now".into(),
            Some("gpt-6.1-sol".into()),
            None,
            tokens.clone(),
            false,
        );
        assert!(record.cost_usd.is_none());
        let mut turns = BTreeMap::new();
        turns.insert(
            "one".into(),
            TurnUsage {
                id: "one".into(),
                status: "complete".into(),
                requests: vec![record],
                ..Default::default()
            },
        );
        let summary = summarize(&turns);
        assert_eq!(summary.cached, 75);
        assert_eq!(summary.missing_cached_requests, 0);
        assert_eq!(summary.missing_cache_write_requests, 1);
        let zero = Tokens::parse(&usage(300, 0, 30)).unwrap();
        assert!(zero.cached_is_known() && zero.writes_are_known());
        let mut write_only = usage(300, 0, 30);
        write_only
            .as_object_mut()
            .unwrap()
            .remove("cached_input_tokens");
        let write_only = Tokens::parse(&write_only).unwrap();
        assert!(!write_only.cached_is_known() && write_only.writes_are_known());
        let mut neither = raw;
        neither
            .as_object_mut()
            .unwrap()
            .remove("cached_input_tokens");
        let neither = Tokens::parse(&neither).unwrap();
        assert!(!neither.cached_is_known() && !neither.writes_are_known());
        let mut legacy = serde_json::to_value(tokens).unwrap();
        for field in ["cachedKnown", "cacheWriteKnown"] {
            legacy.as_object_mut().unwrap().remove(field);
        }
        let legacy: Tokens = serde_json::from_value(legacy).unwrap();
        assert!(!legacy.cached_is_known() && !legacy.writes_are_known());
    }

    #[test]
    fn cost_uses_disjoint_cache_classes_and_never_doubles_reasoning() {
        let raw = json!({"input_tokens":1000,"cached_input_tokens":200,"cache_write_input_tokens":300,"cache_write_input_tokens_details":{"ttl_12h_tokens":100},"output_tokens":100,"reasoning_output_tokens":20,"total_tokens":1100});
        let tokens = Tokens::parse(&raw).unwrap();
        assert_eq!(tokens.output, 100);
        let record = request(
            "k".into(),
            "now".into(),
            Some("gpt-6.1-sol".into()),
            Some("standard"),
            tokens,
            false,
        );
        assert!((record.cost_usd.unwrap() - 0.00302).abs() < 1e-12);
        let mut separate = raw;
        separate["total_tokens"] = 1120.into();
        assert_eq!(Tokens::parse(&separate).unwrap().output, 120);
    }
    #[test]
    fn unknown_prices_and_missing_lifetimes_stay_unpriced() {
        let mut tokens = Tokens::parse(&usage(1000, 100, 50)).unwrap();
        assert!(request(
            "k".into(),
            "now".into(),
            Some("future-model".into()),
            None,
            tokens.clone(),
            false
        )
        .cost_usd
        .is_none());
        tokens.cache_write = 100;
        tokens.write_12h = None;
        assert!(request(
            "k".into(),
            "now".into(),
            Some("gpt-6.1-sol".into()),
            None,
            tokens,
            false
        )
        .cost_usd
        .is_none());
        assert_eq!(
            rates("gpt-6.1-sol", Some("priority"), 272001)
                .unwrap()
                .input,
            8.0
        );
        assert_eq!(
            rates("gpt-6.1-sol", Some("priority"), 272001)
                .unwrap()
                .output,
            30.0
        );
    }
    #[test]
    fn duplicate_notifications_counter_resets_and_model_changes_are_explicit() {
        let mut cursor = Cursor::default();
        let mut turns = BTreeMap::new();
        cursor
            .consume(
                json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"first"}}),
                &mut turns,
            )
            .unwrap();
        cursor
            .consume(
                json!({"type":"turn_context","payload":{"turn_id":"first","model":"gpt-6.1-sol"}}),
                &mut turns,
            )
            .unwrap();
        let first = event("one", usage(1000, 100, 50), usage(1000, 100, 50));
        cursor.consume(first.clone(), &mut turns).unwrap();
        cursor.consume(first, &mut turns).unwrap();
        cursor
            .consume(
                json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"first"}}),
                &mut turns,
            )
            .unwrap();
        cursor
            .consume(
                json!({"type":"turn_context","payload":{"turn_id":"second","model":"unknown"}}),
                &mut turns,
            )
            .unwrap();
        cursor
            .consume(
                event("two", usage(2000, 200, 100), usage(1000, 100, 50)),
                &mut turns,
            )
            .unwrap();
        cursor
            .consume(event("three", usage(10, 0, 1), usage(10, 0, 1)), &mut turns)
            .unwrap();
        let summary = summarize(&turns);
        assert_eq!(summary.requests, 3);
        assert_eq!(summary.total, 2111);
        assert_eq!(summary.completed_turns, 1);
        assert_eq!(summary.unpriced_requests, 2);
        assert!(turns["second"].requests[1].partial);
    }
    #[test]
    fn incremental_reader_restarts_and_waits_for_complete_lines() {
        let path = std::env::temp_dir().join(format!("tugboat-usage-{}.jsonl", std::process::id()));
        let context =
            json!({"type":"turn_context","payload":{"turn_id":"first","model":"gpt-6.1-sol"}});
        let record = event("one", usage(1000, 100, 50), usage(1000, 100, 50));
        let full = format!("{context}\n{record}\n");
        std::fs::write(&path, &full[..full.len() - 2]).unwrap();
        let mut cursor = Cursor::default();
        let mut turns = BTreeMap::new();
        sync_file(&path, &mut cursor, &mut turns).unwrap();
        assert_eq!(summarize(&turns).requests, 0);
        std::fs::write(&path, &full).unwrap();
        sync_file(&path, &mut cursor, &mut turns).unwrap();
        assert_eq!(summarize(&turns).requests, 1);
        let restored = serde_json::to_string(&cursor).unwrap();
        cursor = serde_json::from_str(&restored).unwrap();
        assert!(!sync_file(&path, &mut cursor, &mut turns).unwrap());
        // Rotation replays without charging the same request twice.
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, &full).unwrap();
        sync_file(&path, &mut cursor, &mut turns).unwrap();
        assert_eq!(summarize(&turns).requests, 1);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn an_invalid_token_record_does_not_advance_the_last_trusted_counter() {
        let mut cursor = Cursor::default();
        let mut turns = BTreeMap::new();
        cursor
            .consume(
                json!({"type":"turn_context","payload":{"turn_id":"one","model":"gpt-6.1-sol"}}),
                &mut turns,
            )
            .unwrap();
        cursor
            .consume(
                event("one", usage(100, 0, 10), usage(100, 0, 10)),
                &mut turns,
            )
            .unwrap();
        let corrected = event("two", usage(200, 0, 20), usage(100, 0, 10));
        let mut invalid = corrected.clone();
        invalid.as_object_mut().unwrap().remove("timestamp");
        assert!(cursor.consume(invalid, &mut turns).is_err());
        cursor.consume(corrected, &mut turns).unwrap();
        assert_eq!(summarize(&turns).requests, 2);
        assert_eq!(summarize(&turns).total, 220);
        assert!(!turns["one"].requests[1].partial);
    }

    #[test]
    fn recorded_tiers_follow_applied_settings_and_request_overrides() {
        let mut cursor = Cursor::default();
        let mut turns = BTreeMap::new();
        let settings = |tier| json!({"type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"service_tier":tier}}});
        cursor.consume(settings("default"), &mut turns).unwrap();
        cursor
            .consume(
                json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"one"}}),
                &mut turns,
            )
            .unwrap();
        cursor
            .consume(
                json!({"type":"turn_context","payload":{"turn_id":"one","model":"gpt-6.1-sol"}}),
                &mut turns,
            )
            .unwrap();
        cursor
            .consume(
                event("one", usage(1000, 0, 100), usage(1000, 0, 100)),
                &mut turns,
            )
            .unwrap();
        let first = &turns["one"].requests[0];
        assert_eq!(first.rates.as_ref().unwrap().tier, "default");
        assert!(!first.rates.as_ref().unwrap().tier_assumed);
        assert_eq!(first.tier_source.as_deref(), Some("thread-settings"));
        assert!((first.cost_usd.unwrap() - 0.003).abs() < 1e-12);
        cursor.consume(settings("priority"), &mut turns).unwrap();
        cursor
            .consume(
                event("two", usage(2000, 0, 200), usage(1000, 0, 100)),
                &mut turns,
            )
            .unwrap();
        assert!((turns["one"].requests[1].cost_usd.unwrap() - 0.006).abs() < 1e-12);
        let mut per_request = event("three", usage(3000, 0, 300), usage(1000, 0, 100));
        per_request["payload"]["info"]["service_tier"] = "default".into();
        cursor.consume(per_request, &mut turns).unwrap();
        let third = &turns["one"].requests[2];
        assert_eq!(third.tier_source.as_deref(), Some("request"));
        assert!((third.cost_usd.unwrap() - 0.003).abs() < 1e-12);
        cursor
            .consume(
                json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"two"}}),
                &mut turns,
            )
            .unwrap();
        cursor
            .consume(
                json!({"type":"turn_context","payload":{"turn_id":"two","model":"gpt-6.1-sol"}}),
                &mut turns,
            )
            .unwrap();
        cursor
            .consume(
                event("four", usage(4000, 0, 400), usage(1000, 0, 100)),
                &mut turns,
            )
            .unwrap();
        assert_eq!(
            turns["two"].requests[0].service_tier.as_deref(),
            Some("priority")
        );
        let mut missing = Cursor::default();
        let mut missing_turns = BTreeMap::new();
        missing
            .consume(
                json!({"type":"turn_context","payload":{"turn_id":"one","model":"gpt-6.1-sol"}}),
                &mut missing_turns,
            )
            .unwrap();
        missing
            .consume(
                event("one", usage(1000, 0, 100), usage(1000, 0, 100)),
                &mut missing_turns,
            )
            .unwrap();
        assert!(
            missing_turns["one"].requests[0]
                .rates
                .as_ref()
                .unwrap()
                .tier_assumed
        );
        assert_eq!(
            missing_turns["one"].requests[0].tier_source.as_deref(),
            Some("assumed")
        );
    }

    #[test]
    fn legacy_tier_correction_keeps_dated_prices_and_original_evidence() {
        let tokens = Tokens::parse(&usage(1000, 0, 100)).unwrap();
        let mut legacy = request(
            "key".into(),
            "old".into(),
            Some("gpt-6.1-sol".into()),
            None,
            tokens,
            false,
        );
        legacy.tier_source = None;
        let old = legacy.rates.as_mut().unwrap();
        old.date = "2026-10-01".into();
        old.input = 3.0;
        old.output = 15.0;
        legacy.cost_usd = price(&legacy.tokens, old);
        assert!((legacy.cost_usd.unwrap() - 0.0045).abs() < 1e-12);
        correct_legacy_tier(&mut legacy, "priority", "thread-settings");
        assert_eq!(legacy.rates.as_ref().unwrap().date, "2026-10-01");
        assert_eq!(legacy.rates.as_ref().unwrap().input, 6.0);
        assert!((legacy.cost_usd.unwrap() - 0.009).abs() < 1e-12);
        let correction = legacy.rate_correction.as_ref().unwrap();
        assert_eq!(correction.previous_rates.input, 3.0);
        assert!((correction.previous_cost_usd.unwrap() - 0.0045).abs() < 1e-12);
        correct_legacy_tier(&mut legacy, "priority", "thread-settings");
        assert!((legacy.cost_usd.unwrap() - 0.009).abs() < 1e-12);
        assert_eq!(
            legacy
                .rate_correction
                .as_ref()
                .unwrap()
                .previous_rates
                .input,
            3.0
        );
    }
}
