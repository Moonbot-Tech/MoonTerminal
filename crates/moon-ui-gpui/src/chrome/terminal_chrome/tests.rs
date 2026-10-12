//! Regression coverage for terminal-header geometry helpers.

use moon_core::session::BalanceState;

use super::{BalanceView, ClockFit, SqueezeRoom, fit_header_core_trigger, header_squeeze};
use crate::design::{HeaderLevel, HeaderWidths};

/// `terminal_chrome.rs:fit_header_core_trigger` must add chrome to the fitted label width, not the
/// maximum label budget. Replacing `label_w` with `max_label_w` recreates the large empty tail for
/// ordinary server names while still passing the overflow cap.
#[test]
fn header_core_trigger_follows_content_until_the_label_cap() {
    let measure = |text: &str| text.chars().count() as f32;

    let short = fit_header_core_trigger("AB", 10.0, 4.0, measure);
    let capped = fit_header_core_trigger("ABCDEFGHIJK", 10.0, 4.0, measure);

    assert_eq!(short, ("AB".to_string(), 6.0));
    assert_eq!(capped, ("ABCDEFGHI…".to_string(), 14.0));
}

/// `terminal_chrome.rs` must keep stale header callbacks passive in Auto and revalidate ticker
/// navigation against the current rail-owned core.
///
/// Mutation: restore `select_auto_workspace_core` in the selector callback or replace the ticker's
/// authorized request with `open_on_main`. Either change lets header chrome bypass the Shell rail.
#[test]
fn header_callbacks_cannot_bypass_auto_rail_authority() {
    let source = include_str!("../terminal_chrome.rs");
    let selector = source
        .split("fn core_selector(")
        .nth(1)
        .expect("core selector must exist");

    assert!(!selector.contains("select_auto_workspace_core"));
    assert!(selector.contains("if b.workspace_mode(&group) == WorkspaceMode::AutoTrading"));
    assert!(source.contains("b.open_on_main_if_authorized(Some(&group), (core, market), false)"));
}

/// A synthetic header: every group a distinct width so a wrong level shows as a wrong sum.
fn widths() -> HeaderWidths {
    HeaderWidths {
        fixed: 400.0,
        right_fixed: 220.0,
        delta_1h: 50.0,
        free_sub: 120.0,
        sleep_label: 45.0,
        ticker: 160.0,
        logo: 110.0,
        launcher_labels: 190.0,
        balance_caption: 60.0,
        clock_city: 40.0,
        separators: 18.0,
        right_separators: 9.0,
        usdt_unit: 38.0,
        inactive_mode_seg: 70.0,
        core_name_full: 230.0,
        core_name_cap_l5: 110.0,
        core_name_cap_l6: 70.0,
        core_gear: 36.0,
    }
}

/// `terminal_chrome.rs:header_widths` feeds `HeaderWidths::level`; a header that keeps a group
/// one pixel past its fit clips a figure, one that drops it early hides it for nothing.
#[test]
fn synthetic_header_picks_the_level_that_fits() {
    let w = widths();
    assert_eq!(w.level(5000.0), HeaderLevel(0));
    // Short of the full row by the 1h delta alone: level 1 sheds it with the free sub-figure.
    let full = w.width_at(HeaderLevel(0));
    assert_eq!(w.level(full - 1.0), HeaderLevel(1));
    assert_eq!(w.level(w.width_at(HeaderLevel(3))), HeaderLevel(3));
    assert_eq!(w.level(1.0), HeaderLevel(HeaderLevel::MAX));
}

