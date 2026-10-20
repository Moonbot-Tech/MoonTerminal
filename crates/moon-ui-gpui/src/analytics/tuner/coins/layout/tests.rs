use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, Styled, Window, px,
};
use moon_core::config::{Language, UiThemeMode};
use moon_core::db::ReadFail;
use moon_core::db::metrics::Tally;
use moon_core::db::tuner::VarStats;
use moon_core::db::tuner::coin_pick::{CoinPick, CoinSuggestion, NoiseReason, PickVerdict};
use moon_ui::{MoonInputState, MoonPalette, MoonTheme, v_flex};
use rust_i18n::t;

use super::super::super::kpi::{VarLabel, kpi_matrix_body};
use super::super::super::shared::TunerKind;
use super::super::super::shell::{ToolbarActs, ToolbarWrite, toolbar_el};
use super::super::auto::{
    AutoCardActs, AutoCardModel, AutoView, ResultModel, auto_card_el, tag_line, verdict_line,
};
use super::super::coin_kpi_labels;
use super::super::fields::{
    ChipKind, FieldChip, ListFieldActs, ListFieldModel, list_field_el, pending_band_el,
    pending_text,
};
use super::super::filter_bar::{FilterBarActs, FilterBarModel, filter_bar_el};
use super::super::rows::CoinCounts;
use super::super::state::{CoinFilter, CoinsState, MAX_TOKEN_LEN, PendingDelta, PickStaleCause};
use crate::design;
use crate::load_state::LoadState;

/// Width of the By coin right column, in font px.
const COLUMN_W: f32 = 470.0;

/// The faces the probe walks. Face 0 is the mode without a strategy: empty-state fields, a
/// toolbar without Make a copy / Save, the pick settings disabled. The others carry a
/// strategy and differ in the auto-pick card: running, a useful result, a noise result
/// (stale), a failed read.
const FACES: usize = 5;

/// A saved entry far longer than a field edit accepts (`MAX_TOKEN_LEN`): saved lists are
/// written by the core, not by the field, so the chip itself must keep its x inside.
fn long_token() -> String {
    format!("X{}", "W".repeat(3 * MAX_TOKEN_LEN))
}

fn chip(token: &str, kind: ChipKind) -> FieldChip {
    FieldChip {
        token: token.into(),
        text: token.to_string().into(),
        tip: "tip".into(),
        id: format!("an-pick-chip-{token}").into(),
        kind,
        alpha: 1.0,
    }
}

fn state() -> CoinsState {
    CoinsState {
        filter: CoinFilter::All,
        min_trades: 5,
        kpi_bl: 3,
        kpi_wl: 1,
        kpi_auto: Some((5, 0)),
        ..Default::default()
    }
}

fn counts() -> CoinCounts {
    CoinCounts {
        all: 412,
        black: 3,
        black_traded: 2,
        white: 1,
        changed: 2,
        hidden: 37,
        suggested: 4,
    }
}

fn delta() -> PendingDelta {
    PendingDelta {
        added_bl: vec!["XAAA".to_string(), "XBBB".to_string()],
        removed_bl: vec!["XCCC".to_string()],
        added_wl: Vec::new(),
        removed_wl: vec!["XDDD".to_string()],
    }
}

/// The short entries every length check reads.
fn short_chips() -> Vec<FieldChip> {
    vec![
        chip("XAAA", ChipKind::Saved),
        chip("XBBB", ChipKind::Added),
        chip("XCCC", ChipKind::Removed),
    ]
}

fn black_field(face: usize) -> ListFieldModel {
    if face == 0 {
        return ListFieldModel::new(true, None);
    }
    let mut chips = short_chips();
    chips.push(chip(&long_token(), ChipKind::Saved));
    ListFieldModel::new(true, Some(chips))
}

fn white_field(face: usize) -> ListFieldModel {
    ListFieldModel::new(false, (face != 0).then(Vec::new))
}

