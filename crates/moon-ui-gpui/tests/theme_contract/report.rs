//! Static invariants for the Report table restyle.
//!
//! Report invariants were scattered through `analytics.rs` before this subject module existed
//! (`analytics.rs` still owns the ones spanning both panels). Everything below is specific to
//! Report's own restyle: the export/clipboard header contract stays on raw DB names, `cell_weight`
//! never grows a third resolved font, the side badge stays one shared helper, and every runtime
//! column gets a localized header in all three shipped locales.

use super::support::*;

/// Bypassing the shared composer in either surface brings back raw lease errors or divergent
/// recording warnings. This pins the UI wiring while the composer's unit tests check its text.
#[test]
fn report_and_analytics_share_the_recovery_notice_composer() {
    let load = read_src("load_state.rs");
    let note = code_only(braced_body(&load, "pub(crate) fn note_el("));
    let denial = braced_body(&note, "FailKind::ReplicaAccessDenied =>");
    assert!(denial.contains("crate::report_notice::recovery_notice_text("));
    assert!(denial.contains("MoonAlert::error(id, detail).title(title)"));
    assert!(!denial.contains("msg"));
    let toolbar = read_src("analytics/toolbar.rs");
    let integrity = code_only(braced_body(&toolbar, "pub(super) fn integrity_note("));
    assert!(integrity.contains("crate::report_notice::recovery_notice_text(Some(notice))"));
    let report = read_src("panels/report/render.rs");
    assert!(report.contains("Err(note) => note_el(\"rep-table-note\", note, 12.0, p, cx)"));
}

/// A recovered replica permits reads, so routing its notice only through access denial hides
/// the saved-copy location forever. Keep the notice alongside content and behind failure priority.
#[test]
fn report_successful_recovery_notice_coexists_with_content() {
    let report = read_src("panels/report/render.rs");
    let recovered = code_only(braced_body(&report, "let recovered_notice = match"));
    assert!(recovered.contains("RecoveryNotice::Recovered"));
    assert!(recovered.contains("!matches!(self.data, LoadState::Failed(_))"));
    assert!(recovered.contains("crate::report_notice::recovery_notice_text(Some(notice))"));
    assert!(recovered.contains("MoonAlert::info(\"rep-recovered-note\", detail).title(title)"));
    assert!(report.contains(".children(recovered_notice)\n            .child(table_el)"));
}

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