/// `terminal_chrome.rs:header_squeeze` must give nothing up on a row that fits, and on one that
/// does not shorten the core name to its floor, hide it, drop the clock's seconds, the updater's
/// label, fold the launchers and then the updater, and only then hide the clock — in that order —
/// before the right cluster pins. Hiding the clock before the launchers fold would leave its zone
/// picker with no "More" to be reached from.
#[test]
fn the_narrowest_header_gives_up_name_clock_and_updater_before_pinning() {
    let room = SqueezeRoom {
        name_cap: 70.0,
        name_floor: 24.0,
        name_gap: 0.0,
        clock_seconds: 20.0,
        clock_minutes: 45.0,
        update: 120.0,
        update_min: 26.0,
        launchers: 60.0,
        update_fold: 40.0,
    };
    let fits = header_squeeze(0.0, &room);
    assert_eq!(
        (fits.name, fits.clock, fits.update, fits.pinned),
        (70.0, ClockFit::Seconds, 120.0, false)
    );
    // 30 px short: the name alone gives them up.
    assert_eq!(header_squeeze(30.0, &room).name, 40.0);
    assert_eq!(header_squeeze(30.0, &room).clock, ClockFit::Seconds);
    // Past the floor the name goes whole before the clock is touched.
    let hidden = header_squeeze(50.0, &room);
    assert_eq!((hidden.name, hidden.clock), (0.0, ClockFit::Seconds));
    // Then the seconds.
    assert_eq!(header_squeeze(80.0, &room).clock, ClockFit::Minutes);
    // Then the updater's label, down to its icon cell, the minute clock still drawn.
    let squeezed = header_squeeze(120.0, &room);
    assert_eq!(
        (squeezed.clock, squeezed.update, squeezed.launchers_folded),
        (ClockFit::Minutes, 90.0, false)
    );
    // Then the launchers fold into "More"; the clock stays.
    let folded = header_squeeze(230.0, &room);
    assert_eq!(
        (
            folded.clock,
            folded.update,
            folded.launchers_folded,
            folded.pinned
        ),
        (ClockFit::Minutes, 26.0, true, false)
    );
    // Then the updater folds into that "More"; the clock still stays.
    let update_folded = header_squeeze(250.0, &room);
    assert_eq!(
        (
            update_folded.clock,
            update_folded.update_folded,
            update_folded.pinned
        ),
        (ClockFit::Minutes, true, false)
    );
    // Only then the clock; only the rest pins.
    let no_clock = header_squeeze(300.0, &room);
    assert_eq!(
        (no_clock.clock, no_clock.launchers_folded, no_clock.pinned),
        (ClockFit::Hidden, true, false)
    );
    let pinned = header_squeeze(400.0, &room);
    assert_eq!(
        (pinned.clock, pinned.update_folded, pinned.pinned),
        (ClockFit::Hidden, true, true)
    );
    // No updater offered: nothing to fold, and a hidden clock still has its "More".
    let none = SqueezeRoom {
        update: 0.0,
        update_min: 0.0,
        update_fold: 0.0,
        ..room
    };
    let bare = header_squeeze(400.0, &none);
    assert!(!bare.update_folded);
    assert_eq!(
        (bare.clock, bare.launchers_folded),
        (ClockFit::Hidden, true)
    );
}

/// `terminal_chrome.rs:BalanceView::of` must headline the TOTAL and keep FREE as the sub-figure;
/// swapping them states the uncommitted part as the whole account. A stale figure keeps its
/// numbers and is marked; an unknown one prints no number at all.
#[test]
fn balance_view_maps_total_free_and_staleness() {
    assert_eq!(
        BalanceView::of(Some(BalanceState::Live), 12.5, 100.0),
        BalanceView::Value {
            total: 100.0,
            free: 12.5,
            stale: false,
            missing: 0
        }
    );
    assert_eq!(
        BalanceView::of(Some(BalanceState::Stale), 12.5, 100.0),
        BalanceView::Value {
            total: 100.0,
            free: 12.5,
            stale: true,
            missing: 0
        }
    );
    assert_eq!(
        BalanceView::of(Some(BalanceState::Awaiting), 12.5, 100.0),
        BalanceView::Unknown
    );
    assert_eq!(
        BalanceView::of(Some(BalanceState::Unpriced), 12.5, 100.0),
        BalanceView::Unknown
    );
    assert_eq!(BalanceView::of(None, 0.0, 0.0), BalanceView::NoCore);
}

