//! Collapse decisions and manual-strategy target regressions for the trading toolbar.

use moon_core::config::WorkspaceMode;

use super::{
    SynthToolbar, ToolbarLevel, ToolbarModel, ToolbarWidths, manual_strategy_core,
    shows_no_coin_hint, strat_budget, strat_pod_gate, trailing_pinned,
};

#[test]
/// Regression target: preferring `active_core` over `hovered_core` makes TP/SL/S applicability
/// describe core A while a click or market hotkey submits through the hovered chart on core B.
fn hovered_chart_core_controls_manual_strategy_applicability() {
    assert_eq!(manual_strategy_core(Some(1), Some(2), true), Some(2));
    assert_eq!(manual_strategy_core(Some(1), Some(2), false), Some(1));
    assert_eq!(manual_strategy_core(None, Some(2), true), Some(2));
}

/// A toolbar whose every element group is non-zero, so each level visibly removes something.
fn widths() -> ToolbarWidths {
    ToolbarWidths {
        fixed: 400.0,
        strat_controls: 0.0,
        sell_strip: 200.0,
        sell_drop: 60.0,
        max_readout: 90.0,
        ms_summary: 0.0,
        size_strip: 200.0,
        size_drop: 60.0,
        captions: 180.0,
        chart_tools: 60.0,
        overflow: 26.0,
        own_icon: 30.0,
        tp_pod: 80.0,
        sell_pod: 20.0,
        size_pod: 20.0,
        sl_pod: 70.0,
        live: 60.0,
    }
}

/// Regression target: `fit.rs:shows_no_coin_hint` matching any unset reason would replace the
/// Leverage pod for a missing core or an unreported value too, hiding the tooltip that explains
/// those; matching none would leave two dashes where one sentence says "pick a coin".
#[test]
fn only_the_no_market_reason_selects_the_no_coin_hint() {
    assert!(shows_no_coin_hint(Some("toolbar.lev_unset_no_market")));
    assert!(!shows_no_coin_hint(Some("toolbar.lev_unset_no_core")));
    assert!(!shows_no_coin_hint(Some("toolbar.lev_unset_unreported")));
    assert!(!shows_no_coin_hint(None));
}

/// Regression target: the Strategy pod is MANUAL-only. Drawing it in AUTO — even with a core
/// selected — would offer manual-strategy controls in a mode that never places manual orders, and
/// drawing it in Overview would address the group's first core as if the scope named it.
#[test]
fn auto_with_a_selected_core_shows_no_strategy_pod() {
    assert!(!strat_pod_gate(WorkspaceMode::AutoTrading, false));
    assert!(!strat_pod_gate(WorkspaceMode::AutoTrading, true));
    assert!(!strat_pod_gate(WorkspaceMode::Classic, true));
    assert!(strat_pod_gate(WorkspaceMode::Classic, false));
}

/// Regression target: the summary read-outs are the level-3 rung. Hiding them a level early costs
/// the trader the strategy's numbers at a width that holds them; a level late clips the row.
#[test]
fn strategy_summary_is_absent_at_level_3_and_present_at_level_2() {
    assert!(ToolbarLevel(2).ms_summary());
    assert!(!ToolbarLevel(3).ms_summary());
    assert!(
        !ToolbarLevel(3).size_as_dropdown(),
        "level 3 still draws the size strip"
    );
}

/// Regression target: long strategy names or many slots must hide the summary (L3) before the
/// size strip collapses (L4), and must not shed the pod's own names or slots while any outer
/// level still has something to give.
#[test]
fn a_wide_strategy_pod_hides_the_summary_before_the_size_strip_collapses() {
    let widths = ToolbarWidths {
        strat_controls: 600.0,
        ms_summary: 150.0,
        ..widths()
    };
    // Just enough for level 3: the summary is gone, the size strip is still whole.
    let at_l3 = widths.width_at(ToolbarLevel(3));
    let level = widths.level(at_l3);
    assert_eq!(level, ToolbarLevel(3));
    assert!(!level.ms_summary());
    assert!(!level.size_as_dropdown());
    assert_eq!(strat_budget(&widths, level, at_l3), f32::INFINITY);
    // One pixel less and the size strip collapses next — the summary went first.
    assert_eq!(widths.level(at_l3 - 1.0), ToolbarLevel(4));
}

