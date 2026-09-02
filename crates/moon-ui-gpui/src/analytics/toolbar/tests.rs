//! Regression coverage for Analytics toolbar notices and core-selection controls.
//!
//! The notice is the only thing that says money is missing from every figure on the window,
//! so the two ways it can go wrong — starting open, or being collapsible when the read
//! FAILED — are both worth pinning.

use std::collections::HashSet;

use moon_core::config::{CoreGroup, UI_FONT_DELTA_MAX, UI_FONT_DELTA_MIN};
use moon_core::db::analytics::UndatedCloses;
use moon_core::db::{QuoteBreakdown, QuoteCurrency, QuoteTotal, ReadFail};

use super::super::AnalyticsSessionState;
use super::{
    CoreSelectionCaption, UndatedBanner, analytics_core_filter_ids, presets_row_fits,
    sole_core_name, undated_banner_state,
};

/// `analytics/toolbar.rs:presets_row_fits` must keep the inline presets at the exact available
/// width; changing `>=` to `>` collapses the Analytics period bar one pixel early for a window
/// that still fits every preset.
#[test]
fn presets_row_fits_only_when_available_width_reaches_the_row_width() {
    assert!(
        presets_row_fits(401.0, 400.0),
        "room beyond the measured row keeps the one-click presets inline"
    );
    assert!(
        !presets_row_fits(399.0, 400.0),
        "a row wider than its available room must collapse to the dropdown"
    );
    assert!(
        presets_row_fits(400.0, 400.0),
        "an exact fit is still a fit and must not collapse one pixel early"
    );
}

/// `locales/analytics.yml:analytics.period.last_month` must remain below the font-scaled toolbar
/// ceiling; widening it would elide the Russian Analytics period label with an ellipsis.
#[gpui::test]
fn every_preset_label_fits_its_fitted_cell_without_truncation(cx: &mut gpui::TestAppContext) {
    rust_i18n::set_locale("ru");
    // Every preset in the bar must fit at the default scale -- the task's baseline done-condition.
    for per in super::Period::ALL {
        let title = per.title(chrono_tz::Tz::UTC);
        let (natural, ceiling) = cx.update(|cx| {
            let natural = moon_ui::MoonSegmentItem::new("", title.clone())
                .fit_width(cx, 0.0, f32::MAX)
                .resolved_width();
            let ceiling =
                moon_ui::MoonTheme::active_tokens(cx).font_width(super::PRESET_CELL_MAX_W);
            (natural, ceiling)
        });
        assert!(
            natural < ceiling,
            "{title:?} natural width {natural} does not fit under the {ceiling}px fitted-cell \
             ceiling at the default scale and would render truncated with an ellipsis"
        );
    }

    // Every preset except the pre-existing, out-of-scope Week overflow must survive the supported
    // font-delta extremes, which is the failure class this regression is meant to catch.
    for delta in [UI_FONT_DELTA_MIN as f32, UI_FONT_DELTA_MAX as f32] {
        let ceiling = cx.update(|cx| {
            moon_ui::MoonTheme::global_mut(cx).scale.font_delta = delta;
            moon_ui::MoonTheme::active_tokens(cx).font_width(super::PRESET_CELL_MAX_W)
        });
        for per in super::Period::ALL {
            if matches!(per, super::Period::Week) {
                continue;
            }
            let title = per.title(chrono_tz::Tz::UTC);
            let natural = cx.update(|cx| {
                moon_ui::MoonSegmentItem::new("", title.clone())
                    .fit_width(cx, 0.0, f32::MAX)
                    .resolved_width()
            });
            assert!(
                natural < ceiling,
                "{title:?} natural width {natural} does not fit under the {ceiling}px fitted-cell \
                 ceiling at font delta {delta} and would render truncated with an ellipsis"
            );
        }
    }
    rust_i18n::set_locale("en");
}

/// Some undated trades, with money attached.
///
/// Args:
///     n: Trade and bucket order count.
///
/// Returns:
///     USDT undated totals wrapped as a loaded optional value.
fn found(n: i64) -> Option<UndatedCloses> {
    Some(UndatedCloses {
        totals: QuoteBreakdown {
            totals: vec![QuoteTotal {
                currency: QuoteCurrency::from_report_ordinal(1).expect("USDT report ordinal"),
                profit: -12.5,
                orders: n,
            }],
            unknown_orders: 0,
            orders: n,
            valuation: None,
            traded_volume: Default::default(),
        },
    })
}

