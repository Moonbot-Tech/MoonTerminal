//! Layout probe of the rebuilt Summary blocks: each block renders backend-free on synthetic
//! data across every theme, language, zoom and width, and the probe asserts that boxes stay
//! inside their parents, do not overlap, and are never narrower than their own text.
//!
//! Later blocks add their own `*_fits_*` test on the same harness ([`probe`], [`inside`],
//! [`no_overlap`], [`fits`]).

use std::collections::BTreeSet;
use std::rc::Rc;

use gpui::{
    AnyElement, Bounds, Context, InteractiveElement, IntoElement, ParentElement, Pixels, Render,
    Styled, VisualTestContext, Window, px,
};
use moon_core::config::{Language, UiThemeMode};
use moon_core::db::analytics::{DayPoint, GroupStat, PeriodStats, Summary};
use moon_core::db::{ProfitUnit, QuoteCurrency};
use moon_ui::{MoonPalette, MoonTheme, v_flex};
use rust_i18n::t;

use super::super::kpi::{KpiCols, hero_value, hold_text, kpi_band_el, kpi_cols, metrics};
use super::super::model::{DeltaLine, KpiBand, insights};
use super::{KPI_BAND, kpi_delta, kpi_delta_part, kpi_group, kpi_label, kpi_tile, kpi_value};
use crate::analytics::set_pnl_unit;
use crate::design;

/// Themes the probe walks: the light set and both dark-set modes.
const THEMES: [UiThemeMode; 3] = [UiThemeMode::Light, UiThemeMode::Dark, UiThemeMode::Graphite];
/// Content zooms the probe walks.
const ZOOMS: [f32; 2] = [1.0, 1.25];
/// Window widths the probe walks, at the design reference size: four, two and one column.
const WIDTHS: [f32; 3] = [1400.0, 900.0, 600.0];

/// The KPI band also walks the narrowest four-column width, where its cards are tightest.
const KPI_WIDTHS: [f32; 4] = [1400.0, 1100.0, 900.0, 600.0];

/// The text of one comparison part as the band draws it: 0 is the change, 1 the previous
/// value or the extra fact.
fn delta_part_text(m: &KpiBand, name: &str, k: usize) -> String {
    let (line, extra) = match name {
        "wr" => (m.quality.wr_delta.as_ref(), None),
        "pf" => (m.quality.pf_delta.as_ref(), None),
        "dd" => (m.risk.dd_line.as_ref(), m.risk.dd_of_profit.clone()),
        "tr" => (m.activity.trades_delta.as_ref(), None),
        _ => (
            m.activity.avg_delta.as_ref(),
            Some(hold_text(m.activity.hold_min)),
        ),
    };
    let line = line.expect("a drawn part has a comparison");
    if k == 0 {
        line.text.clone()
    } else {
        line.prev_text.clone().or(extra).unwrap_or_default()
    }
}

/// A loss-making period with long figures, so every value is as wide as it realistically gets.
fn long_summary() -> Summary {
    Summary {
        cur: PeriodStats {
            n: 9_876_543,
            wins: 4_000_000,
            losses: 5_876_543,
            profit: -12_345_678.91,
            pf: 0.87,
            avg: -123_456.78,
            max_dd: 1_234_567.89,
            avg_dur_min: 1_440.0,
            ..Default::default()
        },
        prev: Some(PeriodStats {
            n: 98_765,
            wins: 60_000,
            losses: 38_765,
            profit: 45_678.9,
            pf: 1.94,
            avg: 462.5,
            max_dd: 9_876.54,
            avg_dur_min: 60.0,
            ..Default::default()
        }),
        bucket_secs: 86_400,
        days: (0..30)
            .map(|i| DayPoint {
                start: 1_800_000_000 + i * 86_400,
                profit: if i % 3 == 0 { -900.0 } else { 400.0 },
                trades: 4_000,
            })
            .collect(),
        strategies: vec![GroupStat {
            key: "17@1".into(),
            name: "XSTRAT LONG HIGH VOLUMES 01".into(),
            n: 212,
            wins: 200,
            profit: 2_476.59,
            ..Default::default()
        }],
        best_hour: Some((1, 4_151.85, 121)),
        ..Default::default()
    }
}

/// The band the probe draws: long values, comparison on.
fn band(compare: bool) -> KpiBand {
    let spark: Rc<[f32]> = (0..30).map(|i| (i as f32 * 1.7).sin() * 100.0).collect();
    KpiBand::new(&long_summary(), spark, compare)
}

/// Money mode in USDT, the widest unit caption.
fn usdt() {
    set_pnl_unit(Some(ProfitUnit::Quote(
        QuoteCurrency::from_report_ordinal(1).expect("USDT report ordinal"),
    )));
}

/// Backend-free host of one Summary block at the window's full width.
struct SummaryProbe {
    /// Builds the block under test from the live app context.
    build: Rc<dyn Fn(MoonPalette, &gpui::App) -> AnyElement>,
}

impl Render for SummaryProbe {
    /// Stack the block under test in one full-width column.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        v_flex()
            .w_full()
            .debug_selector(|| "probe-col".into())
            .child((self.build)(p, cx))
    }
}

/// `debug_bounds` takes a `'static` selector; the probe's selectors are built per run.
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// Bounds of a selector, or a panic naming it and the run.
fn bounds(visual: &mut VisualTestContext, sel: &str, at: &str) -> Bounds<Pixels> {
    visual
        .debug_bounds(leak(sel.to_string()))
        .unwrap_or_else(|| panic!("{sel} not laid out ({at})"))
}

/// `child` lies within `parent`, with half a pixel of rounding slack.
fn inside(child: Bounds<Pixels>, parent: Bounds<Pixels>) -> bool {
    child.origin.x + px(0.5) >= parent.origin.x
        && child.right() <= parent.right() + px(0.5)
        && child.origin.y + px(0.5) >= parent.origin.y
        && child.bottom() <= parent.bottom() + px(0.5)
}

/// `a` and `b` share no area beyond rounding slack.
fn no_overlap(a: Bounds<Pixels>, b: Bounds<Pixels>) -> bool {
    a.right() <= b.origin.x + px(0.5)
        || b.right() <= a.origin.x + px(0.5)
        || a.bottom() <= b.origin.y + px(0.5)
        || b.bottom() <= a.origin.y + px(0.5)
}

/// A box is at least as wide as the measured text it draws.
fn fits(b: Bounds<Pixels>, width: f32) -> bool {
    f32::from(b.size.width) + 0.1 >= width
}

/// Percent mode: the unit caption is "%".
fn percent() {
    set_pnl_unit(Some(ProfitUnit::Percent));
}

/// Render one block in USDT money mode; see [`probe_in`].
fn probe(
    cx: &mut gpui::TestAppContext,
    build: impl Fn(f32) -> Rc<dyn Fn(MoonPalette, &gpui::App) -> AnyElement>,
    check: impl FnMut(&mut VisualTestContext, &str, f32),
) {
    probe_in(cx, usdt, build, check);
}

/// Render one block in every theme, language, zoom and width under one profit unit, and hand
/// each laid-out frame to `check` with the run's label and design width.
fn probe_in(
    cx: &mut gpui::TestAppContext,
    unit: fn(),
    build: impl Fn(f32) -> Rc<dyn Fn(MoonPalette, &gpui::App) -> AnyElement>,
    check: impl FnMut(&mut VisualTestContext, &str, f32),
) {
    probe_widths(cx, &WIDTHS, unit, build, check);
}

