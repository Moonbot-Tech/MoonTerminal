//! Report column formatting and persisted quote-identity regression tests.

// Explicit imports on purpose: the parent re-exports `gpui::*`, whose `test` would
// shadow the built-in attribute and make `#[test]` expand recursively.
use super::{
    basecurrency_text, cell, cell_display_text, effective_visible_columns, header_for,
    header_label, is_numeric_report_column, report_columns, toggled_all_columns, value_to_string,
};
use crate::panels::common::side_glyph;
use chrono_tz::Tz;
use moon_core::db::ReportAxis;
use moon_ui::MoonPalette;
use rusqlite::types::Value;
use std::collections::{HashMap, HashSet};

// REPAIRED at the PROVE task, by removal: `cell_tooltip` -- the free-text-column-gated tooltip
// this test probed (`valuation_source_tooltip_keeps_complete_provenance`) -- no longer exists.
// The fix barrier deliberately superseded it: `report_data_cell`'s own docstring now states
// "every non-empty cell now truncates and tooltips its complete text, not only the free-text
// columns", so the exact distinction this test drew (VALUATION_SOURCE_COLUMN tooltips, "coin"
// does not) is not a narrowed contract that regressed -- it is a contract that was intentionally
// widened and has no equivalent function left to call. `report_data_cell` is a private fn
// returning an opaque `MoonDataCell` (no accessor reaches its built tooltip text without a
// `gpui::TestAppContext` render pass), so there is no cheap unit-level successor; the widened
// claim is covered structurally by
// `tests/theme_contract/report.rs::the_date_cell_builds_display_and_tooltip_from_one_resolved_zone_in_the_right_order`
// for the date branch, which is the one branch this PROVE task's ranked list calls out as
// unreviewed production code.

/// A generic text cell uses the shared flattener and trims its edges.
#[test]
fn drawn_cells_fold_breaks_and_trim_edges() {
    let raw = Value::Text(" a\r\nb ".to_string());
    assert_eq!(cell_display_text(&raw), "a ¶ b");
}

/// Text used as an identity remains verbatim.
#[test]
fn identity_values_are_not_reshaped() {
    let raw = Value::Text(" SPKUSDT ".to_string());
    assert_eq!(value_to_string(&raw), " SPKUSDT ");
}

/// Non-text values use the raw conversion formatting.
#[test]
fn non_text_values_are_untouched() {
    assert_eq!(cell_display_text(&Value::Null), "");
    assert_eq!(cell_display_text(&Value::Integer(42)), "42");
    assert_eq!(cell_display_text(&Value::Real(1.5)), "1.5");
}

/// Removing the synthetic Profit % header or numeric classification must fail these assertions;
/// otherwise the new column either exposes its internal key or aligns unlike its values.
#[test]
fn profit_percent_has_a_readable_numeric_column_contract() {
    assert_eq!(header_for("profitpct"), "profit %");
    assert!(is_numeric_report_column("profitpct"));
}

/// Persisted quote ordinals must display as exact tickers rather than opaque integers.
///
/// Removing `columns.rs:basecurrency_text` makes a USDC report row show `8`, hiding the identity
/// that makes its profit and totals meaningfully different from USDT.
#[test]
fn basecurrency_column_decodes_known_quote_ordinals() {
    assert_eq!(basecurrency_text(&Value::Integer(8)), "USDC");
    assert_eq!(basecurrency_text(&Value::Integer(0)), "BTC");
    assert_eq!(
        basecurrency_text(&Value::Integer(26)),
        "26",
        "unknown persisted ordinals must not be guessed"
    );
}

/// `columns.rs:effective_visible_columns` must remove only `core_name` when its AutoCore
/// flag is true. Filtering by saved-set cardinality instead would also reveal the user-hidden
/// `comment` column or hide `core_name` in Overview and Classic.
#[test]
fn auto_core_lens_removes_only_core_name_from_the_saved_visible_set() {
    let cols = ["closedate", "core_name", "coin", "comment"]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let visible = HashSet::from(["core_name".to_string(), "coin".to_string()]);

    assert_eq!(
        effective_visible_columns(&cols, &visible, false)
            .map(|(index, _)| index)
            .collect::<Vec<_>>(),
        vec![1, 2],
        "Classic, Overview, and standalone use the raw saved preference"
    );
    assert_eq!(
        effective_visible_columns(&cols, &visible, true)
            .map(|(index, _)| index)
            .collect::<Vec<_>>(),
        vec![2],
        "AutoCore hides core_name but must not reveal another saved-hidden column"
    );
}

