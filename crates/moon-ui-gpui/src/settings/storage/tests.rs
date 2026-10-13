//! Storage page: the usage shares add up, and the page chrome fits the settings body.

use std::default::Default;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use std::collections::HashSet;

use super::super::layout_probe::{ProbeSpec, Probed, sweep};
use super::core_data::{
    CoreCounts, CoreLine, CoreView, Group, Kinds, core_section_el, lines as core_lines,
};
use super::{
    StorageCtx, StorageInfo, StorageView, figure_chunks, stat_items, storage_body_el, usage_parts,
};

/// A probe host drawing whatever `build` returns inside the body box.
struct Host {
    width: f32,
    height: f32,
    scroll: ScrollHandle,
    build: Rc<dyn Fn(&mut Window, &mut App, f32) -> AnyElement>,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = (self.build)(window, cx, self.width);
        div()
            .w(px(self.width))
            .h(px(self.height))
            .debug_selector(|| "probe-body".into())
            .child(
                div()
                    .id("probe-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(div().w_full().flex_none().child(content)),
            )
    }
}

impl Probed for Host {
    fn scroll(&self) -> Option<ScrollHandle> {
        Some(self.scroll.clone())
    }
}

/// The four database shares add up to the bytes on disk, including each WAL, and a missing file
/// contributes zero.
///
/// The expected total is added here from the snapshot literals. It does not call the share helper
/// a second time.
///
/// Breaks on: `usage_parts` dropping a WAL, counting a missing klines file as anything but zero,
/// or listing the databases in an order other than reports, strategies, trades, klines.
#[test]
fn usage_shares_sum_to_the_total() {
    let _locale = crate::test_locale::force("en");
    let info = StorageInfo {
        reports: Some((400, 12)),
        strategies: Some((38, 0)),
        klines: None,
        trades: Some((200, 31)),
        ..StorageInfo::default()
    };

    let parts = usage_parts(&info);

    let expected = [400 + 12, 38, 200 + 31, 0];
    assert_eq!(
        parts.iter().map(|part| part.bytes).collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        parts.iter().map(|part| part.bytes).sum::<u64>(),
        400 + 12 + 38 + 200 + 31
    );
}

/// The production Storage page from synthetic data: two connected cores, one archived and one
/// syncing in the core table, two help lines open, a counted cleanup.
fn page(fields: &[gpui::Entity<moon_ui::MoonInputState>; 3], cx: &App) -> AnyElement {
    let open: HashSet<&'static str> = ["storage-trades-margin", "storage-core-hint"].into();
    let ctx = StorageCtx {
        weak: gpui::WeakEntity::new_invalid(),
        open: &open,
    };
    let core_line = |uid: u64, name: &str, group: Group, syncing: Option<&str>| CoreLine {
        counts: CoreCounts {
            uid,
            stored_name: name.into(),
            reports: 12_345,
            strategies: 48,
            versions: 960,
            traces: 7_777,
            warnings: 31,
        },
        name: name.into(),
        group,
        syncing: syncing.map(String::from),
    };
    let cores = [
        core_line(11, "Synthetic Binance Futures core", Group::Connected, None),
        core_line(12, "Synthetic Bybit core", Group::Offline, Some("42%")),
        core_line(
            13,
            "A retired synthetic core with a long name",
            Group::Archive,
            None,
        ),
    ]
    .into_iter()
    .map(|line| (line.counts.uid, line))
    .collect();
    let lines = core_lines(
        &[],
        cores,
        &rust_i18n::t!("storage.core_data_group_archive"),
    );
    let core = core_section_el(
        &CoreView {
            lines: Some(Ok(Rc::new(lines))),
            selected: [11, 13].into(),
            kinds: Kinds {
                reports: true,
                strategies: true,
                traces: true,
                warnings: true,
            },
            busy: false,
            icons: Default::default(),
        },
        &ctx,
        cx,
    );
    let info = StorageInfo {
        reports: Some((400 << 20, 12 << 20)),
        strategies: Some((38 << 20, 0)),
        klines: Some((200 << 20, 31 << 20)),
        trades: Some((1 << 30, 64 << 20)),
        report_rows: Some(Ok(1_234_567)),
        strat_live: 120,
        strat_deleted: 7,
        strat_versions: 4_321,
    };
    let [version_limit, max_mb, long_min] = fields.clone();
    // No version limit (the field says so in place of a unit) and a 256 MB file ceiling.
    let mut cfg = moon_core::config::storage::StorageCfg::default();
    cfg.strategies.version_limit = 0;
    cfg.trade_replay.max_mb = 256;
    storage_body_el(
        StorageView {
            info,
            busy: false,
            path: "C:/Users/synthetic-user-with-a-long-name/AppData/Roaming/MoonTerminal/data"
                .into(),
            cfg,
            version_limit,
            max_mb,
            long_min,
            cleanup: None,
            core,
        },
        &ctx,
        cx,
    )
}

