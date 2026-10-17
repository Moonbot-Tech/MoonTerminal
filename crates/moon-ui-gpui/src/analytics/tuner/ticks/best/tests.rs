//! Ordered batch publication and validated-only bulk payload regressions.

use super::super::state::{NowValue, TicksState};
use super::super::summary::{Verdict, Why};
use super::{BestResult, BestRun, bulk_payload};
use crate::analytics::tuner::shared::SaveTarget;
use moon_core::db::metrics::Tally;
use moon_core::db::tuner::ticks::search::{SearchMiss, SearchResult};
use std::collections::{HashMap, HashSet};

/// Selection clicks and axis moves must preserve batch results; workspace changes must retire them.
#[test]
fn selection_and_axis_keep_finished_batch_but_scope_drops_it() {
    let mut state = TicksState::default();
    state.best = Some(BestRun {
        seq: 7,
        keys: vec!["1".into()],
        next: 0,
        results: vec![],
        handle: None,
        checked: HashSet::new(),
        done: false,
    });
    state.best.as_mut().unwrap().land(7, result("1"));
    state.invalidate_selection();
    state.invalidate_for_axis();
    assert!(state.best.as_ref().unwrap().done);
    assert_eq!(state.best.as_ref().unwrap().results.len(), 1);
    state.invalidate();
    assert!(state.best.is_none());
}

/// A comparable sample below the card's floor cannot become advice despite profitable slices.
#[test]
fn too_few_comparable_deals_cannot_be_apply() {
    let mut answer = result("1");
    let found = answer.outcome.as_mut().unwrap();
    found.train.n = 1;
    found.holdout.as_mut().unwrap().n = 10;
    found.fact_train.n = 1;
    found.fact_holdout.as_mut().unwrap().n = 10;
    let answer = BestResult::new(answer.key, answer.target, answer.outcome, answer.now);
    assert!(matches!(answer.verdict, Verdict::Cannot(_)));
    assert!(!answer.is_apply());
}

/// Entry screening may skip trades without shrinking the comparable sample used by the card.
#[test]
fn sample_gate_uses_comparable_deals_instead_of_variant_fills() {
    let mut answer = result("1");
    let found = answer.outcome.as_mut().unwrap();
    found.train.n = 1;
    found.holdout.as_mut().unwrap().n = 10;
    let answer = BestResult::new(answer.key, answer.target, answer.outcome, answer.now);
    assert_eq!(answer.verdict, Verdict::Apply);
}

/// No-op Apply answers must neither auto-check nor occupy a write target or an old-value slot.
#[test]
fn no_op_answer_is_not_checked_counted_or_sent() {
    let mut answer = result("1");
    answer
        .now
        .insert("SellPrice".into(), NowValue::Same("1.5".into()));
    let mut run = BestRun {
        seq: 7,
        keys: vec!["1".into()],
        next: 0,
        results: vec![],
        handle: None,
        checked: HashSet::new(),
        done: false,
    };
    assert!(run.land(7, answer));
    assert!(run.checked.is_empty());
    let (targets, changes, olds) = bulk_payload(&run.results, &HashSet::from(["1".into()]));
    assert!(targets.is_empty() && changes.is_empty() && olds.is_empty());
}

/// Enabled watched fields must follow the independent strategy read into the dialog warning list.
#[test]
fn watched_exit_warning_survives_into_batch_consent() {
    let mut answer = result("1");
    answer.warns = super::super::unmodelled::strategy_warns(
        "DROPS_01",
        &HashMap::from([("UseBV_SV_Stop".into(), "YES".into())]),
        &HashMap::new(),
        &moon_core::feed::strategy_deps::FieldDeps::bundled(),
        false,
    );
    let warns = super::bulk_warns(&[answer], &HashSet::from(["1".into()]));
    assert!(warns.iter().any(|w| w.contains("UseBV_SV_Stop = YES")));
}