/// `terminal_chrome.rs:BalanceView::of_group` heads the Auto overview with the group's folded
/// total. A total that left awaiting or unpriced cores out must say how many (a silent partial
/// sum reads as the whole group's money), one that counted a pre-outage figure is stale, and a
/// group where nothing could be counted prints no number at all.
#[test]
fn group_balance_names_what_the_total_left_out() {
    use moon_core::session::balances::BalanceAggregate;
    let sum = |total: Option<f64>, counted, stale, excluded| BalanceAggregate {
        free: total.map(|t| t / 2.0),
        total,
        counted,
        stale,
        excluded,
        awaiting: excluded,
        unpriced: 0,
    };
    assert_eq!(
        BalanceView::of_group(&sum(Some(100.0), 3, 0, 0), 3),
        BalanceView::Value {
            total: 100.0,
            free: 50.0,
            stale: false,
            missing: 0
        }
    );
    let partial = BalanceView::of_group(&sum(Some(100.0), 2, 1, 1), 3);
    assert_eq!(
        partial,
        BalanceView::Value {
            total: 100.0,
            free: 50.0,
            stale: true,
            missing: 1
        }
    );
    assert!(partial.tagged());
    {
        let _locale = crate::test_locale::force("en");
        let tag = super::balance_tag_text(true, 1);
        assert!(
            tag.contains(&rust_i18n::t!("header.balance_partial", n = 1).to_string())
                && tag.contains(&rust_i18n::t!("assets.balance_stale").to_string()),
            "a partial, stale total must name both: {tag}"
        );
    }
    assert_eq!(
        BalanceView::of_group(&sum(None, 0, 0, 3), 3),
        BalanceView::Unknown
    );
    assert_eq!(
        BalanceView::of_group(&sum(None, 0, 0, 0), 0),
        BalanceView::NoCore
    );
}

/// `terminal_chrome.rs:header_widths` measures figures off a digit template: two balances with
/// the same digit count must measure alike, or the level would flicker with every price tick.
#[gpui::test]
fn balances_with_equal_digit_counts_measure_alike(cx: &mut gpui::TestAppContext) {
    install_theme(cx);
    // Both measurements read localized captions: another test switching the locale between
    // them would fail this one for nothing.
    let _locale = crate::test_locale::force("en");
    cx.update(|cx| {
        let mut low = synthetic_header(HeaderState::Normal);
        let mut high = low.clone();
        low.balance = BalanceView::Value {
            total: 1111.11,
            free: 1000.01,
            stale: false,
            missing: 0,
        };
        high.balance = BalanceView::Value {
            total: 9999.99,
            free: 8888.88,
            stale: false,
            missing: 0,
        };
        assert_eq!(
            super::header_widths(&low, cx),
            super::header_widths(&high, cx)
        );
    });
}

// Layout: the drawn header against its own measurement.

