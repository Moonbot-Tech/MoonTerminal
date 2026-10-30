use super::super::state::CoinsState;
use super::{AutoCardModel, AutoView, tag_line, verdict_line};
use moon_core::db::metrics::Tally;
use moon_core::db::tuner::coin_pick::{CoinPick, CoinSuggestion, NoiseReason, PickVerdict};

fn tally(profit: f64) -> Tally {
    let mut t = Tally::default();
    t.push(profit);
    t
}

fn pick(verdict: PickVerdict) -> CoinPick {
    CoinPick {
        black: vec![CoinSuggestion {
            token: "XAAA".to_string(),
            n: 48,
            profit: -995.64,
        }],
        white: Vec::new(),
        train_n: 70,
        holdout_n: 30,
        fact_holdout: tally(-130.0),
        manual_holdout: tally(-80.0),
        pick_holdout: tally(20.5),
        verdict,
    }
}

/// Every verdict reads as its own sentence, and the useful one carries the pick's figure,
/// the as-traded figure and the gain, each signed by the shared formatter.
#[test]
fn every_verdict_has_its_own_sentence() {
    let _locale = crate::test_locale::force("en");
    let verdicts = [
        PickVerdict::Useful,
        PickVerdict::Noise(NoiseReason::SmallGain),
        PickVerdict::Noise(NoiseReason::HoldoutWorse),
        PickVerdict::Noise(NoiseReason::FewHoldout),
        PickVerdict::NothingFound,
        PickVerdict::TooFewTrades,
    ];
    let lines: Vec<String> = verdicts
        .iter()
        .map(|v| verdict_line(&pick(v.clone()), 30))
        .collect();
    for (i, a) in lines.iter().enumerate() {
        assert!(!a.contains("analytics."), "raw key: {a}");
        for b in &lines[i + 1..] {
            assert_ne!(a, b);
        }
    }
    let useful = &lines[0];
    for figure in [
        moon_core::util::fmt::signed_amount(20.5, 2).0,
        moon_core::util::fmt::signed_amount(-130.0, 2).0,
        moon_core::util::fmt::signed_amount(100.5, 2).0,
    ] {
        assert!(useful.contains(&figure), "{useful} lacks {figure}");
    }
    assert!(useful.contains("30%"));
}

/// A suggested coin's tag names its full-period profit and trade count.
#[test]
fn a_tag_names_profit_and_trades() {
    let _locale = crate::test_locale::force("en");
    let c = &pick(PickVerdict::Useful).black[0];
    assert_eq!(tag_line(c), "XAAA -995.64 · 48 tr.");
}

/// Pick waits for the coin statistics and saved lists: a pick launched mid-reload would
/// score against lists about to change.
#[test]
fn pick_waits_for_the_data() {
    let _locale = crate::test_locale::force("en");
    let s = CoinsState::default();
    assert!(AutoCardModel::new(&s, true, false, true, 551).can_pick());
    assert!(!AutoCardModel::new(&s, true, false, false, 551).can_pick());
    assert!(!AutoCardModel::new(&s, false, false, true, 551).can_pick());
}

/// Keeping an adopted result forever would strand the card after its saved baseline catches up.
#[test]
fn adopted_result_keeps_verdict_until_saved_baseline_arrives() {
    let _locale = crate::test_locale::force("en");
    let mut s = CoinsState::default();
    let seq = s.begin_pick();
    s.land_pick(
        seq,
        s.lists_rev,
        s.report_gen,
        30,
        Ok(pick(PickVerdict::Useful)),
    );
    s.adopt_pick();
    let m = AutoCardModel::new(&s, true, false, true, 551);
    assert!(matches!(
        m.view,
        AutoView::Result(super::ResultModel { adopted: true, .. })
    ));
    assert!(m.pending.is_some());
    assert!(!s.retire_saved_pick(), "unsaved edits retain their verdict");
    s.adopt(s.work.duplicate());
    assert!(s.retire_saved_pick());
    assert!(s.suggested().is_none());
    let m = AutoCardModel::new(&s, true, false, true, 551);
    assert!(matches!(m.view, AutoView::Settings));
    assert!(m.pending.is_none());
    s.add_tokens(true, "XBETA");
    let m = AutoCardModel::new(&s, true, false, true, 551);
    assert!(
        matches!(m.view, AutoView::Settings),
        "a later manual edit must not resurrect the old verdict"
    );
    assert!(m.pending.is_some());
}

/// Gating the model on selection would hide the shared settings or offer an unscoped pick.
#[test]
fn no_strategy_keeps_batch_settings_without_a_pick_target() {
    let _locale = crate::test_locale::force("en");
    let mut s = CoinsState::default();
    let seq = s.begin_pick();
    s.land_pick(
        seq,
        s.lists_rev,
        s.report_gen,
        30,
        Ok(pick(PickVerdict::Useful)),
    );
    let m = AutoCardModel::new(&s, false, false, true, 17);
    assert!(matches!(m.view, AutoView::NoStrategy { strategies: 17 }));
    assert!(!m.can_pick());
    assert!(m.hint.is_none());
    assert_eq!(m.cfg, s.pick_cfg);
}

