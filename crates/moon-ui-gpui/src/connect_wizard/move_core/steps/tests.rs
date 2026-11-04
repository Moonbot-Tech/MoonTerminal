//! The move checklist's words and its layout at the wizard's narrowest width.

use std::time::Duration;

use gpui::{
    Bounds, Context, InteractiveElement, IntoElement, ParentElement, Pixels, Render, Styled,
    Window, div, px,
};
use moon_ui::{MoonTheme, v_flex};

use super::{checklist, clock, detail, title};
use crate::backend::core_move::{StepEvent, StepId, StepNote, StepState, Steps};
use crate::connect_wizard::move_core::{Answer, answer_button_look, answer_help};
use crate::connect_wizard::render::footer;

/// The wizard's minimum window width (`MIN_SIZE`): below the rail's threshold, so the main
/// column is the whole window.
const MIN_W: f32 = super::super::super::MIN_SIZE.0;

fn at(secs: u64) -> Duration {
    Duration::from_secs(secs)
}

fn mark(steps: &mut Steps, id: StepId, state: StepState, note: StepNote, secs: u64) {
    steps.apply(StepEvent::Mark { id, state, note }, at(secs));
}

/// A move whose old core refuses with open deals: the refused screen's list.
fn refused() -> Steps {
    let mut steps = Steps::default();
    mark(
        &mut steps,
        StepId::Settings,
        StepState::Done,
        StepNote::SettingsTaken {
            strategies: Some(142),
            history: true,
        },
        12,
    );
    mark(
        &mut steps,
        StepId::Server,
        StepState::Done,
        StepNote::ServerFits { port: 5160 },
        20,
    );
    mark(
        &mut steps,
        StepId::Install,
        StepState::Done,
        StepNote::Installed {
            dir: "mbcorealpha01".into(),
        },
        65,
    );
    mark(
        &mut steps,
        StepId::StopOld,
        StepState::Hold,
        StepNote::Refused {
            open_deals: Some(12),
        },
        66,
    );
    steps
}

/// A finished move whose new core logged in but whose strategies were not confirmed: the
/// longest detail lines a done list carries.
fn done() -> Steps {
    let mut steps = refused();
    mark(
        &mut steps,
        StepId::StopOld,
        StepState::Done,
        StepNote::OldStopped,
        91,
    );
    mark(
        &mut steps,
        StepId::StartNew,
        StepState::Done,
        StepNote::Started {
            logged_in: false,
            strategies: false,
        },
        134,
    );
    mark(
        &mut steps,
        StepId::Switch,
        StepState::Done,
        StepNote::Switched,
        136,
    );
    steps
}

/// The step words are the approved Russian ones; a key typo would show the raw key instead.
#[test]
fn titles_and_details_are_the_approved_words() {
    let _locale = crate::test_locale::force("ru");
    assert_eq!(title(StepId::Settings), "Забираем настройки с ядра");
    assert_eq!(title(StepId::Switch), "Переключаем терминал и станцию");
    assert_eq!(
        detail(&StepNote::SettingsTaken {
            strategies: Some(42),
            history: true
        }),
        "Биржа и ключи, стратегий: 42, история сделок"
    );
    assert_eq!(
        detail(&StepNote::ServerFits { port: 5160 }),
        "Подходит для MoonBot, порт 5160 свободен"
    );
    assert_eq!(detail(&StepNote::NotNeeded), "Не понадобилось");
    assert_eq!(clock(at(134)), "2:14");
}

