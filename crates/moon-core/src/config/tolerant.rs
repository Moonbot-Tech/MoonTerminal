//! Serde adapter that keeps one unreadable field from failing a whole settings file.
//!
//! `toml_io` treats any deserialization error as a CORRUPT file: it moves `settings.toml` to
//! `.bak` and the session continues on defaults, which the next save writes out. An enum is the
//! field most likely to trip that on a DOWNGRADE: a newer build persists a variant this build has
//! no name for, and that one string would cost the user every setting, not just its own.
//!
//! [`or_default`] reads the field as a raw TOML value first and only then tries the real type, so
//! a value this build cannot interpret degrades THAT field to its default with one warning,
//! while the rest of the file loads as written. Genuinely malformed TOML still fails in the
//! lexer before any field is reached and keeps the `.bak` quarantine.
//!
//! Every enum reachable from `SettingsFile` and `HotkeysConfig` goes through this adapter -
//! including the ones whose own `Deserialize` already maps unknown codes to a default, so one
//! rule holds without exceptions. Those hand-written readers fall back without this warning. The `settings_enum_tolerance` contract in `crates/moon-core/tests/`
//! enforces it.
//!
//! Only builds carrying this module are protected: a build released before it still quarantines
//! a file holding a value it does not know.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};

/// Deserialize `T`, falling back to `T::default()` when the stored value does not fit `T`.
///
/// Use as `#[serde(default, deserialize_with = "crate::config::tolerant::or_default")]`. The
/// `default` keeps an ABSENT field on its usual default; this function handles a PRESENT value
/// this build cannot read. A field whose serde default is a function rather than `T::default()`
/// uses [`or_else`] through a one-line wrapper instead, so both paths land on the same value.
///
/// Returns:
///     The stored value, or `T::default()` with a warning when it does not deserialize as `T`.
pub(crate) fn or_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    or_else(d, T::default)
}

/// Deserialize `T`, falling back to `fallback()` when the stored value does not fit `T`.
///
/// Returns:
///     The stored value, or `fallback()` with a warning when it does not deserialize as `T`.
pub(crate) fn or_else<'de, D, T>(d: D, fallback: fn() -> T) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let raw = toml::Value::deserialize(d)?;
    Ok(T::deserialize(raw).unwrap_or_else(|e| {
        log::warn!(
            "настройки: значение для {} не распознано ({e}); поле сброшено к умолчанию",
            std::any::type_name::<T>()
        );
        fallback()
    }))
}

/// Deserialize a string-keyed map, dropping each entry whose value does not fit `V`.
///
/// Use on a map field instead of [`or_default`], which would degrade the whole map on one
/// unreadable value. A dropped entry reads as absent - the same as never having been set.
///
/// Returns:
///     Every readable entry; each unreadable one is logged and left out.
pub(crate) fn map_values<'de, D, V>(d: D) -> Result<BTreeMap<String, V>, D::Error>
where
    D: Deserializer<'de>,
    V: DeserializeOwned,
{
    let raw = BTreeMap::<String, toml::Value>::deserialize(d)?;
    Ok(raw
        .into_iter()
        .filter_map(|(key, value)| match V::deserialize(value) {
            Ok(v) => Some((key, v)),
            Err(e) => {
                log::warn!(
                    "настройки: значение для {key} ({}) не распознано ({e}); запись пропущена",
                    std::any::type_name::<V>()
                );
                None
            }
        })
        .collect())
}

#[cfg(test)]
mod tests;
