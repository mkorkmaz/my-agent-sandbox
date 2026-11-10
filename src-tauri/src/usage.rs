use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

pub const PRICING_DATE: &str = "2026-11-10";
pub const PRICING_SOURCE: &str = "https://developers.openai.com/api/docs/pricing";

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
}

impl Tokens {
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
            cache_known: self.cache_known && previous.cache_known,
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
    let multiplier = match tier.unwrap_or("standard") {
        "standard" | "default" | "auto" => 1.0,
        "priority" | "fast" => 2.0,
        "flex" | "batch" => 0.5,
        "ultrafast" if matches!(model, "gpt-6-astra" | "gpt-6.1-sol") => 6.0,
        _ => return None,
    };
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
    if !tokens.cache_known {
        reason = Some("Cache usage is not recorded".into());
    }
    if tokens.cache_write > 0 && tokens.write_12h.is_none() {
        reason = Some("Cache-write lifetime is not recorded".into());
    }
    if partial {
        reason = Some("Usage counter reset or aggregated requests; coverage is partial".into());
    }
    let cost_usd = if reason.is_none() {
        rates.as_ref().map(|rate| {
            let write_12h = tokens.write_12h.unwrap_or(0);
            ((tokens.input - tokens.cached - tokens.cache_write) as f64 * rate.input
                + tokens.cached as f64 * rate.cached
                + (tokens.cache_write - write_12h) as f64 * rate.write_30m
                + write_12h as f64 * rate.write_12h
                + tokens.output as f64 * rate.output)
                / 1_000_000.0
        })
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
    }
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnUsage {
    pub id: String,
    pub status: String,
    pub requests: Vec<RequestUsage>,
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
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
    }
}
pub fn summarize(turns: &BTreeMap<String, TurnUsage>) -> Summary {
    let mut result = Summary::default();
    for turn in turns.values() {
        if turn.status == "complete" {
            result.completed_turns += 1;
        }
        for request in &turn.requests {
            let tokens = &request.tokens;
            result.input += tokens.input;
            result.cached += tokens.cached;
            result.cache_write += tokens.cache_write;
            result.output += tokens.output;
            result.reasoning += tokens.reasoning;
            result.total += tokens.input + tokens.output;
            result.requests += 1;
            if !tokens.cache_known {
                result.missing_cache_requests += 1;
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

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cursor {
    offset: u64,
    inode: u64,
    device: u64,
    previous: Option<Tokens>,
    turn_id: Option<String>,
    model: Option<String>,
    tier: Option<String>,
}

impl Cursor {
    fn consume(
        &mut self,
        document: Value,
        turns: &mut BTreeMap<String, TurnUsage>,
    ) -> Result<(), String> {
        let payload = &document["payload"];
        if document["type"] == "turn_context" {
            self.model = payload["model"].as_str().map(str::to_owned);
            self.tier = payload["service_tier"].as_str().map(str::to_owned);
            if let Some(id) = payload["turn_id"].as_str() {
                self.turn_id = Some(id.into());
            }
            return Ok(());
        }
        if document["type"] != "event_msg" {
            return Ok(());
        }
        let kind = payload["type"].as_str().unwrap_or_default();
        if kind == "task_started" {
            self.turn_id = payload["turn_id"].as_str().map(str::to_owned);
            // Each turn_context must supply its own model; a missing context cannot inherit a price.
            self.model = None;
            self.tier = None;
            return Ok(());
        }
        if kind == "task_complete" || kind == "turn_aborted" {
            if let Some(id) = payload["turn_id"].as_str().or(self.turn_id.as_deref()) {
                let turn = turns.entry(id.into()).or_insert_with(|| TurnUsage {
                    id: id.into(),
                    ..Default::default()
                });
                turn.status = if kind == "task_complete" {
                    "complete"
                } else {
                    "interrupted"
                }
                .into();
            }
            return Ok(());
        }
        if kind != "token_count" || payload["info"].is_null() {
            return Ok(());
        }
        let info = &payload["info"];
        let current = Tokens::parse(&info["total_token_usage"])?;
        let last = Tokens::parse(&info["last_token_usage"])?;
        let delta = match &self.previous {
            Some(previous) => current.difference(previous),
            None => Some(current.clone()),
        };
        self.previous = Some(current);
        if delta
            .as_ref()
            .is_some_and(|delta| delta.input == 0 && delta.output == 0)
        {
            return Ok(());
        }
        let partial = delta.as_ref().is_none_or(|delta| delta != &last);
        let tokens = delta.unwrap_or(last);
        if tokens.cached + tokens.cache_write > tokens.input {
            return Err("Invalid token delta".into());
        }
        let at = document["timestamp"]
            .as_str()
            .ok_or("Token record has no timestamp")?
            .to_owned();
        let id = self
            .turn_id
            .clone()
            .unwrap_or_else(|| format!("unattributed-{at}"));
        let key = format!(
            "{at}/{}:{}:{}",
            info["total_token_usage"]["input_tokens"],
            info["total_token_usage"]["output_tokens"],
            info["total_token_usage"]["cached_input_tokens"]
        );
        let turn = turns.entry(id.clone()).or_insert_with(|| TurnUsage {
            id,
            status: "in-progress".into(),
            requests: Vec::new(),
        });
        if turn.requests.iter().any(|request| request.key == key) {
            return Ok(());
        }
        turn.requests.push(request(
            key,
            at,
            self.model.clone(),
            self.tier.as_deref(),
            tokens,
            partial,
        ));
        Ok(())
    }
}

pub fn sync_file(
    path: &Path,
    cursor: &mut Cursor,
    turns: &mut BTreeMap<String, TurnUsage>,
) -> Result<bool, String> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| "Could not read session usage.")?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("Session usage is not a regular file.".into());
    }
    if metadata.ino() != cursor.inode
        || metadata.dev() != cursor.device
        || metadata.len() < cursor.offset
    {
        *cursor = Cursor {
            inode: metadata.ino(),
            device: metadata.dev(),
            ..Default::default()
        };
    }
    file.seek(SeekFrom::Start(cursor.offset))
        .map_err(|error| error.to_string())?;
    // Bound a single synchronization pass, including unrelated conversation-body lines.
    let mut reader = BufReader::new(file.take(64 * 1024 * 1024));
    let mut changed = false;
    loop {
        let mut line = Vec::new();
        reader
            .by_ref()
            .take(16 * 1024 * 1024 + 1)
            .read_until(b'\n', &mut line)
            .map_err(|error| error.to_string())?;
        if line.len() > 16 * 1024 * 1024 {
            return Err("A session record is too large to read.".into());
        }
        if line.last() != Some(&b'\n') {
            break;
        }
        let document: Value = serde_json::from_slice(&line).map_err(|_| {
            "A saved session record is invalid. Refresh after Codex finishes writing."
        })?;
        cursor.consume(document, turns)?;
        cursor.offset += line.len() as u64;
        changed = true;
    }
    Ok(changed)
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
}