/// Every locale the terminal ships.
const LOCALES: [&str; 7] = ["en", "es", "pt", "ru", "tr", "uk", "vi"];
/// Window widths from a wide monitor down to below the narrowest level.
const WIDTHS: [f32; 6] = [1900.0, 1440.0, 1180.0, 960.0, 760.0, 520.0];
/// The top-level header groups, drawn side by side.
const ROW: [&str; 10] = [
    "header-logo",
    "header-mode",
    "header-core-card",
    "header-gear",
    "header-balance",
    "header-ticker",
    "header-quiet",
    "header-clock",
    "header-launchers",
    "terminal-update",
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum HeaderState {
    Normal,
    Emulator,
    Stale,
    NoCores,
    AutoOverview,
    LongName,
}

const STATES: [HeaderState; 6] = [
    HeaderState::Normal,
    HeaderState::Emulator,
    HeaderState::Stale,
    HeaderState::NoCores,
    HeaderState::AutoOverview,
    HeaderState::LongName,
];

/// A synthetic header in `state`; every value invented.
fn synthetic_header(state: HeaderState) -> super::HeaderModel {
    use super::{CoreCard, CoreDot, GearFace, HeaderModel, TickerQuote};
    use moon_core::config::WorkspaceMode;
    let named = |name: &str, emu| CoreCard::Named {
        name: name.to_string(),
        dot: CoreDot::Ready,
        auto: false,
        emu,
    };
    let value = |total, free, stale| BalanceView::Value {
        total,
        free,
        stale,
        missing: 0,
    };
    let (mode, card, gear, balance) = match state {
        HeaderState::Normal => (
            WorkspaceMode::Classic,
            named("Alpha", Some(false)),
            Some(GearFace::Popover),
            value(12345.67, 2345.6, false),
        ),
        HeaderState::Emulator => (
            WorkspaceMode::Classic,
            named("Alpha", Some(true)),
            Some(GearFace::Expert),
            value(1000.0, 1000.0, false),
        ),
        HeaderState::Stale => (
            WorkspaceMode::Classic,
            named("Alpha", Some(false)),
            Some(GearFace::Popover),
            value(98765.43, 123.45, true),
        ),
        HeaderState::NoCores => (
            WorkspaceMode::Classic,
            CoreCard::NoCores,
            None,
            BalanceView::NoCore,
        ),
        HeaderState::AutoOverview => (
            WorkspaceMode::AutoTrading,
            CoreCard::Named {
                name: "Overview".into(),
                dot: CoreDot::Overview,
                auto: true,
                emu: None,
            },
            None,
            // A group total that could not count two cores says so.
            BalanceView::Value {
                total: 48_210.55,
                free: 31_002.1,
                stale: false,
                missing: 2,
            },
        ),
        HeaderState::LongName => (
            WorkspaceMode::Classic,
            named("SYNTH-CORE ~ F-XX / SUB ACCOUNT No 15 / ALPHA", Some(false)),
            Some(GearFace::Popover),
            BalanceView::Unknown,
        ),
    };
    HeaderModel {
        group: "main".into(),
        mode,
        card,
        gear,
        balance,
        ticker_base: "BTC".into(),
        ticker: Some(TickerQuote {
            last: 67890.12,
            delta_1h_pct: -1.23,
            delta_24h_pct: 4.56,
        }),
        // The stale state also sleeps and offers an update, so the sleep label and the updater
        // are measured in some state.
        sleeping: state == HeaderState::Stale,
        clock_zone: chrono_tz::Tz::UTC,
        update: (state == HeaderState::Stale).then(|| super::UpdateView {
            label: rust_i18n::t!("update.available", version = "9.9.9").to_string(),
            tooltip: String::new(),
            available: true,
            busy: false,
        }),
        core_ever_configured: true,
    }
}

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

/// The header drawn inert at `width`, the way `Shell` draws it.
struct HeaderProbe {
    model: super::HeaderModel,
    width: f32,
}

impl gpui::Render for HeaderProbe {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{ParentElement, Styled, div, px};
        let widths = super::header_widths(&self.model, cx);
        let level = widths.level(self.width);
        div().w(px(self.width)).child(super::render_header(
            &self.model,
            &widths,
            level,
            self.width,
            None,
            moon_ui::MoonPalette::active(cx),
            cx,
        ))
    }
}

/// The text each figure and text button carries, measured: the element drawn under the id must
/// be at least this wide or its text is clipped.
fn figure_widths(model: &super::HeaderModel, cx: &gpui::App) -> Vec<(&'static str, f32)> {
    use crate::design;
    let mono = |text: &str, weight| design::mono_body_text_width(cx, text, weight);
    let mut out = vec![
        (
            "header-clock-time",
            crate::chrome::clock::header_clock_part_widths(model.clock_zone, cx).0,
        ),
        (
            "header-mode",
            super::mode_segment_width(cx, &super::mode_labels(model.auto()).0),
        ),
        (
            "header-quiet",
            crate::chrome::quiet::header_quiet_width(cx, model.sleeping, false),
        ),
    ];
    if let Some(q) = model.ticker {
        out.push((
            "header-ticker-price",
            mono(&super::fmt_ticker_price(q.last), super::PRICE_WEIGHT),
        ));
        out.push((
            "header-ticker-delta-24h",
            mono(&super::delta_text(q.delta_24h_pct).0, 400.0),
        ));
        out.push((
            "header-ticker-delta-1h",
            mono(&super::delta_text(q.delta_1h_pct).0, 400.0),
        ));
    }
    if let BalanceView::Value {
        free,
        stale: false,
        missing: 0,
        ..
    } = model.balance
    {
        out.push(("header-balance-free", super::free_width(cx, free)));
    }
    if let Some(view) = &model.update {
        out.push(("terminal-update", super::update_width(cx, view)));
    }
    out
}

