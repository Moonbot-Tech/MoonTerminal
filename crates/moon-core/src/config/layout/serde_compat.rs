//! Lenient deserializers for the hand-editable layout persistence formats.
//!
//! `layout.toml` is decoded as one document, so every helper here confines a malformed field to
//! that field's default or salvage value instead of discarding unrelated window and table state.
//! The persisted declarations and their Serde attributes remain in the parent module so the wire
//! contract stays visible beside each field.

use std::collections::HashMap;
use std::hash::Hash;

use serde::Deserialize;

use super::super::chart_labels::ChartLabelsCfg;
use super::{
    ChartGraphicsCfg, TableSortPreference, clamp_auto_workspace_rail_width,
    clamp_strategies_tree_text_step, def_candle_volume_alpha, def_candle_volume_height,
    def_candle_volume_scale, def_candle_volume_style, def_connector_thickness_px, def_marker_scale,
    def_trade_arrow_scale, def_trade_volume_alpha,
};

/// Read the tuner seed from whatever `layout.toml` happens to hold, never failing.
///
/// This file is deserialized as one document with no schema version, so a single field that does
/// not match its declared type discards the entire saved layout: every window position and every
/// column width. The seed is the field most likely to be typed by hand, and the intuitive way to
/// write it is bare (`analytics_tuner_seed = 123`), which a `String` field rejects. The field
/// therefore accepts a quoted string, a bare integer, or anything else at all, and answers "no
/// seed" rather than taking the rest of the layout down with it.
pub(super) fn de_lenient_seed<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    /// Every shape the seed field might be found in.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Seed {
        /// A quoted decimal seed.
        Text(String),
        /// A bare non-negative integer seed.
        Number(u64),
        /// Anything else: a float, a boolean, or a table. Accepted and discarded.
        Other(serde::de::IgnoredAny),
    }

    Ok(match Option::<Seed>::deserialize(d)? {
        Some(Seed::Text(s)) => Some(s),
        Some(Seed::Number(n)) => Some(n.to_string()),
        Some(Seed::Other(_)) | None => None,
    })
}

/// Read a hand-editable tuner number the same forgiving way as [`de_lenient_seed`].
///
/// The tuner's search settings sit together in this file and are edited by hand together, so they
/// carry the same hazard: one of them written as `"64"` instead of `64`, or as `0.7` instead of a
/// percentage, would take every window position and column width in the document with it. Each
/// answers "unset" instead, and the tuner then applies its own default.
pub(super) fn de_lenient_u32<'de, D>(d: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    /// Every shape one of these fields might be found in.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Num {
        /// A bare non-negative integer.
        Number(u32),
        /// A quoted number, which is how a value gets written when copied from elsewhere.
        Text(String),
        /// Anything else: a float, a negative, a boolean, or a table. Accepted and discarded.
        Other(serde::de::IgnoredAny),
    }

    Ok(match Option::<Num>::deserialize(d)? {
        Some(Num::Number(v)) => Some(v),
        Some(Num::Text(s)) => s.trim().parse().ok(),
        Some(Num::Other(_)) | None => None,
    })
}

/// Read any optional field the same forgiving way as `de_lenient_u32`, with no coercion.
///
/// The three helpers around this one accept a neighbouring shape: a bare seed integer, a quoted
/// number, or a quoted bool. This one only salvages the document: a value of the wrong type reads
/// as "unset" instead of taking every window position and column width down with it. Reach for it
/// whenever a new `Option<T>` field lands in this hand-edited file and needs no coercion of its
/// own; `analytics_tuner_fields` written as a bare `"lev"` instead of `["lev"]` is the shape it is
/// there for. This remains public because the same salvaging applies to the geometry records in
/// `charts.json` and `detached.json`, which are hand-editable for the same reasons.
///
/// Note that it runs only when the key is present: `#[serde(default)]` answers an absent key with
/// `None` without deserializing, which is what keeps "absent" and "present but empty" distinct.
pub fn de_lenient<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    /// The declared shape, or anything else at all.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Or<T> {
        /// The shape the key is written in.
        Val(T),
        /// Anything else. Accepted and discarded.
        Other(serde::de::IgnoredAny),
    }

    Ok(match Option::<Or<T>>::deserialize(d)? {
        Some(Or::Val(v)) => Some(v),
        Some(Or::Other(_)) | None => None,
    })
}

/// Read one [`ChartGraphicsCfg`] boolean leniently, defaulting an unusable value to `true`.
///
/// `ChartGraphicsCfg`'s booleans no longer share one default, so this covers only the ones that
/// default on; see [`de_lenient_false`] for the ones that don't.
pub(super) fn de_lenient_true<'de, D>(d: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(de_lenient::<D, bool>(d)?.unwrap_or(true))
}

