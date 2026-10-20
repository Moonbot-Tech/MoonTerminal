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

/// Build the five approved states, including positive profit that is below Fact.
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
            holdout: (face != 2).then(|| tally(if face == 3 { 5.0 } else { -2.0 })),
            fact_holdout: (face != 2).then(|| tally(if face == 3 { 1.0 } else { 3.0 })),
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

/// A headless view renders the production config and validation components.
struct Probe {
    input: Entity<MoonInputState>,
    face: usize,
}

impl Render for Probe {
    /// Stack the real controls and completed-state block inside the tested dock width.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let select = train_dropdown_el(TunerKind::Time, 100, |_, _| {}, cx);
        v_flex()
            .w_full()
            .debug_selector(|| "time-probe".into())
            .child(super::super::controls::config_row(
                &self.input,
                select,
                false,
                |_, _, _| {},
                p,
                cx,
            ))
            .children(summary(result(self.face).as_ref(), p, cx))
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
                for face in 0..5 {
                    let window =
                        cx.open_window(gpui::size(px(width), px(600.0)), |window, cx| Probe {
                            input: cx.new(|cx| MoonInputState::new(window, cx)),
                            face,
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
                            &crate::analytics::tuner::shell::train_label(100),
                            400.0,
                        )
                    });
                    assert!(
                        select.size.width >= px(need),
                        "selector narrower than its label"
                    );
                    assert_eq!(visual.debug_bounds("time-validation").is_some(), face != 0);
                    if matches!(face, 1 | 2 | 4) {
                        let warn = visual.debug_bounds("time-warning").unwrap();
                        assert!(warn.right() <= root.right() + px(0.5));
                        assert!(warn.size.height >= px(if width == 320.0 { 20.0 } else { 10.0 }));
                    }
                }
            }
        }
    }
}
