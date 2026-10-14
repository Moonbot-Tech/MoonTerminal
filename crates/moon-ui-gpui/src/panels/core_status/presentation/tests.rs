//! Tests for the API-key column's text and colour.
//!
//! Explicit imports (no `use super::*`) per the crate's test convention: the panel's parent module
//! re-exports `gpui::*`, whose own `test` would shadow the built-in attribute.

use super::{
    BuildParts, LoadLevel, api_expiry_level, api_expiry_text, api_expiry_tooltip, build_parts,
    cpu_level, free_mem_level, group_build_parts, status_level, update_tooltip, version_text,
};
use crate::panels::core_status::model::ApiKeyState;
use crate::panels::core_status::model::GroupVersion;
use moon_core::feed::ConnStatus;
use moon_core::feed::UpdateTarget;
use moon_core::session::core_update::{CoreUpdateOutcome, CoreUpdatePhase};

/// Treating every Unchanged verdict as an unchanged version hides a refused named build.
#[test]
fn a_different_reported_build_names_the_expected_and_actual_letters() {
    let _locale = crate::test_locale::force("en");
    let target = UpdateTarget::Named("MoonBot-R3".into());
    for (suffix, expected) in [
        (Some("R2"), "Different build installed: R2 instead of R3"),
        (Some(""), "Different build installed: Release instead of R3"),
        (Some(" r3 "), "Version unchanged"),
        (None, "Version unchanged"),
    ] {
        let phase = CoreUpdatePhase::Done(CoreUpdateOutcome::Unchanged {
            version: 771,
            to_suffix: suffix.map(str::to_string),
        });
        assert_eq!(update_tooltip(&phase, Some(&target)), expected);
    }
}

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
    let _locale = crate::test_locale::force("en");
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

/// `presentation.rs:cpu_level` must include both CPU thresholds. Mutation: change either `>=`
/// comparison to `>` or swap the thresholds; a core exactly at 10% or 25% system CPU would stop
/// being marked, hiding the outlier this view is meant to surface.
#[test]
fn cpu_thresholds_include_the_boundary_that_marks_an_outlier() {
    assert_eq!(cpu_level(None), LoadLevel::Normal);
    assert_eq!(cpu_level(Some(9)), LoadLevel::Normal);
    assert_eq!(cpu_level(Some(10)), LoadLevel::Warning);
    assert_eq!(cpu_level(Some(24)), LoadLevel::Warning);
    assert_eq!(cpu_level(Some(25)), LoadLevel::Critical);
    assert_eq!(cpu_level(Some(100)), LoadLevel::Critical);
}

/// `presentation.rs:free_mem_level` must classify absolute free-memory boundaries. Mutation:
/// change either `<` to `<=` or reconstruct a percentage from process RAM; 299 MB free could
/// render as normal and hide a core approaching memory exhaustion.
#[test]
fn free_memory_uses_absolute_headroom_with_exclusive_limits() {
    assert_eq!(free_mem_level(None), LoadLevel::Normal);
    assert_eq!(free_mem_level(Some(300)), LoadLevel::Normal);
    assert_eq!(free_mem_level(Some(299)), LoadLevel::Warning);
    assert_eq!(free_mem_level(Some(150)), LoadLevel::Warning);
    assert_eq!(free_mem_level(Some(149)), LoadLevel::Critical);
    assert_eq!(free_mem_level(Some(0)), LoadLevel::Critical);
}

/// `presentation.rs:status_level` must keep reconnecting states warning-level and outages
/// critical. Mutation: move `Connecting` or `Stage(_)` to Critical, or `Disconnected` to Warning;
/// ordinary backoff would cause alarm fatigue or a lost core would no longer read as an alarm.
#[test]
fn connection_states_distinguish_reconnects_from_outages() {
    assert_eq!(status_level(&ConnStatus::Ready), LoadLevel::Normal);
    assert_eq!(status_level(&ConnStatus::Connecting), LoadLevel::Warning);
    assert_eq!(
        status_level(&ConnStatus::Stage("reconnecting".to_string())),
        LoadLevel::Warning
    );
    assert_eq!(
        status_level(&ConnStatus::Failed("authentication refused".to_string())),
        LoadLevel::Critical
    );
    assert_eq!(status_level(&ConnStatus::Disconnected), LoadLevel::Critical);
}