/// Regression target: the pod's inner ladder sheds only at the narrowest level, by exactly the
/// shortfall — shedding earlier hides strategy names a wider level could hold.
#[test]
fn the_strategy_pod_sheds_only_at_the_narrowest_level_by_the_shortfall() {
    let widths = ToolbarWidths {
        strat_controls: 300.0,
        ..widths()
    };
    let max = ToolbarLevel(ToolbarLevel::MAX);
    let floor = widths.width_at(max);
    assert_eq!(widths.level(floor - 50.0), max);
    assert_eq!(strat_budget(&widths, max, floor), f32::INFINITY);
    assert_eq!(strat_budget(&widths, max, floor - 50.0), 250.0);
    assert_eq!(strat_budget(&widths, max, floor - 1_000.0), 0.0);
}

/// Regression target: at the group window's 520 px minimum every control must stay reachable —
/// drawn, collapsed, or in "More" — and the trailing cluster must stay inside the row, pinning
/// over the pods only once even the narrowest level is short.
#[test]
fn every_control_stays_reachable_at_the_minimum_window() {
    let min_w = 520.0;
    let widths = ToolbarWidths {
        fixed: 520.0,
        ..widths()
    };
    let level = widths.level(min_w);
    assert_eq!(
        level,
        ToolbarLevel(ToolbarLevel::MAX),
        "a row this wide reaches the narrowest level at 520 px"
    );
    // The chart tools and the own-set icon sit in "More", and from L8 on Take, Sell, Size and Stop
    // are its rows; nothing leaves the row unreachable.
    assert!(level.chart_tools_folded());
    assert!(level.own_icon_folded());
    assert!(level.take_sell_folded() && level.size_folded() && level.sl_folded());
    // Still short: "More" and Live pin themselves over the row's right edge instead of sitting
    // past the window's edge at the end of the flow.
    assert!(trailing_pinned(&widths, level, min_w));
    // One level of room more and nothing pins.
    assert!(!trailing_pinned(&widths, level, widths.width_at(level)));
    // A row that fits keeps the cluster in the flow.
    let roomy = widths.width_at(ToolbarLevel(0));
    assert_eq!(widths.level(roomy), ToolbarLevel(0));
    assert!(!trailing_pinned(&widths, ToolbarLevel(0), roomy));
}

// Layout: the drawn toolbar against its own measurement.

/// Regression target: the no-coin hint is counted whole while the MAX readout it stands in for
/// would be drawn, and capped from L2 on — capping it at every level ellipsized it at 1900 px.
#[gpui::test]
fn the_no_coin_hint_is_whole_below_l2_and_capped_from_l2(cx: &mut gpui::TestAppContext) {
    install_theme(cx);
    cx.update(|cx| {
        let text = "Pick a coin on the chart to see leverage and the order limit, all of it";
        let full = super::no_coin_hint_full_width(cx, text);
        assert_eq!(super::no_coin_hint_width(cx, text, ToolbarLevel(0)), full);
        assert_eq!(super::no_coin_hint_width(cx, text, ToolbarLevel(1)), full);
        assert!(super::no_coin_hint_width(cx, text, ToolbarLevel(2)) < full);
    });
}

/// Every locale the terminal ships.
const LOCALES: [&str; 7] = ["en", "es", "pt", "ru", "tr", "uk", "vi"];
/// Window widths from a wide monitor down to the minimum window.
const WIDTHS: [f32; 6] = [1900.0, 1440.0, 1180.0, 960.0, 760.0, 520.0];
/// The toolbar states drawn.
const STATES: [SynthToolbar; 7] = [
    SynthToolbar::Normal,
    SynthToolbar::Manual,
    SynthToolbar::NoCoin,
    SynthToolbar::LockedOtherCore,
    SynthToolbar::NoCores,
    SynthToolbar::SlOff,
    SynthToolbar::Paused,
];
/// Wide windows drawn with a caret menu open, holding the level at its collapsed form's first
/// level (`StripCaret::min_level`): a held popup's row must still fit and not overlap.
const HELD: [(f32, ToolbarLevel); 2] = [(1900.0, ToolbarLevel(1)), (1900.0, ToolbarLevel(4))];
/// The row's top-level groups, drawn side by side; the trailing cluster closes the row.
const ROW: [&str; 8] = [
    "toolbar-pod-strategy",
    "toolbar-pod-size",
    "toolbar-no-coin",
    "toolbar-pod-lev",
    "toolbar-pod-sl",
    "toolbar-pod-tp",
    "toolbar-pod-sell",
    "toolbar-trailing",
];