/// Nothing known yet, and a clean zero, both mean silence — not an empty band.
#[test]
fn nothing_to_say_renders_no_strip() {
    assert_eq!(
        undated_banner_state(None, None, false),
        UndatedBanner::None,
        "an unknown count says nothing"
    );
    assert_eq!(
        undated_banner_state(None, Some(UndatedCloses::default()), true),
        UndatedBanner::None,
        "zero undated trades says nothing even when expanded"
    );
}

/// The notice starts COLLAPSED and opens only when asked.
///
/// Plausible edit this catches: `AnalyticsSessionState::undated_expanded` is defaulted to
/// `true` (or the `!expanded` test is inverted while "simplifying" the branch), and every
/// user gets the full warning band on the tab they use most.
#[test]
fn the_notice_starts_collapsed_and_opens_only_on_request() {
    // The default is half the claim, so assert it here rather than leaving it to a source
    // grep: `undated_banner_state` only ever sees the value it is handed.
    assert!(
        !AnalyticsSessionState::default().undated_expanded,
        "a fresh process must start with the notice collapsed"
    );
    assert!(
        matches!(
            undated_banner_state(None, found(4), false),
            UndatedBanner::Collapsed(_)
        ),
        "collapsed unless the user opened it"
    );
    assert!(
        matches!(
            undated_banner_state(None, found(4), true),
            UndatedBanner::Full(..)
        ),
        "opened on request"
    );
}

/// A failed read outranks collapsing, in BOTH states.
///
/// Plausible edit this catches: the collapse check is moved above the failure check (it reads
/// like the cheaper guard), and a replica that could not be queried renders as a tidy
/// one-line count — claiming a number for rows nobody managed to read.
#[test]
fn a_read_failure_is_never_collapsed() {
    for expanded in [false, true] {
        assert!(
            matches!(
                undated_banner_state(Some(&ReadFail::NotReady), found(4), expanded),
                UndatedBanner::Failed(..)
            ),
            "a read failure must survive expanded={expanded}"
        );
    }
}

/// The tab bar names exactly one explicitly selected live core.
///
/// The rule is "name it exactly when the Analytics core trigger shows the count 1", i.e. exactly
/// when the name is otherwise unreachable without opening the dropdown.
///
/// Breakage this pins: treating a complete explicit selection as All in this Analytics-only path.
/// A single-core install would then hide the clicked core's name even though the exclusive menu
/// shows that core checked and All unchecked.
#[test]
fn the_sole_core_name_shows_for_one_explicit_live_selection() {
    let two = vec![(1u64, "alpha".to_string()), (2u64, "beta".to_string())];
    let one = vec![(1u64, "alpha".to_string())];

    for (cores, selected, expected, why) in [
        (
            &two,
            vec![2u64],
            Some("beta"),
            "one of two is a real filter",
        ),
        (&two, vec![], None, "the implicit All names nothing"),
        (
            &two,
            vec![1, 2],
            None,
            "two explicit cores do not have one name",
        ),
        (&one, vec![1], Some("alpha"), "one explicit core is not All"),
        (&one, vec![], None, "one core, implicit All"),
        // A deleted core keeps its id in the selection so the query cannot silently broaden. The
        // trigger counts only the ids that still resolve, so this reads as "1" there and must
        // name the one live core here.
        (
            &two,
            vec![1, 99],
            Some("alpha"),
            "one live core plus a stale id is still a sole selection",
        ),
    ] {
        let set: HashSet<u64> = selected.into_iter().collect();
        assert_eq!(sole_core_name(cores, &set), expected, "{why}");
    }
}

/// A selected id with no core behind it names nothing at all.
///
/// A core deleted from config keeps its id in the saved selection (deliberately — a stale id must
/// not silently broaden the query), so this is reachable in normal use.
///
/// Breakage this pins: resolving with a positional fallback such as `cores.first()`. The bar would
/// then name an unrelated core, and the number beside it would belong to a third one.
#[test]
fn a_stale_selected_id_names_no_core() {
    let cores = vec![(1u64, "alpha".to_string()), (2u64, "beta".to_string())];
    let set: HashSet<u64> = [99u64].into_iter().collect();

    assert_eq!(
        sole_core_name(&cores, &set),
        None,
        "an id no core answers to must not borrow another core's name"
    );
}

