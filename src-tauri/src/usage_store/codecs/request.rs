use super::{decode, decode_optional, error, json, optional_json};
use crate::usage::{RequestSource, RequestUsage, Tokens};
use crate::usage_store::integer;
use rusqlite::{named_params, Connection, Row};

pub(crate) fn insert_request(
    connection: &Connection,
    turn_key: i64,
    ordinal: usize,
    request: &RequestUsage,
) -> Result<(), String> {
    write_request(connection, turn_key, ordinal, request, false)
}

pub(crate) fn request_upsert(
    connection: &Connection,
    turn_key: i64,
    ordinal: usize,
    request: &RequestUsage,
) -> Result<(), String> {
    write_request(connection, turn_key, ordinal, request, true)
}

fn write_request(
    connection: &Connection,
    turn_key: i64,
    ordinal: usize,
    request: &RequestUsage,
    upsert: bool,
) -> Result<(), String> {
    let source = request.source.as_ref();
    let cost = request.cost_usd;
    if cost.is_some_and(|cost| !cost.is_finite() || cost < 0.0) {
        return Err("A saved usage cost is not a finite nonnegative number.".into());
    }
    let base="INSERT INTO usage_requests(turn_key,ordinal,entry_key,recorded_at,model,input_tokens,cached_tokens,cache_write_tokens,output_tokens,reasoning_tokens,write_12h_tokens,cache_known,cached_known,cache_write_known,partial,cost_usd,unpriced_reason,service_tier,tier_source,rates_json,rate_correction_json,provider_correction_json,amount_correction_json,anthropic_json,anthropic_corrections_json,source_guest,model_provider,origin_thread_id,origin_turn_id,root_turn_id,response_id) VALUES(:turn_key,:ordinal,:entry_key,:recorded_at,:model,:input_tokens,:cached_tokens,:cache_write_tokens,:output_tokens,:reasoning_tokens,:write_12h_tokens,:cache_known,:cached_known,:cache_write_known,:partial,:cost_usd,:unpriced_reason,:service_tier,:tier_source,:rates_json,:rate_correction_json,:provider_correction_json,:amount_correction_json,:anthropic_json,:anthropic_corrections_json,:source_guest,:model_provider,:origin_thread_id,:origin_turn_id,:root_turn_id,:response_id)";
    let query = if upsert {
        format!("{base} ON CONFLICT(turn_key,ordinal) DO UPDATE SET entry_key=excluded.entry_key,recorded_at=excluded.recorded_at,model=excluded.model,input_tokens=excluded.input_tokens,cached_tokens=excluded.cached_tokens,cache_write_tokens=excluded.cache_write_tokens,output_tokens=excluded.output_tokens,reasoning_tokens=excluded.reasoning_tokens,write_12h_tokens=excluded.write_12h_tokens,cache_known=excluded.cache_known,cached_known=excluded.cached_known,cache_write_known=excluded.cache_write_known,partial=excluded.partial,cost_usd=excluded.cost_usd,unpriced_reason=excluded.unpriced_reason,service_tier=excluded.service_tier,tier_source=excluded.tier_source,rates_json=excluded.rates_json,rate_correction_json=excluded.rate_correction_json,provider_correction_json=excluded.provider_correction_json,amount_correction_json=excluded.amount_correction_json,anthropic_json=excluded.anthropic_json,anthropic_corrections_json=excluded.anthropic_corrections_json,source_guest=excluded.source_guest,model_provider=excluded.model_provider,origin_thread_id=excluded.origin_thread_id,origin_turn_id=excluded.origin_turn_id,root_turn_id=excluded.root_turn_id,response_id=excluded.response_id")
    } else {
        base.into()
    };
    connection.prepare_cached(&query).map_err(error)?.execute(named_params!{
        ":turn_key":turn_key,":ordinal":i64::try_from(ordinal).map_err(error)?,":entry_key":request.key,":recorded_at":request.at,":model":request.model,
        ":input_tokens":integer(request.tokens.input)?,":cached_tokens":integer(request.tokens.cached)?,":cache_write_tokens":integer(request.tokens.cache_write)?,
        ":output_tokens":integer(request.tokens.output)?,":reasoning_tokens":integer(request.tokens.reasoning)?,":write_12h_tokens":request.tokens.write_12h.map(integer).transpose()?,
        ":cache_known":request.tokens.cache_known,":cached_known":request.tokens.cached_known,":cache_write_known":request.tokens.cache_write_known,
        ":partial":request.partial,":cost_usd":cost,":unpriced_reason":request.unpriced_reason,":service_tier":request.service_tier,":tier_source":request.tier_source,
        ":rates_json":optional_json(&request.rates)?,":rate_correction_json":optional_json(&request.rate_correction)?,":provider_correction_json":optional_json(&request.provider_correction)?,
        ":amount_correction_json":optional_json(&request.amount_correction)?,":anthropic_json":optional_json(&request.anthropic)?,":anthropic_corrections_json":json(&request.anthropic_corrections)?,
        ":source_guest":source.map(|source|source.guest_namespace.as_str()),":model_provider":source.map(|source|source.model_provider.as_str()),":origin_thread_id":source.map(|source|source.origin_thread_id.as_str()),
        ":origin_turn_id":source.and_then(|source|source.origin_turn_id.as_deref()),":root_turn_id":source.and_then(|source|source.root_turn_id.as_deref()),":response_id":source.map(|source|source.response_id.as_str())
    }).map_err(error)?;
    Ok(())
}

pub(crate) fn read_request(row: &Row<'_>) -> rusqlite::Result<RequestUsage> {
    #[cfg(test)]
    let _benchmark_detail = crate::conversations::sqlite_benchmark::Detail::new("read_request");
    let source_guest: Option<String> = row.get("source_guest")?;
    let source = source_guest
        .map(|guest_namespace| {
            Ok::<_, rusqlite::Error>(RequestSource {
                guest_namespace,
                model_provider: row.get("model_provider")?,
                origin_thread_id: row.get("origin_thread_id")?,
                origin_turn_id: row.get("origin_turn_id")?,
                root_turn_id: row.get("root_turn_id")?,
                response_id: row.get("response_id")?,
            })
        })
        .transpose()?;
    Ok(RequestUsage {
        key: row.get("entry_key")?,
        at: row.get("recorded_at")?,
        model: row.get("model")?,
        tokens: Tokens {
            input: super::unsigned(row, "input_tokens")?,
            cached: super::unsigned(row, "cached_tokens")?,
            cache_write: super::unsigned(row, "cache_write_tokens")?,
            output: super::unsigned(row, "output_tokens")?,
            reasoning: super::unsigned(row, "reasoning_tokens")?,
            write_12h: super::optional_unsigned(row, "write_12h_tokens")?,
            cache_known: row.get("cache_known")?,
            cached_known: row.get("cached_known")?,
            cache_write_known: row.get("cache_write_known")?,
        },
        rates: decode_optional(row, "rates_json")?,
        cost_usd: row.get("cost_usd")?,
        partial: row.get("partial")?,
        unpriced_reason: row.get("unpriced_reason")?,
        service_tier: row.get("service_tier")?,
        tier_source: row.get("tier_source")?,
        rate_correction: decode_optional(row, "rate_correction_json")?,
        provider_correction: decode_optional(row, "provider_correction_json")?,
        source,
        amount_correction: decode_optional(row, "amount_correction_json")?,
        anthropic: decode_optional(row, "anthropic_json")?,
        anthropic_corrections: decode(row, "anthropic_corrections_json")?,
    })
}
