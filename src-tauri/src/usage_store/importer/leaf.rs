use serde::de::{DeserializeOwned, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::Serialize;
use serde_json::{Map, Number, Value};
use std::cell::Cell;
use std::fmt;

pub(super) const LEAF_LIMIT: usize = 64 * 1024 * 1024;

pub(super) struct Leaf<'a> {
    budget: &'a Cell<usize>,
    depth: usize,
}

impl<'a> Leaf<'a> {
    pub fn new(budget: &'a Cell<usize>) -> Self {
        Self { budget, depth: 0 }
    }
    fn child(&self) -> Self {
        Self {
            budget: self.budget,
            depth: self.depth + 1,
        }
    }
    fn charge<E: serde::de::Error>(&self, count: usize) -> Result<(), E> {
        let remaining = self
            .budget
            .get()
            .checked_sub(count)
            .ok_or_else(|| E::custom("Decoded legacy leaf exceeds 64 MiB."))?;
        self.budget.set(remaining);
        Ok(())
    }
}

impl<'de> DeserializeSeed<'de> for Leaf<'_> {
    type Value = Value;
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        if self.depth > 128 {
            return Err(serde::de::Error::custom(
                "Legacy JSON exceeds 128 nesting levels.",
            ));
        }
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Leaf<'_> {
    type Value = Value;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a bounded typed legacy leaf")
    }
    fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Value, E> {
        self.charge::<E>(1)?;
        Ok(Value::Bool(value))
    }
    fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Value, E> {
        self.charge::<E>(8)?;
        Ok(Value::Number(Number::from(value)))
    }
    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Value, E> {
        self.charge::<E>(8)?;
        Ok(Value::Number(Number::from(value)))
    }
    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Value, E> {
        self.charge::<E>(8)?;
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("Non-finite legacy number."))
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Value, E> {
        self.charge::<E>(value.len())?;
        Ok(Value::String(value.into()))
    }
    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Value, E> {
        self.charge::<E>(value.len())?;
        Ok(Value::String(value))
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
        self.charge::<E>(1)?;
        Ok(Value::Null)
    }
    fn visit_none<E: serde::de::Error>(self) -> Result<Value, E> {
        self.visit_unit()
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(self.child())? {
            self.charge::<A::Error>(1)?;
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            self.charge::<A::Error>(key.len() + 1)?;
            if values.contains_key(&key) {
                return Err(serde::de::Error::custom("Duplicate field in legacy leaf."));
            }
            values.insert(key, map.next_value_seed(self.child())?);
        }
        Ok(Value::Object(values))
    }
}

pub(super) fn next_leaf<'de, A: MapAccess<'de>>(map: &mut A) -> Result<Value, A::Error> {
    map.next_value_seed(Leaf::new(&Cell::new(LEAF_LIMIT)))
}

fn omitted_default(key: &str, value: &Value, object: &Map<String, Value>) -> bool {
    let session = object.contains_key("turns") && object.contains_key("cursors");
    let request = object.contains_key("key") && object.contains_key("tokens");
    let rates = object.contains_key("write30m") && object.contains_key("tierAssumed");
    (session && matches!(key, "agentKind" | "claudeSource" | "guestNamespace") && value.is_null())
        || ((request && matches!(key, "providerCorrection" | "anthropic"))
            || (rates && key == "anthropic"))
            && value.is_null()
        || (request && key == "anthropicCorrections" && value.as_array().is_some_and(Vec::is_empty))
}

// A typed decode alone can silently discard future fields. Check all supplied
// object keys against its typed serialization, including nested corrections.
fn retained_keys(input: &Value, typed: &Value) -> Result<(), String> {
    if let (Some(input), Some(typed)) = (input.as_object(), typed.as_object()) {
        for (key, value) in input {
            match typed.get(key) {
                Some(saved) => retained_keys(value, saved)?,
                None if omitted_default(key, value, typed) => {}
                None => return Err("Unsupported field in a typed legacy leaf.".into()),
            }
        }
    }
    if let (Some(input), Some(typed)) = (input.as_array(), typed.as_array()) {
        for (value, saved) in input.iter().zip(typed) {
            retained_keys(value, saved)?;
        }
    }
    Ok(())
}

pub(super) fn typed<T: DeserializeOwned + Serialize>(value: Value) -> Result<T, String> {
    let typed: T = serde_json::from_value(value.clone())
        .map_err(|_| "The legacy leaf does not match its stored type.")?;
    let saved = serde_json::to_value(&typed).map_err(|error| error.to_string())?;
    retained_keys(&value, &saved)?;
    Ok(typed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decoded_leaf_limit_is_inclusive_and_duplicate_fields_fail() {
        for (size, succeeds) in [(LEAF_LIMIT, true), (LEAF_LIMIT + 1, false)] {
            let input = format!("\"{}\"", "x".repeat(size));
            let mut parser = serde_json::Deserializer::from_str(&input);
            assert_eq!(
                Leaf::new(&Cell::new(LEAF_LIMIT))
                    .deserialize(&mut parser)
                    .is_ok(),
                succeeds
            );
        }
        let mut parser = serde_json::Deserializer::from_str(r#"{"offset":1,"offset":2}"#);
        assert!(Leaf::new(&Cell::new(LEAF_LIMIT))
            .deserialize(&mut parser)
            .is_err());
    }
}
