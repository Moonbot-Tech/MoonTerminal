//! Serde adapter for the 64-bit ids this terminal receives from a core and then persists.
//!
//! TOML has no unsigned integer: `toml_edit`'s value serializer converts every `u64` through
//! `i64::try_from` and answers anything above `i64::MAX` with `out-of-range value for u64 type`.
//! Moonbot strategy ids use the WHOLE `u64` range — a real one already in a live config reads
//! `7394783480262116308`, four fifths of the way to that ceiling — so roughly half of them carry
//! the top bit and cannot be written at all.
//!
//! What made that worth a module rather than a clamp: the failure is not local to the field. It
//! aborts `toml::to_string` for the entire `settings.toml`, and `AppConfig::save` writes the
//! config PAIR — `servers.enc` first, `settings.toml` second — so one such id turns every save in
//! the application into a half-completed pair write plus an error message. The user sees "a
//! hotkey will not save"; nothing points at a strategy id.
//!
//! The representation therefore stays a plain integer for every value that fits `i64`, which is
//! every value any existing file holds, and becomes a decimal string only for the ones that do
//! not. That keeps a typical `settings.toml` byte-identical to what earlier builds wrote — a
//! blanket switch to strings would make every user's file unreadable to an older build over a
//! problem almost none of them have — and confines the new shape to configs that today cannot be
//! saved at all. Reading accepts both shapes — and only the string one can carry a value above
//! the ceiling, because a bare integer that large is refused by TOML's own lexer long before
//! serde is reached, which is precisely why the writer does not produce one.
//!
//! The price of the string form, stated rather than implied: a build without this module reads it
//! as `invalid type: string`, and `toml_io` classifies that as a corrupt file — `settings.toml`
//! moves to `.bak` and the session continues on defaults, losing every core's group, market,
//! color and feed flags. That is the reason the shape is confined to ids above the ceiling
//! instead of applied to all of them: such an id cannot be in a file an older build wrote, since
//! writing it is the thing that was impossible.

use serde::de::IgnoredAny;
use serde::{Deserialize, Deserializer, Serializer};

/// Writes the id as a TOML integer when it fits, and as a decimal string when it does not.
///
/// Args:
///     value: Id to write.
///     serializer: Target serializer.
///
/// Returns:
///     The serializer's own output.
pub(super) fn serialize<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match i64::try_from(*value) {
        Ok(fits) => serializer.serialize_i64(fits),
        Err(_) => serializer.serialize_str(&value.to_string()),
    }
}

/// Reads an id written as either an integer or a decimal string.
///
/// A value that is neither — a negative number, a non-numeric string, a table, an array, a date,
/// anything a hand edit can leave behind — resolves to `0`, which every reader of these fields
/// already treats as "no strategy": `ManualStratState::id` falls back to its strategy NAME, which
/// is that field's own documented recovery for a pin it can no longer trust, and
/// `ServerMeta::default_alert_strategy` assigns nothing. Failing instead would reject the whole
/// `settings.toml`, quarantine it, and take every unrelated setting down with one bad number —
/// the same disproportion this module exists to remove, pointed the other way.
///
/// Shaped as an untagged enum rather than a hand-written `Visitor`, which is the crate's existing
/// form for a lenient config read (`layout::serde_compat::de_lenient`, `de_lenient_u32` and their
/// neighbours). It matters beyond line count: `IgnoredAny` absorbs EVERY remaining shape by
/// construction, so no future TOML type can arrive at a missing `visit_*` arm and take the file
/// down through a gap nobody thought to close.
///
/// Args:
///     deserializer: Source deserializer.
///
/// Returns:
///     The id, or `0` when the stored value does not name one.
pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    /// Every shape a stored id can be found in.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stored {
        /// A bare non-negative integer: every id at or below the ceiling, which is every id any
        /// earlier build was able to write.
        Number(u64),
        /// A quoted decimal id — the only shape that can carry a value above the ceiling, and
        /// also how a value arrives when copied from somewhere that quotes it.
        Text(String),
        /// Anything else at all. Accepted and discarded.
        Other(IgnoredAny),
    }

    Ok(match Stored::deserialize(deserializer)? {
        Stored::Number(id) => id,
        // `trim` for the same reason `de_lenient_u32` trims: this file is hand-editable.
        Stored::Text(text) => text.trim().parse().unwrap_or_else(|_| reject(&text)),
        Stored::Other(_) => reject("a value of another type"),
    })
}

/// Logs a stored value that does not name an id and answers "no strategy"; see [`deserialize`]
/// for why that is not an error.
///
/// Args:
///     found: The stored value, for the log line.
///
/// Returns:
///     Always `0`.
fn reject(found: &str) -> u64 {
    log::warn!("config strategy id is {found} — read as \"none\"");
    0
}

#[cfg(test)]
mod tests;