/// `columns.rs:toggled_all_columns` must modify only columns available in the current context.
/// Rebuilding the set from available names would erase dormant `core_name`, so returning to
/// Overview would lose the user's prior preference.
#[test]
fn auto_core_all_toggle_preserves_the_dormant_core_name_preference() {
    let cols = ["closedate", "core_name", "coin"]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let partial = HashSet::from(["core_name".to_string(), "coin".to_string()]);

    let all_on = toggled_all_columns(&cols, &partial, true);
    assert_eq!(
        all_on,
        HashSet::from([
            "closedate".to_string(),
            "core_name".to_string(),
            "coin".to_string(),
        ])
    );
    assert_eq!(
        toggled_all_columns(&cols, &all_on, true),
        HashSet::from(["closedate".to_string(), "core_name".to_string()]),
        "turning available columns off keeps the first available and dormant core_name"
    );

    let core_name_hidden = HashSet::from(["coin".to_string()]);
    assert_eq!(
        toggled_all_columns(&cols, &core_name_hidden, true),
        HashSet::from(["closedate".to_string(), "coin".to_string()]),
        "All must not re-enable a dormant preference that the user saved hidden"
    );
}

/// Breakage: `report_columns` passes `header_label(col)` as the `MoonDataTableColumn` KEY as well
/// as its title, instead of only its title. Consequence: every user's persisted column widths and
/// sort column are stored under the raw name, so a switch to the localized key orphans every saved
/// layout silently on upgrade and the table resets.
#[test]
fn report_columns_key_stays_the_raw_name_while_the_title_is_the_label() {
    let cols = vec!["profitbtc".to_string(), "coin".to_string()];
    let vis = vec![0usize, 1usize];
    let widths: HashMap<String, f32> = HashMap::new();
    let built = report_columns(&cols, &vis, &widths);

    assert_eq!(
        built[0].key.to_string(),
        "profitbtc",
        "the persisted key must stay the raw DB name"
    );
    assert_eq!(
        built[0].title.to_string(),
        header_label("profitbtc"),
        "the visible title is the human label"
    );
    assert_eq!(built[1].key.to_string(), "coin");
}

/// Breakage: `header_label`'s fallback for a runtime column outside `DISPLAY_COLUMNS` implemented
/// as a bare `t!(format!("report.col.{col}"))`. `t!` returns the literal key text on a miss, and the
/// runtime schema genuinely carries columns outside `DISPLAY_COLUMNS`
/// (`moon-core/src/db/report_read.rs`), so an unknown core column's header would render as the raw
/// text `report.col.whatever` instead of falling back to the column's own name.
#[test]
fn header_label_falls_back_to_the_raw_name_for_an_unkeyed_column() {
    let unkeyed = "a_core_column_outside_display_columns";
    let label = header_label(unkeyed);
    assert_eq!(
        label,
        header_for(unkeyed),
        "an unkeyed column must fall back to the same raw text header_for uses"
    );
    assert!(
        !label.starts_with("report.col."),
        "a locale miss must never leak the raw key text: got {label:?}"
    );
}

/// Breakage: compacting the date INSIDE `cell()` instead of in the render path.
/// `widths.rs::natural_widths` measures `cell()`'s own output, so on a page where every visible row
/// is from today the date column would be measured at clock width and then clip the full
/// timestamps that appear the instant an older row scrolls into view -- a width that jitters with
/// the data. `cell()` must therefore always return the LONG form; only the render path may compact.
#[test]
fn cell_keeps_the_long_date_form_even_for_a_row_from_today() {
    let now = moon_core::util::time::now_unix_secs() as i64;
    let axis = ReportAxis::identity_core_local();
    let p = MoonPalette::default();

    let (text, _) = cell(
        "closedate",
        &Value::Integer(now),
        None,
        p,
        &axis,
        0,
        Tz::UTC,
    );

    assert_eq!(
        text,
        moon_core::util::display_time::format_minute(now, axis.zone()),
        "cell() must always render the full timestamp; compaction belongs to the render path"
    );
}

/// Breakage: the shared badge glyph changes (`common::side_glyph`) without `cell()`'s `"isshort"`
/// arm changing to match. Consequence: `widths.rs::natural_widths` measures `cell()`'s text for a
/// column that is actually painted through the dedicated side badge cell, so the column is sized
/// for text it never paints.
#[test]
fn cell_isshort_text_matches_the_shared_side_glyph() {
    let axis = ReportAxis::identity_core_local();
    let p = MoonPalette::default();

    let (short_text, _) = cell("isshort", &Value::Integer(1), None, p, &axis, 0, Tz::UTC);
    let (long_text, _) = cell("isshort", &Value::Integer(0), None, p, &axis, 0, Tz::UTC);

    assert_eq!(short_text, side_glyph(true));
    assert_eq!(long_text, side_glyph(false));
}