fn install_theme(cx: &mut gpui::TestAppContext) {
    // The seven-locale dictionary overflows a default test thread unless warmed on its own.
    crate::test_locale::warm();
    cx.update(|cx| {
        moon_ui::MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(
                moon_core::config::UiThemeMode::Dark,
                1.0,
            ),
            cx,
        );
    });
}

/// The toolbar drawn inert at `width`, the way `Shell` draws it; `floor` is the level an open
/// caret menu holds (`Shell::render`), `ToolbarLevel(0)` for none.
struct ToolbarProbe {
    state: SynthToolbar,
    width: f32,
    floor: ToolbarLevel,
}

impl gpui::Render for ToolbarProbe {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{ParentElement, Styled, div, px};
        let model = ToolbarModel::synthetic(self.state, cx);
        let widths = super::toolbar_widths(&model, cx);
        let level = widths.level_within(self.width, self.floor, ToolbarLevel(ToolbarLevel::MAX));
        div().w(px(self.width)).child(super::render_toolbar(
            model,
            &widths,
            level,
            self.width,
            None,
            moon_ui::MoonPalette::active(cx),
            cx,
        ))
    }
}

/// Left and right edge of the element recorded under `id`, if it was drawn.
fn edges(visual: &mut gpui::VisualTestContext, id: &'static str) -> Option<(f32, f32)> {
    visual
        .debug_bounds(id)
        .map(|b| (f32::from(b.origin.x), f32::from(b.origin.x + b.size.width)))
}