/// [`probe_in`] over an explicit list of design widths.
fn probe_widths(
    cx: &mut gpui::TestAppContext,
    widths: &[f32],
    unit: fn(),
    build: impl Fn(f32) -> Rc<dyn Fn(MoonPalette, &gpui::App) -> AnyElement>,
    mut check: impl FnMut(&mut VisualTestContext, &str, f32),
) {
    for mode in THEMES {
        cx.update(|cx| {
            MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(mode, 1.0),
                cx,
            )
        });
        for language in Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            unit();
            for zoom in ZOOMS {
                for &width in widths {
                    let build = build(width);
                    let window = cx
                        .open_window(gpui::size(px(width * zoom), px(1200.0 * zoom)), |_, _| {
                            SummaryProbe { build }
                        });
                    let mut visual = VisualTestContext::from_window(window.into(), cx);
                    visual.update(|window, cx| window.set_content_zoom(zoom, cx));
                    for _ in 0..3 {
                        visual.update(|window, cx| {
                            let _ = window.draw(cx);
                        });
                    }
                    let at = format!("{} {mode:?} zoom {zoom} width {width}", language.code());
                    check(&mut visual, &at, width);
                }
            }
        }
    }
}

/// Tight line height of the mockup's insight rows (`.ins` lines at 1.25), as a multiple of
/// the font size: the ceiling a row's three lines are held to, independent of the renderer.
const MOCKUP_TIGHT_LINE: f32 = 1.25;

/// Group a KPI box belongs to, by its metric name.
fn kpi_home(name: &str) -> usize {
    match name {
        "hero" | "unit" => 0,
        "wr" | "pf" => 1,
        "dd" | "rf" => 2,
        _ => 3,
    }
}

