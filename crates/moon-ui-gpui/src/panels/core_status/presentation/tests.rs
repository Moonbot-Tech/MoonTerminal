//! Tests for the API-key column's text and colour.
//!
//! Explicit imports (no `use super::*`) per the crate's test convention: the panel's parent module
//! re-exports `gpui::*`, whose own `test` would shadow the built-in attribute.

use super::{LoadLevel, api_expiry_level, api_expiry_text};
use crate::panels::core_status::model::ApiKeyState;

/// "We could not check" and "this key never expires" are different facts, and the column is the
/// only place an operator can tell them apart. Rendering both as one marker would make a broken
/// check look like a healthy perpetual key.
#[test]
fn no_answer_and_no_expiry_read_differently() {
    let unchecked = api_expiry_text(ApiKeyState::Unknown);
    let perpetual = api_expiry_text(ApiKeyState::Perpetual);

    assert_eq!(unchecked, "-", "no answer yet");
    assert_eq!(perpetual, "\u{221e}", "checked: this key has no expiry");
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

/// A key with no expiration, and one never checked, must never colour: the engine cannot warn about
/// either, so the row has nothing to report.
#[test]
fn a_perpetual_or_unchecked_key_is_never_coloured() {
    assert_eq!(
        api_expiry_level(ApiKeyState::Perpetual, false),
        LoadLevel::Normal
    );
    assert_eq!(
        api_expiry_level(ApiKeyState::Unknown, false),
        LoadLevel::Normal
    );
}