/// Build independent slice figures supporting a strict gain on a sufficient holdout.
fn result(key: &str) -> BestResult {
    let tally = |profit| {
        let mut t = Tally::default();
        t.n = 30;
        t.profit = profit;
        t
    };
    BestResult::new(
        key.into(),
        SaveTarget {
            sid: key.parse().unwrap_or(1),
            core: Some(11),
            name: "DROPS_01".into(),
        },
        Ok(SearchResult {
            values: vec![("SellPrice".into(), format!("{key}.5"))],
            searched: vec!["SellPrice".into()],
            train: tally(20.0),
            fact_train: tally(10.0),
            holdout: Some(tally(8.0)),
            fact_holdout: Some(tally(4.0)),
            holdout_open: 0,
            holdout_open_profit: 0.0,
            holdout_loses: false,
            seed: 42,
            stats: Default::default(),
        }),
        HashMap::from([("SellPrice".into(), NowValue::Same("1".into()))]),
    )
}

/// Replacing ordered publication with unconditional push would admit a stale or wrong strategy.
#[test]
fn stale_and_out_of_order_landings_do_not_advance_the_batch() {
    let mut run = BestRun {
        seq: 7,
        keys: vec!["1".into(), "2".into()],
        next: 0,
        results: vec![],
        handle: None,
        checked: HashSet::new(),
        done: false,
    };
    assert!(!run.land(6, result("1")));
    assert!(!run.land(7, result("2")));
    assert!(run.land(7, result("1")));
    assert!(run.land(7, result("2")));
    assert_eq!(
        run.results
            .iter()
            .map(|r| r.key.as_str())
            .collect::<Vec<_>>(),
        ["1", "2"]
    );
    assert!(run.done);
}

/// Cancellation removes the continuation and cancels its independent search handle.
#[test]
fn cancel_retires_the_run_before_any_next_strategy_can_be_started() {
    let handle = moon_core::db::tuner::threshold_search::SearchHandle::new();
    let mut state = TicksState::default();
    state.best = Some(BestRun {
        seq: 7,
        keys: vec!["1".into(), "2".into()],
        next: 0,
        results: vec![],
        handle: Some(handle.clone()),
        checked: HashSet::new(),
        done: false,
    });
    state.best_running = true;
    state.cancel_best();
    assert!(state.best.is_none());
    assert!(!state.best_running);
    assert!(handle.is_cancelled());
}

/// Either producer starting through the shared gate would otherwise cancel the other's lane.
#[test]
fn card_and_batch_search_exclude_each_other() {
    use super::super::state::SuggState;
    let mut state = TicksState::default();
    assert!(state.search_available());
    state.best_running = true;
    assert!(!state.search_available());
    state.best_running = false;
    state.sugg = SuggState::Running {
        handle: moon_core::db::tuner::threshold_search::SearchHandle::new(),
        total: 20,
        started: std::time::Instant::now(),
        estimate: None,
        sample: (30, 10),
    };
    assert!(!state.search_available());
}

/// An open candidate with better train profit must never replace the independently validated answer.
#[test]
fn payload_and_changes_use_the_answer_even_with_a_different_open_candidate() {
    use moon_core::db::tuner::ticks::search::{Candidate, Suggested};
    let answer = result("1");
    let suggested = Suggested {
        result: answer.outcome,
        candidate: Some(Candidate {
            values: vec![("SellPrice".into(), "999".into())],
            train: Tally::default(),
            open: 2,
            deals: 30,
        }),
        searched: vec!["SellPrice".into()],
    };
    let kept = BestResult::new(
        "1".into(),
        answer.target,
        super::validated_answer(suggested, 60),
        answer.now,
    );
    assert_eq!(kept.verdict, Verdict::Apply);
    let (_, changes, _) = bulk_payload(&[kept], &HashSet::from(["1".into()]));
    assert_eq!(changes, [vec![("SellPrice".into(), "1.5".into())]]);
}

/// Filtering targets and edits independently would misalign the per-strategy payload.
#[test]
fn payload_excludes_unchecked_and_non_apply_and_keeps_pick_order() {
    let mut reject = result("2");
    reject
        .outcome
        .as_mut()
        .unwrap()
        .holdout
        .as_mut()
        .unwrap()
        .profit = -1.0;
    let reject = BestResult::new(reject.key, reject.target, reject.outcome, reject.now);
    assert_eq!(reject.verdict, Verdict::DontApply(Why::HoldoutWorse));
    let mut miss = result("4");
    miss.outcome = Err(SearchMiss::Nothing);
    miss.verdict = Verdict::Idle;
    let results = vec![result("3"), reject, result("1"), miss, result("5")];
    let checked = HashSet::from(["1".into(), "2".into(), "3".into(), "4".into()]);
    let (targets, changes, _) = bulk_payload(&results, &checked);
    assert_eq!(targets.iter().map(|t| t.sid).collect::<Vec<_>>(), [3, 1]);
    assert_eq!(
        changes,
        vec![
            vec![("SellPrice".into(), "3.5".into())],
            vec![("SellPrice".into(), "1.5".into())]
        ]
    );
}