/// A synthetic finished pick with the given verdict.
fn pick(verdict: PickVerdict) -> CoinPick {
    let tally = |profit: f64| {
        let mut t = Tally::default();
        t.push(profit);
        t
    };
    let coin = |token: &str, n: i64, profit: f64| CoinSuggestion {
        token: token.to_string(),
        n,
        profit,
    };
    CoinPick {
        black: vec![coin("XAAA", 48, -995.64), coin("XBBB", 12, -120.0)],
        white: vec![coin("XEEE", 64, 410.2)],
        train_n: 700,
        holdout_n: 300,
        fact_holdout: tally(-130.0),
        manual_holdout: tally(-80.0),
        pick_holdout: tally(20.5),
        verdict,
    }
}

/// The auto card in face `face`: settings without a strategy, running, a useful result,
/// a noise result, a failed read.
fn auto_model(face: usize) -> AutoCardModel {
    let s = state();
    let mut m = AutoCardModel::new(&s, face != 0, false, true);
    m.view = match face {
        0 => AutoView::Settings,
        1 => AutoView::Running {
            line: t!("analytics.coins.running", pct = 30).to_string(),
        },
        2 | 3 => {
            let p = pick(if face == 2 {
                PickVerdict::Useful
            } else {
                PickVerdict::Noise(NoiseReason::SmallGain)
            });
            AutoView::Result(ResultModel {
                line: verdict_line(&p, 30),
                useful: face == 2,
                black: p.black.iter().map(tag_line).collect(),
                white: p.white.iter().map(tag_line).collect(),
            })
        }
        _ => AutoView::Failed(ReadFail::NotReady),
    };
    m.stale = (face == 3).then_some(PickStaleCause::Lists);
    m
}

fn kpi_stats() -> LoadState<Vec<VarStats>> {
    let col = |n: i64, profit: f64| VarStats {
        n,
        wins: n / 2,
        profit,
        pf: 1.4,
        avg: profit / n as f64,
        avg_win: 12.5,
        avg_loss: 8.25,
        avg_spent: 100.0,
        max_dd: -310.75,
    };
    LoadState::Ready(Arc::new(vec![col(1250, -412.6), col(1180, 96.4)]))
}

/// The right column's production pieces, backend-free, at the column's width.
struct CoinsLayoutProbe {
    face: usize,
    search: Entity<MoonInputState>,
    min_trades: Entity<MoonInputState>,
    black_in: Entity<MoonInputState>,
    white_in: Entity<MoonInputState>,
    pick_min: Entity<MoonInputState>,
    pick_hold: Entity<MoonInputState>,
}

