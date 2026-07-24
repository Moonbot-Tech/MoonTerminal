//! Regression coverage for shared core-selector grouping and summaries.

use std::collections::{HashMap, HashSet};

use super::{
    core_menu_sections, fit_core_menu_label, normalized_core_filter_ids, selection_summary,
    toggle_all_core_selection,
};
use crate::design::core_menu_width_for_scales;

/// `core_combo.rs:core_menu_sections` must keep unidentified cores first, sort exchange sections,
/// and preserve the incoming canonical member order. Replacing the shared section helper with one
/// direct pass over `cores` removes the exchange hierarchy from every core dropdown.
#[test]
fn menu_sections_are_unknown_first_alphabetical_and_member_stable() {
    let cores = vec![
        (1, "Bybit first".to_string()),
        (2, "Unknown".to_string()),
        (3, "Binance first".to_string()),
        (4, "Bybit second".to_string()),
        (5, "Binance second".to_string()),
    ];
    let exchange_names = HashMap::from([
        (1, "Bybit".to_string()),
        (3, "Binance Futures".to_string()),
        (4, "Bybit".to_string()),
        (5, "Binance Futures".to_string()),
    ]);

    let sections = core_menu_sections(&cores, &exchange_names);
    let actual: Vec<(Option<&str>, Vec<u64>)> = sections
        .into_iter()
        .map(|(exchange, members)| {
            (
                exchange,
                members.into_iter().map(|(core, _)| core).collect(),
            )
        })
        .collect();

    assert_eq!(
        actual,
        vec![
            (None, vec![2]),
            (Some("Binance Futures"), vec![3, 5]),
            (Some("Bybit"), vec![1, 4]),
        ]
    );
}

/// `core_combo.rs:selection_summary` must format a one-core partial selection as a count. Restoring
/// the old sole-name branch exposes arbitrary server text instead of the compact summary promised
/// by the shared selector.
#[test]
fn one_selected_core_uses_the_compact_count_summary() {
    let cores = vec![
        (1, "A deliberately very long server name".to_string()),
        (2, "Second".to_string()),
        (3, "Third".to_string()),
    ];
    let selected = HashSet::from([1]);

    let (summary, all_on) =
        selection_summary(&cores, &selected, "All cores", &|n| format!("Cores: {n}"));

    assert_eq!(summary, "Cores: 1");
    assert!(!all_on);
}

/// `core_combo.rs:selection_summary` must preserve empty/full All semantics and reject a stale
/// equal-cardinality selection. Replacing membership counting with `selected.len() == cores.len()`
/// makes missing current cores appear selected and labels an empty result as All.
#[test]
fn all_summary_requires_every_available_core() {
    let cores = vec![(1, "First".to_string()), (2, "Second".to_string())];

    let empty = selection_summary(&cores, &HashSet::new(), "All cores", &|n| {
        format!("Cores: {n}")
    });
    let full = selection_summary(&cores, &HashSet::from([1, 2]), "All cores", &|n| {
        format!("Cores: {n}")
    });
    let full_with_stale =
        selection_summary(&cores, &HashSet::from([1, 2, 99]), "All cores", &|n| {
            format!("Cores: {n}")
        });
    let stale_equal_cardinality =
        selection_summary(&cores, &HashSet::from([98, 99]), "All cores", &|n| {
            format!("Cores: {n}")
        });

    assert_eq!(empty, ("All cores".to_string(), true));
    assert_eq!(full, ("All cores".to_string(), true));
    assert_eq!(stale_equal_cardinality, ("Cores: 0".to_string(), false));
    assert_eq!(full_with_stale, ("All cores".to_string(), true));
}

/// `core_combo.rs:toggle_all_core_selection` must compare available ids, not set cardinality.
/// Replacing the membership check with `selected.len() == available.len()` clears a stale
/// equal-sized selection and leaves the newly available cores absent from filtered results.
#[test]
fn all_toggle_replaces_stale_equal_cardinality_selection() {
    let available = HashSet::from([1, 2]);
    let mut selected = HashSet::from([98, 99]);

    toggle_all_core_selection(&mut selected, available.clone());
    assert_eq!(selected, available);

    selected.insert(99);
    toggle_all_core_selection(&mut selected, available);
    assert!(selected.is_empty());
}

/// `core_combo.rs:normalized_core_filter_ids` must compare available membership before returning
/// the empty no-filter form. Replacing it with a cardinality check makes a stale equal-sized
/// Analytics selection query every core while the trigger reports a partial selection.
#[test]
fn query_filter_keeps_stale_equal_cardinality_selection_explicit() {
    let stale = HashSet::from([98, 99]);
    let explicit = normalized_core_filter_ids([1, 2], &stale);
    assert_eq!(explicit.into_iter().collect::<HashSet<_>>(), stale);

    let full_with_stale = HashSet::from([1, 2, 99]);
    assert!(normalized_core_filter_ids([1, 2], &full_with_stale).is_empty());
}

/// `core_combo.rs:fit_core_menu_label` must preserve an exact boundary and ellipsize a one-character
/// overflow. Replacing the bounded fit with the raw label lets a longer configured name escape the
/// stable menu width and obscure neighbouring UI.
#[test]
fn menu_label_preserves_the_boundary_and_ellipsizes_one_past_it() {
    let sample = "AWS$22 ~ F-BN / SHOT_FUT (SUB_11) USDC";
    let measure = |text: &str| text.chars().count() as f32;
    let limit = measure(sample);

    assert_eq!(fit_core_menu_label(sample, limit, measure), sample);
    assert_eq!(
        fit_core_menu_label(&format!("{sample}X"), limit, measure),
        "AWS$22 ~ F-BN / SHOT_FUT (SUB_11) USD…"
    );
}

/// `design.rs:core_menu_width_for_scales` must reserve enough compact-menu text width for the
/// screenshot label at every supported Font-slider value, independently of UI scale. Reducing
/// `CORE_MENU_LABEL_W` to 180 truncates the named assertion and makes ordinary configured servers
/// unreadable in every core selector.
#[test]
fn core_menu_geometry_keeps_the_screenshot_label_at_extreme_scales() {
    const SAMPLE: &str = "AWS$22 ~ F-BN / SHOT_FUT (SUB_11) USDC";
    // A conservative 6px advance represents one 9.5px Geist Mono glyph. The 30px scaled chrome
    // and 14px fixed chrome are independently derived from MoonPopupMenu's compact checked row.
    for compact_text_scale in [7.5 / 9.5, 1.0, 11.5 / 9.5, 15.5 / 9.5] {
        for ui_scale in [0.25, 1.0, 3.0] {
            let menu_w = core_menu_width_for_scales(compact_text_scale, ui_scale, 0.0);
            let label_w = menu_w - 30.0 * ui_scale - 14.0;
            let measure = |text: &str| text.chars().count() as f32 * 6.0 * compact_text_scale;

            assert_eq!(
                fit_core_menu_label(SAMPLE, label_w, measure),
                SAMPLE,
                "label clipped at compact_text_scale={compact_text_scale}, ui_scale={ui_scale}"
            );
        }
    }
}