/// `toolbar/fit.rs:toolbar_widths` must never measure narrower than `render_toolbar` draws: for
/// every locale, width and toolbar state the row stays inside the window, its pods do not
/// overlap and none stands under the trailing cluster — the minimum window included — the metric
/// figures are drawn at their measured width, and the level is the lowest that fits. A locked
/// row's overlay covers only the order pods, never Live or the trailing cluster, and its sentence
/// stays inside it with the core name no shorter than its fitted cut; the no-coin hint is whole
/// at 1900 px.
#[gpui::test]
fn the_toolbar_draws_within_its_measurement(cx: &mut gpui::TestAppContext) {
    install_theme(cx);
    for locale in LOCALES {
        let _locale = crate::test_locale::force(locale);
        for state in STATES {
            for (width, floor) in WIDTHS
                .into_iter()
                .map(|width| (width, ToolbarLevel(0)))
                .chain(HELD)
            {
                let window = cx.add_window(|_, _| ToolbarProbe {
                    state,
                    width,
                    floor,
                });
                let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                let (widths, level, figures) = visual.update(|window, cx| {
                    let _ = window.draw(cx);
                    let model = ToolbarModel::synthetic(state, cx);
                    let widths = super::toolbar_widths(&model, cx);
                    let figures = [
                        ("toolbar-lev", model.lev_value.clone()),
                        ("toolbar-sl", model.sl_value.clone()),
                        ("toolbar-tp", model.tp_value.clone()),
                    ]
                    .map(|(id, value)| (id, super::metric_value_width(cx, &value)));
                    let level = widths.level_within(width, floor, ToolbarLevel(ToolbarLevel::MAX));
                    (widths, level, figures)
                });
                let held = level != widths.level(width);
                let case = format!("{locale} {state:?} {width}px L{} held {held}", level.0);
                let pinned = trailing_pinned(&widths, level, width);
                let mut row: Vec<(&str, f32, f32)> = ROW
                    .iter()
                    .filter_map(|id| edges(&mut visual, id).map(|(l, r)| (*id, l, r)))
                    .collect();
                let trailing = row
                    .iter()
                    .find(|(id, ..)| *id == "toolbar-trailing")
                    .map(|(_, l, _)| *l)
                    .expect("trailing cluster");
                // (a) inside the window, and no pod under the trailing cluster, pinned or not.
                for (id, _, end) in &row {
                    assert!(*end <= width + 0.5, "{case}: {id} ends at {end}");
                    if *id != "toolbar-trailing" {
                        assert!(
                            *end <= trailing + 0.5,
                            "{case}: {id} ends at {end} under the cluster at {trailing}"
                        );
                    }
                }
                for id in ["toolbar-live", "toolbar-overflow"] {
                    if let Some((_, end)) = edges(&mut visual, id) {
                        assert!(end <= width + 0.5, "{case}: {id} ends at {end}");
                    }
                }
                // (b) pods side by side; the pinned cluster is excluded, it overlays by design.
                if pinned {
                    row.retain(|(id, ..)| *id != "toolbar-trailing");
                }
                row.sort_by(|a, b| a.1.total_cmp(&b.1));
                for pair in row.windows(2) {
                    let (a, b) = (pair[0], pair[1]);
                    assert!(
                        a.2 <= b.1 + 0.5,
                        "{case}: {} [{}..{}] overlaps {} [{}..{}]",
                        a.0,
                        a.1,
                        a.2,
                        b.0,
                        b.1,
                        b.2
                    );
                }
                // (c) every metric figure at least as wide as its text.
                for (id, text) in figures {
                    if let Some((l, r)) = edges(&mut visual, id) {
                        assert!(r - l + 0.5 >= text, "{case}: {id} {} < {text}", r - l);
                    }
                }
                // (g) tight: unpinned, the drawn content — the row less its spacer — is the
                // measured width, so an over-measurement shows as well as an under-measurement.
                if !pinned {
                    let (_, pods_r) = edges(&mut visual, "toolbar-pods").expect("pods span");
                    let gap = visual
                        .update(|_, cx| crate::design::ui_value(cx, crate::design::CHROME_GAP));
                    let content = width - (trailing - pods_r - 2.0 * gap);
                    let measured = widths.width_at(level);
                    assert!(
                        (content - measured).abs() <= 2.0,
                        "{case}: drawn content {content} vs measured {measured}"
                    );
                }
                // A held level always fits: a caret menu never holds the row past its window.
                if held {
                    assert!(
                        widths.width_at(level) <= width,
                        "{case}: held level overflows"
                    );
                }
                // (d) the lowest level that fits; the narrowest when none does, with "More" and
                // Live still drawn.
                if level.0 > 0 && !held {
                    assert!(
                        widths.width_at(ToolbarLevel(level.0 - 1)) > width,
                        "{case}: a lower level fits"
                    );
                }
                if widths.width_at(ToolbarLevel(ToolbarLevel::MAX)) > width {
                    assert_eq!(level.0, ToolbarLevel::MAX, "{case}");
                    assert!(edges(&mut visual, "toolbar-live").is_some(), "{case}: Live");
                }
                if level.chart_tools_folded() {
                    assert!(
                        edges(&mut visual, "toolbar-overflow").is_some(),
                        "{case}: folded tools need More"
                    );
                }
                // (e) the lock overlay covers the order pods only, and its sentence fits it.
                if let Some((lock_l, lock_r)) = edges(&mut visual, "toolbar-lock") {
                    for id in ["toolbar-live", "toolbar-trailing"] {
                        let (l, _) = edges(&mut visual, id).expect("trailing drawn");
                        assert!(
                            lock_r <= l + 0.5,
                            "{case}: lock [..{lock_r}] over {id} at {l}"
                        );
                    }
                    let fitted = visual.update(|_, cx| {
                        let model = ToolbarModel::synthetic(state, cx);
                        let room = super::order_pods_width(&widths, level, width, cx);
                        let lock = model.lock.clone().expect("locked state");
                        super::LockSentence::fit(&lock, room, cx)
                    });
                    for id in [
                        "toolbar-lock-before",
                        "toolbar-lock-name",
                        "toolbar-lock-after",
                    ] {
                        if let Some((l, r)) = edges(&mut visual, id) {
                            assert!(
                                l + 0.5 >= lock_l && r <= lock_r + 0.5,
                                "{case}: {id} [{l}..{r}] outside the lock [{lock_l}..{lock_r}]"
                            );
                        }
                    }
                    if let (Some((l, r)), Some((_, name_w))) =
                        (edges(&mut visual, "toolbar-lock-name"), fitted.name)
                    {
                        assert!(r - l + 0.5 >= name_w, "{case}: name {} < {name_w}", r - l);
                    }
                }
                // (f) with room to spare the no-coin hint is drawn whole.
                if width >= 1900.0
                    && !held
                    && let Some((l, r)) = edges(&mut visual, "toolbar-no-coin")
                {
                    let text = visual.update(|_, cx| {
                        let model = ToolbarModel::synthetic(state, cx);
                        crate::design::ui_caption_text_width(cx, &model.no_coin_text, 400.0)
                    });
                    assert!(r - l + 0.5 >= text, "{case}: no-coin {} < {text}", r - l);
                }
            }
        }
    }
}