/// The refused screen's list, choices and footer, as the wizard lays them out.
struct Probe {
    steps: Steps,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = moon_ui::MoonPalette::active(cx);
        let button = |answer: Answer, name: &'static str| {
            div()
                .debug_selector(move || name.into())
                .child(answer_button_look(answer).render())
                .into_any_element()
        };
        v_flex()
            .size_full()
            .child(
                v_flex()
                    .debug_selector(|| "cw-probe-main".into())
                    .flex_1()
                    .min_w(px(0.0))
                    .p(crate::design::ui_px(cx, 22.0))
                    .gap(crate::design::ui_px(cx, 12.0))
                    .child(checklist(self.steps.rows(), cx))
                    .child(answer_help()),
            )
            .child(footer(
                vec![button(Answer::Cancel, "cw-probe-cancel")],
                vec![
                    button(Answer::CheckNow, "cw-probe-retry"),
                    button(Answer::Wait, "cw-probe-wait"),
                ],
                p,
                cx,
            ))
    }
}

fn inside(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    inner.origin.x + px(0.5) >= outer.origin.x && inner.right() <= outer.right() + px(0.5)
}

/// A detail line that pushes the time out of its row, a title that runs under the mark, or
/// a footer that wraps its three answers onto two rows breaks the refused screen in some
/// language at the wizard's minimum width.
#[gpui::test]
fn every_locale_fits_the_checklist_and_refused_footer_at_the_minimum_width(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(
                moon_core::config::UiThemeMode::Light,
                1.0,
            ),
            cx,
        )
    });
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for steps in [refused(), done()] {
            let window = cx.open_window(gpui::size(px(MIN_W), px(900.0)), move |_, _| Probe {
                steps,
            });
            let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
            for _ in 0..3 {
                visual.update(|window, cx| {
                    let _ = window.draw(cx);
                });
            }
            let lang = language.code();
            let main = visual.debug_bounds("cw-probe-main").expect("main laid out");
            assert!(
                f32::from(main.size.width) <= MIN_W + 0.5,
                "{lang}: the column outgrew the window"
            );
            let list = visual.debug_bounds("cw-move-steps").expect("list laid out");
            assert!(inside(list, main), "{lang}: the list escapes the column");
            for i in 0..6 {
                let row = visual
                    .debug_bounds(sel(format!("cw-move-step-{i}")))
                    .expect("row laid out");
                assert!(inside(row, list), "{lang}: row {i} escapes the list");
                let text = visual
                    .debug_bounds(sel(format!("cw-move-step-text-{i}")))
                    .expect("text laid out");
                assert!(inside(text, row), "{lang}: row {i}'s text escapes its row");
                let title = visual
                    .debug_bounds(sel(format!("cw-move-step-title-{i}")))
                    .expect("title laid out");
                assert!(inside(title, text), "{lang}: title {i} is clipped");
                if let Some(detail) = visual.debug_bounds(sel(format!("cw-move-step-detail-{i}"))) {
                    assert!(inside(detail, text), "{lang}: detail {i} is clipped");
                    assert!(
                        f32::from(detail.size.height) > 0.0,
                        "{lang}: detail {i} collapsed"
                    );
                }
                if let Some(time) = visual.debug_bounds(sel(format!("cw-move-step-time-{i}"))) {
                    assert!(
                        time.origin.x >= text.right() && time.right() <= row.right(),
                        "{lang}: row {i}'s time overlaps its text or leaves the row"
                    );
                }
            }
            let choices = visual
                .debug_bounds("cw-move-choices")
                .expect("choices laid out");
            assert!(
                inside(choices, main),
                "{lang}: the choices escape the column"
            );
            let buttons = ["cw-probe-cancel", "cw-probe-retry", "cw-probe-wait"]
                .map(|id| visual.debug_bounds(id).expect("button laid out"));
            for b in &buttons {
                assert!(
                    b.origin.x >= px(0.0) && f32::from(b.right()) <= MIN_W + 0.5,
                    "{lang}: a footer button leaves the window"
                );
                assert!(
                    (f32::from(b.origin.y) - f32::from(buttons[0].origin.y)).abs() < 1.0,
                    "{lang}: the footer's answers wrap onto two rows: {buttons:?}"
                );
            }
        }
    }
}

fn sel(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}