/// Render the production pane builders without a backend, cores or database reads.
struct PaneProbe {
    face: usize,
}

impl gpui::Render for PaneProbe {
    /// Cover running, mixed results and both forms of the empty explainer.
    fn render(
        &mut self,
        _: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{IntoElement, ParentElement, Styled};
        let p = moon_ui::MoonPalette::active(cx);
        if self.face == 4 {
            let answer = result("1");
            return crate::analytics::tuner::save::batch_group_el(
                &answer.target,
                "CORE_A",
                &answer.changes(),
                &[Some("1.25".into())],
                cx,
            );
        }
        if self.face >= 2 {
            return super::empty_pane_el(self.face == 3, p, cx);
        }
        let mut body = moon_ui::v_flex().w_full();
        for i in 1..=12 {
            let mut row = result(&i.to_string());
            if i % 3 == 0 {
                row.verdict = Verdict::DontApply(Why::HoldoutWorse);
            }
            if i % 4 == 0 {
                row.outcome = Err(SearchMiss::Nothing);
                row.verdict = Verdict::Idle;
            }
            body = body.child(super::result_card(&row, true, None, "CORE_A", p, cx));
        }
        let header = crate::analytics::tuner::list::progress_row(
            rust_i18n::t!(
                "analytics.ticks.best.progress",
                k = 2,
                n = 5,
                name = "DROPS_01"
            )
            .to_string(),
            false,
            (self.face == 0).then_some(0.3),
            false,
            p,
            cx,
        );
        let footer = super::footer_el(3, self.face == 1, "CORE_A, CORE_B", None, p, cx);
        super::shell_el(header, body.into_any_element(), footer)
    }
}

/// A scroll-body regression must not push the explicit confirmation action below the viewport.
#[gpui::test]
fn best_pane_fits_every_language_theme_and_zoom_with_a_pinned_footer(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::px;
    use moon_core::config::{Language, UiThemeMode};
    use moon_ui::MoonTheme;
    for theme in [UiThemeMode::Light, UiThemeMode::Dark] {
        cx.update(|cx| {
            MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(theme, 1.0),
                cx,
            )
        });
        for language in Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            for zoom in [1.0, 1.25] {
                for face in 0..5 {
                    let width = if face == 4 { 360.0 } else { 470.0 };
                    let window = cx
                        .open_window(gpui::size(px(width * zoom), px(620.0 * zoom)), |_, _| {
                            PaneProbe { face }
                        });
                    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                    visual.update(|window, cx| window.set_content_zoom(zoom, cx));
                    for _ in 0..3 {
                        visual.update(|w, cx| {
                            let _ = w.draw(cx);
                        });
                    }
                    if face < 2 {
                        let pane = visual.debug_bounds("an-best-pane").expect("pane");
                        let scroll = visual.debug_bounds("an-best-scroll").expect("scroll");
                        let footer = visual.debug_bounds("an-best-footer").expect("footer");
                        assert!(footer.bottom() <= pane.bottom() + px(0.5));
                        assert!(footer.top() + px(0.5) >= scroll.bottom());
                        assert!(footer.right() <= pane.right() + px(0.5));
                        assert!(scroll.size.height > px(0.0));
                    } else if face < 4 {
                        let empty = visual.debug_bounds("an-best-empty").expect("explainer");
                        assert!(empty.size.width <= px(470.0 * zoom + 0.5));
                    } else {
                        let group = visual
                            .debug_bounds("an-best-save-group")
                            .expect("per-strategy preview");
                        assert!(group.size.width <= px(360.0 * zoom + 0.5));
                    }
                }
            }
        }
    }
}
