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

/// The engine's warning flag still controls the Warning branch before the panel's independent
/// notice band. Letting notice replace it would leave a number blue or grey under the engine's
/// yellow warning triangle.
#[test]
fn the_colour_follows_the_engines_decision_not_its_own_thresholds() {
    // 30 days left, and the user set a 60-day horizon: the engine warns, so the cell must colour.
    assert_eq!(
        api_expiry_level(ApiKeyState::Days(30), true, false),
        LoadLevel::Warning
    );
    // Same 30 days under the default horizon: no warning, no colour.
    assert_eq!(
        api_expiry_level(ApiKeyState::Days(30), false, false),
        LoadLevel::Normal
    );
}

/// `presentation.rs:api_expiry_level` must reject stale warning flags when `state.days()` is
/// absent. Mutation: delete the no-day-count guard; an unknown or perpetual key would render as a
/// warning even though the terminal has no expiring key to report.
#[test]
fn an_unknown_or_perpetual_key_ignores_stale_warning_flags() {
    assert_eq!(
        api_expiry_level(ApiKeyState::Unknown, true, false),
        LoadLevel::Normal,
        "an absent expiry cannot be a warning"
    );
    assert_eq!(
        api_expiry_level(ApiKeyState::Perpetual, true, true),
        LoadLevel::Normal,
        "a perpetual key cannot be a warning"
    );
    assert_eq!(
        api_expiry_level(ApiKeyState::Days(30), true, false),
        LoadLevel::Warning,
        "a reported day count still obeys the engine warning flag"
    );
}

/// An expired key is red on its own, whether or not the axis is currently warning — a disabled axis
/// must not repaint a dead key as healthy.
#[test]
fn an_expired_key_is_red_regardless() {
    assert_eq!(
        api_expiry_level(ApiKeyState::Days(-3), false, false),
        LoadLevel::Critical
    );
}

/// An unlimited key, and one nothing is known about, must never colour: the engine cannot warn
/// about either, so the row has nothing to report.
#[test]
fn an_unlimited_or_unknown_key_is_never_coloured() {
    assert_eq!(
        api_expiry_level(ApiKeyState::Perpetual, false, false),
        LoadLevel::Normal
    );
    assert_eq!(
        api_expiry_level(ApiKeyState::Unknown, false, false),
        LoadLevel::Normal
    );
}

/// The quota cell is a bare count for the same reason the day count is: the noun belongs to the
/// heading. An absent quota is a plain dash — every core but a HyperLiquid one reports none, and
/// rendering that as `0` would read as an exhausted budget on the whole fleet.
#[test]
fn a_quota_renders_as_a_bare_count_or_a_dash() {
    assert_eq!(super::api_quota_text(Some(1_065_447)), "1065447");
    assert_eq!(super::api_quota_text(Some(0)), "0", "a real zero is a zero");
    assert_eq!(super::api_quota_text(None), "-");
}

/// The colour follows the engine's flag, but only while there is a number to colour. A core that
/// stopped publishing must fall back to no colour even if the flag has not been rebuilt yet —
/// otherwise a dash paints yellow and claims a budget nobody reported.
#[test]
fn a_quota_colours_only_while_it_has_a_number() {
    use super::LoadLevel;
    assert_eq!(
        super::api_quota_level(Some(900), true),
        LoadLevel::Warning,
        "the engine warns and there is a number"
    );
    assert_eq!(super::api_quota_level(Some(900), false), LoadLevel::Normal);
    assert_eq!(
        super::api_quota_level(None, true),
        LoadLevel::Normal,
        "a stale flag must not colour an absence"
    );
}
