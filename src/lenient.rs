//! Settings are read field by field. A field whose stored value no longer
//! parses (an old enum name, a string where a number belongs) keeps that
//! field's documented default - the struct's own `Default`, not the field
//! type's zero value - instead of failing the whole settings object, which
//! would leave the control dead until reconfigured. Numbers are clamped into
//! their range rather than rejected, so an out-of-range value from a property
//! inspector becomes the nearest valid one.
//!
//! Each settings struct implements `Deserialize` by starting from its
//! `Default` and calling `read`/`number` per field.

use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use std::ops::RangeInclusive;

pub struct Fields(Map<String, Value>);

impl Fields {
    /// The stored object. Anything else is an error, so a garbled nested
    /// object (e.g. a gesture stored as a string) falls back as a whole.
    pub fn from_deserializer<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match Value::deserialize(d)? {
            Value::Object(map) => Ok(Self(map)),
            other => Err(D::Error::custom(format!(
                "expected a settings object, got {other}"
            ))),
        }
    }

    /// Overwrites `slot` with `key`'s value when it parses as `T`.
    pub fn read<T: DeserializeOwned>(&self, key: &str, slot: &mut T) {
        if let Some(value) = self.0.get(key)
            && let Ok(parsed) = T::deserialize(value)
        {
            *slot = parsed;
        }
    }

    /// Overwrites `slot` with `key`'s number, rounded and clamped into
    /// `range`. Non-numbers leave the default.
    pub fn number<T>(&self, key: &str, range: RangeInclusive<T>, slot: &mut T)
    where
        T: Copy + Into<f64> + TryFrom<i64>,
    {
        let Some(n) = self.0.get(key).and_then(Value::as_f64) else {
            return;
        };
        if !n.is_finite() {
            return;
        }
        let clamped = n
            .round()
            .clamp((*range.start()).into(), (*range.end()).into());
        // In range by construction, so the conversion cannot fail.
        if let Ok(v) = T::try_from(clamped as i64) {
            *slot = v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fields(v: Value) -> Fields {
        Fields::from_deserializer(v).unwrap()
    }

    #[test]
    fn numbers_clamp_and_round() {
        let f = fields(json!({ "a": 300, "b": -4, "c": 7.6, "d": "x" }));
        let (mut a, mut b, mut c, mut d) = (50u8, 50u8, 50u8, 50u8);
        f.number("a", 0..=100, &mut a);
        f.number("b", 1..=50, &mut b);
        f.number("c", 0..=100, &mut c);
        f.number("d", 0..=100, &mut d);
        assert_eq!((a, b, c, d), (100, 1, 8, 50));
    }

    #[test]
    fn unparseable_or_missing_fields_keep_the_default() {
        let f = fields(json!({ "s": 5 }));
        let mut s = "default".to_string();
        let mut missing = true;
        f.read("s", &mut s);
        f.read("missing", &mut missing);
        assert_eq!((s.as_str(), missing), ("default", true));
    }

    #[test]
    fn a_non_object_is_an_error() {
        assert!(Fields::from_deserializer(json!("x")).is_err());
    }
}
