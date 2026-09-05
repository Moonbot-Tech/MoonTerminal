//! Static invariants for the Report table restyle.
//!
//! Report invariants were scattered through `analytics.rs` before this subject module existed
//! (`analytics.rs` still owns the ones spanning both panels). Everything below is specific to
//! Report's own restyle: the export/clipboard header contract stays on raw DB names, `cell_weight`
//! never grows a third resolved font, the side badge stays one shared helper, and every runtime
//! column gets a localized header in all three shipped locales.

use super::support::*;

/// Breakage: someone "helpfully" points `export.rs` (CSV, XLSX) or `selection.rs` (clipboard TSV)
/// at the pretty `columns::header_label` instead of the raw `columns::header_for`. Every exported
/// file and every clipboard paste is a machine-readable extract consumed outside this repo; a
/// silent switch changes the header row to localized text and breaks every downstream parser.
#[test]
fn export_and_clipboard_headers_stay_on_the_raw_column_name() {
    for (rel, what) in [
        ("panels/report/export.rs", "CSV/XLSX export"),
        ("panels/report/selection.rs", "clipboard TSV"),
    ] {
        let src = read_src(rel);
        assert!(
            src.contains("header_for("),
            "{what} must build its header row through columns::header_for"
        );
        assert!(
            !src.contains("header_label("),
            "{what} must never use the localized columns::header_label for its header row"
        );
    }
}

/// Breakage: `cell_weight` grows a third resolved weight (`MEDIUM` or `BOLD`) to express more
/// hierarchy. `design::MonoBodyFontSignature` encodes only the normal and semibold `FontId`s, and
/// that signature keys the natural-width cache -- a third weight's resolved font would sit outside
/// the key, so a theme change altering only that weight's resolution would leave stale widths
/// cached with nothing to invalidate them.
#[test]
fn cell_weight_never_grows_a_third_font_weight() {
    let columns = read_src("panels/report/columns.rs");
    let body = code_only(braced_body(&columns, "pub(super) fn cell_weight("));

    assert!(
        body.contains("FontWeight::SEMIBOLD") && body.contains("FontWeight::NORMAL"),
        "cell_weight must still resolve exactly the two cached weights"
    );
    assert!(
        !body.contains("FontWeight::MEDIUM") && !body.contains("FontWeight::BOLD"),
        "a third resolved weight would sit outside MonoBodyFontSignature's cache key: {body}"
    );
}

/// Breakage: the Report side cell and the Analytics side badge drift apart -- either site keeps
/// `.tone(MoonTone::Negative)`, which resolves to `p.orange` rather than `p.red` on the dark theme
/// (`moon-ui-components/src/moon/tokens.rs`, `MoonTone::color`), or either re-inlines a second
/// `MoonBadge::new` instead of sharing `common::side_badge`. That drift is the exact thing one
/// shared helper exists to prevent.
#[test]
fn the_side_badge_is_the_one_shared_helper_in_both_panels() {
    for (rel, what) in [
        ("panels/report/columns.rs", "Report"),
        ("analytics/summary/mod.rs", "Analytics"),
    ] {
        let src = read_src(rel);
        assert!(
            src.contains("common::side_badge("),
            "{what} must render the side badge through the shared common::side_badge helper"
        );
        assert!(
            !src.contains("MoonBadge::new("),
            "{what} must not re-inline its own MoonBadge, or the two badges can drift apart"
        );
    }
}

/// Repaired at the PROVE task: the original draft checked every `moon_core::db::DISPLAY_COLUMNS`
/// entry, which is WRONG -- `header_label`'s own docstring says the runtime schema genuinely
/// carries columns outside the keyed set (six deliberately-untranslated technical names, `lev`,
/// `fname`, every `*delta`/`*ratio` metric). The real invariant is that `is_keyed_report_header`'s
/// `matches!` list and `locales/report.yml` are "kept in sync by hand -- one decision in two
/// places" (`columns.rs`'s own docstring): every column NAMED THERE needs its `report.col.<name>`
/// locale entry, in all three shipped locales, or `header_label`'s miss path leaks the raw locale
/// key text for a column someone added to one list and forgot in the other.
#[test]
fn every_keyed_report_header_has_a_localized_entry_in_all_three_locales() {
    let columns_src = read_src("panels/report/columns.rs");
    let body = code_only(braced_body(&columns_src, "fn is_keyed_report_header("));
    let names: Vec<&str> = body.split('"').skip(1).step_by(2).collect();
    assert!(
        names.len() > 20,
        "expected a substantial keyed-column list; got {names:?} -- did the function shape change?"
    );

    let locales = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../locales/report.yml"),
    )
    .expect("read Report locales")
    .replace("\r\n", "\n");

    for column in names {
        let key = format!("report.col.{column}:\n");
        let block = locale_block(&locales, &key);
        for locale in ["ru", "en", "es"] {
            assert!(
                block
                    .lines()
                    .any(|line| line.starts_with(&format!("  {locale}: "))),
                "report.col.{column} must define {locale}"
            );
        }
    }
}

/// Breakage: `report_data_cell`'s date branch swaps the two formatting calls (the compact form
/// reaching the tooltip and the full form reaching the display), or passes `ctx.display_zone`
/// where the axis-resolved zone belongs. The first makes every date cell show the wrong text; the
/// silent second makes a REPLICATED column (`buydate`/`closedate`/`sellsetdate`) get the display
/// zone applied on top of the axis projection, shifting every timestamp by the core's offset with
/// nothing looking broken. `date_cell_instant` resolves `(secs, zone)` exactly ONCE for exactly
/// this reason -- the two format calls in this branch must both consume ITS `zone`, never
/// `ctx.display_zone` directly.
#[test]
fn the_date_cell_builds_display_and_tooltip_from_one_resolved_zone_in_the_right_order() {
    let columns_src = read_src("panels/report/columns.rs");
    let body = code_only(braced_body(&columns_src, "fn report_data_cell("));
    let forms = chain_between(
        &body,
        "Some((secs, zone)) => (",
        "None => (String::new()",
        "date_cell_instant match arm",
    );

    let compact_at = forms
        .find("format_minute_or_clock(")
        .expect("the compact form must come from format_minute_or_clock");
    let full_at = forms
        .find("format_minute(")
        .expect("the tooltip's full form must come from format_minute");
    assert!(
        compact_at < full_at,
        "format_minute_or_clock must build the DISPLAYED compact form first, format_minute the \
         tooltip's full form second -- swapping them shows the wrong text in each place: {forms}"
    );
    assert!(
        !forms.contains("ctx.display_zone"),
        "both format calls must use date_cell_instant's own resolved zone, never ctx.display_zone \
         directly -- that is what keeps a replicated column off the display zone: {forms}"
    );
}

/// Slice the indented lines directly under a `key:\n` locale anchor -- the same shape
/// `chain_between` isolates in `analytics.rs`, but bounded by indentation instead of a known next
/// key, since the column list this test walks is data-driven rather than a fixed sequence.
fn locale_block<'a>(locales: &'a str, key: &str) -> &'a str {
    let after = locales
        .split_once(key)
        .unwrap_or_else(|| panic!("missing locale block for {key}"))
        .1;
    let end: usize = after
        .lines()
        .take_while(|line| line.starts_with("  "))
        .map(|line| line.len() + 1)
        .sum();
    &after[..end.min(after.len())]
}
