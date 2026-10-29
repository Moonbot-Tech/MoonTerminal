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
use super::super::auto::{AutoCardActs, AutoCardModel, AutoView, auto_card_el};
use super::super::coin_kpi_labels;
use super::super::fields::{
    ChipKind, FieldChip, ListFieldActs, ListFieldModel, list_field_el, pending_text,
};
use super::super::filter_bar::{FilterBarActs, FilterBarModel, filter_bar_el};
use super::super::rows::CoinCounts;
use super::super::state::{CoinFilter, CoinsState, MAX_TOKEN_LEN, PendingDelta, PickStaleCause};
use crate::design;
use crate::load_state::LoadState;

/// Width of the By coin right column, in font px.
const COLUMN_W: f32 = 470.0;

/// The faces the probe walks. Face 0 is the mode without a strategy: empty-state fields, a
/// toolbar without Make a copy / Save, live batch settings and a disabled Pick. The others carry a
/// strategy and cover running, useful/noise/empty results, a read failure, adopted/manual
/// pending edits, report/list staleness, and exclusions.
const FACES: usize = 14;

/// Card-only probe keeps narrow-width assertions independent of the unchanged KPI table.
struct AutoLayoutProbe(usize);

impl Render for AutoLayoutProbe {
    /// Render the production card with synthetic state and inert handlers.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let noop: super::super::fields::Act = Rc::new(|_, _| {});
        let acts = AutoCardActs {
            on_pick: noop.clone(),
            on_reset: noop.clone(),
            on_one_by_one: noop.clone(),
            on_add: noop.clone(),
            on_save: noop.clone(),
            on_revert: noop,
            on_exclude: Rc::new(|_, _, _, _| {}),
            on_black: Rc::new(|_, _, _| {}),
            on_white: Rc::new(|_, _, _| {}),
        };
        let card = auto_card_el(
            &auto_model(self.0),
            None,
            &acts,
            MoonPalette::active(cx),
            cx,
        );
        v_flex().w_full().child(card).child(
            gpui::div()
                .debug_selector(|| "probe-caption-line".into())
                .font_family(design::mono())
                .text_size(design::t_caption(cx))
                .child("X"),
        )
    }
}

