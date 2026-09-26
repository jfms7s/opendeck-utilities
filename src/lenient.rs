//! Serde helper: a settings field whose stored value no longer parses
//! (an old enum name, a string where a number belongs) falls back to its
//! type's default instead of failing the whole settings object - a failed
//! parse would otherwise leave the control dead until reconfigured.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

pub fn lenient<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    let value = Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}
