use super::{
    correct_legacy_tier, price, request, AmountCorrection, RequestSource, RequestUsage, Tokens,
    TurnUsage, PRICING_DATE,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Exact {
    source: RequestSource,
    usage: Tokens,
    cumulative: Tokens,
    at: String,
    model: Option<String>,
    tier: Option<String>,
    tier_source: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Counter {
    owner: Option<String>,
    turn: Option<String>,
    usage: Tokens,
    cumulative: Tokens,
    key: Option<String>,
    notification_key: String,
    tier: Option<String>,
    tier_source: String,
}
#[derive(Default, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cursor {
    offset: u64,
    inode: u64,
    device: u64,
    previous: Option<Tokens>,
    turn_id: Option<String>,
    model: Option<String>,
    tier: Option<String>,
    #[serde(default)]
    thread_tier: Option<String>,
    #[serde(default)]
    namespace: Option<String>,
    #[serde(default)]
    expected_owner: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    active_owner: Option<String>,
    #[serde(default)]
    fork_origin: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    providers: BTreeMap<String, String>,
    #[serde(default)]
    header_fingerprint: Option<String>,
    #[serde(default)]
    ownership_problem: Option<String>,
    #[serde(default)]
    provenance_problem: bool,
    #[serde(default)]
    pending_exact: Vec<Exact>,
    #[serde(default)]
    pending_counters: Vec<Counter>,
    #[serde(default)]
    matched_counters: BTreeMap<String, (String, String)>,
}

fn signature(usage: &Tokens, cumulative: &Tokens) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(usage, cumulative)).expect("token serialization"))
    )
}

fn request_tier(turns: &mut BTreeMap<String, TurnUsage>, identity: &str, tier: &str) {
    for record in turns
        .values_mut()
        .flat_map(|turn| &mut turn.requests)
        .filter(|request| {
            request
                .source
                .as_ref()
                .is_some_and(|source| source.physical_key() == identity)
        })
    {
        if record.tier_source.as_deref() == Some("request")
            && record.service_tier.as_deref() == Some(tier)
        {
            continue;
        }
        if record.tier_source.as_deref() == Some("request")
            && record
                .service_tier
                .as_deref()
                .is_some_and(|previous| previous != tier)
        {
            record.cost_usd = None;
            record.unpriced_reason = Some("Recorded request service tiers conflict".into());
            continue;
        }
        if record.rates.as_ref().is_some_and(|rates| {
            super::tier_multiplier(record.model.as_deref().unwrap_or_default(), &rates.tier)
                .is_none()
        }) || super::tier_multiplier(record.model.as_deref().unwrap_or_default(), tier).is_none()
        {
            record.service_tier = Some(tier.into());
            record.tier_source = Some("request".into());
            record.cost_usd = None;
            record.unpriced_reason = Some("The recorded request tier price is unavailable".into());
            continue;
        }
        let earlier = record.rate_correction.clone();
        record.tier_source = None;
        correct_legacy_tier(record, tier, "request");
        if earlier.is_some() {
            record.rate_correction = earlier;
        }
    }
}

fn valid_id(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|id| crate::conversations::valid_id(id))
        .map(str::to_owned)
}
fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|value| {
            !value.is_empty() && value.len() <= 1024 && !value.chars().any(char::is_control)
        })
        .map(str::to_owned)
}
fn matches_counter(exact: &Exact, counter: &Counter) -> bool {
    counter.owner.as_deref() == Some(exact.source.origin_thread_id.as_str())
        && counter.turn == exact.source.origin_turn_id
        && counter.usage == exact.usage
        && counter.cumulative == exact.cumulative
}
fn find_key<'a>(
    turns: &'a mut BTreeMap<String, TurnUsage>,
    key: &str,
) -> Option<&'a mut RequestUsage> {
    turns
        .values_mut()
        .flat_map(|turn| &mut turn.requests)
        .find(|request| request.key == key)
}
fn compatible_class(record: &RequestUsage, next: &Tokens) -> bool {
    let Some(rates) = record.rates.as_ref() else {
        return false;
    };
    if next.input == record.tokens.input
        || (!rates.long_context && next.input < record.tokens.input)
    {
        return true;
    }
    record.model.as_deref() == Some("gpt-6.1-sol")
        && rates.date == PRICING_DATE
        && rates.long_context == (next.input > 272_000)
}
fn attach(record: &mut RequestUsage, exact: &Exact) -> Result<(), String> {
    if record
        .source
        .as_ref()
        .is_some_and(|source| source.physical_key() != exact.source.physical_key())
    {
        return Ok(());
    }
    if record.source.is_some() && record.tokens != exact.usage {
        return Err("Conflicting exact usage for the same physical response".into());
    }
    if let Some(previous) = record.source.as_ref() {
        if previous
            .origin_turn_id
            .as_ref()
            .zip(exact.source.origin_turn_id.as_ref())
            .is_some_and(|(previous, next)| previous != next)
            || previous
                .root_turn_id
                .as_ref()
                .zip(exact.source.root_turn_id.as_ref())
                .is_some_and(|(previous, next)| previous != next)
        {
            return Err("Conflicting turn attribution for the same physical response".into());
        }
    }
    if record.tokens != exact.usage || record.partial {
        let compatible = record
            .model
            .as_ref()
            .zip(exact.model.as_ref())
            .is_some_and(|(previous, next)| previous == next)
            && exact.source.model_provider == "openai"
            && compatible_class(record, &exact.usage);
        if record.amount_correction.is_none() {
            record.amount_correction = Some(AmountCorrection {
                previous_tokens: record.tokens.clone(),
                previous_cost_usd: record.cost_usd,
                previous_partial: record.partial,
                previous_unpriced_reason: record.unpriced_reason.clone(),
                reason: "Recovered exact request usage; inherited counters are not new spending."
                    .into(),
            });
        }
        record.tokens = exact.usage.clone();
        record.partial = false;
        record.cost_usd = None;
        record.unpriced_reason = Some("A compatible saved historical rate is unavailable".into());
        if compatible
            && record.tokens.cached_is_known()
            && record.tokens.writes_are_known()
            && (record.tokens.cache_write == 0 || record.tokens.write_12h.is_some())
        {
            record.cost_usd = record
                .rates
                .as_ref()
                .and_then(|rates| price(&record.tokens, rates));
            record.unpriced_reason = record
                .cost_usd
                .is_none()
                .then(|| "A compatible saved historical rate is unavailable".into());
        }
    }
    let mut source = exact.source.clone();
    if let Some(previous) = record.source.as_ref() {
        source.origin_turn_id = previous.origin_turn_id.clone().or(source.origin_turn_id);
        source.root_turn_id = previous.root_turn_id.clone().or(source.root_turn_id);
    }
    record.source = Some(source);
    Ok(())
}