/// Explicit saved-group provenance must name the group without being inferred from membership.
///
/// Named breakage (`toolbar.rs:CoreSelectionCaption::manual_selection_changed`): preserving the
/// previous group instead of clearing it makes a manually rebuilt multi-select resurrect the old
/// group caption, falsely claiming the user applied that group again.
#[test]
fn core_caption_tracks_explicit_group_application_without_surviving_manual_edits() {
    let cores = vec![
        (1u64, "alpha".to_string()),
        (2u64, "beta".to_string()),
        (3u64, "gamma".to_string()),
    ];
    let groups = vec![
        CoreGroup {
            name: "Shots".to_string(),
            cores: vec![1, 2, 3],
        },
        CoreGroup {
            name: "Solo".to_string(),
            cores: vec![1],
        },
    ];
    let mut caption = CoreSelectionCaption::default();

    let mut selected = HashSet::from([1]);
    assert_eq!(
        caption.visible_name(&groups, &cores, &selected),
        Some("alpha"),
        "a manual sole-core selection keeps the existing core caption"
    );

    assert!(caption.set_applied_group(Some("Solo".to_string())));
    assert_eq!(
        caption.visible_name(&groups, &cores, &selected),
        Some("Solo"),
        "an explicitly applied one-member group outranks the incidental core name"
    );

    selected = HashSet::from([1, 2, 3]);
    assert!(caption.set_applied_group(Some("Shots".to_string())));
    assert_eq!(
        caption.visible_name(&groups, &cores, &selected),
        Some("Shots"),
        "an explicitly applied exact group shows its user-supplied name"
    );

    selected.remove(&3);
    assert!(caption.manual_selection_changed());
    assert_eq!(
        caption.visible_name(&groups, &cores, &selected),
        None,
        "an ordinary manual multi-select keeps only the numeric trigger"
    );

    selected.insert(3);
    assert_eq!(
        caption.visible_name(&groups, &cores, &selected),
        None,
        "manually rebuilding the membership must not resurrect group provenance"
    );

    assert!(caption.set_applied_group(Some("Shots".to_string())));
    assert_eq!(
        caption.visible_name(&groups, &cores, &selected),
        Some("Shots"),
        "applying the group again restores its caption"
    );
}

/// `toolbar.rs:analytics_core_filter_ids` must keep a complete explicit selection as a bounded
/// query filter, never broadening it to the unfiltered form reserved for the exclusive All state.
///
/// Plausible edit this catches: changing `analytics_core_filter_ids` to return `Vec::new()` for a
/// complete explicit set. A newly reported core would then enter Analytics results despite never
/// being checked.
///
/// The All-row toggle itself (`None => selected.clear()`) moved to the shared
/// `core_quick.rs:toggle_core_selection`, covered in `controls::core_quick::tests`.
#[test]
fn a_complete_explicit_selection_stays_a_bounded_query_filter() {
    let selected = HashSet::from([1, 2]);
    assert_eq!(
        analytics_core_filter_ids(&selected, None, None, &[])
            .into_iter()
            .collect::<HashSet<_>>(),
        selected,
        "a complete explicit selection must remain a bounded query filter"
    );
    assert!(
        analytics_core_filter_ids(&HashSet::new(), None, None, &[]).is_empty(),
        "only the exclusive All state may produce an unfiltered query"
    );
}

/// Assigning Auto ids into `sel_cores`, or bypassing the workspace argument in
/// `analytics/mod.rs:cores_selected`, would either destroy the retained Classic filter or query
/// its hidden cores while Auto is pinned.
#[test]
fn workspace_query_wiring_preserves_retained_classic_selection() {
    let retained = HashSet::from([3, 5]);

    assert_eq!(
        analytics_core_filter_ids(&retained, Some(&[11]), None, &[]),
        vec![11]
    );
    assert_eq!(retained, HashSet::from([3, 5]));
    assert_eq!(
        analytics_core_filter_ids(&retained, None, None, &[])
            .into_iter()
            .collect::<HashSet<_>>(),
        retained
    );

    let analytics = include_str!("../mod.rs");
    let cores_selected = analytics
        .split("fn cores_selected(&self)")
        .nth(1)
        .and_then(|tail| tail.split("\n    }").next())
        .expect("Analytics query core selector must exist");
    assert!(cores_selected.contains("self.read_core_ids()"));

    let observer = analytics
        .split("cx.observe(&workspace_revision")
        .nth(1)
        .and_then(|tail| tail.split(".detach();").next())
        .expect("workspace observer must exist");
    assert!(!observer.contains("sel_cores ="));
}

/// `toolbar.rs:analytics_core_filter_ids` returning an empty vector for an empty Auto owner would
/// broaden a temporarily coreless workspace to every core in the reports database.
#[test]
fn empty_workspace_scope_is_an_explicit_no_match_query() {
    assert_eq!(
        analytics_core_filter_ids(&HashSet::new(), Some(&[]), None, &[]),
        vec![0]
    );
}
