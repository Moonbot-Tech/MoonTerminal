//! Unit checks for the core-settings write-address guard, and for where a blacklist reveal is shown.
//!
//! Explicit imports throughout: the parent re-exports `gpui::*`, whose own `test` shadows the
//! built-in attribute and makes `#[test]` expand recursively.

use moon_core::session::CoreId;

use super::draft::{fmt_hhmm, parse_num};
use super::{GotoAction, core_settings_goto_action, resolve_core_settings_write};

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

/// The compact popup only when it can write this core. Every other case stays on the expert page.
#[test]
fn goto_action_uses_the_popup_only_for_the_active_core() {
    let target = 2u64;
    let other = 9u64;
    assert_eq!(
        core_settings_goto_action(target, Some(target), true),
        GotoAction::CompactPopup
    );
    assert_eq!(
        core_settings_goto_action(target, Some(other), true),
        GotoAction::ExpertPage
    );
    assert_eq!(
        core_settings_goto_action(target, None, true),
        GotoAction::ExpertPage
    );
    assert_eq!(
        core_settings_goto_action(target, Some(target), false),
        GotoAction::ExpertPage
    );
}

/// The drain must not name either active-core setter. The pin is the function, not the file:
/// other shell code still switches cores for the header.
#[test]
fn drain_does_not_switch_the_active_core() {
    assert_reveal_does_not_switch(
        include_str!("../core_settings.rs"),
        "pub(super) fn drain_core_settings_goto(",
    );
    assert_reveal_does_not_switch(
        include_str!("../../core_expert.rs"),
        "pub(crate) fn open_core_expert_on(",
    );
    assert_reveal_does_not_switch(
        include_str!("../../core_expert/sync.rs"),
        "pub(super) fn select_exact_core(",
    );
    assert_reveal_does_not_switch(
        include_str!("../../core_expert/sync.rs"),
        "pub(super) fn reveal_core_blacklist(",
    );
    assert_reveal_does_not_switch(
        include_str!("../../panels/chart/filter_headers.rs"),
        "pub(super) fn try_open_filter_line(",
    );
}

/// The named function must not call either active-core setter.
///
/// Args:
///     source: Rust source containing the target function.
///     signature: Unique prefix that starts the target function.
fn assert_reveal_does_not_switch(source: &str, signature: &str) {
    let body = braced_body(source, signature);
    assert!(
        !body.contains("set_active_trade_core"),
        "{signature} must not change the active trading core"
    );
    assert!(
        !body.contains("select_auto_workspace_core"),
        "{signature} must not change the auto workspace core"
    );
}

/// Return the brace-balanced body that starts at `signature`.
///
/// Args:
///     source: Rust source containing the target function.
///     signature: Unique prefix that starts the target function.
///
/// Returns:
///     The source slice from `signature` through its matching closing brace.
fn braced_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("expected `{signature}` in source"));
    let open = source[start..]
        .find('{')
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("expected opening brace after `{signature}`"));
    let mut depth = 0usize;
    for (offset, ch) in source[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..=open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced brace after `{signature}`");
}