/// Left and right edge of the element recorded under `id`, if it was drawn.
fn edges(visual: &mut gpui::VisualTestContext, id: &'static str) -> Option<(f32, f32)> {
    visual
        .debug_bounds(id)
        .map(|b| (f32::from(b.origin.x), f32::from(b.origin.x + b.size.width)))
}

/// `terminal_chrome.rs:header_widths` must never measure narrower than `render_header` draws:
/// for every locale, width and header state the row stays inside the window, its groups do not
/// overlap, figures are drawn at their measured width, the level is the lowest that fits, the
/// core name is whole below level 5 and the ticker popup's right offset lands on the ticker.
#[gpui::test]
fn the_header_draws_within_its_measurement(cx: &mut gpui::TestAppContext) {
    install_theme(cx);
    for locale in LOCALES {
        let _locale = crate::test_locale::force(locale);
        for state in STATES {
            for width in WIDTHS {
                let model = synthetic_header(state);
                let window = cx.add_window(|_, _| HeaderProbe {
                    model: model.clone(),
                    width,
                });
                let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                let (widths, level, total_w, name_w, mut texts, squeeze, gap) =
                    visual.update(|window, cx| {
                        let _ = window.draw(cx);
                        let texts = figure_widths(&model, cx);
                        let widths = super::header_widths(&model, cx);
                        let level = widths.level(width);
                        let squeeze = super::header_fit(&model, &widths, level, width, cx);
                        let gap = crate::design::ui_value(cx, crate::design::CHROME_GAP);
                        let total_w = match model.balance {
                            BalanceView::Value { total, .. } => {
                                Some(crate::design::mono_body_text_width(
                                    cx,
                                    &moon_core::util::fmt::usd_grouped(total),
                                    super::BALANCE_WEIGHT,
                                ))
                            }
                            _ => None,
                        };
                        let name_w = match &model.card {
                            super::CoreCard::Named { name, .. } => {
                                Some(super::core_name_width(cx, name))
                            }
                            super::CoreCard::NoCores => None,
                        };
                        (widths, level, total_w, name_w, texts, squeeze, gap)
                    });
                // The narrowest header may draw its clock at minute precision.
                if squeeze.clock != ClockFit::Seconds {
                    texts.retain(|(id, _)| *id != "header-clock-time");
                }
                let case = format!("{locale} {state:?} {width}px L{}", level.0);
                // (a) inside the window and (b) groups side by side.
                let mut row: Vec<(&str, f32, f32)> = ROW
                    .iter()
                    .filter_map(|id| edges(&mut visual, id).map(|(l, r)| (*id, l, r)))
                    .collect();
                // Past the narrowest level the right cluster stands over the left groups: each
                // side stays in order on its own and every left control keeps a clickable part
                // (readouts such as the balance may run on under it).
                let pinned = squeeze.pinned;
                let right = if pinned {
                    &[
                        "header-quiet",
                        "header-clock",
                        "header-launchers",
                        "terminal-update",
                    ][..]
                } else {
                    &["header-quiet", "header-clock", "header-launchers"][..]
                };
                let controls = ["header-mode", "header-core-card", "header-gear"];
                for (id, _, end) in &row {
                    if pinned && !right.contains(id) {
                        continue;
                    }
                    assert!(*end <= width + 0.5, "{case}: {id} ends at {end}");
                }
                // The balance figures are never covered by the right cluster, pinned or not —
                // the minimum window included.
                let cluster_l = edges(&mut visual, "header-right").expect("right cluster").0;
                for id in ["header-balance-total", "header-balance-free"] {
                    if let Some((_, r)) = edges(&mut visual, id) {
                        assert!(
                            r <= cluster_l + 0.5,
                            "{case}: {id} ends at {r} under the right cluster at {cluster_l}"
                        );
                    }
                }
                // (g) tight: where the level fits, the drawn content — the row less its drag
                // spacer — is the measured width, so an over-measurement shows as well as an
                // under-measurement.
                if widths.width_at(level) <= width {
                    let left_end = ["header-core-card", "header-gear", "header-balance"]
                        .into_iter()
                        .filter_map(|id| edges(&mut visual, id).map(|(_, r)| r))
                        .fold(f32::MIN, f32::max);
                    let right_start = ["terminal-update", "header-ticker", "header-right"]
                        .into_iter()
                        .filter_map(|id| edges(&mut visual, id).map(|(l, _)| l))
                        .fold(f32::MAX, f32::min);
                    let content = width - (right_start - left_end - 2.0 * gap);
                    let measured = widths.width_at(level);
                    assert!(
                        (content - measured).abs() <= 2.0,
                        "{case}: drawn content {content} vs measured {measured}"
                    );
                }
                row.sort_by(|a, b| a.1.total_cmp(&b.1));
                if pinned {
                    let cluster = edges(&mut visual, "header-right").expect("right cluster").0;
                    for (id, left, _) in row.iter().filter(|(id, ..)| controls.contains(id)) {
                        assert!(
                            *left + 8.0 <= cluster,
                            "{case}: {id} covered by the cluster"
                        );
                    }
                    row.retain(|(id, ..)| !right.contains(id));
                }
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
                // (c) the balance figure at least as wide as its text.
                if let (Some(total), Some((l, r))) =
                    (total_w, edges(&mut visual, "header-balance-total"))
                {
                    assert!(r - l + 0.5 >= total, "{case}: balance {} < {total}", r - l);
                }
                if level.launcher_labels() {
                    let labels = visual.update(|_, cx| {
                        super::launcher_labels().map(|l| super::launcher_label_width(cx, &l))
                    });
                    let ids = ["header-strategies", "header-analytics", "header-settings"];
                    for (id, text) in ids.into_iter().zip(labels) {
                        let (l, r) = edges(&mut visual, id).expect("launcher drawn");
                        assert!(r - l + 0.5 >= text, "{case}: {id} {} < {text}", r - l);
                    }
                }
                for (id, text) in texts {
                    // A pinned or squeezed updater gives its label up to the room left (see
                    // `update_button`).
                    if id == "terminal-update" && (pinned || squeeze.update < text) {
                        continue;
                    }
                    if let Some((l, r)) = edges(&mut visual, id) {
                        assert!(r - l + 0.5 >= text, "{case}: {id} {} < {text}", r - l);
                    }
                }
                // (d) the lowest level that fits; the narrowest when none does.
                if level.0 > 0 {
                    assert!(
                        widths.width_at(HeaderLevel(level.0 - 1)) > width,
                        "{case}: a lower level fits"
                    );
                }
                if widths.width_at(HeaderLevel(HeaderLevel::MAX)) > width {
                    assert_eq!(level.0, HeaderLevel::MAX, "{case}");
                }
                // (e) the core name whole below level 5.
                if let (true, Some(name), Some((l, r))) =
                    (level.0 < 5, name_w, edges(&mut visual, "header-core-name"))
                {
                    let cap = name.min(widths.core_name_full);
                    assert!(r - l + 0.5 >= cap, "{case}: core name {} < {cap}", r - l);
                }
                // (f) the ticker popup's right offset lands on the ticker's right edge.
                if let Some((_, r)) = edges(&mut visual, "header-ticker") {
                    let offset = widths.right_of_ticker(level);
                    assert!(
                        ((width - r) - offset).abs() <= 1.0,
                        "{case}: popup offset {offset} vs ticker gap {}",
                        width - r
                    );
                }
            }
        }
    }
}