/// Read one [`ChartGraphicsCfg`] boolean leniently, defaulting an unusable value to `false`.
///
/// A separate helper from [`de_lenient_true`] on purpose: reusing that one for a field whose
/// documented default is `false` would make a malformed value fall back to `true` instead,
/// flipping the field on for every user with a bad read.
pub(super) fn de_lenient_false<'de, D>(d: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(de_lenient::<D, bool>(d)?.unwrap_or(false))
}

/// Read the arrow-size multiplier leniently, defaulting an unusable value.
pub(super) fn de_arrow_scale<'de, D>(d: D) -> Result<f32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(de_lenient::<D, f32>(d)?.unwrap_or_else(def_trade_arrow_scale))
}

/// Read the connector thickness leniently, defaulting an unusable value.
pub(super) fn de_connector_thickness<'de, D>(d: D) -> Result<f32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(de_lenient::<D, f32>(d)?.unwrap_or_else(def_connector_thickness_px))
}

/// Read the trade-marker size multiplier leniently, defaulting an unusable value.
pub(super) fn de_marker_scale<'de, D>(d: D) -> Result<f32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(de_lenient::<D, f32>(d)?.unwrap_or_else(def_marker_scale))
}

/// Read the per-trade volume opacity leniently, defaulting an unusable value.
pub(super) fn de_trade_volume_alpha<'de, D>(d: D) -> Result<f32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(de_lenient::<D, f32>(d)?.unwrap_or_else(def_trade_volume_alpha))
}

/// Read the bottom-volume style id leniently, defaulting an unusable value.
pub(super) fn de_candle_volume_style<'de, D>(d: D) -> Result<u8, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(de_lenient::<D, u8>(d)?.unwrap_or_else(def_candle_volume_style))
}

/// Read the bottom-volume band height leniently, defaulting an unusable value.
pub(super) fn de_candle_volume_height<'de, D>(d: D) -> Result<f32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(de_lenient::<D, f32>(d)?.unwrap_or_else(def_candle_volume_height))
}

/// Read the bottom-volume opacity leniently, defaulting an unusable value.
pub(super) fn de_candle_volume_alpha<'de, D>(d: D) -> Result<f32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(de_lenient::<D, f32>(d)?.unwrap_or_else(def_candle_volume_alpha))
}

/// Read the volume-scale colour leniently, defaulting an unusable value.
pub(super) fn de_candle_volume_scale<'de, D>(d: D) -> Result<[u8; 3], D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(de_lenient::<D, [u8; 3]>(d)?.unwrap_or_else(def_candle_volume_scale))
}

/// Read [`ChartGraphicsCfg`] leniently, falling back to the defaults for an unusable table.
///
/// The whole document is one deserialization, so a hand-edited chart graphics table must never
/// cost every window position in the file.
pub(super) fn de_lenient_graphics<'de, D>(d: D) -> Result<ChartGraphicsCfg, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(de_lenient::<D, ChartGraphicsCfg>(d)?.unwrap_or_default())
}

/// Read [`ChartLabelsCfg`] leniently, repairing what a hand-edited file states.
///
/// Two failures are handled here rather than downstream: an unusable table costs the labels only
/// and not every window position in the document, and a usable one is still sanitized because the
/// drawing pass has no way to honour what a hand-edited file can state: a hole between the
/// captions of a row, or a size outside the drawable range.
pub(super) fn de_lenient_chart_labels<'de, D>(d: D) -> Result<ChartLabelsCfg, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let mut cfg = de_lenient::<D, ChartLabelsCfg>(d)?.unwrap_or_default();
    cfg.sanitize();
    Ok(cfg)
}

/// Read a hand-editable map without allowing one malformed preference to reject the layout.
///
/// Args:
///     d: Serde deserializer positioned at the complete map value.
///
/// Returns:
///     The decoded map, or an empty map when its shape or any entry is unusable.
///
/// Errors:
///     Propagates only deserializer failures that cannot be consumed as ignored input.
pub(super) fn de_lenient_map<'de, D, K, V>(d: D) -> Result<HashMap<K, V>, D::Error>
where
    D: serde::Deserializer<'de>,
    K: Deserialize<'de> + Eq + Hash,
    V: Deserialize<'de>,
{
    Ok(de_lenient(d)?.unwrap_or_default())
}