impl Cursor {
    pub fn guest_namespace(&self) -> Option<&str> {
        self.namespace.as_deref()
    }
    pub fn bind_guest_namespace_if_missing(&mut self, namespace: &str) -> bool {
        if self.namespace.is_some() {
            return false;
        }
        self.namespace = Some(namespace.into());
        true
    }
    pub fn has_ownership_problem(&self) -> bool {
        self.ownership_problem.is_some()
            || self.provenance_problem
            || (self.owner.is_some() && self.active_owner != self.owner)
    }
    pub fn revision_state(&self) -> Value {
        serde_json::json!({"offset":self.offset,"inode":self.inode,"device":self.device,"namespace":self.namespace,"owner":self.owner,"activeOwner":self.active_owner,"version":self.version,"header":self.header_fingerprint,"ownershipProblem":self.ownership_problem,"provenanceProblem":self.provenance_problem,"turn":self.turn_id,"model":self.model,"tier":self.tier,"threadTier":self.thread_tier,"bindings":self.matched_counters.len(),"pendingExact":self.pending_exact.len(),"pendingCounters":self.pending_counters.len()})
    }
    fn supported(&self) -> bool {
        self.version.as_deref() == Some("0.167.0") && self.namespace.is_some()
    }
    fn lifecycle_allowed(&self) -> bool {
        self.owner.is_none() || self.active_owner.is_some()
    }
    fn touch_turn(&self, id: &str, payload: &Value, turns: &mut BTreeMap<String, TurnUsage>) {
        if !self.lifecycle_allowed() {
            return;
        }
        let turn = turns.entry(id.into()).or_insert_with(|| TurnUsage {
            id: id.into(),
            status: "in-progress".into(),
            ..Default::default()
        });
        turn.origin_thread_id = self.active_owner.clone();
        turn.root_turn_id = valid_id(&payload["root_turn_id"])
            .or_else(|| valid_id(&payload["turn_attribution"]["root_turn_id"]))
            .or_else(|| turn.root_turn_id.clone());
        turn.parent_turn_id = valid_id(&payload["turn_attribution"]["parent_turn_id"])
            .or_else(|| turn.parent_turn_id.clone());
    }
    fn metadata(&mut self, payload: &Value) {
        let Some(id) = valid_id(&payload["id"]) else {
            self.ownership_problem = Some("Session owner metadata is invalid".into());
            return;
        };
        if self.owner.is_none() {
            self.owner = Some(id.clone());
            self.active_owner = Some(id.clone());
            self.version = text(&payload["cli_version"]);
            self.fork_origin = valid_id(&payload["forked_from_id"]);
            if self
                .expected_owner
                .as_ref()
                .is_some_and(|expected| expected != &id)
            {
                self.active_owner = None;
                self.ownership_problem = Some("Session owner changed while reading".into());
                return;
            }
        } else if self.owner.as_ref() != Some(&id) {
            if self.fork_origin.as_ref() != Some(&id) {
                self.active_owner = None;
                self.ownership_problem = Some("Copied metadata has no verified fork owner".into());
                return;
            }
            self.active_owner = Some(id.clone());
        }
        if let Some(provider) = text(&payload["model_provider"]) {
            self.providers.insert(id, provider);
        }
    }
    fn exact(
        &mut self,
        payload: &Value,
        document: &Value,
        turns: &mut BTreeMap<String, TurnUsage>,
    ) -> Result<(), String> {
        let usage = Tokens::parse(&payload["usage"])?;
        let cumulative = Tokens::parse(&payload["thread_token_usage"])?;
        let Some(origin) = valid_id(&payload["thread_id"]) else {
            self.provenance_problem = true;
            return Ok(());
        };
        let Some(response) = text(&payload["response_id"]) else {
            self.provenance_problem = true;
            return Ok(());
        };
        let Some(provider) = self.providers.get(&origin).cloned() else {
            self.provenance_problem = true;
            return Ok(());
        };
        if !self.supported() {
            self.provenance_problem = true;
            return Ok(());
        }
        let source = RequestSource {
            guest_namespace: self.namespace.clone().unwrap(),
            model_provider: provider,
            origin_thread_id: origin,
            origin_turn_id: valid_id(&payload["turn_id"]),
            root_turn_id: valid_id(&payload["root_turn_id"]),
            response_id: response,
        };
        let (tier, tier_source) = self.current_tier(&Value::Null);
        let mut exact = Exact {
            model: if source.origin_turn_id == self.turn_id {
                self.model.clone()
            } else {
                None
            },
            source,
            usage,
            cumulative,
            at: text(&document["timestamp"]).ok_or("Token record has no timestamp")?,
            tier,
            tier_source,
        };
        if let Some(index) = self
            .pending_counters
            .iter()
            .position(|counter| matches_counter(&exact, counter))
        {
            let counter = self.pending_counters.remove(index);
            if counter.tier_source == "request" {
                exact.tier = counter.tier.clone();
                exact.tier_source = counter.tier_source.clone();
            }
            if let Some(record) = counter.key.as_deref().and_then(|key| find_key(turns, key)) {
                attach(record, &exact)?;
            }
            self.matched_counters
                .entry(counter.notification_key)
                .or_insert_with(|| {
                    (
                        exact.source.physical_key(),
                        signature(&counter.usage, &counter.cumulative),
                    )
                });
        }
        self.store_exact(&exact, turns)?;
        if exact.tier_source == "request" {
            if let Some(tier) = exact.tier.as_deref() {
                request_tier(turns, &exact.source.physical_key(), tier);
            }
        }
        if !self
            .pending_exact
            .iter()
            .any(|pending| pending.source.physical_key() == exact.source.physical_key())
        {
            self.pending_exact.push(exact);
        }
        self.bound_pending();
        Ok(())
    }
    fn store_exact(
        &self,
        exact: &Exact,
        turns: &mut BTreeMap<String, TurnUsage>,
    ) -> Result<(), String> {
        let identity = exact.source.physical_key();
        if let Some(record) =
            turns
                .values_mut()
                .flat_map(|turn| &mut turn.requests)
                .find(|record| {
                    record
                        .source
                        .as_ref()
                        .is_some_and(|source| source.physical_key() == identity)
                })
        {
            return attach(record, exact);
        }
        let id = exact
            .source
            .origin_turn_id
            .clone()
            .unwrap_or_else(|| format!("unattributed-{}", exact.at));
        let turn = turns.entry(id.clone()).or_insert_with(|| TurnUsage {
            id,
            status: "in-progress".into(),
            origin_thread_id: Some(exact.source.origin_thread_id.clone()),
            root_turn_id: exact.source.root_turn_id.clone(),
            ..Default::default()
        });
        let mut recorded = request(
            format!("source:{identity}"),
            exact.at.clone(),
            exact.model.clone(),
            exact.tier.as_deref(),
            exact.usage.clone(),
            false,
        );
        recorded.tier_source = Some(exact.tier_source.clone());
        if exact.source.model_provider != "openai" {
            recorded.rates = None;
            recorded.cost_usd = None;
            recorded.unpriced_reason = Some("Provider price is unavailable".into());
        }
        recorded.source = Some(exact.source.clone());
        turn.requests.push(recorded);
        Ok(())
    }
    fn bound_pending(&mut self) {
        if self.pending_exact.len() > 256 {
            self.pending_exact.remove(0);
            self.provenance_problem = true;
        }
        if self.pending_counters.len() > 256 {
            self.pending_counters.remove(0);
            self.provenance_problem = true;
        }
    }
    fn current_tier(&self, payload: &Value) -> (Option<String>, String) {
        if let Some(tier) = payload["info"]["service_tier"]
            .as_str()
            .or(payload["service_tier"].as_str())
        {
            return (Some(tier.into()), "request".into());
        }
        if let Some(tier) = self.thread_tier.as_ref() {
            return (Some(tier.clone()), "thread-settings".into());
        }
        (
            self.tier.clone(),
            if self.tier.is_some() {
                "turn-context"
            } else {
                "assumed"
            }
            .into(),
        )
    }
    fn counter(
        &mut self,
        payload: &Value,
        document: &Value,
        turns: &mut BTreeMap<String, TurnUsage>,
    ) -> Result<(), String> {
        let info = &payload["info"];
        let current = Tokens::parse(&info["total_token_usage"])?;
        let last = Tokens::parse(&info["last_token_usage"])?;
        let at = text(&document["timestamp"]).ok_or("Token record has no timestamp")?;
        let key = format!(
            "{at}/{}:{}:{}",
            info["total_token_usage"]["input_tokens"],
            info["total_token_usage"]["output_tokens"],
            info["total_token_usage"]["cached_input_tokens"]
        );
        let mut counter = Counter {
            owner: self.active_owner.clone(),
            turn: self.turn_id.clone(),
            usage: last.clone(),
            cumulative: current.clone(),
            key: None,
            notification_key: key.clone(),
            tier: self.current_tier(payload).0,
            tier_source: self.current_tier(payload).1,
        };
        if let Some((identity, previous_signature)) = self.matched_counters.get(&key) {
            if previous_signature != &signature(&last, &current) {
                return Err("Conflicting token notification for a known physical request".into());
            }
            if counter.tier_source == "request" {
                if let Some(tier) = counter.tier.as_deref() {
                    request_tier(turns, identity, tier);
                }
            }
            return Ok(());
        }
        if let Some(index) = self
            .pending_exact
            .iter()
            .position(|exact| matches_counter(exact, &counter))
        {
            let exact = self.pending_exact.remove(index);
            if let Some(record) = find_key(turns, &key) {
                attach(record, &exact)?;
            }
            self.matched_counters.insert(
                key,
                (exact.source.physical_key(), signature(&last, &current)),
            );
            if counter.tier_source == "request" {
                if let Some(tier) = counter.tier.as_deref() {
                    request_tier(turns, &exact.source.physical_key(), tier);
                }
            }
            self.previous = Some(current);
            return Ok(());
        }
        let delta = self
            .previous
            .as_ref()
            .map(|previous| current.difference(previous))
            .unwrap_or_else(|| Some(current.clone()));
        if delta
            .as_ref()
            .is_none_or(|tokens| tokens.input != 0 || tokens.output != 0)
        {
            let partial = delta.as_ref().is_none_or(|delta| delta != &last);
            let tokens = delta.unwrap_or(last);
            if tokens.cached + tokens.cache_write > tokens.input {
                return Err("Invalid token delta".into());
            }
            let id = if self.owner.is_some() && self.active_owner.is_none() {
                format!("unverified-{at}")
            } else {
                self.turn_id
                    .clone()
                    .unwrap_or_else(|| format!("unattributed-{at}"))
            };
            self.touch_turn(&id, payload, turns);
            let turn = turns.entry(id.clone()).or_insert_with(|| TurnUsage {
                id,
                status: "in-progress".into(),
                origin_thread_id: (self.owner.is_some() && self.active_owner.is_none())
                    .then(|| "unverified".into()),
                ..Default::default()
            });
            let (tier, tier_source) = self.current_tier(payload);
            if let Some(recorded) = turn.requests.iter_mut().find(|request| request.key == key) {
                recorded.tokens.recover_cache_knowledge(&tokens);
                if let Some(tier) = tier.as_deref() {
                    correct_legacy_tier(recorded, tier, &tier_source);
                }
            } else {
                let mut recorded = request(
                    key.clone(),
                    at,
                    self.model.clone(),
                    tier.as_deref(),
                    tokens,
                    partial,
                );
                recorded.tier_source = Some(tier_source);
                turn.requests.push(recorded);
            }
            counter.key = Some(key);
        }
        self.previous = Some(current);
        if self.supported() {
            self.pending_counters.push(counter);
            self.bound_pending();
        }
        Ok(())
    }
    pub(super) fn consume(
        &mut self,
        document: Value,
        turns: &mut BTreeMap<String, TurnUsage>,
    ) -> Result<(), String> {
        let payload = &document["payload"];
        if document["type"] == "session_meta" {
            self.metadata(payload);
            return Ok(());
        }
        if document["type"] == "turn_context" {
            self.model = payload["model"].as_str().map(str::to_owned);
            self.tier = payload["service_tier"].as_str().map(str::to_owned);
            if let Some(id) = payload["turn_id"].as_str() {
                self.turn_id = Some(id.into());
                self.touch_turn(id, payload, turns);
            }
            return Ok(());
        }
        if document["type"] == "token_usage_record" {
            return self.exact(payload, &document, turns);
        }
        if document["type"] != "event_msg" {
            return Ok(());
        }
        let kind = payload["type"].as_str().unwrap_or_default();
        if kind == "thread_settings_applied" {
            if let Some(id) = valid_id(&payload["thread_id"]) {
                if self.owner.as_ref() == Some(&id) {
                    self.active_owner = Some(id.clone());
                }
                if let Some(provider) = text(&payload["thread_settings"]["model_provider_id"]) {
                    self.providers.insert(id, provider);
                }
            }
            if self.owner.is_none() || self.active_owner == self.owner {
                self.thread_tier = payload["thread_settings"]["service_tier"]
                    .as_str()
                    .map(str::to_owned);
            }
            return Ok(());
        }
        if kind == "task_started" {
            self.turn_id = payload["turn_id"].as_str().map(str::to_owned);
            self.model = None;
            self.tier = None;
            if let Some(id) = self.turn_id.as_deref() {
                self.touch_turn(id, payload, turns);
            }
            return Ok(());
        }
        if kind == "task_complete" || kind == "turn_aborted" {
            if self.lifecycle_allowed() {
                if let Some(id) = payload["turn_id"].as_str().or(self.turn_id.as_deref()) {
                    self.touch_turn(id, payload, turns);
                    turns.get_mut(id).unwrap().status = if kind == "task_complete" {
                        "complete"
                    } else {
                        "interrupted"
                    }
                    .into();
                }
            }
            return Ok(());
        }
        if kind == "token_count" && !payload["info"].is_null() {
            return self.counter(payload, &document, turns);
        }
        Ok(())
    }
}

