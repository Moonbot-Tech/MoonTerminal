//! Rules of the cores card: the colour square follows the chart's line mode and picks, the long
//! list folds and unfolds, and a row click reports its core.
//!
//! Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
//! would shadow the built-in `#[test]` attribute.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use gpui::{
    Context, IntoElement, Modifiers, ParentElement, Render, Styled, VisualTestContext, Window, px,
};
use moon_core::db::analytics::CoreSeries;
use moon_ui::{MoonPalette, v_flex};

use super::{CoreRow, CoresActs, RowModel, cores_el, row_models, row_tint};
use crate::analytics::summary::model::{Concentration, DivergingScale, core_rows};
use crate::analytics::summary::state::{CoreFilter, CoreLines, SummaryUi};

/// `n` synthetic cores; core `i` earns `i - n/2` per trade over ten trades.
fn cores(n: u64) -> Vec<CoreSeries> {
    (0..n)
        .map(|i| {
            let v = i as f64 - (n / 2) as f64;
            CoreSeries {
                uid: 700 + i,
                name: format!("XALPHA ~ F-{i:02} / SUB 01"),
                total: v * 10.0,
                trades: 10,
                wins: if v > 0.0 { 7 } else { 3 },
                win_sum: v.max(0.0) * 10.0 + 5.0,
                loss_sum: (-v).max(0.0) * 10.0 + 5.0,
                ..Default::default()
            }
        })
        .collect()
}

/// Core rows of a render model, in order.
fn core_lines(lines: &[RowModel]) -> Vec<&CoreRow> {
    lines
        .iter()
        .filter_map(|l| match l {
            RowModel::Core(r) => Some(r),
            RowModel::Fold { .. } => None,
        })
        .collect()
}

/// The square is filled exactly for the cores the chart draws: none, the five leaders, or the
/// picked set; the row highlight follows the picks alone.
#[test]
fn square_state_follows_lines_and_picks() {
    let _locale = crate::test_locale::force("en");
    let c = cores(10);
    let scale = DivergingScale::of(&c);
    let rows = core_rows(&c, CoreFilter::All, false);
    let filled = |ui: &SummaryUi| -> BTreeSet<u64> {
        core_lines(&row_models(&c, &rows, ui, &scale))
            .into_iter()
            .filter(|r| r.filled)
            .map(|r| r.uid)
            .collect()
    };
    let picked: BTreeSet<u64> = [700, 703].into();
    let mut ui = SummaryUi {
        picked: picked.clone(),
        ..Default::default()
    };
    ui.lines = CoreLines::None;
    assert!(filled(&ui).is_empty());
    ui.lines = CoreLines::Top5;
    assert_eq!(filled(&ui), [705, 706, 707, 708, 709].into());
    ui.lines = CoreLines::Selected;
    assert_eq!(filled(&ui), picked);
    let hi: BTreeSet<u64> = core_lines(&row_models(&c, &rows, &ui, &scale))
        .into_iter()
        .filter(|r| r.picked)
        .map(|r| r.uid)
        .collect();
    assert_eq!(hi, picked);
    // PF below 1 is flagged for the warning colour; the losers here all have it.
    for r in core_lines(&row_models(&c, &rows, &ui, &scale)) {
        assert_eq!(r.pf_low, r.total < 0.0, "{}", r.name);
    }
}

/// A folded long list shows one fold line; an unfolded one lists every core and ends with the
/// collapse line; a filter never folds.
#[test]
fn long_list_folds_and_unfolds() {
    let _locale = crate::test_locale::force("en");
    let c = cores(30);
    let scale = DivergingScale::of(&c);
    let ui = SummaryUi::default();
    let folded = row_models(&c, &core_rows(&c, CoreFilter::All, false), &ui, &scale);
    let folds: Vec<_> = folded
        .iter()
        .filter_map(|l| match l {
            RowModel::Fold { text, unfolded } => Some((text.clone(), *unfolded)),
            RowModel::Core(_) => None,
        })
        .collect();
    assert_eq!(folds.len(), 1);
    assert!(!folds[0].1 && folds[0].0.contains("17"), "{folds:?}");
    let open = SummaryUi {
        unfolded: true,
        ..Default::default()
    };
    let all = row_models(&c, &core_rows(&c, CoreFilter::All, true), &open, &scale);
    assert_eq!(core_lines(&all).len(), 30);
    assert!(matches!(
        all.last(),
        Some(RowModel::Fold { unfolded: true, .. })
    ));
    let pos = SummaryUi {
        unfolded: true,
        core_filter: CoreFilter::Pos,
        ..Default::default()
    };
    let only_pos = row_models(&c, &core_rows(&c, CoreFilter::Pos, true), &pos, &scale);
    assert!(only_pos.iter().all(|l| matches!(l, RowModel::Core(_))));
}

