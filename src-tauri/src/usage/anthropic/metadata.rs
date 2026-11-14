use super::*;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    pub cache_creation: Option<CacheCreation>,
    pub speed: Option<String>,
    pub service_tier: Option<String>,
    pub inference_geo: Option<String>,
    // Persist invalid presence so restart and offline reads cannot assume defaults.
    pub invalid_fields: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheCreation {
    pub ephemeral_5m_input_tokens: u64,
    pub ephemeral_1h_input_tokens: u64,
}

fn string_field(
    value: &Value,
    name: &str,
    allowed: &[&str],
    invalid: &mut Vec<String>,
) -> Option<String> {
    let value = value.get(name).filter(|value| !value.is_null())?;
    let recorded = value
        .as_str()
        .filter(|value| value.len() <= 256)
        .map(str::to_owned);
    if !recorded
        .as_deref()
        .is_some_and(|value| allowed.contains(&value))
    {
        invalid.push(name.into());
    }
    recorded
}

pub fn parse(value: &Value, cache_write: u64) -> Metadata {
    let mut result = Metadata::default();
    result.speed = string_field(
        value,
        "speed",
        &["standard", "fast"],
        &mut result.invalid_fields,
    );
    result.service_tier = string_field(
        value,
        "service_tier",
        &["standard", "batch", "priority"],
        &mut result.invalid_fields,
    );
    result.inference_geo = string_field(
        value,
        "inference_geo",
        &["global", "us"],
        &mut result.invalid_fields,
    );
    let Some(value) = value.get("cache_creation").filter(|value| !value.is_null()) else {
        return result;
    };
    let counts = value["ephemeral_5m_input_tokens"]
        .as_u64()
        .zip(value["ephemeral_1h_input_tokens"].as_u64());
    let Some((five_minute_tokens, one_hour_tokens)) =
        counts.filter(|(five, hour)| five.checked_add(*hour) == Some(cache_write))
    else {
        result.invalid_fields.push("cache_creation".into());
        return result;
    };
    result.cache_creation = Some(CacheCreation {
        ephemeral_5m_input_tokens: five_minute_tokens,
        ephemeral_1h_input_tokens: one_hour_tokens,
    });
    result
}

impl Metadata {
    pub(super) fn writes(&self, total: u64) -> Option<(u64, u64)> {
        if self
            .invalid_fields
            .iter()
            .any(|field| field == "cache_creation")
        {
            return None;
        }
        let Some(split) = &self.cache_creation else {
            return Some((total, 0));
        };
        let five = split.ephemeral_5m_input_tokens;
        let hour = split.ephemeral_1h_input_tokens;
        (five.checked_add(hour) == Some(total)).then_some((five, hour))
    }

    pub(super) fn assumptions(&self, cache_write: u64) -> Vec<String> {
        let mut result = Vec::new();
        for (missing, assumption) in [
            (self.speed.is_none(), "Standard speed assumed."),
            (
                self.service_tier.is_none(),
                "Standard service tier assumed.",
            ),
            (self.inference_geo.is_none(), "Global inference assumed."),
            (
                self.cache_creation.is_none() && cache_write > 0,
                "5-minute cache writes assumed; lifetime split is not recorded.",
            ),
        ] {
            if missing {
                result.push(assumption.into());
            }
        }
        result
    }
}

fn merge_field<T: Clone + PartialEq>(
    saved: &mut Option<T>,
    next: &Option<T>,
    field: &str,
    invalid: &mut Vec<String>,
    next_invalid: &[String],
) -> bool {
    let previous_invalid = invalid.iter().any(|name| name == field);
    let supplied_invalid = next_invalid.iter().any(|name| name == field);
    if next.is_none() && !supplied_invalid {
        return false;
    }
    if !previous_invalid && saved.is_some() {
        return supplied_invalid || saved != next;
    }
    *saved = next.clone();
    invalid.retain(|name| name != field);
    if supplied_invalid {
        invalid.push(field.into());
    }
    false
}

// Only missing or invalid metadata can be recovered. Conflicting verified values
// retain the first saved evidence, matching the request amount ownership rules.
pub fn merge(saved: &mut Metadata, next: &Metadata) -> bool {
    let mut conflict = false;
    conflict |= merge_field(
        &mut saved.speed,
        &next.speed,
        "speed",
        &mut saved.invalid_fields,
        &next.invalid_fields,
    );
    conflict |= merge_field(
        &mut saved.service_tier,
        &next.service_tier,
        "service_tier",
        &mut saved.invalid_fields,
        &next.invalid_fields,
    );
    conflict |= merge_field(
        &mut saved.inference_geo,
        &next.inference_geo,
        "inference_geo",
        &mut saved.invalid_fields,
        &next.invalid_fields,
    );
    conflict |= merge_field(
        &mut saved.cache_creation,
        &next.cache_creation,
        "cache_creation",
        &mut saved.invalid_fields,
        &next.invalid_fields,
    );
    saved.invalid_fields.sort();
    saved.invalid_fields.dedup();
    conflict
}