/// A shrinking action, duplicate primary or overflowing wrapped row breaks an approved card face.
#[gpui::test]
fn every_auto_face_has_one_primary_and_fits_narrow_and_default_widths(
    cx: &mut gpui::TestAppContext,
) {
    const BUTTONS: &[(&str, &str)] = &[
        ("an-pick-run", "analytics.coins.pick_btn"),
        ("an-pick-stop", "analytics.coins.stop_btn"),
        ("an-pick-rerun", "analytics.coins.rerun"),
        ("an-pick-add-save", "analytics.coins.add_save"),
        ("an-pick-add-only", "analytics.coins.add_only"),
        ("an-pick-one", "analytics.coins.one_by_one"),
        ("an-coin-save", "analytics.coins.save_strategy"),
        ("an-coin-revert", "analytics.coins.revert_edits"),
    ];
    for mode in [
        UiThemeMode::Light,
        UiThemeMode::Graphite,
        UiThemeMode::LightExperimental,
        UiThemeMode::DarkExperimental,
        UiThemeMode::Dark,
    ] {
        cx.update(|cx| {
            MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(mode, 1.0),
                cx,
            )
        });
        for language in Language::ALL {
            let language = language.code();
            let _locale = crate::test_locale::force(language);
            // No side splitter/minimum exists: the stacked page receives host minus padding.
            // Cover the native minimum window (860 px), the reported 300 px side, and
            // the preferred side in the default 1240 px window (also the narrowest native side).
            let widths = cx.update(|cx| {
                let native = f32::from(super::super::super::layout::side_width(860.0, cx));
                let preferred = f32::from(super::super::super::layout::side_width(1240.0, cx));
                [native, 300.0, preferred]
            });
            for width in widths {
                for face in 0..FACES {
                    let window = cx.open_window(gpui::size(px(width), px(1200.0)), |_, _| {
                        AutoLayoutProbe(face)
                    });
                    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                    for _ in 0..3 {
                        visual.update(|window, cx| {
                            let _ = window.draw(cx);
                        });
                    }
                    let card = visual.debug_bounds("an-pick-card").expect("card");
                    assert!(f32::from(card.size.width) <= width + 0.5);
                    let model = auto_model(face);
                    let mut vertical = Vec::new();
                    for id in [
                        "an-pick-hint",
                        "an-pick-settings-body",
                        "an-pick-scope",
                        "an-pick-running",
                        "an-pick-progress",
                        "an-pick-stop",
                        "an-pick-verdict",
                        "an-pick-stale",
                        "an-pick-adopted",
                        "an-pick-suggestions",
                        "an-pick-actions",
                        "an-coin-pending",
                    ] {
                        if let Some(b) = visual.debug_bounds(id) {
                            assert!(
                                b.left() >= card.left() && b.right() <= card.right() + px(0.5),
                                "{id} escaped {language} width {width} face {face}: {b:?}, card {card:?}"
                            );
                            vertical.push((id, b));
                        }
                    }
                    for pair in vertical.windows(2) {
                        assert!(
                            pair[0].1.bottom() <= pair[1].1.top() + px(0.5),
                            "vertical overlap {language} {width} face {face}: {pair:?}"
                        );
                    }
                    for id in [
                        "an-pick-result",
                        "an-pick-settings-fields",
                        "an-pick-min",
                        "an-pick-hold",
                        "an-pick-bl-ctl",
                        "an-pick-wl-ctl",
                        "an-coin-pending-lbl",
                        "an-coin-pending-text",
                    ] {
                        if let Some(b) = visual.debug_bounds(id) {
                            assert!(
                                b.left() >= card.left() && b.right() <= card.right() + px(0.5),
                                "descendant {id} escaped {language} {width} face {face}: {b:?}"
                            );
                            assert!(b.bottom() <= card.bottom() + px(0.5), "{id} below card");
                        }
                    }
                    if let AutoView::Result(r) = model.view {
                        for (black, chips) in [(true, r.black), (false, r.white)] {
                            for chip in chips {
                                if r.adopted {
                                    continue;
                                }
                                let chip_box = visual
                                    .debug_bounds(leak(format!(
                                        "an-suggestion-{black}-{}-chip",
                                        chip.token
                                    )))
                                    .unwrap();
                                let remove = visual
                                    .debug_bounds(leak(format!(
                                        "an-suggestion-{black}-{}-exclude",
                                        chip.token
                                    )))
                                    .unwrap();
                                let label = visual
                                    .debug_bounds(leak(format!(
                                        "an-suggestion-{black}-{}-text",
                                        chip.token
                                    )))
                                    .unwrap();
                                assert!(
                                    label.right() <= remove.left() + px(0.5)
                                        && remove.right() <= chip_box.right() + px(0.5),
                                    "chip contents overlap"
                                );
                                let needed = visual.update(|_, cx| {
                                    design::mono_caption_text_width(cx, &chip.text, 400.0)
                                });
                                let row = visual.debug_bounds("an-pick-suggestions").unwrap();
                                if px(needed) + remove.size.width + px(6.0) <= row.size.width {
                                    assert!(
                                        label.size.width + px(0.5) >= px(needed),
                                        "short caption collapsed"
                                    );
                                } else {
                                    assert!(
                                        label.size.width > px(0.0),
                                        "ellipsis lost its viewport"
                                    );
                                }
                                for kind in ["text", "chip"] {
                                    let id = format!("an-suggestion-{black}-{}-{kind}", chip.token);
                                    let b = visual.debug_bounds(leak(id.clone())).expect(&id);
                                    assert!(
                                        b.right() <= card.right() + px(0.5),
                                        "{id} escaped card"
                                    );
                                    let row = visual.debug_bounds("an-pick-suggestions").unwrap();
                                    assert!(
                                        b.top() >= row.top()
                                            && b.bottom() <= row.bottom() + px(0.5),
                                        "{id} paints outside its measured row"
                                    );
                                    // A chip includes the native Xs remove button; its text is one caption line.
                                    let text = f32::from(
                                        visual
                                            .debug_bounds("probe-caption-line")
                                            .unwrap()
                                            .size
                                            .height,
                                    );
                                    let line = visual.update(|_, cx| {
                                        if kind == "text" {
                                            text
                                        } else {
                                            text.max(design::ui_value(
                                                cx,
                                                moon_ui::MoonSize::Xs.control_metrics().height,
                                            ))
                                        }
                                    });
                                    assert!(
                                        (f32::from(b.size.height) - line).abs() <= 1.0,
                                        "{id} wrapped vertically {language} {width} face {face}: {b:?}, line {line}"
                                    );
                                }
                            }
                        }
                    }
                    let mut primaries = Vec::new();
                    let mut buttons = Vec::new();
                    for (id, key) in BUTTONS {
                        let key = if *id == "an-pick-add-save" {
                            add_label_key(face)
                        } else {
                            key
                        };
                        if let Some(bounds) = visual.debug_bounds(leak(id.to_string())) {
                            assert!(
                                bounds.origin.x >= card.origin.x
                                    && bounds.right() <= card.right() + px(0.5),
                                "{id} outside {language} {width} face {face}"
                            );
                            let needed = visual
                                .update(|_, cx| design::ui_caption_text_width(cx, &t!(key), 400.0));
                            assert!(
                                f32::from(bounds.size.width) + 0.1 >= needed,
                                "{id} label clipped {language} {width} face {face}"
                            );
                            assert!(bounds.bottom() <= card.bottom(), "{id} below card");
                            buttons.push((*id, bounds));
                        }
                        if visual
                            .debug_bounds(leak(format!("an-primary-{id}")))
                            .is_some()
                        {
                            primaries.push(*id);
                        }
                    }
                    for (i, (id, a)) in buttons.iter().enumerate() {
                        for (other, b) in &buttons[i + 1..] {
                            assert!(
                                a.right() <= b.left() + px(0.5)
                                    || b.right() <= a.left() + px(0.5)
                                    || a.bottom() <= b.top() + px(0.5)
                                    || b.bottom() <= a.top() + px(0.5),
                                "buttons {id} and {other} overlap {language} {width} face {face}"
                            );
                        }
                    }
                    assert_eq!(
                        primaries.len(),
                        1,
                        "{language} {mode:?} {width} face {face}: {primaries:?}"
                    );
                    let expected = match face {
                        0 | 4 => "an-pick-run",
                        1 => "an-pick-stop",
                        5 | 8 => "an-pick-rerun",
                        6 | 9 => "an-coin-save",
                        7 | 11 => "an-pick-rerun",
                        _ => "an-pick-add-save",
                    };
                    assert_eq!(primaries[0], expected);
                    assert!(visual.debug_bounds("an-pick-reset").is_none());
                    assert!(visual.debug_bounds("an-pick-anyway").is_none());
                    assert!(visual.debug_bounds("an-pick-summary-row").is_none());
                    assert!(visual.debug_bounds("an-pick-settings").is_none());
                    let mut fields = Vec::new();
                    for id in [
                        "an-pick-min",
                        "an-pick-hold",
                        "an-pick-bl-ctl",
                        "an-pick-wl-ctl",
                    ] {
                        fields.push(visual.debug_bounds(id).expect("settings stay visible"));
                    }
                    for (i, a) in fields.iter().enumerate() {
                        for b in &fields[i + 1..] {
                            assert!(
                                a.right() <= b.left() + px(0.5)
                                    || b.right() <= a.left() + px(0.5)
                                    || a.bottom() <= b.top() + px(0.5)
                                    || b.bottom() <= a.top() + px(0.5),
                                "settings overlap {language} {width} face {face}"
                            );
                        }
                    }
                    let settings = visual.debug_bounds("an-pick-settings-body").unwrap();
                    if let Some(verdict) = visual.debug_bounds("an-pick-verdict") {
                        assert!(settings.bottom() <= verdict.top());
                        assert!(visual.debug_bounds("an-pick-run").is_none());
                        assert!(visual.debug_bounds("an-pick-rerun").is_some());
                    }
                    if face == 0 {
                        let scope = visual
                            .debug_bounds("an-pick-scope")
                            .expect("batch scope note");
                        let run = visual.debug_bounds("an-pick-run").expect("disabled pick");
                        assert!(settings.bottom() <= scope.top() && scope.bottom() <= run.top());
                        if width == 300.0 {
                            let line = visual.debug_bounds("probe-caption-line").unwrap();
                            assert!(
                                scope.size.height > line.size.height * 2.0,
                                "scope must wrap"
                            );
                        }
                    }
                }
            }
        }
    }
}