/// Backend-free host of the cores card.
struct Host {
    /// Cores the card lists.
    cores: Vec<CoreSeries>,
    /// Callbacks under test.
    acts: CoresActs,
}

impl Render for Host {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let rows = core_rows(&self.cores, CoreFilter::All, false);
        let scale = DivergingScale::of(&self.cores);
        let conc = Concentration::of(&self.cores);
        v_flex().w_full().child(cores_el(
            &self.cores,
            &rows,
            &scale,
            conc.as_ref(),
            &SummaryUi::default(),
            &[],
            &self.acts,
            p,
            cx,
        ))
    }
}

/// Clicking a core row hands that row's uid to `on_pick` and nothing else.
#[gpui::test]
fn row_click_calls_on_pick_with_the_uid(cx: &mut gpui::TestAppContext) {
    let _locale = crate::test_locale::force("en");
    let got: Rc<RefCell<Vec<u64>>> = Rc::default();
    let sink = got.clone();
    let acts = CoresActs {
        on_pick: Rc::new(move |uid, _, _| sink.borrow_mut().push(uid)),
        on_filter: Rc::new(|_, _, _| {}),
        on_fold: Rc::new(|_, _, _| {}),
    };
    let c = cores(6);
    let window = cx.open_window(gpui::size(px(1200.0), px(900.0)), |_, _| Host {
        cores: c.clone(),
        acts,
    });
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    for _ in 0..2 {
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    // Row 0 is the leader (largest total), row 5 the biggest loser.
    for (k, uid) in [(0usize, 705u64), (5, 700)] {
        let sel: &'static str = Box::leak(format!("sum-cores-row-{k}").into_boxed_str());
        let b = visual
            .debug_bounds(sel)
            .unwrap_or_else(|| panic!("{sel} not laid out"));
        visual.simulate_click(b.center(), Modifiers::none());
        assert_eq!(got.borrow().last(), Some(&uid), "row {k}");
    }
    assert_eq!(got.borrow().len(), 2);
}

/// A picked row is tinted with its own chart colour at a low alpha, so the row and its line read
/// as one; an unpicked row keeps the table background.
#[test]
fn picked_row_takes_a_faint_tint_of_its_chart_colour() {
    let color = gpui::hsla(0.33, 0.7, 0.45, 1.0);
    let tint = row_tint(true, color).expect("picked rows are tinted");
    assert_eq!((tint.h, tint.s, tint.l), (color.h, color.s, color.l));
    assert!(tint.a > 0.0 && tint.a <= 0.2, "{}", tint.a);
    assert_eq!(row_tint(false, color), None);
}

/// The caption beside the title sums profit, not core counts: net of every core, the gaining
/// cores' total and the losing cores' signed total. Feeding it the counts again would print
/// "total 5 · gains 2 · losses 2" for a book that netted zero.
#[test]
fn sub_caption_sums_profit_not_core_counts() {
    let _locale = crate::test_locale::force("en");
    // Totals -20, -10, 0, +10, +20: net 0, gains +30, losses -30.
    let text = super::sub_text(&cores(5));
    let signed = crate::analytics::summary::fmt_signed;
    assert!(text.contains(&signed(30.0)), "{text}");
    assert!(text.contains(&signed(-30.0)), "{text}");
    assert!(!text.contains(" 5 ") && !text.contains(" 2 "), "{text}");
}