impl Render for CoinsLayoutProbe {
    /// Stack every coin control of the right column, as the view does, in one column.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let noop: Rc<dyn Fn(&mut Window, &mut gpui::App)> = Rc::new(|_, _| {});
        let field_acts = ListFieldActs {
            on_remove: Rc::new(|_, _, _| {}),
            on_restore: Rc::new(|_, _, _| {}),
            on_copy: noop.clone(),
            on_clear: noop.clone(),
        };
        let bar_acts = FilterBarActs {
            on_filter: Rc::new(|_, _, _| {}),
            on_hide_zero: Rc::new(|_, _, _| {}),
        };
        let auto_acts = AutoCardActs {
            on_pick: noop.clone(),
            on_reset: noop.clone(),
            on_one_by_one: noop.clone(),
            on_add: Rc::new(|_, _, _| {}),
            on_black: Rc::new(|_, _, _| {}),
            on_white: Rc::new(|_, _, _| {}),
        };
        let s = state();
        let base = VarLabel::new(t!("analytics.tuner.fact").to_string());
        let toolbar_acts = ToolbarActs {
            on_copy: noop.clone(),
            on_save: noop.clone(),
        };
        let write = ToolbarWrite {
            enabled: true,
            copy: true,
            dirty: true,
        };
        let face = self.face;
        let pieces: Vec<AnyElement> = vec![
            filter_bar_el(
                &FilterBarModel::new(&s, counts(), &[]),
                &self.search,
                &self.min_trades,
                &bar_acts,
                p,
                cx,
            ),
            kpi_matrix_body(
                &kpi_stats(),
                "XSTRAT".to_string(),
                &base,
                &coin_kpi_labels(&s),
                false,
                noop.clone(),
                p,
                cx,
            ),
            auto_card_el(
                &auto_model(self.face),
                Some((&self.pick_min, &self.pick_hold)),
                &auto_acts,
                p,
                cx,
            ),
            toolbar_el(
                TunerKind::Coins,
                t!("analytics.tuner.fields_title").to_string(),
                (face != 0).then_some(&write),
                &toolbar_acts,
                cx,
            ),
            list_field_el(
                &black_field(face),
                (face != 0).then_some(&self.black_in),
                &field_acts,
                p,
                cx,
            ),
            list_field_el(
                &white_field(face),
                (face != 0).then_some(&self.white_in),
                &field_acts,
                p,
                cx,
            ),
            pending_band_el(pending_text(&delta()).unwrap_or_default(), noop, p, cx),
        ];
        v_flex()
            .w_full()
            .debug_selector(|| "probe-col".into())
            .children(pieces)
    }
}

/// `debug_bounds` takes a `'static` selector; the probe's selectors are built per run.
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// One text that must be drawn whole: the selector of its box and its minimum width.
struct Need {
    sel: String,
    width: f32,
}

fn caption(cx: &gpui::App, sel: &str, text: &str) -> Need {
    Need {
        sel: sel.to_string(),
        width: design::ui_caption_text_width(cx, text, 400.0),
    }
}

/// What the probe asserts per language: every one-line text whose box must not clip it.
fn needs(cx: &gpui::App, face: usize) -> Vec<Need> {
    let mut out = vec![
        caption(cx, "an-coin-hide-lbl", &t!("analytics.coins.hide_lbl")),
        caption(
            cx,
            "an-coin-pending-lbl",
            &t!("analytics.coins.pending_band"),
        ),
        caption(cx, "an-coin-revert", &t!("analytics.coins.revert_edits")),
        caption(cx, "an-coin-bl-copy", &t!("analytics.coins.copy")),
        caption(cx, "an-coin-bl-clear", &t!("analytics.coins.clear")),
        caption(cx, "an-coin-wl-copy", &t!("analytics.coins.copy")),
        caption(cx, "an-coin-wl-clear", &t!("analytics.coins.clear")),
    ];
    for side in [black_field(face), white_field(face)] {
        out.push(Need {
            sel: format!("an-coin-{}-title", if side.black { "bl" } else { "wl" }),
            width: design::ui_body_text_width(cx, &side.title, 600.0),
        });
    }
    if face != 0 {
        for c in short_chips() {
            out.push(Need {
                sel: c.id.to_string(),
                width: design::mono_caption_text_width(cx, &c.text, 400.0),
            });
        }
        out.push(caption(cx, "tun-copy-c", &t!("analytics.tuner.copy_btn")));
        out.push(caption(cx, "tun-save-c", &t!("analytics.tuner.save_btn")));
    }
    for c in FilterBarModel::new(&state(), counts(), &[]).chips {
        out.push(caption(cx, c.id, &c.label));
    }
    let auto: &[(&str, &str)] = match face {
        0 => &[
            ("an-pick-min", "analytics.coins.pick_min_lbl"),
            ("an-pick-hold", "analytics.coins.pick_hold_lbl"),
            ("an-pick-bl-ctl", "analytics.coins.pick_bl"),
            ("an-pick-wl-ctl", "analytics.coins.pick_wl"),
            ("an-pick-run", "analytics.coins.pick_btn"),
        ],
        1 => &[("an-pick-stop", "analytics.coins.stop_btn")],
        2 => &[
            ("an-pick-add-bl", "analytics.coins.add_bl"),
            ("an-pick-add-wl", "analytics.coins.add_wl"),
            ("an-pick-one", "analytics.coins.one_by_one"),
            ("an-pick-reset", "analytics.coins.reset_btn"),
        ],
        3 => &[
            ("an-pick-anyway", "analytics.coins.add_anyway"),
            ("an-pick-reset", "analytics.coins.reset_btn"),
        ],
        _ => &[("an-pick-reset", "analytics.coins.reset_btn")],
    };
    for (sel, key) in auto {
        out.push(caption(cx, sel, &t!(*key)));
    }
    out
}