/// Use the production Parameters header/viewport boundary: a tall chip must move the lists down.
struct IntegratedPickProbe;

impl Render for IntegratedPickProbe {
    /// Render the noise result and both real list editors through the page's side allocation.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let noop: super::super::fields::Act = Rc::new(|_, _| {});
        let acts = AutoCardActs {
            on_pick: noop.clone(),
            on_reset: noop.clone(),
            on_one_by_one: noop.clone(),
            on_add: noop.clone(),
            on_save: noop.clone(),
            on_revert: noop.clone(),
            on_exclude: Rc::new(|_, _, _, _| {}),
            on_black: Rc::new(|_, _, _| {}),
            on_white: Rc::new(|_, _, _| {}),
        };
        let fields = ListFieldActs {
            on_remove: Rc::new(|_, _, _| {}),
            on_restore: Rc::new(|_, _, _| {}),
            on_copy: noop.clone(),
            on_clear: noop.clone(),
        };
        let params = super::super::super::layout::parameters(
            v_flex()
                .w_full()
                .child(toolbar_el(
                    TunerKind::Coins,
                    t!("analytics.tuner.fields_title").to_string(),
                    None,
                    &ToolbarActs {
                        on_copy: noop.clone(),
                        on_save: noop,
                    },
                    cx,
                ))
                .child(auto_card_el(&auto_model(12), None, &acts, p, cx))
                .into_any_element(),
            v_flex()
                .w_full()
                .child(list_field_el(&black_field(1), None, &fields, p, cx))
                .child(list_field_el(&white_field(1), None, &fields, p, cx))
                .into_any_element(),
            p,
            window,
            cx,
        );
        let side = super::super::super::layout::side(
            gpui::div().h(px(80.0)).flex_none().into_any_element(),
            params,
            cx,
        );
        let narrow =
            super::super::super::layout::is_narrow(f32::from(window.viewport_size().width), cx);
        let page = super::super::super::layout::page(
            gpui::div().into_any_element(),
            gpui::div().into_any_element(),
            Some(side),
            narrow,
            window,
            cx,
        );
        v_flex().size_full().child(page).child(
            gpui::div()
                .absolute()
                .debug_selector(|| "probe-caption-line".into())
                .font_family(design::mono())
                .text_size(design::t_caption(cx))
                .child("X"),
        )
    }
}