/// A result-only model must not discard the live settings needed above its verdict.
#[test]
fn finished_pick_keeps_its_verdict_and_live_settings() {
    let _locale = crate::test_locale::force("en");
    let mut s = CoinsState::default();
    s.pick_cfg.min_trades = 13;
    s.pick_cfg.holdout_pct = 40;
    let seq = s.begin_pick();
    s.land_pick(
        seq,
        s.lists_rev,
        s.report_gen,
        40,
        Ok(pick(PickVerdict::Useful)),
    );
    let m = AutoCardModel::new(&s, true, false, true, 17);
    let AutoView::Result(r) = &m.view else {
        panic!("finished result missing")
    };
    assert!(r.line.contains("40%"));
    assert_eq!(m.cfg.min_trades, 13);
    assert_eq!(m.cfg.holdout_pct, 40);
    assert!(m.can_pick());
}

/// A generic list label must not replace the approved per-list confirmation sentence.
#[test]
fn adopted_hint_uses_the_approved_black_and_white_sentences() {
    let _locale = crate::test_locale::force("ru");
    let mut s = CoinsState::default();
    let mut pick = pick(PickVerdict::Useful);
    pick.black[0].token = "XBETA".into();
    let seq = s.begin_pick();
    s.land_pick(seq, s.lists_rev, s.report_gen, 30, Ok(pick));
    s.adopt_pick();
    let AutoView::Result(mut result) = AutoCardModel::new(&s, true, false, true, 551).view else {
        panic!("result")
    };
    assert_eq!(
        super::adopted_hint(&result),
        "XBETA добавлена в чёрный список ниже."
    );
    result.white = std::mem::take(&mut result.black);
    assert_eq!(
        super::adopted_hint(&result),
        "XBETA добавлена в белый список ниже."
    );
}

/// Exercise production buttons and checkboxes without a database or an Analytics backend.
struct ActionProbe {
    face: usize,
    picks: std::rc::Rc<std::cell::Cell<usize>>,
    resets: std::rc::Rc<std::cell::Cell<usize>>,
    flags: std::rc::Rc<std::cell::Cell<usize>>,
}

impl gpui::Render for ActionProbe {
    /// Render unselected, running and finished states with observable action callbacks.
    fn render(
        &mut self,
        _: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use std::rc::Rc;
        let mut state = CoinsState::default();
        if self.face == 1 {
            state.begin_pick();
        } else if self.face >= 2 {
            let mut result = pick(PickVerdict::Useful);
            if self.face == 3 {
                result.black.clear();
                result.verdict = PickVerdict::NothingFound;
            }
            let seq = state.begin_pick();
            state.land_pick(seq, 0, 0, 30, Ok(result));
            if self.face == 4 {
                state.adopt_pick();
            }
        }
        let action = |counter: &Rc<std::cell::Cell<usize>>| -> super::super::fields::Act {
            let counter = counter.clone();
            Rc::new(move |_, _| counter.set(counter.get() + 1))
        };
        let noop: super::super::fields::Act = Rc::new(|_, _| {});
        let flags = self.flags.clone();
        let acts = super::AutoCardActs {
            on_pick: action(&self.picks),
            on_reset: action(&self.resets),
            on_one_by_one: noop.clone(),
            on_add: noop.clone(),
            on_save: noop.clone(),
            on_revert: noop,
            on_exclude: Rc::new(|_, _, _, _| {}),
            on_black: Rc::new(|_, _, _| {}),
            on_white: Rc::new(move |_, _, _| flags.set(flags.get() + 1)),
        };
        super::auto_card_el(
            &AutoCardModel::new(&state, self.face != 0, false, true, 17),
            None,
            &acts,
            moon_ui::MoonPalette::active(cx),
            cx,
        )
    }
}

/// Disabling global settings or wiring rerun to Reset would break these real control clicks.
#[gpui::test]
fn settings_are_live_without_selection_and_results_rerun_the_pick(cx: &mut gpui::TestAppContext) {
    use gpui::px;
    use std::cell::Cell;
    use std::rc::Rc;
    let _locale = crate::test_locale::force("en");
    cx.update(moon_ui::init);
    cx.update(|cx| {
        moon_ui::MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(
                moon_core::config::UiThemeMode::Light,
                1.0,
            ),
            cx,
        )
    });
    for face in 0..5 {
        let picks = Rc::new(Cell::new(0));
        let resets = Rc::new(Cell::new(0));
        let flags = Rc::new(Cell::new(0));
        let window = cx.open_window(gpui::size(px(470.0), px(900.0)), |_, _| ActionProbe {
            face,
            picks: picks.clone(),
            resets: resets.clone(),
            flags: flags.clone(),
        });
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        for _ in 0..3 {
            visual.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        let flag = visual
            .debug_bounds("an-pick-wl-ctl")
            .expect("visible whitelist setting");
        visual.simulate_click(flag.center(), gpui::Modifiers::none());
        assert_eq!(
            flags.get(),
            usize::from(face != 1),
            "only running locks settings"
        );
        let id = match face {
            0 => "an-pick-run",
            1 => "an-pick-stop",
            _ => "an-pick-rerun",
        };
        let button = visual.debug_bounds(id).expect("state action");
        visual.simulate_click(button.center(), gpui::Modifiers::none());
        assert_eq!(
            picks.get(),
            usize::from(face >= 2),
            "disabled Pick or live Rerun"
        );
        assert_eq!(resets.get(), usize::from(face == 1), "only Stop resets");
    }
}
