//! Decimal identifiers preserve all 64 bits through JavaScript MCP clients.

use serde::{Deserialize, Deserializer, Serializer};

/// Encode protocol identity as text; JSON number transport can silently round large UIDs.
pub fn serialize<S: Serializer>(id: &u64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&id.to_string())
}

/// Reject numeric coercion and malformed identifiers before selecting an account object.
pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let text = String::deserialize(deserializer)?;
    parse_text(&text).map_err(serde::de::Error::custom)
}

/// Use the same exact representation in tool arguments, snapshots and retained previews.
pub fn parse(value: &serde_json::Value) -> anyhow::Result<u64> {
    parse_text(
        value
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("identifier must be a decimal string"))?,
    )
}

/// Only unsigned decimal digits are accepted; overflow is an error rather than truncation.
fn parse_text(text: &str) -> anyhow::Result<u64> {
    anyhow::ensure!(
        !text.is_empty() && text.len() <= 20 && text.bytes().all(|b| b.is_ascii_digit()),
        "invalid decimal identifier"
    );
    Ok(text.parse()?)
}

/// Optional strategy selection uses null/absence for no strategy, not a magic numeric identity.
pub mod optional {
    use super::*;

    /// Serialize the same string representation when an identity is present.
    pub fn serialize<S: Serializer>(id: &Option<u64>, serializer: S) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&id.map(|id| id.to_string()), serializer)
    }

    /// Decode optional text without accepting a rounded JSON number.
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<u64>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|id| parse_text(&id).map_err(serde::de::Error::custom))
            .transpose()
    }
}

#[cfg(test)]
mod tests;