/// Removing width containment or shrinking the header must not paint a suggestion over the lists.
#[gpui::test]
fn coins_noise_pick_stays_above_integrated_lists(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Light, 1.0),
            cx,
        )
    });
    for language in ["ru", "en"] {
        let _locale = crate::test_locale::force(language);
        for width in [320.0, 860.0, 1240.0] {
            let window = cx.open_window(gpui::size(px(width), px(1800.0)), |_, _| {
                IntegratedPickProbe
            });
            let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
            for _ in 0..3 {
                visual.update(|window, cx| {
                    let _ = window.draw(cx);
                });
            }
            let card = visual.debug_bounds("an-pick-card").unwrap();
            let side = visual.debug_bounds("tuner-side").unwrap();
            let expected =
                visual.update(|_, cx| super::super::super::layout::side_width(width, cx));
            assert!(
                (side.size.width - expected).abs() <= px(0.5),
                "probe must use production width"
            );
            assert!(card.right() <= side.right() + px(0.5));
            for id in [
                "an-pick-settings-body",
                "an-pick-min",
                "an-pick-verdict",
                "an-pick-suggestions",
                "an-pick-actions",
                "an-pick-add-save",
                "an-pick-rerun",
            ] {
                let b = visual.debug_bounds(id).unwrap();
                assert!(
                    b.left() >= card.left() && b.right() <= card.right() + px(0.5),
                    "integrated {id} escaped {language} {width}"
                );
            }
            let black = visual.debug_bounds("an-coin-bl-field").unwrap();
            assert!(
                black.top() >= card.bottom(),
                "lists overlap pick {language} {width}: {black:?} {card:?}"
            );
            let chip = visual.debug_bounds("an-suggestion-true-XAAA-text").unwrap();
            let line = visual.debug_bounds("probe-caption-line").unwrap();
            assert!(
                (chip.size.height - line.size.height).abs() <= px(0.5),
                "single suggestion wrapped {language} {width}: {chip:?}"
            );
            assert!(
                chip.bottom() <= black.top(),
                "suggestion paints over blacklist"
            );
        }
    }
}

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
    let mut s = state();
    if face == 1 {
        s.begin_pick();
    } else if face == 4 {
        s.pick = super::super::state::PickState::Failed(ReadFail::NotReady);
    } else if face != 0 && face != 9 {
        let verdict = match face {
            3 => PickVerdict::Noise(NoiseReason::SmallGain),
            12 => PickVerdict::Noise(NoiseReason::FewHoldout),
            5 => PickVerdict::NothingFound,
            8 => PickVerdict::TooFewTrades,
            _ => PickVerdict::Useful,
        };
        let mut pick = pick(verdict);
        if face == 2 {
            pick.white.clear();
        }
        if face == 3 {
            pick.black.clear();
        }
        if face == 12 || face == 13 {
            pick.black.truncate(1);
            pick.white.clear();
            if face == 13 {
                pick.black[0].token = long_token();
            }
        }
        if face == 5 || face == 8 {
            pick.black.clear();
            pick.white.clear();
        }
        let seq = s.begin_pick();
        s.land_pick(seq, s.lists_rev, s.report_gen, 30, Ok(pick));
        if face == 6 {
            s.adopt_pick();
        }
        if face == 10 {
            for (black, token) in [(true, "XAAA"), (true, "XBBB"), (false, "XEEE")] {
                s.toggle_exclude(black, token.to_string());
            }
        }
    }
    if face == 9 || face == 11 {
        s.add_tokens(true, "XALPHA");
    }
    let mut m = AutoCardModel::new(&s, face != 0, false, true, 551);
    if face == 7 {
        m.stale = Some(PickStaleCause::Report);
    }
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
            on_add: noop.clone(),
            on_save: noop.clone(),
            on_revert: noop.clone(),
            on_exclude: Rc::new(|_, _, _, _| {}),
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
            changed_coins: 2,
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