#[cfg(test)]
pub fn sync_file(
    path: &Path,
    cursor: &mut Cursor,
    turns: &mut BTreeMap<String, TurnUsage>,
) -> Result<bool, String> {
    read_file(path, None, None, cursor, turns)
}
pub fn sync_file_bound(
    path: &Path,
    namespace: &str,
    owner: &str,
    cursor: &mut Cursor,
    turns: &mut BTreeMap<String, TurnUsage>,
) -> Result<bool, String> {
    read_file(path, Some(namespace), Some(owner), cursor, turns)
}
fn read_file(
    path: &Path,
    namespace: Option<&str>,
    owner: Option<&str>,
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
    let mut first_line = Vec::new();
    BufReader::new(&mut file)
        .take(1_048_577)
        .read_until(b'\n', &mut first_line)
        .map_err(|error| error.to_string())?;
    if first_line.len() > 1_048_576 {
        return Err("Saved session metadata is too large".into());
    }
    let fingerprint = format!("{:x}", Sha256::digest(&first_line));
    if metadata.ino() != cursor.inode
        || metadata.dev() != cursor.device
        || metadata.len() < cursor.offset
        || cursor.header_fingerprint.as_deref() != Some(&fingerprint)
        || cursor.namespace.as_deref() != namespace
        || cursor.expected_owner.as_deref() != owner
    {
        *cursor = Cursor {
            inode: metadata.ino(),
            device: metadata.dev(),
            namespace: namespace.map(str::to_owned),
            expected_owner: owner.map(str::to_owned),
            header_fingerprint: Some(fingerprint),
            ..Default::default()
        };
    }
    file.seek(SeekFrom::Start(cursor.offset))
        .map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(file.take(64 * 1024 * 1024));
    let mut changed = false;
    let mut failure: Option<(Cursor, String)> = None;
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
        let checkpoint = cursor.clone();
        let consumed = serde_json::from_slice(&line)
            .map_err(|_| {
                "A saved session record is invalid. Refresh after Codex finishes writing."
                    .to_owned()
            })
            .and_then(|document| cursor.consume(document, turns));
        if let Err(message) = consumed {
            if failure.is_none() {
                failure = Some((checkpoint, message));
            }
        }
        cursor.offset += line.len() as u64;
        changed = true;
    }
    if let Some((checkpoint, message)) = failure {
        *cursor = checkpoint;
        return Err(message);
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{own_turns, summarize, RateCorrection, Summary};
    use serde_json::json;
    use std::collections::BTreeSet;

    const OWNER: &str = "30000000-0000-4000-8000-000000000001";
    const TURN: &str = "40000000-0000-4000-8000-000000000001";
    const NEXT: &str = "40000000-0000-4000-8000-000000000002";

    fn fixtures() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/codex-subagents/v0.167.0")
    }
    fn tokens(input: u64, output: u64) -> Value {
        json!({"input_tokens":input,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":output,"reasoning_output_tokens":0,"total_tokens":input+output})
    }
    fn exact(response: &str, usage: Value, cumulative: Value) -> Value {
        json!({"type":"token_usage_record","timestamp":response,"payload":{"thread_id":OWNER,"turn_id":TURN,"root_turn_id":TURN,"response_id":response,"usage":usage,"thread_token_usage":cumulative}})
    }
    fn counter(at: &str, usage: Value, cumulative: Value) -> Value {
        json!({"type":"event_msg","timestamp":at,"payload":{"type":"token_count","info":{"last_token_usage":usage,"total_token_usage":cumulative}}})
    }
    fn setup() -> (Cursor, BTreeMap<String, TurnUsage>) {
        let mut cursor = Cursor {
            namespace: Some("verified-test-guest".into()),
            ..Default::default()
        };
        let mut turns = BTreeMap::new();
        for document in [
            json!({"type":"session_meta","payload":{"id":OWNER,"cli_version":"0.167.0","model_provider":"openai","source":"cli"}}),
            json!({"type":"event_msg","payload":{"type":"thread_settings_applied","thread_id":OWNER,"thread_settings":{"service_tier":"default"}}}),
            json!({"type":"event_msg","payload":{"type":"task_started","turn_id":TURN}}),
            json!({"type":"turn_context","payload":{"turn_id":TURN,"model":"gpt-6.1-sol"}}),
        ] {
            cursor.consume(document, &mut turns).unwrap();
        }
        (cursor, turns)
    }
    fn assert_tokens(summary: &Summary, expected: &Value) {
        assert_eq!(
            (
                summary.input,
                summary.cached,
                summary.cache_write,
                summary.output,
                summary.reasoning,
                summary.total
            ),
            (
                expected["input_tokens"].as_u64().unwrap(),
                expected["cached_input_tokens"].as_u64().unwrap(),
                expected["cache_write_input_tokens"].as_u64().unwrap(),
                expected["output_tokens"].as_u64().unwrap(),
                expected["reasoning_output_tokens"].as_u64().unwrap(),
                expected["total_tokens"].as_u64().unwrap()
            )
        );
    }

    #[test]
    fn real_guest_requests_reconcile_all_six_classes_and_physical_ids() {
        let expected: Value =
            serde_json::from_slice(&std::fs::read(fixtures().join("p02/expected.json")).unwrap())
                .unwrap();
        let mut ids = BTreeSet::new();
        for file in expected["files"].as_array().unwrap() {
            let name = file["file"].as_str().unwrap();
            let owner = file["threadId"].as_str().unwrap();
            let mut cursor = Cursor::default();
            let mut turns = BTreeMap::new();
            sync_file_bound(
                &fixtures().join(name),
                "verified-test-guest",
                owner,
                &mut cursor,
                &mut turns,
            )
            .unwrap();
            let own = own_turns(&turns, owner);
            let summary = summarize(&own);
            assert_tokens(&summary, &file["ownTokens"]);
            assert_eq!(
                summary.requests as usize,
                file["ownRequests"].as_array().unwrap().len(),
                "{name}"
            );
            assert!(!cursor.has_ownership_problem(), "{name}");
            for request in own.values().flat_map(|turn| &turn.requests) {
                let source = request.source.as_ref().unwrap();
                let proof = file["ownRequests"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|proof| proof["responseId"] == source.response_id)
                    .unwrap();
                assert_eq!(source.origin_thread_id, owner);
                assert_eq!(source.root_turn_id.as_deref(), proof["rootTurnId"].as_str());
                assert_eq!(source.origin_turn_id.as_deref(), proof["turnId"].as_str());
                assert_eq!(request.model.as_deref(), proof["model"].as_str());
                assert_eq!(
                    request.service_tier.as_deref(),
                    proof["serviceTier"].as_str()
                );
                assert!(ids.insert(source.response_id.clone()));
            }
            if name == "normal-fork.jsonl" {
                assert_eq!(summary.total, 15_882);
            }
            if name == "p02/history-child.jsonl" {
                assert_eq!(
                    (own.len(), summary.completed_turns, summary.unmeasured_turns),
                    (1, 1, 0)
                );
                assert_eq!(turns.len(), 3); // copied lifecycle remains raw evidence
            }
            let before = serde_json::to_value(&turns).unwrap();
            assert!(!sync_file_bound(
                &fixtures().join(name),
                "verified-test-guest",
                owner,
                &mut cursor,
                &mut turns
            )
            .unwrap());
            assert_eq!(serde_json::to_value(&turns).unwrap(), before);
        }
        assert_eq!(ids.len(), 37);
        assert_eq!(
            ids.into_iter().collect::<Vec<_>>(),
            expected["globalUniqueResponseIds"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| id.as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn synthetic_replay_reset_missing_malformed_copy_and_empty_cases_use_production_cursor() {
        let data: Value = serde_json::from_slice(
            &std::fs::read(fixtures().join("p02/synthetic-cases.json")).unwrap(),
        )
        .unwrap();
        for case in data["cases"].as_array().unwrap() {
            let files = case["files"]
                .as_array()
                .cloned()
                .unwrap_or_else(|| vec![json!({"records":case["records"],"role":"own"})]);
            let mut combined = Summary::default();
            let mut errors = 0;
            let mut exact_count = BTreeSet::new();
            for file in files {
                let records = file["records"].as_array().unwrap();
                let owner = records[0]["payload"]["id"].as_str().unwrap();
                let mut cursor = Cursor {
                    namespace: Some("verified-test-guest".into()),
                    ..Default::default()
                };
                let mut turns = BTreeMap::new();
                for document in records {
                    errors += usize::from(cursor.consume(document.clone(), &mut turns).is_err());
                }
                let own = own_turns(&turns, owner);
                let summary = summarize(&own);
                for request in own.values().flat_map(|turn| &turn.requests) {
                    if let Some(source) = &request.source {
                        exact_count.insert(source.physical_key());
                    }
                }
                let expected = &case["expected"];
                let token_key = match file["role"].as_str().unwrap() {
                    "root" => "rootOwnTokens",
                    "child" => "childOwnTokens",
                    _ if expected["ownTokens"].is_object() => "ownTokens",
                    _ => "legacyCounterTokens",
                };
                assert_tokens(&summary, &expected[token_key]);
                if let Some(count) = expected["ownTurns"].as_u64() {
                    assert_eq!(own.len() as u64, count);
                }
                if let Some(count) = expected["noMeasurementOwnTurns"].as_u64() {
                    assert_eq!(summary.unmeasured_turns, count);
                }
                combined.add(&summary);
            }
            assert_eq!(
                exact_count.len() as u64,
                case["expected"]["exactRequests"].as_u64().unwrap(),
                "{}",
                case["name"]
            );
            assert_eq!(
                errors as u64,
                case["expected"]["rejectedExactRecords"]
                    .as_u64()
                    .unwrap_or(0)
            );
            if case["expected"]["combinedTokens"].is_object() {
                assert_tokens(&combined, &case["expected"]["combinedTokens"]);
            }
        }
    }

    #[test]
    fn copied_lifecycle_needs_the_explicit_owner_boundary_even_with_trigger_metadata() {
        let records: Vec<Value> =
            std::fs::read_to_string(fixtures().join("p02/history-child.jsonl"))
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
        let owner = records[0]["payload"]["id"].as_str().unwrap();
        for conflicting_fork in [false, true] {
            let mut cursor = Cursor {
                namespace: Some("verified-test-guest".into()),
                ..Default::default()
            };
            let mut turns = BTreeMap::new();
            for record in &records {
                if !conflicting_fork && record["payload"]["type"] == "thread_settings_applied" {
                    continue;
                }
                let mut record = record.clone();
                if conflicting_fork
                    && record["type"] == "session_meta"
                    && record["payload"]["id"] == owner
                {
                    record["payload"]["forked_from_id"] = NEXT.into();
                }
                cursor.consume(record, &mut turns).unwrap();
            }
            assert!(cursor.has_ownership_problem());
            let own = own_turns(&turns, owner);
            assert_eq!(own.len(), 1);
            let summary = summarize(&own);
            assert_eq!(
                (summary.requests, summary.total, summary.unmeasured_turns),
                (1, 15729, 0)
            );
            assert_eq!(summary.completed_turns, u64::from(conflicting_fork));
        }
    }

    #[test]
    fn request_tier_override_has_the_same_saved_price_in_both_arrival_orders() {
        for exact_first in [true, false] {
            let (mut cursor, mut turns) = setup();
            let amount = tokens(1000, 100);
            let exact = exact("response-priority", amount.clone(), amount.clone());
            let mut notification = counter("notification-priority", amount.clone(), amount);
            notification["payload"]["info"]["service_tier"] = "priority".into();
            let order = if exact_first {
                [exact, notification]
            } else {
                [notification, exact]
            };
            for record in order {
                cursor.consume(record, &mut turns).unwrap();
            }
            let own = own_turns(&turns, OWNER);
            let summary = summarize(&own);
            assert_eq!(summary.requests, 1);
            assert!((summary.cost_usd - 0.006).abs() < 1e-12);
            let request = &own[TURN].requests[0];
            assert_eq!(request.service_tier.as_deref(), Some("priority"));
            assert_eq!(request.tier_source.as_deref(), Some("request"));
            assert_eq!(request.rates.as_ref().unwrap().date, PRICING_DATE);
        }
    }

    #[test]
    fn delayed_replayed_notification_is_bound_durably_and_never_becomes_a_reset_request() {
        let (mut cursor, mut turns) = setup();
        let first = tokens(100, 10);
        let second = tokens(200, 20);
        let cumulative = tokens(300, 30);
        let first_counter = counter("counter-one", first.clone(), first.clone());
        for document in [
            exact("response-one", first.clone(), first),
            first_counter.clone(),
            exact("response-two", second.clone(), cumulative.clone()),
            counter("counter-two", second, cumulative),
        ] {
            cursor.consume(document, &mut turns).unwrap();
        }
        let saved = serde_json::to_vec(&cursor).unwrap();
        let mut cursor: Cursor = serde_json::from_slice(&saved).unwrap();
        cursor.consume(first_counter, &mut turns).unwrap();
        let summary = summarize(&own_turns(&turns, OWNER));
        assert_eq!((summary.requests, summary.total), (2, 330));
        assert_eq!(cursor.previous.unwrap().input, 300);
    }

    #[test]
    fn conflicting_exact_amount_and_known_attribution_preserve_first_physical_record() {
        let (mut cursor, mut turns) = setup();
        let amount = tokens(100, 10);
        let original = exact("response-one", amount.clone(), amount.clone());
        cursor.consume(original.clone(), &mut turns).unwrap();
        let saved = serde_json::to_value(&turns).unwrap();
        let mut wrong = original.clone();
        wrong["payload"]["usage"] = tokens(101, 10);
        assert!(cursor.consume(wrong, &mut turns).is_err());
        let mut wrong = original.clone();
        wrong["payload"]["root_turn_id"] = NEXT.into();
        assert!(cursor.consume(wrong, &mut turns).is_err());
        assert_eq!(serde_json::to_value(&turns).unwrap(), saved);
        cursor.consume(original.clone(), &mut turns).unwrap();
        assert_eq!(summarize(&own_turns(&turns, OWNER)).requests, 1);
        let (mut cursor, mut turns) = setup();
        let mut missing = original.clone();
        missing["payload"]["root_turn_id"] = Value::Null;
        cursor.consume(missing, &mut turns).unwrap();
        cursor.consume(original, &mut turns).unwrap();
        assert_eq!(
            own_turns(&turns, OWNER)[TURN].requests[0]
                .source
                .as_ref()
                .unwrap()
                .root_turn_id
                .as_deref(),
            Some(TURN)
        );
    }

    #[test]
    fn inherited_amount_correction_keeps_legacy_key_dated_rate_and_prior_tier_evidence() {
        let path = fixtures().join("normal-fork.jsonl");
        let records: Vec<Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let owner = records[0]["payload"]["id"].as_str().unwrap();
        let mut legacy = Cursor::default();
        let mut turns = BTreeMap::new();
        for record in &records {
            if record["type"] != "token_usage_record" {
                legacy.consume(record.clone(), &mut turns).unwrap();
            }
        }
        assert_eq!(summarize(&turns).total, 139_918);
        let request = turns
            .values_mut()
            .flat_map(|turn| &mut turn.requests)
            .next()
            .unwrap();
        request.rates.as_mut().unwrap().date = "2025-01-02".into();
        request.unpriced_reason = Some("previous incomplete amount".into());
        request.rate_correction = Some(RateCorrection {
            previous_rates: request.rates.clone().unwrap(),
            previous_cost_usd: Some(0.125),
            reason: "earlier tier correction".into(),
        });
        let previous = request.clone();
        sync_file_bound(
            &path,
            "verified-test-guest",
            owner,
            &mut Cursor::default(),
            &mut turns,
        )
        .unwrap();
        let own = own_turns(&turns, owner);
        let summary = summarize(&own);
        assert_eq!((summary.requests, summary.total), (1, 15_882));
        let corrected = own.values().flat_map(|turn| &turn.requests).next().unwrap();
        assert_eq!(corrected.key, previous.key);
        assert_eq!(corrected.rates.as_ref().unwrap().date, "2025-01-02");
        assert_eq!(
            serde_json::to_value(&corrected.rate_correction).unwrap(),
            serde_json::to_value(&previous.rate_correction).unwrap()
        );
        let evidence = corrected.amount_correction.as_ref().unwrap();
        assert_eq!(evidence.previous_tokens, previous.tokens);
        assert_eq!(evidence.previous_cost_usd, previous.cost_usd);
        assert_eq!(evidence.previous_partial, previous.partial);
        assert_eq!(evidence.previous_unpriced_reason, previous.unpriced_reason);
        assert!(corrected.cost_usd.is_some());
        let mut unproven = previous;
        unproven.tokens.input = 300_000;
        unproven.rates.as_mut().unwrap().long_context = true;
        unproven.source = None;
        let proof = Exact {
            source: corrected.source.clone().unwrap(),
            usage: corrected.tokens.clone(),
            cumulative: corrected.tokens.clone(),
            at: corrected.at.clone(),
            model: corrected.model.clone(),
            tier: Some("default".into()),
            tier_source: "thread-settings".into(),
        };
        attach(&mut unproven, &proof).unwrap();
        assert!(unproven.cost_usd.is_none());
        assert_eq!(unproven.rates.as_ref().unwrap().date, "2025-01-02");
    }

    #[test]
    fn exact_files_wait_for_half_line_restart_and_rotation_without_changing_keys_or_saved_prices() {
        use std::io::Write;
        let body = std::fs::read_to_string(fixtures().join("normal-fork.jsonl")).unwrap();
        let records: Vec<Value> = body
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let owner = records[0]["payload"]["id"].as_str().unwrap();
        let exact_index = records
            .iter()
            .position(|record| record["type"] == "token_usage_record")
            .unwrap();
        let counter_index = records
            .iter()
            .position(|record| record["payload"]["type"] == "token_count")
            .unwrap();
        for legacy_first in [false, true] {
            let root = std::env::temp_dir().join(format!(
                "tugboat-exact-file-{}-{legacy_first}",
                std::process::id()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let path = root.join("rollout.jsonl");
            let (first, last) = if legacy_first {
                (counter_index, exact_index)
            } else {
                (exact_index, counter_index)
            };
            let mut prefix = records[..exact_index]
                .iter()
                .map(|record| format!("{record}\n"))
                .collect::<String>();
            prefix.push_str(&format!("{}\n", records[first]));
            let last_line = records[last].to_string();
            let half = last_line.len() / 2;
            std::fs::write(&path, format!("{prefix}{}", &last_line[..half])).unwrap();
            let mut cursor = Cursor::default();
            let mut turns = BTreeMap::new();
            sync_file_bound(
                &path,
                "verified-fs-test-guest",
                owner,
                &mut cursor,
                &mut turns,
            )
            .unwrap();
            assert_eq!(cursor.offset, prefix.len() as u64);
            let summary = summarize(&own_turns(&turns, owner));
            assert_eq!(summary.requests, 1);
            assert_eq!(summary.total, if legacy_first { 139918 } else { 15882 });
            let stored = turns
                .values_mut()
                .flat_map(|turn| &mut turn.requests)
                .next()
                .unwrap();
            let key = stored.key.clone();
            stored.rates.as_mut().unwrap().date = "2025-04-05".into();
            stored.rate_correction = Some(RateCorrection {
                previous_rates: stored.rates.clone().unwrap(),
                previous_cost_usd: stored.cost_usd,
                reason: "previous recorded tier evidence".into(),
            });
            let saved_tier = serde_json::to_value(&stored.rate_correction).unwrap();
            let old_tokens = stored.tokens.clone();
            let old_cost = stored.cost_usd;
            let old_partial = stored.partial;
            let old_reason = stored.unpriced_reason.clone();
            // Reopen exactly between the two source representations, while the final line is incomplete.
            let mut cursor: Cursor =
                serde_json::from_slice(&serde_json::to_vec(&cursor).unwrap()).unwrap();
            let mut turns = serde_json::from_slice(&serde_json::to_vec(&turns).unwrap()).unwrap();
            assert!(!sync_file_bound(
                &path,
                "verified-fs-test-guest",
                owner,
                &mut cursor,
                &mut turns
            )
            .unwrap());
            let mut append = OpenOptions::new().append(true).open(&path).unwrap();
            append
                .write_all(format!("{}\n", &last_line[half..]).as_bytes())
                .unwrap();
            for record in &records[counter_index + 1..] {
                writeln!(append, "{record}").unwrap();
            }
            drop(append);
            sync_file_bound(
                &path,
                "verified-fs-test-guest",
                owner,
                &mut cursor,
                &mut turns,
            )
            .unwrap();
            let own = own_turns(&turns, owner);
            let summary = summarize(&own);
            assert_eq!((summary.requests, summary.total), (1, 15882));
            let request = own.values().flat_map(|turn| &turn.requests).next().unwrap();
            assert_eq!(request.key, key);
            assert_eq!(request.rates.as_ref().unwrap().date, "2025-04-05");
            assert_eq!(
                serde_json::to_value(&request.rate_correction).unwrap(),
                saved_tier
            );
            if legacy_first {
                let correction = request.amount_correction.as_ref().unwrap();
                assert_eq!(correction.previous_tokens, old_tokens);
                assert_eq!(correction.previous_cost_usd, old_cost);
                assert_eq!(correction.previous_partial, old_partial);
                assert_eq!(correction.previous_unpriced_reason, old_reason);
            }
            let expected = serde_json::to_value(&own).unwrap();
            let previous_inode = cursor.inode;
            let rotation = root.join("replacement.jsonl");
            std::fs::write(&rotation, &body).unwrap();
            std::fs::rename(rotation, &path).unwrap();
            sync_file_bound(
                &path,
                "verified-fs-test-guest",
                owner,
                &mut cursor,
                &mut turns,
            )
            .unwrap();
            assert_ne!(cursor.inode, previous_inode);
            assert_eq!(
                serde_json::to_value(own_turns(&turns, owner)).unwrap(),
                expected
            );
            let mut cursor = Cursor::default();
            sync_file_bound(
                &path,
                "verified-fs-test-guest",
                owner,
                &mut cursor,
                &mut turns,
            )
            .unwrap();
            assert_eq!(
                serde_json::to_value(own_turns(&turns, owner)).unwrap(),
                expected
            );
            let mut append = OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(
                append,
                "{}\n{}",
                records[exact_index], records[counter_index]
            )
            .unwrap();
            drop(append);
            sync_file_bound(
                &path,
                "verified-fs-test-guest",
                owner,
                &mut cursor,
                &mut turns,
            )
            .unwrap();
            assert_eq!(
                serde_json::to_value(own_turns(&turns, owner)).unwrap(),
                expected
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
