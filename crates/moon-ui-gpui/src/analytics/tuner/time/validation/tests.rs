use super::{summary, warning};
use crate::analytics::tuner::{shared::TunerKind, split::train_dropdown_el};
use gpui::{
    AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render, Styled,
    Window, px,
};
use moon_core::config::{Language, UiThemeMode};
use moon_core::db::metrics::Tally;
use moon_core::db::tuner::TimeEvaluation;
use moon_ui::{MoonInputState, MoonPalette, MoonTheme, v_flex};

/// Build idle, refused, unsplit, improved, losing and positive-but-below-Fact states.
fn result(face: usize) -> Option<TimeEvaluation> {
    let tally = |p| {
        let mut t = Tally::default();
        t.push(p);
        t
    };
    match face {
        0 => None,
        1 => Some(TimeEvaluation::default()),
        _ => Some(TimeEvaluation {
            train: Some(tally(20.0)),
            holdout: (face != 2).then(|| tally(if matches!(face, 3 | 5) { 5.0 } else { -2.0 })),
            fact_holdout: (face != 2).then(|| tally(if face == 3 { 1.0 } else { 6.0 })),
            ..Default::default()
        }),
    }
}

/// Idle renders nothing, full-history warns, and unseen deterioration warns against Fact.
#[test]
fn validation_state_decision() {
    assert!(result(0).is_none());
    assert_eq!(
        warning(&result(1).unwrap()),
        Some("analytics.tuner.time_small")
    );
    assert_eq!(
        warning(&result(2).unwrap()),
        Some("analytics.tuner.time_no_split")
    );
    assert_eq!(warning(&result(3).unwrap()), None);
    assert_eq!(
        warning(&result(4).unwrap()),
        Some("analytics.tuner.time_below_fact")
    );
    let mut positive = result(3).unwrap();
    positive.fact_holdout.as_mut().unwrap().profit = 6.0;
    assert_eq!(warning(&positive), Some("analytics.tuner.time_below_fact"));
}

/// Reverting the production holdout renderer to profit-sign logic must change its assigned style.
#[gpui::test]
fn positive_below_fact_renders_danger(cx: &mut gpui::TestAppContext) {
    let _locale = crate::test_locale::force("en");
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Light, 1.0),
            cx,
        );
        let p = MoonPalette::active(cx);
        let mut line = super::holdout_line(&result(5).unwrap(), p, cx).unwrap();
        assert_eq!(
            line.style().text.color,
            Some(crate::design::moon(crate::design::danger_color(p))),
            "the actual holdout renderer must assign danger, despite its positive profit"
        );
    });
    let window = cx.open_window(gpui::size(px(540.0), px(600.0)), |window, cx| Probe {
        input: cx.new(|cx| MoonInputState::new(window, cx)),
        face: 5,
        report_changed: false,
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(visual.debug_bounds("time-holdout-line").is_some());
    assert!(visual.debug_bounds("time-train-line").is_some());
    assert!(visual.debug_bounds("split-fact-holdout-line").is_some());
    assert!(visual.debug_bounds("time-warning").is_some());
}

/// A headless view renders the production config and validation components.
struct Probe {
    input: Entity<MoonInputState>,
    face: usize,
    report_changed: bool,
}

impl Render for Probe {
    /// Stack the real controls and completed-state block inside the tested dock width.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let mut state = super::super::state::TimeTunerState::load();
        state.min_trades = "11".into();
        state.evaluation = result(self.face);
        state.train_pct = if self.face >= 3 { 70 } else { 100 };
        if self.report_changed {
            state.mark_report_stale();
        }
        // The production Change/Blur handler receives the same raw value after completion.
        state.set_min_trades("11".into());
        let select = train_dropdown_el(TunerKind::Time, state.train_pct, |_, _| {}, cx);
        v_flex()
            .w_full()
            .debug_selector(|| "time-probe".into())
            .child(super::super::controls::config_row(
                &self.input,
                select,
                false,
                gpui::div().into_any_element(),
                |_, _, _| {},
                p,
                cx,
            ))
            .children(summary(
                state.evaluation.as_ref(),
                state.evaluation_stale,
                p,
                cx,
            ))
    }
}

