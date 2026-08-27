//! Unit tests for Versions-pane fact priority and narrow-width degradation.

use chrono_tz::UTC;
use moon_core::strat_db::stats::VersionInfo;
use moon_core::util::fmt;
use rust_i18n::t;

use super::{VersionSlot, version_row_facts};

/// Build a version whose optional row facts all exist, so each clip-priority position is visible.
fn populated_version() -> VersionInfo {
    VersionInfo {
        valid_from: 1_000_000,
        valid_to: None,
        change_kind: "restored".to_string(),
        origin: Some("local".to_string()),
        n_changed: 3,
        trades: 3,
        profit: 12.5,
        open_left: 0,
    }
}

/// `version_facts.rs::version_row_facts` must retain slot, changed-count, kind, origin, then age
/// in that order; swapping two tail pushes makes a clipped row hide a more useful fact first.
#[test]
fn version_row_facts_keep_the_tail_clip_priority() {
    let facts = version_row_facts(
        &populated_version(),
        VersionSlot::InEffect,
        UTC,
        1_120_000,
        false,
        200,
        "T",
    );

    let tail = facts
        .tail
        .iter()
        .map(|fact| fact.text.as_str())
        .collect::<Vec<_>>();
    let expected = vec![
        t!("strat.version_open").to_string(),
        t!("strat.version_changed_n", n = 3).to_string(),
        t!("strat.version_kind_restored").to_string(),
        t!("strat.version_origin_local").to_string(),
        t!("strat.version_age_m", n = 2).to_string(),
    ];

    assert_eq!(tail, expected);
    assert_eq!(
        facts.tail.iter().map(|fact| fact.badge).collect::<Vec<_>>(),
        vec![true, true, false, false, false],
        "only the slot and changed-count facts remain badges"
    );
}

/// `version_facts.rs::version_row_facts` must use full money beside the stamp, then compact money,
/// then a full-money second line, and finally SI money; skipping a rung either wastes a line or
/// shows less financial context than the available narrow-pane width permits.
#[test]
fn version_row_facts_degrade_money_before_adding_or_shortening_lines() {
    let version = populated_version();
    let full = t!("strat.version_profit", amount = "+12.50", n = 3).to_string();
    let compact = "+12.50$".to_string();
    let si = format!("+{}$", fmt::compact_si(12.5));

    let full_beside = version_row_facts(
        &version,
        VersionSlot::Older,
        UTC,
        1_120_000,
        false,
        1 + 1 + full.chars().count(),
        "T",
    );
    assert_eq!(
        full_beside
            .head
            .iter()
            .map(|line| line
                .iter()
                .map(|fact| fact.text.as_str())
                .collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        vec![vec!["T", full.as_str()]],
        "the full money sentence stays beside a fitting stamp"
    );

    let compact_beside = version_row_facts(
        &version,
        VersionSlot::Older,
        UTC,
        1_120_000,
        false,
        1 + 1 + compact.chars().count(),
        "T",
    );
    assert_eq!(
        compact_beside
            .head
            .iter()
            .map(|line| line
                .iter()
                .map(|fact| fact.text.as_str())
                .collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        vec![vec!["T", compact.as_str()]],
        "a fitting compact amount avoids a second line"
    );

    let full_on_second_line = version_row_facts(
        &version,
        VersionSlot::Older,
        UTC,
        1_120_000,
        false,
        full.chars().count(),
        "a deliberately long stamp",
    );
    assert_eq!(
        full_on_second_line
            .head
            .iter()
            .map(|line| line
                .iter()
                .map(|fact| fact.text.as_str())
                .collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        vec![vec!["a deliberately long stamp"], vec![full.as_str()]],
        "full money gets its own line before it is abbreviated"
    );

    let si_on_second_line = version_row_facts(
        &version,
        VersionSlot::Older,
        UTC,
        1_120_000,
        false,
        compact.chars().count() - 1,
        "a deliberately long stamp",
    );
    assert_eq!(
        si_on_second_line
            .head
            .iter()
            .map(|line| line
                .iter()
                .map(|fact| fact.text.as_str())
                .collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        vec![vec!["a deliberately long stamp"], vec![si.as_str()]],
        "only a compact amount that cannot fit alone reaches the SI form"
    );
}