/// `presentation.rs:api_expiry_tooltip` must explain the infinity glyph for a perpetual key.
/// Mutation: return `None` for `ApiKeyState::Perpetual`; the API-days column would show an
/// unexplained infinity glyph again.
#[test]
fn perpetual_api_keys_keep_an_explanatory_tooltip() {
    let _locale = crate::test_locale::force("en");
    let perpetual_tip = api_expiry_tooltip(ApiKeyState::Perpetual)
        .expect("a perpetual key needs a tooltip that explains its infinity glyph");
    assert!(
        !perpetual_tip.trim().is_empty(),
        "the glyph explanation must contain readable text"
    );
    for state in [
        ApiKeyState::Unknown,
        ApiKeyState::Days(0),
        ApiKeyState::Days(-1),
    ] {
        assert_eq!(
            api_expiry_tooltip(state),
            None,
            "only the infinity glyph needs this explanatory tooltip"
        );
    }
}

/// A same-number Succeeded (a named build installed) reads "installed"; a moved number reads
/// "updated". Breaks when the key ignores `from == to`.
#[test]
fn core_update_same_number_success_reads_installed() {
    use moon_core::session::core_update::CoreUpdateOutcome;
    assert_eq!(
        super::succeeded_locale_key(&CoreUpdateOutcome::Succeeded {
            from: Some(771),
            to: 771,
            to_suffix: None,
        }),
        "core_update.phase.installed"
    );
    assert_eq!(
        super::succeeded_locale_key(&CoreUpdateOutcome::Succeeded {
            from: Some(770),
            to: 771,
            to_suffix: None,
        }),
        "core_update.phase.succeeded"
    );
}

/// A named build prints its letter. A release and a core that sent no letter both print the
/// bare number, and a missing number stays the dash.
#[test]
fn version_text_appends_a_named_letter() {
    assert_eq!(version_text(Some(771), Some("R3")), "7.71 R3");
    assert_eq!(version_text(Some(771), Some("")), "7.71");
    assert_eq!(version_text(Some(771), None), "7.71");
    assert_eq!(version_text(None, Some("R3")), "-");
}

/// Joining the letter back onto the number would shift a named row's digits off the column
/// again. A release, a blank letter and an older core must not grow a tag, and a letter that
/// arrived without a number must not paint one.
#[test]
fn build_parts_keeps_the_number_and_lifts_only_a_real_letter() {
    assert_eq!(
        build_parts(Some(771), Some("R3")),
        BuildParts {
            number: "7.71".to_string(),
            tag: Some("R3".to_string()),
        }
    );
    assert_eq!(
        build_parts(Some(771), Some(" R3 ")),
        BuildParts {
            number: "7.71".to_string(),
            tag: Some("R3".to_string()),
        }
    );
    for suffix in [Some(""), Some("   "), None] {
        assert_eq!(
            build_parts(Some(771), suffix),
            BuildParts {
                number: "7.71".to_string(),
                tag: None,
            },
            "a release, a blank letter and a missing letter are the bare number"
        );
    }
    assert_eq!(
        build_parts(None, Some("R3")),
        BuildParts {
            number: "-".to_string(),
            tag: None,
        }
    );
}

/// Tagging a collapsed server that does not agree would claim one core's letter for the
/// whole machine. Only a uniform named build wears the tag; mixed and absent stay unmarked.
#[test]
fn group_build_parts_tags_only_an_agreed_letter() {
    assert_eq!(
        group_build_parts(&GroupVersion::Uniform {
            number: 771,
            suffix: Some("R3".to_string()),
        }),
        BuildParts {
            number: "7.71".to_string(),
            tag: Some("R3".to_string()),
        }
    );
    for suffix in [Some(String::new()), None] {
        assert_eq!(
            group_build_parts(&GroupVersion::Uniform {
                number: 771,
                suffix,
            }),
            BuildParts {
                number: "7.71".to_string(),
                tag: None,
            }
        );
    }
    assert_eq!(
        group_build_parts(&GroupVersion::Mixed),
        BuildParts {
            number: "\u{2026}".to_string(),
            tag: None,
        }
    );
    assert_eq!(
        group_build_parts(&GroupVersion::Absent),
        BuildParts {
            number: "-".to_string(),
            tag: None,
        }
    );
}