/// Every analytics.coins key of one language's dictionary file.
fn coin_keys(code: &str) -> std::collections::BTreeSet<String> {
    let path = format!(
        "{}/../../locales/{code}/analytics.{code}.yml",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{path}: {e}"))
        .lines()
        .filter_map(|l| l.split_once(':').map(|(k, _)| k.trim().to_string()))
        .filter(|k| k.starts_with("analytics.coins."))
        .collect()
}

/// Every language's dictionary carries exactly the English coin keys, and no model string
/// the column draws is a raw key. The keys are read from the files themselves: at runtime a
/// missing translation falls back to English, so asking `t!` could never catch one.
#[test]
fn no_coin_text_is_a_raw_key() {
    let en = coin_keys("en");
    assert!(en.len() > 50, "coin keys not found");
    for language in Language::ALL {
        let keys = coin_keys(language.code());
        let missing: Vec<_> = en.difference(&keys).collect();
        let extra: Vec<_> = keys.difference(&en).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "{}: missing {missing:?}, extra {extra:?}",
            language.code()
        );
        let _locale = crate::test_locale::force(language.code());
        let bar = FilterBarModel::new(&state(), counts(), &[]);
        let mut drawn: Vec<String> = bar.chips.iter().map(|c| c.label.clone()).collect();
        drawn.push(bar.caption.clone());
        for m in [
            black_field(0),
            white_field(0),
            black_field(1),
            white_field(1),
        ] {
            drawn.push(m.title.clone());
            drawn.extend(m.legend.iter().cloned());
            drawn.extend(m.empty.iter().cloned());
        }
        for face in 0..FACES {
            let m = auto_model(face);
            drawn.extend(m.hint);
            if let AutoView::Result(r) = m.view {
                drawn.push(r.line);
                drawn.extend(r.black);
                drawn.extend(r.white);
            }
        }
        for l in coin_kpi_labels(&state()) {
            drawn.push(l.title.clone());
            drawn.extend(l.sub.clone());
            drawn.extend(l.tip.clone());
        }
        drawn.extend(pending_text(&delta()));
        for text in drawn {
            assert!(
                !text.contains("analytics."),
                "{}: raw key in {text}",
                language.code()
            );
        }
    }
}