/// Independent fixture labels cover black-only, white-only and combined suggestions.
fn add_label_key(face: usize) -> &'static str {
    match face {
        2 | 12 | 13 => "analytics.coins.add_save_bl",
        3 => "analytics.coins.add_save_wl",
        _ => "analytics.coins.add_save",
    }
}

/// What the probe asserts per language: every one-line text whose box must not clip it.
fn needs(cx: &gpui::App, face: usize) -> Vec<Need> {
    let mut out = vec![caption(
        cx,
        "an-coin-hide-lbl",
        &t!("analytics.coins.hide_lbl"),
    )];
    if face != 0 {
        for (id, key) in [
            ("an-coin-bl-copy", "analytics.coins.copy"),
            ("an-coin-bl-clear", "analytics.coins.clear"),
            ("an-coin-wl-copy", "analytics.coins.copy"),
            ("an-coin-wl-clear", "analytics.coins.clear"),
        ] {
            out.push(caption(cx, id, &t!(key)));
        }
    }
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
        0 | 4 => &[("an-pick-run", "analytics.coins.pick_btn")],
        1 => &[("an-pick-stop", "analytics.coins.stop_btn")],
        5 | 8 => &[("an-pick-rerun", "analytics.coins.rerun")],
        6 | 9 => &[
            ("an-coin-save", "analytics.coins.save_strategy"),
            ("an-coin-revert", "analytics.coins.revert_edits"),
        ],
        7 | 11 => &[
            ("an-pick-rerun", "analytics.coins.rerun"),
            ("an-pick-add-save", "analytics.coins.add_save"),
        ],
        _ => &[
            ("an-pick-add-save", "analytics.coins.add_save"),
            ("an-pick-add-only", "analytics.coins.add_only"),
            ("an-pick-rerun", "analytics.coins.rerun"),
        ],
    };
    for (sel, key) in auto {
        let key = if *sel == "an-pick-add-save" {
            add_label_key(face)
        } else {
            key
        };
        out.push(caption(cx, sel, &t!(key)));
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
            drawn.push(m.legend.clone());
            drawn.extend(m.empty.iter().cloned());
        }
        for face in 0..FACES {
            let m = auto_model(face);
            drawn.extend(m.hint);
            if let AutoView::Result(r) = m.view {
                drawn.push(r.line);
                drawn.extend(r.black.into_iter().map(|c| c.text));
                drawn.extend(r.white.into_iter().map(|c| c.text));
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
                    for sel in ["an-coin-filter-bar", "an-coin-bl-field", "an-coin-wl-field"] {
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