/// Open the production Storage page ([`page`]) in a window of `size`, its three number fields
/// seeded with `seeds`, and draw it until the layout settles.
///
/// Returns:
///     The drawn window and the three field states.
fn draw_page(
    cx: &mut gpui::TestAppContext,
    size: (f32, f32),
    seeds: [u32; 3],
) -> (
    gpui::VisualTestContext,
    [gpui::Entity<moon_ui::MoonInputState>; 3],
) {
    let mut states = None;
    let window = cx.open_window(gpui::size(px(size.0), px(size.1)), |window, cx| {
        let (width, height) = super::super::layout_probe::body_box(size, cx);
        let made = seeds.map(|seed| super::super::common::number_state(seed, window, cx));
        states = Some(made.clone());
        // A `Root` above the page, as the Settings window has: the number input's `+` reads it.
        let host = gpui::AppContext::new(cx, |_| Host {
            width,
            height,
            scroll: ScrollHandle::new(),
            build: Rc::new(move |_, cx, _| page(&made, cx)),
        });
        moon_ui::Root::new(gpui::AnyView::from(host), window, cx)
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    for _ in 0..3 {
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    (visual, states.expect("window built"))
}

/// A figures line splits only before ` (` and ` · `, so every figure stays with its label.
///
/// Breaks on: a split at every space (a label torn from its figure) or a lost piece.
#[test]
fn figure_chunks_keep_each_figure_with_its_label() {
    assert_eq!(
        figure_chunks("Strategies: 1699 (deleted: 211) \u{b7} Versions: 2822"),
        [
            "Strategies: 1699",
            "(deleted: 211)",
            "\u{b7} Versions: 2822"
        ]
    );
    assert_eq!(
        figure_chunks("Size: 137.4 MB (+ WAL 7.2 MB)"),
        ["Size: 137.4 MB", "(+ WAL 7.2 MB)"]
    );
    assert_eq!(figure_chunks("No file yet"), ["No file yet"]);
}

/// The strategies figures become three stat items without the single-line separators.
///
/// Breaks on: a stat item keeping its `·` or its parentheses, or a figure lost.
#[test]
fn stat_items_drop_the_line_separators() {
    assert_eq!(
        stat_items("Strategies: 1699 (deleted: 211) \u{b7} Versions: 2822"),
        ["Strategies: 1699", "deleted: 211", "Versions: 2822"]
    );
}

/// The Storage cards follow the mockup's two-wide grid: the light column and the trades column
/// side by side end on one line, each column's last card reaching its bottom, so no card stands
/// as an empty box; every setting row's control starts one row gap after its label column (or
/// under it when the row wrapped); and each strategies stat item stays on one line — in every
/// language at every swept size and a wide window.
///
/// Breaks on: columns of different heights, a control pinned to the card's far edge with a hole
/// after the label, or a stat item wrapping inside "1699".
#[gpui::test]
fn storage_cards_align_to_the_grid(cx: &mut gpui::TestAppContext) {
    use super::super::layout_probe::{SIZES, leak};
    cx.update(moon_ui::init);
    let rows = [
        "storage-strat-enabled",
        "storage-version-limit",
        "storage-trades-enabled",
        "storage-trades-margin",
        "storage-trades-max",
        "storage-trades-long",
        "storage-trades-autoload",
        "storage-trades-cleanup",
    ];
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for size in SIZES.into_iter().chain([(1940.0, 1180.0)]) {
            let at = format!("{} at {}x{}", language.code(), size.0, size.1);
            let (mut visual, _states) = draw_page(cx, size, [100, 256, 5]);
            let (line, gap) = visual.update(|_, cx| {
                (
                    f32::from(crate::design::line_px(cx, 14.0)),
                    crate::design::ui_value(cx, 16.0),
                )
            });
            let mut get = |sel: String| {
                visual
                    .debug_bounds(leak(sel.clone()))
                    .unwrap_or_else(|| panic!("{sel} was not laid out ({at})"))
            };
            let light = get("storage-col-light".into());
            let trades = get("storage-col-trades".into());
            if light.origin.y == trades.origin.y {
                let dh = (f32::from(light.size.height) - f32::from(trades.size.height)).abs();
                assert!(
                    dh <= 0.5,
                    "the two card columns differ in height by {dh} ({at})"
                );
                for (column, last) in [
                    (light, "storage-card-klines"),
                    (trades, "storage-card-trades"),
                ] {
                    let gap = f32::from(column.bottom_left().y - get(last.into()).bottom_left().y);
                    assert!(
                        gap.abs() <= 0.5,
                        "{last} ends {gap} px above its column ({at})"
                    );
                }
            }
            for row in rows {
                let label = get(format!("{row}-label"));
                let control = get(format!("{row}-control"));
                if control.origin.y < label.bottom_left().y {
                    let hole = f32::from(control.origin.x - label.right());
                    assert!(
                        (hole - gap).abs() <= 0.5,
                        "{row}'s control is {hole} px from its label column, not one {gap} gap ({at})"
                    );
                } else {
                    let dx = f32::from(control.origin.x - label.origin.x).abs();
                    assert!(
                        dx <= 0.5,
                        "{row}'s wrapped control is not under its label ({at})"
                    );
                }
            }
            // The zero version limit reads «без лимита» on one line beside its field, in the card.
            let note = get("storage-version-limit-off".into());
            let card = get("storage-card-strategies".into());
            assert!(
                f32::from(note.right()) <= f32::from(card.right()) + 0.5
                    && f32::from(note.size.height) < 2.0 * line,
                "the no-limit note is cut or past its card ({at})"
            );
            for i in 0..3 {
                let chunk = get(format!("storage-strat-rows-{i}"));
                assert!(
                    f32::from(chunk.size.height) < 2.0 * line,
                    "stat item {i} wraps: {} tall ({at})",
                    f32::from(chunk.size.height)
                );
            }
        }
    }
}

/// Every number field on the production Storage page holds its seeded digits after drawing, and
/// its box fits both step buttons plus the widest value the field accepts plus its unit (МБ, мин)
/// and stays inside its card; a zero version limit shows «без лимита» beside its field, inside
/// the card, in every language
/// at every swept size.
///
/// The oracle is built here from the control's own metrics: two square buttons (one control
/// height each) plus the mono width of the range's widest end and of the unit. It does not call
/// the field helper.
///
/// Breaks on: a field seeded empty or only on blur, a field drawn over a state other than the
/// page's, or a row/card that squeezes the field so the value has no room between `-` and `+`
/// (the live symptom: an empty ~12 px box), or a field drawn without its unit.
#[gpui::test]
fn storage_number_fields_show_their_digits(cx: &mut gpui::TestAppContext) {
    use super::super::layout_probe::{SIZES, leak};
    cx.update(moon_ui::init);
    // The unit each field must show beside its digits; a bare count has none.
    let fields = [
        ("storage-version-limit-n", 100u32, "10000", ""),
        ("storage-trades-max-n", 256, "100000", "storage.unit_mb"),
        ("storage-trades-long-n", 5, "120", "storage.unit_min"),
    ];
    let cards = [
        "storage-card-strategies",
        "storage-card-trades",
        "storage-card-trades",
    ];
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        // The swept sizes plus a wide window, where the card columns sit side by side.
        for size in SIZES.into_iter().chain([(1940.0, 1180.0)]) {
            let (mut visual, states) = draw_page(cx, size, [fields[0].1, fields[1].1, fields[2].1]);
            for (((sel, seeded, widest, unit_key), state), card) in
                fields.iter().zip(states.iter()).zip(cards)
            {
                let text = visual.update(|_, cx| state.read(cx).value().to_string());
                assert_eq!(
                    text,
                    seeded.to_string(),
                    "{sel} lost its value ({} at {}x{})",
                    language.code(),
                    size.0,
                    size.1
                );
                let b = visual
                    .debug_bounds(leak(sel.to_string()))
                    .unwrap_or_else(|| panic!("{sel} was not laid out"));
                let unit = if unit_key.is_empty() {
                    String::new()
                } else {
                    rust_i18n::t!(*unit_key).to_string()
                };
                let need = visual.update(|_, cx| {
                    let m = crate::design::CONTROL_TIER.control_metrics();
                    2.0 * crate::design::ui_value(cx, m.height)
                        + crate::design::ui_text_width_zoomed(cx, widest, m.font_size, 500.0, true)
                        + crate::design::ui_text_width_zoomed(
                            cx,
                            &unit,
                            m.font_size - 2.0,
                            400.0,
                            true,
                        )
                });
                let card_box = visual
                    .debug_bounds(card)
                    .unwrap_or_else(|| panic!("{card} was not laid out"));
                assert!(
                    f32::from(b.right()) <= f32::from(card_box.right()) + 0.5,
                    "{sel} with its unit {unit:?} paints past {card} ({} at {}x{})",
                    language.code(),
                    size.0,
                    size.1
                );
                assert!(
                    f32::from(b.size.width) + 0.5 >= need,
                    "{sel} is {} wide, needs {need} for {widest:?} and {unit:?} between its buttons ({} at {}x{})",
                    f32::from(b.size.width),
                    language.code(),
                    size.0,
                    size.1
                );
                // The wrapper alone can be wide while the control inside it collapses or is bound
                // to some other state. A click just inside the wrapper's right edge must land on
                // `+` and step THIS state: the control spans the whole box and draws its value.
                // Only where the field is on screen without scrolling (the wide window always).
                // On screen means inside the scrolled body, not merely above the window's edge.
                let body = visual
                    .debug_bounds("probe-body")
                    .expect("probe body laid out");
                if b.bottom_left().y < body.bottom_left().y {
                    let plus =
                        gpui::point(b.right() - gpui::px(4.0), b.origin.y + b.size.height / 2.0);
                    visual.simulate_click(plus, gpui::Modifiers::none());
                    visual.run_until_parked();
                    let stepped = visual.update(|_, cx| state.read(cx).value().to_string());
                    assert_eq!(
                        stepped,
                        (seeded + 1).to_string(),
                        "{sel}'s + is not at its right edge or not bound to its state ({} at {}x{})",
                        language.code(),
                        size.0,
                        size.1
                    );
                } else {
                    assert!(size.0 < 1900.0, "{sel} is off screen in the wide window");
                }
            }
        }
    }
}

/// The production Storage page — usage panel, the four translated database cards with their
/// actions, number fields, switches and cleanup, and the real core table — keeps every card, the
/// card actions, the core rows' footer and the open help inside the body, the cards apart, the
/// help uncut, in every language at every swept size.
#[gpui::test]
fn storage_page_fits_the_body(cx: &mut gpui::TestAppContext) {
    sweep(
        cx,
        |window, cx: &mut Context<Host>, (width, height)| {
            let fields = [
                super::super::common::number_state(100u32, window, cx),
                super::super::common::number_state(256u32, window, cx),
                super::super::common::number_state(5u32, window, cx),
            ];
            Host {
                width,
                height,
                scroll: ScrollHandle::new(),
                build: Rc::new(move |_, cx, _| page(&fields, cx)),
            }
        },
        |_| ProbeSpec {
            container: "probe-body",
            wraps: vec![
                (
                    "storage-trades-margin-more-text".into(),
                    rust_i18n::t!("storage.trades_margin_hint").to_string(),
                ),
                (
                    "storage-core-hint-more-text".into(),
                    rust_i18n::t!("storage.core_data_hint").to_string(),
                ),
            ],
            inside: [
                "storage-usage",
                "storage-card-reports",
                "storage-card-strategies",
                "storage-card-trades",
                "storage-card-klines",
                "storage-trades-margin-more-text",
                "storage-core",
                "storage-core-hint-more-text",
            ]
            .map(String::from)
            .to_vec(),
            no_overlap: vec![
                [
                    "storage-card-reports",
                    "storage-card-strategies",
                    "storage-card-trades",
                    "storage-card-klines",
                ]
                .map(String::from)
                .to_vec(),
            ],
            last: Some("storage-core".into()),
            ..ProbeSpec::default()
        },
    );
}