/// Read table-sort preferences while discarding only the malformed entries.
///
/// Args:
///     d: Serde deserializer positioned at the complete `table_sorts` value.
///
/// Returns:
///     Every well-formed entry, or an empty map when the outer value is not a map.
///
/// Errors:
///     Propagates only deserializer failures that cannot be consumed as ignored input.
pub(super) fn de_table_sort_map<'de, D>(
    d: D,
) -> Result<HashMap<String, TableSortPreference>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    /// One usable preference or an ignored malformed entry.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Entry {
        /// Exact table-sort shape.
        Valid(TableSortPreference),
        /// Any unsupported entry shape.
        Other(serde::de::IgnoredAny),
    }

    /// The expected map or an ignored malformed outer value.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stored {
        /// Context ids mapped to independently recoverable entries.
        Map(HashMap<String, Entry>),
        /// Any unsupported outer shape.
        Other(serde::de::IgnoredAny),
    }

    Ok(match Stored::deserialize(d)? {
        Stored::Map(entries) => entries
            .into_iter()
            .filter_map(|(id, entry)| match entry {
                Entry::Valid(preference) => Some((id, preference)),
                Entry::Other(_) => None,
            })
            .collect(),
        Stored::Other(_) => HashMap::new(),
    })
}

/// Decode the hand-editable Auto rail width without rejecting the complete layout document.
///
/// Args:
///     d: Serde deserializer positioned at the present rail-width value.
///
/// Returns:
///     A clamped numeric value, accepting quoted numbers and defaulting every malformed shape.
///
/// Errors:
///     Propagates only deserializer failures that cannot be consumed as ignored input.
pub(super) fn de_auto_workspace_rail_width<'de, D>(d: D) -> Result<Option<f32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    /// Every shape a hand-edited rail width may use.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Width {
        /// A bare TOML number.
        Number(f32),
        /// A quoted number copied from another settings surface.
        Text(String),
        /// Any unsupported shape accepted only to salvage the complete document.
        Other(serde::de::IgnoredAny),
    }

    let width = match Option::<Width>::deserialize(d)? {
        Some(Width::Number(width)) => Some(width),
        Some(Width::Text(width)) => width.trim().parse().ok(),
        Some(Width::Other(_)) | None => None,
    };
    Ok(width.map(clamp_auto_workspace_rail_width))
}

/// Decode the hand-editable Strategies tree text step without rejecting the complete layout
/// document.
///
/// Args:
///     d: Serde deserializer positioned at the present text-step value.
///
/// Returns:
///     A clamped, rounded numeric value, accepting quoted numbers and defaulting every malformed
///     shape.
///
/// Errors:
///     Propagates only deserializer failures that cannot be consumed as ignored input.
pub(super) fn de_strategies_tree_text_step<'de, D>(d: D) -> Result<Option<f32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    /// Every shape a hand-edited text step may use.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Step {
        /// A bare TOML number.
        Number(f32),
        /// A quoted number copied from another settings surface.
        Text(String),
        /// Any unsupported shape accepted only to salvage the complete document.
        Other(serde::de::IgnoredAny),
    }

    let step = match Option::<Step>::deserialize(d)? {
        Some(Step::Number(step)) => Some(step),
        Some(Step::Text(step)) => step.trim().parse().ok(),
        Some(Step::Other(_)) | None => None,
    };
    Ok(step.map(clamp_strategies_tree_text_step))
}

/// Read the optional clock zone without conflating a malformed present key with an absent one.
///
/// Args:
///     d: Serde deserializer positioned at a present `header_clock_zone` value.
///
/// Returns:
///     The saved string, or an empty invalid sentinel for any other shape. An absent key bypasses
///     this function through `#[serde(default)]` and remains `None` for first-run detection.
///
/// Errors:
///     Propagates deserializer errors for values that cannot be visited at all.
pub(super) fn de_clock_zone<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    /// A valid text zone or any malformed value that must remain distinguishable from absence.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ClockZone {
        /// Persisted IANA identifier.
        Text(String),
        /// Wrong-typed value accepted only to salvage the surrounding layout document.
        Invalid(serde::de::IgnoredAny),
    }

    Ok(match Option::<ClockZone>::deserialize(d)? {
        Some(ClockZone::Text(value)) => Some(value),
        Some(ClockZone::Invalid(_)) | None => Some(String::new()),
    })
}

/// Read a hand-editable flag the same forgiving way as [`de_lenient_u32`].
///
/// A quoted `"true"` is the natural typo for someone flipping a display lens by hand, and a plain
/// `bool` field would answer it by discarding every window position and column width in the
/// document. A quoted boolean is therefore read as that boolean, case-insensitively, and every
/// other shape reads as `false`, matching the field's default when the key is absent.
pub fn de_lenient_bool<'de, D>(d: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    /// Every shape the flag might be found in.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Flag {
        /// A bare boolean, which is the shape this key is written in.
        Bool(bool),
        /// A quoted boolean, which is how one gets typed by hand.
        Text(String),
        /// Anything else: a number, a list, or a table. Accepted and discarded.
        Other(serde::de::IgnoredAny),
    }

    Ok(match Option::<Flag>::deserialize(d)? {
        Some(Flag::Bool(v)) => v,
        Some(Flag::Text(s)) => s.trim().eq_ignore_ascii_case("true"),
        Some(Flag::Other(_)) | None => false,
    })
}