/// One frame of the band: cards inside and apart and stacked by width; every tile inside ITS
/// card, the paired tiles apart and equally tall; label above value, value as wide as its
/// measured text, each comparison part one line; the hero clear of its unit.
fn kpi_check(visual: &mut VisualTestContext, at: &str, width: f32) {
    let col = bounds(visual, "probe-col", at);
    let band_b = bounds(visual, KPI_BAND, at);
    assert!(inside(band_b, col), "band escaped the window ({at})");
    let groups: Vec<_> = (0..4).map(|i| bounds(visual, &kpi_group(i), at)).collect();
    for (i, g) in groups.iter().enumerate() {
        assert!(inside(*g, band_b), "group {i} escaped the band ({at})");
        for (j, h) in groups.iter().enumerate().skip(i + 1) {
            assert!(no_overlap(*g, *h), "groups {i} and {j} overlap ({at})");
        }
    }
    let row = |i: usize| groups[i].origin.y;
    match kpi_cols(width, 1.0) {
        KpiCols::Four => assert!(
            (1..4).all(|i| row(i) == row(0)),
            "four columns must share one row ({at})"
        ),
        KpiCols::Two => assert!(
            row(1) == row(0) && row(3) == row(2) && row(2) > row(0),
            "two columns must pair up ({at})"
        ),
        KpiCols::One => assert!(
            (1..4).all(|i| row(i) > row(i - 1)),
            "one column must stack ({at})"
        ),
    }
    // (name, measured value width, has a change line, has a second line)
    let (tiles, line_h) = visual.update(|_, cx| {
        let m = band(true);
        let parts = |d: Option<&DeltaLine>, extra: bool| {
            d.map_or((false, false), |d| (true, d.prev_text.is_some() || extra))
        };
        let tiles: Vec<(&'static str, f32, (bool, bool))> = metrics(&m, MoonPalette::active(cx))
            .iter()
            .map(|x| {
                let lines = match x.name {
                    "wr" => parts(m.quality.wr_delta.as_ref(), false),
                    "pf" => parts(m.quality.pf_delta.as_ref(), false),
                    "dd" => parts(m.risk.dd_line.as_ref(), m.risk.dd_of_profit.is_some()),
                    "tr" => parts(m.activity.trades_delta.as_ref(), false),
                    "avg" => parts(m.activity.avg_delta.as_ref(), true),
                    _ => (false, false),
                };
                (
                    x.name,
                    design::mono_title_text_width(cx, &x.value, 600.0),
                    lines,
                )
            })
            .collect();
        (tiles, f32::from(design::t_caption(cx)) * 1.8)
    });
    let hero = bounds(visual, &kpi_value("hero"), at);
    let unit = bounds(visual, &kpi_value("unit"), at);
    assert!(
        inside(hero, groups[0]) && inside(unit, groups[0]),
        "hero escaped the Result group ({at})"
    );
    assert!(
        hero.right() <= unit.origin.x + px(0.5),
        "hero runs under its unit ({at})"
    );
    if let Some(d) = visual.debug_bounds("sum-kpi-dhero") {
        assert!(inside(d, groups[0]), "hero comparison escaped ({at})");
    }
    for &(name, value_w, (change, tail)) in &tiles {
        let g = groups[kpi_home(name)];
        let t = bounds(visual, &kpi_tile(name), at);
        let v = bounds(visual, &kpi_value(name), at);
        let k = bounds(visual, &kpi_label(name), at);
        assert!(inside(t, g), "tile {name} escaped its card ({at})");
        assert!(inside(v, t), "value {name} escaped its tile ({at})");
        assert!(inside(k, t), "label {name} escaped its tile ({at})");
        assert!(fits(v, value_w * 0.98), "value {name} clipped ({at})");
        assert!(
            k.bottom() <= v.origin.y + px(0.5),
            "label {name} not above its value ({at})"
        );
        let d = bounds(visual, &kpi_delta(name), at);
        assert!(inside(d, t), "caption {name} escaped its tile ({at})");
        assert!(
            v.bottom() <= d.origin.y + px(0.5),
            "caption {name} not under its value ({at})"
        );
        let mut prev: Option<Bounds<Pixels>> = None;
        for (k, present) in [(0, change), (1, tail)] {
            if !present {
                continue;
            }
            let part = bounds(visual, &kpi_delta_part(name, k), at);
            assert!(
                inside(part, d),
                "{name} part {k} escaped its caption ({at})"
            );
            let need = visual.update(|_, cx| {
                let text = delta_part_text(&band(true), name, k);
                design::mono_caption_text_width(cx, &text, 400.0) * 0.98
            });
            assert!(fits(part, need), "{name} part {k} cut ({at})");
            assert!(
                f32::from(part.size.height) <= line_h,
                "{name} part {k} split over two lines ({at})"
            );
            if let Some(prev) = prev {
                assert!(
                    prev.bottom() <= part.origin.y + px(0.5),
                    "{name} change and comparison not on two lines ({at})"
                );
            }
            prev = Some(part);
        }
    }
    // Each card's two tiles sit apart; side by side they are equally tall, and a card too
    // narrow for both stacks them rather than letting one escape.
    for pair in tiles.chunks(2) {
        let a = bounds(visual, &kpi_tile(pair[0].0), at);
        let b = bounds(visual, &kpi_tile(pair[1].0), at);
        assert!(no_overlap(a, b), "tiles {} overlap ({at})", pair[0].0);
        if a.origin.y == b.origin.y {
            assert!(
                a.right() <= b.origin.x + px(0.5),
                "tiles {} out of order ({at})",
                pair[0].0
            );
            assert!(
                (a.size.height - b.size.height).abs() < px(0.5),
                "tiles {} unequal height ({at})",
                pair[0].0
            );
        } else {
            assert!(
                a.bottom() <= b.origin.y + px(0.5),
                "tiles {} neither paired nor stacked ({at})",
                pair[0].0
            );
        }
    }
}

/// The band keeps its four cards inside and apart and stacks them 4 / 2x2 / 1 by width; in each
/// card the two metric tiles sit side by side, apart and equally tall, with the label above an
/// unclipped value and a comparison whose change and previous value are two whole lines, in
/// money and percent mode and in every theme, language and zoom. Letting a value overflow its
/// tile, putting the delta back on one wrapping line, dropping the tiles' stretch or moving a
/// breakpoint breaks it.
#[gpui::test]
fn kpi_band_fits_every_language_theme_zoom_and_width(cx: &mut gpui::TestAppContext) {
    let build = |width| -> Rc<dyn Fn(MoonPalette, &gpui::App) -> AnyElement> {
        let cols = kpi_cols(width, 1.0);
        Rc::new(move |p, cx| kpi_band_el(&band(true), cols, p, cx))
    };
    probe_widths(cx, &KPI_WIDTHS, usdt, build, kpi_check);
    probe_widths(cx, &KPI_WIDTHS, percent, build, kpi_check);
}

/// In percent mode the caption beside the hero is "%", so the hero itself carries no unit (F11);
/// in money mode the caption names the quote and the figure stays bare too.
#[test]
fn hero_names_its_unit_once() {
    percent();
    let unit = crate::analytics::pnl_unit_label().to_string();
    assert!(unit.contains('%'), "{unit:?}");
    assert!(!hero_value(-12.5, &unit).contains('%'));
    assert_eq!(hero_value(0.0, ""), super::super::fmt_signed(0.0));
    usdt();
}

/// Every analytics.sum key of one language's dictionary file.
fn sum_keys(code: &str) -> BTreeSet<String> {
    let path = format!(
        "{}/../../locales/{code}/analytics.{code}.yml",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{path}: {e}"))
        .lines()
        .filter_map(|l| l.split_once(':').map(|(k, _)| k.trim().to_string()))
        .filter(|k| k.starts_with("analytics.sum."))
        .collect()
}

/// Every language carries exactly the English Summary keys, and no text the Summary model
/// draws is a raw key. Keys are read from the files: a missing translation falls back to
/// English at runtime, so asking `t!` could never catch one.
#[test]
fn no_summary_text_is_a_raw_key() {
    let en = sum_keys("en");
    assert!(en.len() > 80, "summary keys not found");
    for language in Language::ALL {
        let keys = sum_keys(language.code());
        let missing: Vec<_> = en.difference(&keys).collect();
        let extra: Vec<_> = keys.difference(&en).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "{}: missing {missing:?}, extra {extra:?}",
            language.code()
        );
        let _locale = crate::test_locale::force(language.code());
        usdt();
        let m = band(true);
        let mut drawn: Vec<String> = metrics(&m, MoonPalette::LIGHT)
            .into_iter()
            .map(|metric| metric.label)
            .collect();
        drawn.push(hold_text(m.activity.hold_min));
        for line in [
            &m.result.was,
            &m.quality.wr_delta,
            &m.quality.pf_delta,
            &m.risk.dd_line,
            &m.activity.trades_delta,
            &m.activity.avg_delta,
        ]
        .into_iter()
        .flatten()
        {
            drawn.push(line.text.clone());
            drawn.extend(line.prev_text.clone());
        }
        drawn.extend(m.risk.dd_of_profit.clone());
        for ins in insights(&long_summary(), chrono_tz::UTC) {
            drawn.push(ins.what);
            drawn.push(ins.value);
            drawn.extend(ins.why);
        }
        for key in [
            "analytics.sum.kpi.result",
            "analytics.sum.kpi.quality",
            "analytics.sum.kpi.risk",
            "analytics.sum.kpi.activity",
            "analytics.sum.kpi.recovery_hint",
            "analytics.sum.meter_tip",
        ] {
            drawn.push(t!(key).to_string());
        }
        for text in drawn {
            assert!(
                !text.contains("analytics.") && !text.is_empty(),
                "{}: raw key or empty text {text:?}",
                language.code()
            );
        }
    }
}

/// A wide synthetic profit snapshot: thirty days, seven cores with long names, a real drawdown.
fn chart_summary() -> std::sync::Arc<Summary> {
    use moon_core::db::analytics::CoreSeries;
    let days = 30usize;
    let core_days: Vec<CoreSeries> = (0..7u64)
        .map(|i| {
            let per_bucket: Vec<f64> = (0..days)
                .map(|d| ((d as f64 * 1.3 + i as f64).sin() * 900.0) + 120.0 - i as f64 * 40.0)
                .collect();
            CoreSeries {
                uid: 500 + i,
                name: format!("XALPHA F-XX / SUB {i:02} LONG VOLUMES"),
                total: per_bucket.iter().sum(),
                per_bucket,
                per_bucket_trades: vec![12; days],
                trades: 12 * days as i64,
                ..Default::default()
            }
        })
        .collect();
    std::sync::Arc::new(Summary {
        bucket_secs: 86_400,
        days: (0..days)
            .map(|d| DayPoint {
                start: 1_800_000_000 + d as i64 * 86_400,
                profit: core_days.iter().map(|c| c.per_bucket[d]).sum(),
                trades: 84,
            })
            .collect(),
        core_days,
        ..long_summary()
    })
}

/// The profit card keeps its title clear of the line control and its scope marker, legend and
/// plot inside the card; the axis ticks stay inside the plot and apart; the peak, trough and
/// bar extreme labels stay inside the plot, apart from each other and from every tick; and a
/// hovered bucket's popup stays inside the window — un-hovered, at the first and the last
/// bucket, and with seven picked cores, at every width, theme, zoom and language.
#[gpui::test]
fn profit_chart_card_fits_every_language_theme_and_width(cx: &mut gpui::TestAppContext) {
    use super::super::profit_chart::{ChartActs, ProfitChartModel, profit_chart_el};
    use super::super::state::{CoreLines, SummaryUi};
    let data = chart_summary();
    let all: BTreeSet<u64> = data.core_days.iter().map(|c| c.uid).collect();
    let last = data.days.len() - 1;
    for (lines, picked, hover, shown) in [
        (CoreLines::Top5, BTreeSet::new(), None, 5usize),
        (CoreLines::Top5, BTreeSet::new(), Some(0usize), 5),
        (CoreLines::Top5, BTreeSet::new(), Some(last), 5),
        (CoreLines::Selected, all.clone(), Some(last / 2), 7),
    ] {
        probe(
            cx,
            |_| {
                let (data, picked) = (data.clone(), picked.clone());
                Rc::new(move |p, cx| {
                    let derived = super::super::derived::derive_for_test(data.clone(), p);
                    let ui = SummaryUi {
                        lines,
                        picked: picked.clone(),
                        ..Default::default()
                    };
                    let marker = Some(("XSCOPE".to_string(), "tip".to_string()));
                    let m = ProfitChartModel::new(&derived, &ui, chrono_tz::UTC, marker, p);
                    let acts = ChartActs {
                        on_lines: Rc::new(|_, _, _| {}),
                        on_area: Rc::new(|_, _, _| {}),
                        on_bucket: Rc::new(|_, _, _| {}),
                        on_popup: Rc::new(|_, _, _, _| {}),
                    };
                    profit_chart_el(&m, hover, &acts, p, cx)
                })
            },
            |visual, at, width| {
                let at = format!("{at} {lines:?} hover {hover:?}");
                let at = at.as_str();
                let card = bounds(visual, "sum-chart-card", at);
                let title = bounds(visual, "sum-chart-title", at);
                let lines_b = bounds(visual, "sum-chart-lines", at);
                let plot = bounds(visual, "sum-chart-plot", at);
                let marker = bounds(visual, "sum-chart-marker", at);
                assert!(inside(title, card), "title escaped ({at})");
                assert!(inside(lines_b, card), "line control escaped ({at})");
                assert!(
                    no_overlap(title, lines_b),
                    "title under the line control ({at})"
                );
                assert!(inside(marker, card), "scope marker escaped ({at})");
                assert!(inside(plot, card), "plot escaped ({at})");
                for i in 0..shown + 2 {
                    let item = bounds(visual, &format!("sum-chart-legend-{i}"), at);
                    assert!(inside(item, card), "legend item {i} escaped ({at})");
                }
                let ticks: Vec<_> = (0..7)
                    .map(|i| bounds(visual, &format!("sum-chart-tick-{i}"), at))
                    .collect();
                for (i, t) in ticks.iter().enumerate() {
                    assert!(
                        t.origin.x + px(0.5) >= plot.origin.x
                            && t.right() <= plot.right() + px(0.5),
                        "tick {i} escaped the plot ({at})"
                    );
                    for (j, u) in ticks.iter().enumerate().skip(i + 1) {
                        assert!(no_overlap(*t, *u), "ticks {i} and {j} overlap ({at})");
                    }
                }
                let marks: Vec<(&str, Bounds<Pixels>)> = [
                    "sum-chart-peak",
                    "sum-chart-trough",
                    "sum-chart-bar-max",
                    "sum-chart-bar-min",
                ]
                .into_iter()
                .filter_map(|sel| visual.debug_bounds(sel).map(|b| (sel, b)))
                .collect();
                assert!(marks.len() >= 3, "extreme labels missing ({at})");
                for (k, (sel, b)) in marks.iter().enumerate() {
                    assert!(inside(*b, plot), "{sel} escaped the plot ({at})");
                    for (other, c) in marks.iter().skip(k + 1) {
                        assert!(no_overlap(*b, *c), "{sel} over {other} ({at})");
                    }
                    for (i, t) in ticks.iter().enumerate() {
                        assert!(no_overlap(*b, *t), "{sel} over tick {i} ({at})");
                    }
                }
                if hover.is_some() {
                    let popup = bounds(visual, "sum-chart-popup", at);
                    let zoom = f32::from(bounds(visual, "probe-col", at).size.width) / width;
                    assert!(
                        popup.origin.x >= px(-0.5)
                            && popup.right() <= px(width * zoom + 0.5)
                            && popup.origin.y >= px(-0.5),
                        "popup escaped the window ({at})"
                    );
                }
            },
        );
    }
}

/// A profitable period whose leader, losing coin, worst day and best hour all have facts, with
/// long synthetic names.
fn side_summary() -> Summary {
    let mut d = long_summary();
    d.cur.profit = 45_678.91;
    d.cur.pf = 1.21;
    d.strategies[0].name = "XSTRAT ".repeat(30).trim_end().to_string();
    d.coins = vec![GroupStat {
        key: "XVERYLONGCOINNAMEUSDT".into(),
        name: "XVERYLONGCOINNAMEUSDT".into(),
        n: 40,
        wins: 10,
        profit: -2_345.67,
        ..Default::default()
    }];
    d
}

/// Five synthetic trades with a long coin and a long strategy name.
fn side_trades() -> Vec<moon_core::db::analytics::TopTrade> {
    (0..5)
        .map(|i| moon_core::db::analytics::TopTrade {
            closedate: 1_800_000_000 + i * 3_600,
            coin: if i == 0 {
                "XVERYLONGCOINNAMEUSDT".to_string()
            } else {
                "XCOINUSDT".to_string()
            },
            strategy: "XALPHA ~ F-01 / SUB 01 LONG HIGH VOLUMES".to_string(),
            strategy_is_id: false,
            strategy_id: "7".to_string(),
            strategy_alive: Some(2),
            core_name: "XCORE-01".to_string(),
            profit: 12_345.67 - i as f64 * 1_000.0,
            is_short: i % 2 == 0,
        })
        .collect()
}

/// The side column: every insight row holds its tone stripe, a text column and the value in that
/// order; the category, subject and reason lines stay one line each and end before the value
/// column (the fixture's strategy name is far too long, so only an ellipsis passes), and every
/// value keeps its full width on one shared right edge. The trades table sits in its own scroll,
/// each coin cell holds the whole coin and stays clear of the profit cell. Letting the subject
/// wrap or overflow, or sizing each value column to its own text, breaks it.
#[gpui::test]
fn side_column_fits_every_language_theme_and_width(cx: &mut gpui::TestAppContext) {
    use super::super::side::{TradesActs, insights_el, period_caption, trades_el};
    use super::super::state::TradeSide;
    let trades = Rc::new(side_trades());
    probe(
        cx,
        |_| {
            let trades = trades.clone();
            Rc::new(move |p, cx| {
                let d = side_summary();
                let acts = TradesActs {
                    on_side: Rc::new(|_, _, _| {}),
                };
                v_flex()
                    .w_full()
                    .gap(px(8.0))
                    .child(insights_el(
                        &insights(&d, chrono_tz::UTC),
                        period_caption(&d),
                        p,
                        cx,
                    ))
                    .child(trades_el(
                        &trades,
                        TradeSide::Best,
                        chrono_tz::UTC,
                        &acts,
                        p,
                        cx,
                    ))
                    .into_any_element()
            })
        },
        |visual, at, _| {
            let col = bounds(visual, "probe-col", at);
            let card = bounds(visual, "sum-ins-card", at);
            assert!(inside(card, col), "insights card escaped ({at})");
            let items = insights(&side_summary(), chrono_tz::UTC);
            assert!(items.len() >= 3, "fixture lost its insights ({at})");
            let line_h = visual.update(|_, cx| f32::from(design::t_body(cx)) * 1.8);
            let mut value_right: Option<Pixels> = None;
            for (i, it) in items.iter().enumerate() {
                let get = |visual: &mut VisualTestContext, part: &str| {
                    bounds(visual, &format!("sum-ins-{part}-{i}"), at)
                };
                let row = get(visual, "row");
                let stripe = get(visual, "stripe");
                let text = get(visual, "text");
                let value = get(visual, "value");
                assert!(inside(row, card), "insight {i} escaped the card ({at})");
                for (name, b) in [("stripe", stripe), ("text", text), ("value", value)] {
                    assert!(inside(b, row), "insight {i} {name} escaped its row ({at})");
                }
                assert!(
                    stripe.right() <= text.origin.x + px(0.5)
                        && text.right() <= value.origin.x + px(0.5),
                    "insight {i} stripe, text and value out of order ({at})"
                );
                assert!(
                    (stripe.size.height - row.size.height).abs() < px(1.5),
                    "insight {i} stripe does not run the row's height ({at})"
                );
                // Loose default line boxes or roomy padding made five rows ~370 px tall and
                // pushed the side column far below the chart.
                let (tight, slack) = visual.update(|_, cx| {
                    (
                        f32::from(design::t_body(cx)) * MOCKUP_TIGHT_LINE,
                        f32::from(design::ui_px(cx, 16.0)),
                    )
                });
                assert!(
                    f32::from(row.size.height) <= 3.0 * tight + slack,
                    "insight {i} row {} px tall, over three tight lines ({at})",
                    f32::from(row.size.height)
                );
                let mut lines = vec!["cat", "what"];
                if it.why.is_some() {
                    lines.push("why");
                }
                for name in lines {
                    let b = get(visual, name);
                    assert!(
                        inside(b, text),
                        "insight {i} {name} escaped its text ({at})"
                    );
                    assert!(
                        b.right() <= value.origin.x + px(0.5),
                        "insight {i} {name} runs into the value column ({at})"
                    );
                    assert!(
                        f32::from(b.size.height) <= line_h,
                        "insight {i} {name} wrapped instead of ending in an ellipsis ({at})"
                    );
                    // A line box shorter than its own font clips the glyphs vertically.
                    let font = visual.update(|_, cx| {
                        f32::from(if name == "what" {
                            design::t_body(cx)
                        } else {
                            design::t_caption(cx)
                        })
                    });
                    assert!(
                        f32::from(b.size.height) + 0.5 >= font,
                        "insight {i} {name} clipped vertically ({at})"
                    );
                }
                let need = visual
                    .update(|_, cx| design::mono_body_text_width(cx, &it.value, 600.0) * 0.98);
                assert!(fits(value, need), "insight {i} value cut ({at})");
                let right = *value_right.get_or_insert(value.right());
                assert!(
                    (value.right() - right).abs() < px(0.5),
                    "insight {i} value off the shared right edge ({at})"
                );
            }
            let tcard = bounds(visual, "sum-trades-card", at);
            let title = bounds(visual, "sum-trades-title", at);
            let seg = bounds(visual, "sum-trades-seg", at);
            let scroll = bounds(visual, "sum-trades-scroll", at);
            let table = bounds(visual, "sum-trades-table", at);
            assert!(inside(tcard, col), "trades card escaped ({at})");
            assert!(
                inside(title, tcard) && inside(seg, tcard) && no_overlap(title, seg),
                "trades header crowded ({at})"
            );
            assert!(inside(scroll, tcard), "trades scroll escaped ({at})");
            assert!(
                (table.origin.x - scroll.origin.x).abs() < px(0.5),
                "table not anchored in its scroll ({at})"
            );
            for (k, tr) in side_trades().iter().enumerate() {
                let row = bounds(visual, &format!("sum-trades-row-{k}"), at);
                let coin = bounds(visual, &format!("sum-trades-coin-{k}"), at);
                let profit = bounds(visual, &format!("sum-trades-profit-{k}"), at);
                assert!(inside(row, table), "trade {k} escaped the table ({at})");
                assert!(
                    inside(coin, row) && inside(profit, row),
                    "trade {k} cell escaped its row ({at})"
                );
                assert!(
                    coin.right() <= profit.origin.x,
                    "trade {k} coin under profit ({at})"
                );
                // Drawn boxes, not the width the code reserved: the coin text, its badge and
                // the strategy cell must sit side by side whatever font the cells render in.
                let text = bounds(visual, &format!("sum-trades-coin-text-{k}"), at);
                let badge = bounds(visual, &format!("sum-trades-badge-{k}"), at);
                let strat = bounds(visual, &format!("sum-trades-strat-{k}"), at);
                assert!(
                    no_overlap(text, badge) && no_overlap(badge, strat) && no_overlap(text, strat),
                    "trade {k} coin runs into its strategy ({at})"
                );
                assert!(
                    text.right() <= badge.origin.x + px(0.5)
                        && badge.right() <= strat.origin.x + px(0.5),
                    "trade {k} coin, badge and strategy out of order ({at})"
                );
                // The close time stays on one line and clear of the coin: a wrapped time
                // grew taller than its row and printed over the next one.
                let closed = bounds(visual, &format!("sum-trades-closed-{k}"), at);
                assert!(
                    closed.size.height <= row.size.height + px(0.5)
                        && closed.right() <= coin.origin.x + px(0.5),
                    "trade {k} close time wrapped or ran into the coin ({at})"
                );
                let line = visual.update(|_, cx| f32::from(design::t_body(cx)) * 1.8);
                assert!(
                    f32::from(closed.size.height) <= line,
                    "trade {k} close time on two lines ({at})"
                );
                let need =
                    visual.update(|_, cx| design::mono_body_text_width(cx, &tr.coin, 400.0) * 0.98);
                assert!(fits(text, need), "trade {k} coin cut ({at})");
            }
        },
    );
}

/// Twenty synthetic cores with long names, winners and losers.
fn rank_cores() -> Vec<moon_core::db::analytics::CoreSeries> {
    (0..20u64)
        .map(|i| {
            let v = (i as f64 - 8.0) * 1_234.5;
            // Core 0 carries a far longer name, so the name column must grow for it.
            let name = if i == 0 {
                "XALPHA ~ F-00 / SUB 01 LONG HIGH VOLUMES WIDE".to_string()
            } else {
                format!("XALPHA ~ F-{i:02} / SUB 01 LONG VOLUMES")
            };
            moon_core::db::analytics::CoreSeries {
                uid: 900 + i,
                name,
                total: v,
                trades: 12_345,
                wins: 6_000,
                win_sum: v.max(0.0) + 100.0,
                loss_sum: (-v).max(0.0) + 100.0,
                ..Default::default()
            }
        })
        .collect()
}

/// The cores card: title clear of the filter, concentration captions inside the card, every
/// row's cells in order without overlap and inside the row, each bar inside its contribution
/// track, and the fold text clear of its caret, at every width, theme, zoom and language.
#[gpui::test]
fn cores_card_fits_every_language_theme_and_width(cx: &mut gpui::TestAppContext) {
    use super::super::cores::{CoresActs, RowModel, cores_el, row_models};
    use super::super::model::{Concentration, DivergingScale, core_rows};
    use super::super::state::{CoreFilter, CoreLines, SummaryUi};
    // `row_models` below translates text before the probe takes a locale: load the dictionary
    // on its big-stack thread first, or a lone run of this test overflows the default stack.
    crate::test_locale::warm();
    let data = rank_cores();
    let ui = SummaryUi {
        lines: CoreLines::Top5,
        ..Default::default()
    };
    let rows = core_rows(&data, CoreFilter::All, false);
    let scale = DivergingScale::of(&data);
    let lines = row_models(&data, &rows, &ui, &scale);
    {
        let (data, ui, rows) = (data.clone(), ui.clone(), rows.clone());
        probe(
            cx,
            move |_| {
                let (data, ui, rows) = (data.clone(), ui.clone(), rows.clone());
                Rc::new(move |p, cx| {
                    let acts = CoresActs {
                        on_pick: Rc::new(|_, _, _| {}),
                        on_filter: Rc::new(|_, _, _| {}),
                        on_fold: Rc::new(|_, _, _| {}),
                    };
                    cores_el(
                        &data,
                        &rows,
                        &DivergingScale::of(&data),
                        Concentration::of(&data).as_ref(),
                        &ui,
                        &[],
                        &acts,
                        p,
                        cx,
                    )
                })
            },
            |visual, at, _| {
                let col = bounds(visual, "probe-col", at);
                let card = bounds(visual, "sum-cores-card", at);
                let title = bounds(visual, "sum-cores-title", at);
                let filter = bounds(visual, "sum-cores-filter", at);
                let scroll = bounds(visual, "sum-cores-scroll", at);
                let table = bounds(visual, "sum-cores-table", at);
                assert!(inside(card, col), "cores card escaped ({at})");
                assert!(
                    inside(title, card) && inside(filter, card) && no_overlap(title, filter),
                    "title under the filter ({at})"
                );
                assert!(inside(scroll, card), "table scroll escaped ({at})");
                assert!(
                    inside(bounds(visual, "sum-cores-conc-bar", at), card),
                    "concentration bar escaped ({at})"
                );
                for i in 0..4 {
                    let cap = bounds(visual, &format!("sum-cores-conc-cap-{i}"), at);
                    assert!(
                        inside(cap, card),
                        "concentration caption {i} escaped ({at})"
                    );
                }
                for (k, line) in lines.iter().enumerate() {
                    if let RowModel::Fold { .. } = line {
                        let fold = bounds(visual, "sum-cores-fold", at);
                        let text = bounds(visual, "sum-cores-fold-text", at);
                        let btn = bounds(visual, "sum-cores-fold-btn", at);
                        assert!(inside(fold, table), "fold row escaped ({at})");
                        assert!(
                            no_overlap(text, btn) && inside(btn, fold),
                            "fold text under its caret ({at})"
                        );
                        continue;
                    }
                    let row = bounds(visual, &format!("sum-cores-row-{k}"), at);
                    assert!(inside(row, table), "core row {k} escaped the table ({at})");
                    // The drawn name, not its reserved column, must end before the next column.
                    let name = bounds(visual, &format!("sum-cores-name-text-{k}"), at);
                    let trades = bounds(visual, &format!("sum-cores-trades-{k}"), at);
                    assert!(
                        no_overlap(name, trades) && name.right() <= trades.origin.x + px(0.5),
                        "row {k} name runs into the trades column ({at})"
                    );
                    let cells: Vec<_> = ["name", "trades", "wr", "pf", "contrib", "profit"]
                        .iter()
                        .map(|c| bounds(visual, &format!("sum-cores-{c}-{k}"), at))
                        .collect();
                    for (j, c) in cells.iter().enumerate() {
                        assert!(inside(*c, row), "row {k} cell {j} escaped ({at})");
                        if j > 0 {
                            assert!(
                                cells[j - 1].right() <= c.origin.x + px(0.5),
                                "row {k} cells {} and {j} overlap ({at})",
                                j - 1
                            );
                        }
                    }
                    let bar = bounds(visual, &format!("sum-cores-bar-{k}"), at);
                    assert!(
                        bar.origin.x + px(0.5) >= cells[4].origin.x
                            && bar.right() <= cells[4].right() + px(0.5),
                        "row {k} bar escaped its track ({at})"
                    );
                }
            },
        );
    }
}

/// The page faces the probe composes like `summary_tab` does.
#[derive(Clone, Copy, Debug)]
enum PageFace {
    Data,
    CatchUp,
    Loading,
    Empty,
    Error,
}

/// Build one Summary face exactly as `summary_tab` assembles it, at one design width.
fn page(face: PageFace, width: f32) -> Rc<dyn Fn(MoonPalette, &gpui::App) -> AnyElement> {
    use super::super::cores::{CoresActs, cores_el};
    use super::super::model::{Concentration, DivergingScale, core_rows};
    use super::super::profit_chart::{ChartActs, ProfitChartModel, profit_chart_el};
    use super::super::side::{TradesActs, insights_el, period_caption, trades_el};
    use super::super::state::{CoreFilter, SummaryUi, TradeSide};
    use super::super::states::{
        DataBlocks, StateActs, catchup_banner_el, data_page_el, empty_el, error_el, skeleton_el,
    };
    let data = chart_summary();
    let trades = Rc::new(side_trades());
    Rc::new(move |p, cx| {
        let cols = kpi_cols(width, 1.0);
        let acts = StateActs {
            on_show_year: Rc::new(|_, _| {}),
            on_reset: Rc::new(|_, _| {}),
            on_retry: Rc::new(|_, _| {}),
        };
        let body = match face {
            PageFace::Loading => skeleton_el(cols, p, cx),
            PageFace::Empty => empty_el(
                Some(30),
                "“Side: Long”, “XALPHA ~ F-01 / SUB 01”".to_string(),
                Some(("XSCOPE".to_string(), "tip".to_string())),
                &acts,
                p,
                cx,
            ),
            PageFace::Error => error_el(
                moon_core::db::FailKind::Busy,
                "data/reports.sqlite".into(),
                "summary".into(),
                moon_core::db::FailCode::None,
                &acts,
                p,
                cx,
            ),
            PageFace::Data | PageFace::CatchUp => {
                let derived = super::super::derived::derive_for_test(data.clone(), p);
                let ui = SummaryUi::default();
                let rows = core_rows(&data.core_days, CoreFilter::All, false);
                let blocks = DataBlocks {
                    banner: matches!(face, PageFace::CatchUp)
                        .then(|| catchup_banner_el("12:04".to_string())),
                    kpi: kpi_band_el(&band(true), cols, p, cx),
                    chart: profit_chart_el(
                        &ProfitChartModel::new(&derived, &ui, chrono_tz::UTC, None, p),
                        None,
                        &ChartActs {
                            on_lines: Rc::new(|_, _, _| {}),
                            on_area: Rc::new(|_, _, _| {}),
                            on_bucket: Rc::new(|_, _, _| {}),
                            on_popup: Rc::new(|_, _, _, _| {}),
                        },
                        p,
                        cx,
                    ),
                    insights: insights_el(
                        &insights(&side_summary(), chrono_tz::UTC),
                        period_caption(&data),
                        p,
                        cx,
                    ),
                    trades: trades_el(
                        &trades,
                        TradeSide::Best,
                        chrono_tz::UTC,
                        &TradesActs {
                            on_side: Rc::new(|_, _, _| {}),
                        },
                        p,
                        cx,
                    ),
                    cores: cores_el(
                        &data.core_days,
                        &rows,
                        &DivergingScale::of(&data.core_days),
                        Concentration::of(&data.core_days).as_ref(),
                        &ui,
                        &[],
                        &CoresActs {
                            on_pick: Rc::new(|_, _, _| {}),
                            on_filter: Rc::new(|_, _, _| {}),
                            on_fold: Rc::new(|_, _, _| {}),
                        },
                        p,
                        cx,
                    ),
                };
                return data_page_el(cols, blocks, cx).into_any_element();
            }
        };
        gpui::div()
            .w_full()
            .p(design::ui_px(cx, 10.0))
            .child(body)
            .into_any_element()
    })
}

/// Every Summary face composes without overlap: on the data page the banner, KPI band, main row
/// and cores card stack in order inside the tab, the chart sits beside the side column when wide
/// and above it below 1100, the side cards pair at 900 and stack at 600, and the KPI band follows
/// its 4 / 2x2 / 1 layout; the empty and error buttons are never narrower than their labels and
/// the banner keeps its text inside — in every theme, language and zoom.
fn assert_summary_tab_blocks_never_overlap(cx: &mut gpui::TestAppContext, face: PageFace) {
    probe(
        cx,
        |width| page(face, width),
        |visual, at, width| {
            let at = format!("{at} {face:?}");
            let at = at.as_str();
            let col = bounds(visual, "probe-col", at);
            match face {
                PageFace::Data | PageFace::CatchUp => {
                    let page = bounds(visual, "sum-page", at);
                    assert!(inside(page, col), "page escaped the tab ({at})");
                    let mut stack = Vec::new();
                    if matches!(face, PageFace::CatchUp) {
                        stack.push(("banner", bounds(visual, "sum-banner", at)));
                    }
                    stack.push(("kpi", bounds(visual, "sum-page-kpi", at)));
                    stack.push(("main", bounds(visual, "sum-main", at)));
                    stack.push(("cores", bounds(visual, "sum-page-cores", at)));
                    for (k, (name, b)) in stack.iter().enumerate() {
                        assert!(inside(*b, page), "{name} escaped the page ({at})");
                        if k > 0 {
                            let (prev, a) = stack[k - 1];
                            assert!(
                                a.bottom() <= b.origin.y + px(0.5),
                                "{prev} runs into {name} ({at})"
                            );
                        }
                    }
                    let main = bounds(visual, "sum-main", at);
                    let chart = bounds(visual, "sum-main-chart", at);
                    let side = bounds(visual, "sum-side", at);
                    let ins = bounds(visual, "sum-ins-card", at);
                    let tr = bounds(visual, "sum-trades-card", at);
                    for (name, b) in [
                        ("chart", chart),
                        ("side", side),
                        ("insights", ins),
                        ("trades", tr),
                    ] {
                        assert!(inside(b, main), "{name} escaped the main row ({at})");
                    }
                    assert!(no_overlap(chart, side), "chart over the side column ({at})");
                    assert!(no_overlap(ins, tr), "insights over trades ({at})");
                    match kpi_cols(width, 1.0) {
                        KpiCols::Four => {
                            assert!(
                                chart.right() <= side.origin.x + px(0.5),
                                "wide: side not beside the chart ({at})"
                            );
                            // A row that aligns its columns to the top (or a chart slot
                            // that is not a column) leaves a hole under the chart card
                            // whenever the side column is the taller one.
                            let card = bounds(visual, "sum-chart-card", at);
                            assert!(
                                (card.bottom() - side.bottom()).abs() <= px(2.0)
                                    && card.bottom() + px(2.0) >= tr.bottom(),
                                "wide: chart card ends {} above the side column ({at})",
                                f32::from(side.bottom() - card.bottom())
                            );
                            let plot = bounds(visual, "sum-chart-area", at);
                            assert!(
                                inside(plot, card),
                                "wide: grown plot escaped its card ({at})"
                            );
                            assert!(
                                chart.size.width > side.size.width,
                                "wide: the chart must take the larger share ({at})"
                            );
                        }
                        KpiCols::Two => {
                            assert!(
                                chart.bottom() <= side.origin.y + px(0.5),
                                "side not under the chart ({at})"
                            );
                            assert!(
                                ins.right() <= tr.origin.x + px(0.5),
                                "side cards not paired ({at})"
                            );
                        }
                        KpiCols::One => {
                            assert!(
                                chart.bottom() <= side.origin.y + px(0.5),
                                "side not under the chart ({at})"
                            );
                            assert!(
                                ins.bottom() <= tr.origin.y + px(0.5),
                                "side cards not stacked ({at})"
                            );
                        }
                    }
                    let g: Vec<_> = (0..4).map(|i| bounds(visual, &kpi_group(i), at)).collect();
                    match kpi_cols(width, 1.0) {
                        KpiCols::Four => assert!(
                            (1..4).all(|i| g[i].origin.y == g[0].origin.y),
                            "KPI not in one row ({at})"
                        ),
                        KpiCols::Two => assert!(
                            g[1].origin.y == g[0].origin.y && g[2].origin.y > g[0].origin.y,
                            "KPI not 2x2 ({at})"
                        ),
                        KpiCols::One => assert!(
                            (1..4).all(|i| g[i].origin.y > g[i - 1].origin.y),
                            "KPI not stacked ({at})"
                        ),
                    }
                    if matches!(face, PageFace::CatchUp) {
                        let banner = bounds(visual, "sum-banner", at);
                        assert!(inside(banner, page), "banner escaped ({at})");
                    }
                }
                PageFace::Loading => {
                    let sk = bounds(visual, "sum-skeleton", at);
                    let cap = bounds(visual, "sum-skeleton-caption", at);
                    assert!(inside(sk, col), "skeleton escaped ({at})");
                    assert!(inside(cap, sk), "loading caption escaped ({at})");
                }
                PageFace::Empty | PageFace::Error => {
                    let sel = if matches!(face, PageFace::Empty) {
                        "sum-state-empty"
                    } else {
                        "sum-state-error"
                    };
                    let frame = bounds(visual, sel, at);
                    let title = bounds(visual, "sum-state-title", at);
                    let body = bounds(visual, "sum-state-body", at);
                    assert!(inside(frame, col), "state escaped ({at})");
                    assert!(
                        inside(title, frame) && inside(body, frame),
                        "state text escaped ({at})"
                    );
                    assert!(
                        title.bottom() <= body.origin.y + px(0.5),
                        "title over body ({at})"
                    );
                    let labels: Vec<String> = if matches!(face, PageFace::Empty) {
                        vec![
                            t!("analytics.sum.empty.show_year").to_string(),
                            t!("analytics.sum.empty.reset").to_string(),
                        ]
                    } else {
                        vec![t!("analytics.sum.error.retry").to_string()]
                    };
                    for (k, label) in labels.iter().enumerate() {
                        let btn = bounds(visual, &format!("sum-state-btn-{k}"), at);
                        assert!(inside(btn, frame), "button {k} escaped ({at})");
                        let need = visual.update(|_, cx| {
                            // A MoonButton label renders at its tier's size under the UI zoom only.
                            design::ui_text_width_zoomed(cx, label, design::BODY_TEXT, 500.0, false)
                        });
                        assert!(
                            fits(btn, need),
                            "button {k} narrower than {label:?} {} vs {need} ({at})",
                            f32::from(btn.size.width)
                        );
                    }
                }
            }
        },
    );
}

/// A data-page geometry regression must fail the complete language/theme/width probe.
#[gpui::test]
fn summary_tab_data_never_overlaps_every_language_theme_and_width(cx: &mut gpui::TestAppContext) {
    assert_summary_tab_blocks_never_overlap(cx, PageFace::Data);
}

/// A catch-up banner must not overlap the data page in any presentation.
#[gpui::test]
fn summary_tab_catch_up_never_overlaps_every_language_theme_and_width(
    cx: &mut gpui::TestAppContext,
) {
    assert_summary_tab_blocks_never_overlap(cx, PageFace::CatchUp);
}

/// Loading captions must stay inside their skeleton in every presentation.
#[gpui::test]
fn summary_tab_loading_never_overlaps_every_language_theme_and_width(
    cx: &mut gpui::TestAppContext,
) {
    assert_summary_tab_blocks_never_overlap(cx, PageFace::Loading);
}

/// Empty-state text and actions must fit without overlap in every presentation.
#[gpui::test]
fn summary_tab_empty_never_overlaps_every_language_theme_and_width(cx: &mut gpui::TestAppContext) {
    assert_summary_tab_blocks_never_overlap(cx, PageFace::Empty);
}

/// Error-state text and retry actions must fit without overlap in every presentation.
#[gpui::test]
fn summary_tab_error_never_overlaps_every_language_theme_and_width(cx: &mut gpui::TestAppContext) {
    assert_summary_tab_blocks_never_overlap(cx, PageFace::Error);
}

/// With the comparison off a profitable period still shows "N% of profit" under the drawdown:
/// the fact is not a comparison. A caption builder that drops the extra fact whenever there is
/// no change line silently hides it the moment compare is switched off.
#[gpui::test]
fn risk_keeps_dd_of_profit_without_a_comparison(cx: &mut gpui::TestAppContext) {
    let summary = || {
        let mut d = long_summary();
        d.cur.profit = 12_345.67;
        d
    };
    let build = move |width| -> Rc<dyn Fn(MoonPalette, &gpui::App) -> AnyElement> {
        let cols = kpi_cols(width, 1.0);
        Rc::new(move |p, cx| {
            let m = KpiBand::new(&summary(), Rc::from(vec![0.0_f32; 2]), false);
            kpi_band_el(&m, cols, p, cx)
        })
    };
    probe(cx, build, |visual, at, _| {
        let m = KpiBand::new(&summary(), Rc::from(vec![0.0_f32; 2]), false);
        assert!(
            m.risk.dd_line.is_none() && m.risk.dd_of_profit.is_some(),
            "{at}"
        );
        let dd = bounds(visual, &kpi_delta("dd"), at);
        assert!(dd.size.width > px(0.0), "dd_of_profit line is empty ({at})");
    });
}

/// Five long core names at a 1300 px card show whole in the legend, beside the total and the
/// drawdown labels: no name is shortened, no two entries overlap, and the legend wraps rather
/// than squeezing, in every language. A truncating name sized by its content collapses to "…".
#[gpui::test]
fn profit_legend_keeps_five_long_names_whole(cx: &mut gpui::TestAppContext) {
    use super::super::profit_chart::{ChartActs, ProfitChartModel, profit_chart_el};
    use super::super::state::{CoreLines, SummaryUi};
    let mut d = (*chart_summary()).clone();
    for (i, c) in d.core_days.iter_mut().enumerate() {
        c.name = format!("CORE_A{i} ~ EX-XX / ALPHA_GRID LONG VOLUMES (UNIT_0{i})");
    }
    let data = std::sync::Arc::new(d);
    let picked: BTreeSet<u64> = data.core_days[..5].iter().map(|c| c.uid).collect();
    probe_widths(
        cx,
        &[1300.0],
        usdt,
        |_| {
            let (data, picked) = (data.clone(), picked.clone());
            Rc::new(move |p, cx| {
                let derived = super::super::derived::derive_for_test(data.clone(), p);
                let ui = SummaryUi {
                    lines: CoreLines::Selected,
                    picked: picked.clone(),
                    ..Default::default()
                };
                let m = ProfitChartModel::new(&derived, &ui, chrono_tz::UTC, None, p);
                let acts = ChartActs {
                    on_lines: Rc::new(|_, _, _| {}),
                    on_area: Rc::new(|_, _, _| {}),
                    on_bucket: Rc::new(|_, _, _| {}),
                    on_popup: Rc::new(|_, _, _, _| {}),
                };
                profit_chart_el(&m, None, &acts, p, cx)
            })
        },
        |visual, at, _| {
            let card = bounds(visual, "sum-chart-card", at);
            let total = super::super::fmt_signed(data.days.iter().map(|d| d.profit).sum());
            let mut texts = vec![t!("analytics.sum.chart.total", v = total).to_string()];
            texts.extend(data.core_days[..5].iter().map(|c| c.name.clone()));
            texts.push(t!("analytics.sum.chart.drawdown").to_string());
            assert!(
                !texts[0].trim().is_empty() && !texts[6].trim().is_empty(),
                "{at}"
            );
            let items: Vec<_> = (0..texts.len())
                .map(|i| bounds(visual, &format!("sum-chart-legend-{i}"), at))
                .collect();
            for (i, text) in texts.iter().enumerate() {
                let name = bounds(visual, &format!("sum-chart-legend-name-{i}"), at);
                let need = visual.update(|_, cx| design::mono_caption_text_width(cx, text, 400.0));
                assert!(fits(name, need), "legend name {i} shortened ({at})");
                assert!(inside(items[i], card), "legend entry {i} escaped ({at})");
                for (j, other) in items.iter().enumerate().skip(i + 1) {
                    assert!(no_overlap(items[i], *other), "legend {i} over {j} ({at})");
                }
            }
            let rows = items
                .iter()
                .map(|b| b.origin.y)
                .fold(Vec::new(), |mut v, y| {
                    if !v.iter().any(|r: &Pixels| (*r - y).abs() < px(1.0)) {
                        v.push(y);
                    }
                    v
                });
            assert!(rows.len() > 1, "legend did not wrap ({at})");
        },
    );
}

/// A legend entry whose core name is wider than the card stays inside the card: the name
/// shortens beside its swatch while the value keeps its whole text, at every width. An entry
/// that cannot shrink pushes the value, or the whole entry, past the card's right edge.
#[gpui::test]
fn profit_legend_shrinks_a_very_long_name(cx: &mut gpui::TestAppContext) {
    use super::super::profit_chart::{ChartActs, ProfitChartModel, profit_chart_el};
    use super::super::state::{CoreLines, SummaryUi};
    let mut d = (*chart_summary()).clone();
    d.core_days[0].name = "XALPHA F-XX / SUB 00 LONG VOLUMES ".repeat(6);
    let data = std::sync::Arc::new(d);
    let picked: BTreeSet<u64> = [data.core_days[0].uid].into_iter().collect();
    probe(
        cx,
        |_| {
            let (data, picked) = (data.clone(), picked.clone());
            Rc::new(move |p, cx| {
                let derived = super::super::derived::derive_for_test(data.clone(), p);
                let ui = SummaryUi {
                    lines: CoreLines::Selected,
                    picked: picked.clone(),
                    ..Default::default()
                };
                let m = ProfitChartModel::new(&derived, &ui, chrono_tz::UTC, None, p);
                let acts = ChartActs {
                    on_lines: Rc::new(|_, _, _| {}),
                    on_area: Rc::new(|_, _, _| {}),
                    on_bucket: Rc::new(|_, _, _| {}),
                    on_popup: Rc::new(|_, _, _, _| {}),
                };
                profit_chart_el(&m, None, &acts, p, cx)
            })
        },
        |visual, at, _| {
            let card = bounds(visual, "sum-chart-card", at);
            let item = bounds(visual, "sum-chart-legend-1", at);
            let name = bounds(visual, "sum-chart-legend-name-1", at);
            let value = bounds(visual, "sum-chart-legend-value-1", at);
            assert!(inside(item, card), "legend entry escaped the card ({at})");
            assert!(
                inside(value, card) && no_overlap(name, value),
                "legend value pushed out or under the name ({at})"
            );
            let text = super::super::fmt_signed(data.core_days[0].total);
            let need = visual.update(|_, cx| design::mono_caption_text_width(cx, &text, 400.0));
            assert!(fits(value, need * 0.98), "legend value cut ({at})");
        },
    );
}
