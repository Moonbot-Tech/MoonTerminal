//! Tests for the API-key column's text and colour.
//!
//! Explicit imports (no `use super::*`) per the crate's test convention: the panel's parent module
//! re-exports `gpui::*`, whose own `test` would shadow the built-in attribute.

use super::{LoadLevel, api_expiry_level, api_expiry_text};
use crate::panels::core_status::model::ApiKeyState;

/// The cell carries a BARE number — the unit lives in the column heading. A per-row "дн" would
/// repeat itself down the whole column and push the heading's meaning into the data.
#[test]
fn a_day_count_renders_without_its_unit() {
    assert_eq!(api_expiry_text(ApiKeyState::Days(45)), "45");
    assert_eq!(api_expiry_text(ApiKeyState::Days(0)), "0", "its last day");
}

/// A key past its date reads as a WORD, not as a negative number: "-3" under a heading that says
/// days would look like a count, and this is the one arm an operator must not have to decode.
#[test]
fn an_expired_key_reads_as_a_word() {
    let text = api_expiry_text(ApiKeyState::Days(-3));
    assert!(
        text.parse::<i32>().is_err(),
        "a count would be read as days remaining, got {text:?}"
    );
    assert!(!text.is_empty(), "and it has to say something");
    // Distinct from every other arm, so "expired" cannot be mistaken for "nothing known".
    for other in [ApiKeyState::Unknown, ApiKeyState::Perpetual] {
        assert_ne!(text, api_expiry_text(other));
    }
}

/// "Nothing is known" and "effectively unlimited" are different facts and must not share a marker:
/// a failed check that rendered as ∞ would look like a healthy key nobody has to think about.
#[test]
fn unknown_and_unlimited_read_differently() {
    assert_eq!(api_expiry_text(ApiKeyState::Unknown), "-");
    assert_eq!(api_expiry_text(ApiKeyState::Perpetual), "\u{221e}");
}

/// The colour follows the ENGINE's warning decision, which carries the user's day threshold. A
/// second set of steps here would leave the number grey under a lit warning triangle as soon as the
/// threshold is not the default — the disagreement `lat_level` exists to prevent for the pings.
#[test]
fn the_colour_follows_the_engines_decision_not_its_own_thresholds() {
    // 30 days left, and the user set a 60-day horizon: the engine warns, so the cell must colour.
    assert_eq!(
        api_expiry_level(ApiKeyState::Days(30), true),
        LoadLevel::Warning
    );
    // Same 30 days under the default horizon: no warning, no colour.
    assert_eq!(
        api_expiry_level(ApiKeyState::Days(30), false),
        LoadLevel::Normal
    );
}

/// An expired key is red on its own, whether or not the axis is currently warning — a disabled axis
/// must not repaint a dead key as healthy.
#[test]
fn an_expired_key_is_red_regardless() {
    assert_eq!(
        api_expiry_level(ApiKeyState::Days(-3), false),
        LoadLevel::Critical
    );
}

/// An unlimited key, and one nothing is known about, must never colour: the engine cannot warn
/// about either, so the row has nothing to report.
#[test]
fn an_unlimited_or_unknown_key_is_never_coloured() {
    assert_eq!(
        api_expiry_level(ApiKeyState::Perpetual, false),
        LoadLevel::Normal
    );
    assert_eq!(
        api_expiry_level(ApiKeyState::Unknown, false),
        LoadLevel::Normal
    );
}
