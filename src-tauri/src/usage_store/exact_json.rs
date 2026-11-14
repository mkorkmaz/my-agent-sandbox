//! Decode generated JSON leaves without a second decimal-to-binary rounding.
//! The legacy importer deliberately retains the existing legacy JSON decoder.
use serde::de::DeserializeOwned;
use serde_json::{value::RawValue, Map, Number, Value};
use std::collections::BTreeMap;

fn number(source: &str) -> Result<Value, String> {
    if !source.contains(['.', 'e', 'E']) {
        if let Ok(value) = source.parse::<u64>() {
            return Ok(Value::Number(value.into()));
        }
        if let Ok(value) = source.parse::<i64>() {
            return Ok(Value::Number(value.into()));
        }
    }
    let value = source
        .parse::<f64>()
        .map_err(|_| "Invalid number in a stored usage leaf.")?;
    Number::from_f64(value)
        .map(Value::Number)
        .ok_or_else(|| "Non-finite number in a stored usage leaf.".into())
}

fn value(source: &RawValue, depth: usize) -> Result<Value, String> {
    if depth > 128 {
        return Err("Stored usage JSON exceeds 128 nesting levels.".into());
    }
    let source = source.get().trim();
    match source.as_bytes().first() {
        Some(b'{') => {
            let raw: BTreeMap<String, Box<RawValue>> =
                serde_json::from_str(source).map_err(|_| "Invalid stored usage object.")?;
            let mut result = Map::new();
            for (key, raw) in raw {
                result.insert(key, value(&raw, depth + 1)?);
            }
            Ok(Value::Object(result))
        }
        Some(b'[') => {
            let raw: Vec<Box<RawValue>> =
                serde_json::from_str(source).map_err(|_| "Invalid stored usage array.")?;
            raw.iter()
                .map(|raw| value(raw, depth + 1))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        Some(b'"') => serde_json::from_str::<String>(source)
            .map(Value::String)
            .map_err(|_| "Invalid stored usage string.".into()),
        Some(b't') if source == "true" => Ok(Value::Bool(true)),
        Some(b'f') if source == "false" => Ok(Value::Bool(false)),
        Some(b'n') if source == "null" => Ok(Value::Null),
        Some(_) => number(source),
        None => Err("Empty stored usage leaf.".into()),
    }
}

pub(crate) fn decode<T: DeserializeOwned>(source: &str) -> Result<T, String> {
    let raw: Box<RawValue> =
        serde_json::from_str(source).map_err(|_| "Invalid stored usage JSON.")?;
    serde_json::from_value(value(&raw, 0)?)
        .map_err(|_| "The stored usage leaf does not match its type.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stored_decimal_floats_keep_all_bits_without_changing_the_legacy_decoder() {
        let original = 0.012345678901234567_f64;
        let source = serde_json::to_string(&original).unwrap();
        assert_eq!(
            decode::<f64>(&source).unwrap().to_bits(),
            original.to_bits()
        );
        let value = serde_json::json!({"rate":original,"corrections":[{"previous":original}],"unknown":null,"zero":0.0});
        let source = serde_json::to_string(&value).unwrap();
        assert_eq!(decode::<Value>(&source).unwrap(), value);
        assert_eq!(
            decode::<Value>("18446744073709551615").unwrap(),
            serde_json::json!(u64::MAX)
        );
        assert!(decode::<f64>("1e400").is_err());
    }
}
