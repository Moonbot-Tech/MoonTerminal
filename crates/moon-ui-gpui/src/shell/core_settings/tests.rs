//! Unit checks for the core-settings write-address guard.
//!
//! Explicit imports throughout: the parent re-exports `gpui::*`, whose own `test` shadows the
//! built-in attribute and makes `#[test]` expand recursively.

use moon_core::session::CoreId;

use super::draft::{fmt_hhmm, parse_num};
use super::resolve_core_settings_write;

/// Dropping finite filtering or zero normalization changes the draft's wire comparisons.
#[test]
fn numeric_draft_rejects_infinity_and_canonicalizes_zero() {
    assert_eq!(parse_num("inf"), None);
    let zero = parse_num("-0").expect("zero is a valid threshold");
    assert_eq!(zero, 0.0);
    assert!(zero.is_sign_positive());
    assert_eq!(parse_num(" 1,5 "), Some(1.5));
}

/// Wrapping at midnight would change the existing draft formatter's unrestricted hours.
#[test]
fn draft_time_formatter_keeps_hours_beyond_one_day() {
    assert_eq!(fmt_hhmm(1500), "25:00");
}

/// A core that has never been seen as `seeded` or `active` in any case below, so a wrongly
/// permissive guard returning it would be visible rather than accidentally matching a fixture.
const OTHER: CoreId = 999;

const SEEDED: CoreId = 7;
const ACTIVE_SAME: CoreId = 7;
const ACTIVE_DIFFERENT: CoreId = 8;

/// `file:symbol` this protects: `shell/core_settings.rs::resolve_core_settings_write`.
///
/// Plausible future edit: a later author "simplifies" the guard to `seeded.or(active)`, or drops
/// the equality check and returns `active` whenever both are `Some`. Either shortcut lets a write
/// seeded from one core land on whatever core is active at commit time — Global TP / Trailing /
/// V-Stop / blacklist / checkbox edits for real money silently applied to the wrong core.
///
/// The four arms below are the guard's whole truth table.
#[test]
fn resolve_core_settings_write_only_permits_the_seeded_core() {
    // Seeded and active agree: the popup still belongs to the core it opened against.
    assert_eq!(
        resolve_core_settings_write(Some(SEEDED), Some(ACTIVE_SAME)),
        Some(SEEDED),
        "a write must be permitted when the active core still matches the seeded one"
    );

    // Seeded and active disagree: the active core moved out from under the open popup.
    assert_eq!(
        resolve_core_settings_write(Some(SEEDED), Some(ACTIVE_DIFFERENT)),
        None,
        "a write must be refused once the active core no longer matches the seeded one"
    );

    // No seed at all: there is no displayed value to commit, regardless of what is active.
    // This is the arm `seeded.or(active)` would break: an unseeded popup has nothing to write,
    // but that shortcut would still hand back `active` as if any core were an acceptable target.
    assert_eq!(
        resolve_core_settings_write(None, Some(ACTIVE_DIFFERENT)),
        None,
        "an unseeded target must never fall through to whatever core is active"
    );

    // Seeded but nothing active (e.g. the group lost its last core): still refused.
    assert_eq!(
        resolve_core_settings_write(Some(SEEDED), None),
        None,
        "a seeded target with no live active core must be refused, not treated as still valid"
    );

    // Sanity: the guard never invents a third core out of nowhere.
    assert_ne!(
        resolve_core_settings_write(Some(SEEDED), Some(ACTIVE_SAME)),
        Some(OTHER)
    );
}