/// At the minimum window, for every locale and header state, the clock's zone picker must stay
/// reachable: either the clock is drawn (its click opens the picker), or the header's "More" is
/// drawn with the picker's anchor beside it and lists the «Time zone» row that opens it. Hiding
/// the clock before the launchers fold, or dropping the row, strands the zone setting.
#[gpui::test]
fn the_clock_zone_picker_stays_reachable_at_the_minimum_window(cx: &mut gpui::TestAppContext) {
    install_theme(cx);
    let width = WIDTHS[WIDTHS.len() - 1];
    for locale in LOCALES {
        let _locale = crate::test_locale::force(locale);
        for state in STATES {
            let model = synthetic_header(state);
            let window = cx.add_window(|_, _| HeaderProbe {
                model: model.clone(),
                width,
            });
            let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
            let squeeze = visual.update(|window, cx| {
                let _ = window.draw(cx);
                let widths = super::header_widths(&model, cx);
                let level = widths.level(width);
                super::header_fit(&model, &widths, level, width, cx)
            });
            let case = format!("{locale} {state:?} {width}px");
            if squeeze.clock != ClockFit::Hidden {
                assert!(
                    edges(&mut visual, "header-clock").is_some(),
                    "{case}: clock not drawn"
                );
                assert!(
                    edges(&mut visual, "header-clock-more-anchor").is_none(),
                    "{case}: picker anchored under More beside a drawn clock"
                );
                continue;
            }
            assert!(squeeze.launchers_folded, "{case}: clock hidden, no More");
            assert!(
                edges(&mut visual, "header-clock").is_none(),
                "{case}: hidden clock drawn"
            );
            assert!(
                edges(&mut visual, "header-overflow").is_some(),
                "{case}: More not drawn"
            );
            assert!(
                edges(&mut visual, "header-clock-more-anchor").is_some(),
                "{case}: picker anchor not drawn"
            );
            let lead = super::header_more_lead(None, true, None);
            assert!(
                lead.iter()
                    .any(|row| row.key().as_ref() == "header-clock-zone-menu"),
                "{case}: no time-zone row in More"
            );
        }
    }
    // The row is listed only while the clock is hidden, after the folded updater's row.
    assert!(super::header_more_lead(None, false, None).is_empty());
    let update = moon_ui::MoonMenuItem::with_key("header-update-menu", "update");
    let keys: Vec<String> = super::header_more_lead(Some(update), true, None)
        .iter()
        .map(|row| row.key().to_string())
        .collect();
    assert_eq!(keys, ["header-update-menu", "header-clock-zone-menu"]);
}

/// `clock.rs:header_clock_picker_anchor` must open the SAME picker the clock opens: one popover
/// id and one menu builder. A copy of the menu or a second id would let the folded surface drift
/// from the clock's (another city list, a lost selection mark, a focus stranded on a swap).
#[test]
fn the_folded_clock_opens_the_clock_s_own_picker() {
    let source = include_str!("../clock.rs");
    let body = |name: &str| {
        let start = source.find(name).expect("function present");
        let rest = &source[start..];
        let end = rest[1..]
            .find("\nfn ")
            .or_else(|| rest[1..].find("\npub(crate) fn "));
        &rest[..end.map_or(rest.len(), |e| e + 1)]
    };
    for name in ["fn clock_with_picker(", "fn header_clock_picker_anchor("] {
        let body = body(name);
        assert!(
            body.contains("zone_picker_menu("),
            "{name} builds its own menu"
        );
        assert!(
            body.contains("zone_popover("),
            "{name} configures its own popover"
        );
    }
    assert!(
        body("fn header_clock_picker_anchor(")
            .contains("MoonPopover::new(\"header-clock-popover\")")
    );
}