/// Production controls stay in bounds across all locales and themes; orange prose gains height.
#[gpui::test]
fn time_validation_layout(cx: &mut gpui::TestAppContext) {
    for mode in [UiThemeMode::Light, UiThemeMode::Dark, UiThemeMode::Graphite] {
        cx.update(|cx| {
            MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(mode, 1.0),
                cx,
            )
        });
        for language in Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            for width in [320.0, 540.0] {
                for face in 0..6 {
                    for report_changed in [false, true] {
                        let window =
                            cx.open_window(gpui::size(px(width), px(600.0)), |window, cx| Probe {
                                input: cx.new(|cx| MoonInputState::new(window, cx)),
                                face,
                                report_changed,
                            });
                        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                        for _ in 0..3 {
                            visual.update(|window, cx| {
                                let _ = window.draw(cx);
                            });
                        }
                        let root = visual.debug_bounds("time-probe").unwrap();
                        for sel in ["time-config", "time-train-select", "time-run"] {
                            let b = visual.debug_bounds(sel).unwrap();
                            assert!(
                                b.origin.x >= root.origin.x && b.right() <= root.right() + px(0.5),
                                "{sel} {} {width}",
                                language.code()
                            );
                        }
                        let run = visual.debug_bounds("time-run").unwrap();
                        let inset = visual.update(|_, cx| crate::design::ui_px(cx, 12.0));
                        assert!(
                            (run.right() - (root.right() - inset)).abs() <= px(0.5),
                            "run must stay right-aligned after wrapping"
                        );
                        let select = visual.debug_bounds("time-train-select").unwrap();
                        let need = visual.update(|_, cx| {
                            crate::design::ui_caption_text_width(
                                cx,
                                &crate::analytics::tuner::shell::train_label(if face >= 3 {
                                    70
                                } else {
                                    100
                                }),
                                400.0,
                            )
                        });
                        assert!(
                            select.size.width >= px(need),
                            "selector narrower than its label"
                        );
                        assert_eq!(visual.debug_bounds("time-validation").is_some(), face != 0);
                        for selector in ["time-result-line", "time-train-line"] {
                            assert_eq!(
                                visual.debug_bounds(selector).is_some(),
                                face >= 2,
                                "completed suggestion lost {selector}"
                            );
                        }
                        assert_eq!(
                            visual.debug_bounds("time-holdout-line").is_some(),
                            face >= 3
                        );
                        assert_eq!(
                            visual.debug_bounds("split-fact-holdout-line").is_some(),
                            face >= 3
                        );
                        assert_eq!(
                            visual.debug_bounds("time-stale-warning").is_some(),
                            report_changed && face != 0
                        );
                        if matches!(face, 1 | 2 | 4 | 5) {
                            let warn = visual.debug_bounds("time-warning").unwrap();
                            assert!(warn.right() <= root.right() + px(0.5));
                            assert!(
                                warn.size.height >= px(if width == 320.0 { 20.0 } else { 10.0 })
                            );
                        }
                    }
                }
            }
        }
    }
}

/// An unchanged Blur preserves a fitted summary and request; a real budget edit retires both.
#[test]
fn livefix_time_result_survives_an_unchanged_settings_blur() {
    let mut state = super::super::state::TimeTunerState::load();
    state.min_trades = "11".into();
    state.evaluation = result(3);
    state.sugg_busy = true;
    let generation = state.sugg_seq;
    state.set_min_trades("11".into());
    assert!(state.evaluation.is_some());
    assert!(state.sugg_busy);
    assert_eq!(state.sugg_seq, generation);
    state.set_min_trades("12".into());
    assert!(state.evaluation.is_none());
    assert!(!state.sugg_busy);
    assert_ne!(state.sugg_seq, generation);
}

/// A report append must retire active requests without deleting the completed validation snapshot.
#[test]
fn livefix_completed_time_validation_survives_report_append() {
    let mut state = super::super::state::TimeTunerState::load();
    state.min_trades = "11".into();
    state.evaluation = result(3);
    state.sugg_busy = true;
    let generation = state.sugg_seq;
    state.mark_report_stale();
    assert!(
        state.evaluation.is_some(),
        "a background report append erased the completed validation block"
    );
    assert!(!state.sugg_busy);
    assert!(state.evaluation_stale);
    assert_ne!(state.sugg_seq, generation);
    state.set_min_trades("11".into());
    assert!(state.evaluation.is_some() && state.evaluation_stale);
    state.set_min_trades("12".into());
    assert!(
        state.evaluation.is_none(),
        "real settings edits must retire the snapshot"
    );
    assert!(!state.evaluation_stale);
}