/// Regression target: the folded "More" rows (Take, the Stop switch and value, the own-set check,
/// every S and B preset) reached the active core while the row was locked to another core or had
/// no cores. Every order gate is closed under either lock at L10, where all of them are rows.
#[gpui::test]
fn a_locked_row_closes_every_order_gate(cx: &mut gpui::TestAppContext) {
    install_theme(cx);
    cx.update(|cx| {
        for state in [SynthToolbar::LockedOtherCore, SynthToolbar::NoCores] {
            let model = ToolbarModel::synthetic(state, cx);
            assert!(!model.interactive(), "{state:?}");
            assert_eq!(
                model.order_gates(),
                super::OrderGates {
                    take: false,
                    sl_toggle: false,
                    sl_value: false,
                    sell: false,
                    size: false,
                    own_set: false,
                },
                "{state:?}"
            );
        }
        let open = ToolbarModel::synthetic(SynthToolbar::Normal, cx).order_gates();
        assert!(open.take && open.sl_toggle && open.sl_value && open.sell && open.size);
        assert!(open.own_set);
        let max = ToolbarLevel(ToolbarLevel::MAX);
        assert!(max.take_sell_folded() && max.size_folded() && max.sl_folded());
        assert!(max.own_icon_folded());
    });
}

/// Regression target: a caret menu held the level after its strip stopped taking edits or folded
/// into "More", so the floor never released and the menu reopened. It closes exactly then; a
/// wider window keeps it, held at its collapsed form's first level.
#[test]
fn a_caret_menu_closes_when_its_strip_locks_or_folds() {
    use crate::controls::StripCaret;
    for caret in [StripCaret::Sell, StripCaret::Size] {
        for n in 0..=ToolbarLevel::MAX {
            let level = ToolbarLevel(n);
            assert_eq!(
                caret.menu_gone(level, true),
                level > caret.max_level(),
                "{caret:?} L{n}"
            );
            assert!(caret.menu_gone(level, false), "{caret:?} L{n} disabled");
        }
        assert!(!caret.menu_gone(ToolbarLevel(0), true));
    }
}

/// Regression target: a TP or SL popup opened from "More" held the level back to draw its pod,
/// which the window could not fit. It anchors under "More" exactly where its pod is folded.
#[test]
fn a_folded_metric_popup_anchors_under_more() {
    use crate::controls::TradeMetric;
    for n in 0..=ToolbarLevel::MAX {
        let level = ToolbarLevel(n);
        assert_eq!(
            super::metric_anchors_under_more(TradeMetric::Tp, level),
            level.take_sell_folded()
        );
        assert_eq!(
            super::metric_anchors_under_more(TradeMetric::Sl, level),
            level.sl_folded()
        );
        assert!(!super::metric_anchors_under_more(TradeMetric::Lev, level));
    }
}

/// The layout tests draw the inert path only; that is enough because neither band's measurement
/// reads its actions: the widths come from the model and the fonts alone.
#[test]
fn band_geometry_does_not_read_the_actions() {
    let body = |source: &'static str, sig: &str| -> &'static str {
        source
            .split(sig)
            .nth(1)
            .and_then(|rest| rest.split("\n}\n").next())
            .expect("measurement function")
    };
    let toolbar = body(include_str!("fit.rs"), "pub(crate) fn toolbar_widths(");
    let header = body(
        include_str!("../../chrome/terminal_chrome.rs"),
        "pub(crate) fn header_widths(",
    );
    for (name, body) in [("toolbar_widths", toolbar), ("header_widths", header)] {
        for word in ["actions", "Shell", "shell", "Entity<Backend>"] {
            assert!(!body.contains(word), "{name} reads {word}");
        }
    }
}