/// Every coin control of the right column stays inside the column and is never narrower
/// than its own text, in both themes, every language and both zooms. Shrinking a button
/// box, letting a label flex-shrink or widening a fixed KPI column breaks it.
#[gpui::test]
fn the_right_column_fits_every_language_theme_and_zoom(cx: &mut gpui::TestAppContext) {
    for mode in [UiThemeMode::Light, UiThemeMode::Dark] {
        cx.update(|cx| {
            MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(mode, 1.0),
                cx,
            )
        });
        for language in Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            for zoom in [1.0, 1.25] {
                for face in 0..FACES {
                    let viewport = COLUMN_W * zoom;
                    let window =
                        cx.open_window(gpui::size(px(viewport), px(1600.0)), |window, cx| {
                            let mut input = |cx: &mut Context<CoinsLayoutProbe>| {
                                cx.new(|cx| MoonInputState::new(window, cx))
                            };
                            CoinsLayoutProbe {
                                face,
                                search: input(cx),
                                min_trades: input(cx),
                                black_in: input(cx),
                                white_in: input(cx),
                                pick_min: input(cx),
                                pick_hold: input(cx),
                            }
                        });
                    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                    visual.update(|window, cx| window.set_content_zoom(zoom, cx));
                    for _ in 0..3 {
                        visual.update(|window, cx| {
                            let _ = window.draw(cx);
                        });
                    }
                    let col = visual.debug_bounds("probe-col").expect("column laid out");
                    let at = format!("{} {mode:?} zoom {zoom} face {face}", language.code());
                    let inside = |b: gpui::Bounds<gpui::Pixels>, what: &str| {
                        assert!(
                            b.origin.x + px(0.5) >= col.origin.x
                                && b.right() <= col.right() + px(0.5),
                            "{what} escaped the column ({at})"
                        );
                    };
                    for sel in [
                        "an-coin-filter-bar",
                        "an-coin-bl-field",
                        "an-coin-wl-field",
                        "an-coin-pending",
                    ] {
                        inside(visual.debug_bounds(sel).expect(sel), sel);
                    }
                    inside(
                        visual.debug_bounds("tun-toolbar-c").expect("toolbar"),
                        "toolbar",
                    );
                    if face != 0 {
                        // The longest entry a field accepts: chip and its x stay in the field.
                        let field = visual.debug_bounds("an-coin-bl-field").expect("field");
                        let id = format!("an-pick-chip-{}", long_token());
                        for sel in [format!("{id}-chip"), format!("{id}-x")] {
                            let b = visual
                                .debug_bounds(leak(sel.clone()))
                                .unwrap_or_else(|| panic!("{sel} not laid out ({at})"));
                            assert!(
                                b.origin.x >= field.origin.x && b.right() <= field.right(),
                                "{sel} escaped the field ({at})"
                            );
                        }
                    }
                    for need in visual.update(|_, cx| needs(cx, face)) {
                        let b = visual
                            .debug_bounds(leak(need.sel.clone()))
                            .unwrap_or_else(|| panic!("{} not laid out ({at})", need.sel));
                        inside(b, &need.sel);
                        assert!(
                            f32::from(b.size.width) + 0.1 >= need.width,
                            "{} narrower than its text ({at})",
                            need.sel
                        );
                    }
                    // The KPI matrix: every row and heading inside, every metric name whole.
                    let row = visual.debug_bounds("kpi-row-0").expect("kpi row");
                    inside(row, "kpi row");
                    for i in 0..3 {
                        let head = visual
                            .debug_bounds(leak(format!("kpi-head-{i}")))
                            .unwrap_or_else(|| panic!("kpi-head-{i} missing ({at})"));
                        assert!(head.right() <= row.right(), "kpi-head-{i} escaped ({at})");
                    }
                    let metrics = visual.update(|_, _| {
                        [
                            t!("analytics.kpi.trades").to_string(),
                            t!(
                                "analytics.kpi.profit",
                                unit = crate::analytics::pnl_unit_label()
                            )
                            .to_string(),
                            t!("analytics.kpi.avg_short").to_string(),
                            t!("analytics.kpi.winrate").to_string(),
                            t!("analytics.col.pf").to_string(),
                            t!("analytics.tuner.avg_win").to_string(),
                            t!("analytics.tuner.avg_loss").to_string(),
                            t!("analytics.kpi.maxdd").to_string(),
                        ]
                    });
                    for (i, text) in metrics.iter().enumerate() {
                        let b = visual
                            .debug_bounds(leak(format!("kpi-metric-{i}")))
                            .unwrap_or_else(|| panic!("kpi-metric-{i} missing ({at})"));
                        let need =
                            visual.update(|_, cx| design::ui_body_text_width(cx, text, 400.0));
                        assert!(
                            f32::from(b.size.width) + 0.1 >= need,
                            "metric {text} clipped ({at})"
                        );
                    }
                }
            }
        }
    }
}
