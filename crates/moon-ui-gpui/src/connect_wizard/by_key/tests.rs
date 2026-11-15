//! Fixture-only layout and attempt lifecycle regressions; vertical body overflow is allowed.

use super::logic::Footer;
use super::{ByKeyState, FailScreen, Failure, Host, Step, failure, logic, parts, steps};
use crate::connect_wizard::render::{Screen, note};
use crate::connect_wizard::{MIN_SIZE, render};
use crate::design;
use gpui::{
    App, Bounds, Context, Font, FontWeight, IntoElement, Pixels, Render, SharedString,
    TestAppContext, TextRun, VisualTestContext, Window, font, px, size,
};
use moon_core::config::{Language, UiThemeMode};
use moon_ui::MoonTheme;
use rust_i18n::t;

/// Production content builders and frame, without a live session or persistence worker.
struct Probe {
    state: &'static str,
    inputs: ByKeyState,
}

impl Render for Probe {
    /// Render every instruction and recovery body with the actual header, scroll column and footer.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state;
        let input = &self.inputs;
        let screen = match state {
            "port-server" | "port-local" => steps::port_screen(
                steps::PortOptions {
                    host: if state == "port-local" {
                        Host::ThisComputer
                    } else {
                        Host::WindowsServer
                    },
                    command_port: 3000,
                    copied: false,
                },
                &input.port,
                |_, _, _, _| {},
                |_, _, _| {},
                |_, _, _| {},
                cx,
            ),
            "moonbot" => steps::moonbot_screen(3000, cx),
            "key" | "key-mismatch" => steps::key_screen(
                &input.name,
                &input.key,
                (state == "key-mismatch").then_some(5010),
                3000,
                |_, _, _| {},
                cx,
            ),
            "connecting" => steps::connecting_screen("203.0.113.24:3000", cx),
            "noresp-silent" | "noresp-partial" => failure::no_response_screen(
                failure::NoResponse {
                    port: 3000,
                    address: "203.0.113.24:3000",
                    seconds: 15,
                    silent: state == "noresp-silent",
                    fault: t!("connect_wizard.by_key.connecting_note").to_string(),
                    copied: false,
                },
                |_, _, _| {},
                cx,
            ),
            "refused" | "refused-mismatch" => failure::key_refused_screen(
                &input.key,
                (state == "refused-mismatch").then_some(5010),
                3000,
                |_, _, _| {},
                cx,
            ),
            "noaddr" => failure::no_address_screen(&input.ip, 3000, cx),
            "fallback" => Screen::new(t!("connect_wizard.by_key.fail_title"))
                .child(note(t!("connect_wizard.by_key.connecting_note"), cx)),
            // The session-owned done_body cannot be constructed without a live Backend.
            "done" => Screen::new(t!("connect_wizard.done.title"))
                .child(note(t!("connect_wizard.done.license_pending"), cx)),
            _ => unreachable!(),
        };
        let footer = match state {
            "connecting" => Footer::Connecting,
            "noresp-silent" | "noresp-partial" => Footer::Failure(FailScreen::NoResponse {
                silent: state == "noresp-silent",
            }),
            "refused" | "refused-mismatch" => Footer::Failure(FailScreen::KeyRefused),
            "noaddr" => Footer::Failure(FailScreen::NoAddress),
            "fallback" => Footer::Failure(FailScreen::Fallback),
            "done" => Footer::Done,
            "moonbot" => Footer::Instructions(Step::Moonbot),
            "port-server" | "port-local" => Footer::Instructions(Step::Port),
            "key" | "key-mismatch" => Footer::Instructions(Step::Key),
            _ => unreachable!(),
        };
        let screen = parts::with_footer(
            screen,
            footer,
            matches!(state, "key-mismatch" | "refused-mismatch").then_some(5010),
            |_, _, _, _| {},
        );
        let main = render::screen_body(screen.title, screen.sub, screen.body, true, cx);
        render::frame(
            main,
            screen.left,
            screen.right,
            [
                "connect_wizard.rail.way",
                "connect_wizard.rail.port",
                "connect_wizard.rail.moonbot",
                "connect_wizard.rail.key",
                "connect_wizard.rail.done",
            ]
            .into_iter()
            .map(|key| t!(key).to_string())
            .collect(),
            if state == "done" { 4 } else { 3 },
            f32::from(window.viewport_size().width),
            cx,
        )
    }
}

/// Long translations must stay within the 416/386 px column and retain their measured text height.
/// The production body scrolls vertically, so its bottom may extend below the viewport.
#[gpui::test]
fn all_by_key_states_fit_both_minimum_columns(cx: &mut TestAppContext) {
    crate::test_locale::warm();
    cx.update(|app| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Light, 1.0),
            app,
        )
    });
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for width in [MIN_SIZE.0, render::RAIL_MIN_WINDOW_W] {
            for state in [
                "port-server",
                "port-local",
                "moonbot",
                "key",
                "key-mismatch",
                "connecting",
                "noresp-silent",
                "noresp-partial",
                "refused",
                "refused-mismatch",
                "noaddr",
                "fallback",
                "done",
            ] {
                let handle = cx.open_window(size(px(width), px(MIN_SIZE.1)), |window, cx| Probe {
                    state,
                    inputs: ByKeyState::new(window, cx),
                });
                let mut visual = VisualTestContext::from_window(handle.into(), cx);
                for _ in 0..3 {
                    visual.update(|window, app| {
                        let _ = window.draw(app);
                    });
                }
                let x = if width >= render::RAIL_MIN_WINDOW_W {
                    render::RAIL_W
                } else {
                    0.0
                } + 22.0;
                let inside = |bounds: Bounds<Pixels>| {
                    bounds.origin.x >= px(x - 0.5) && bounds.right() <= px(width - 21.5)
                };
                let mut previous = visual.debug_bounds("cw-screen-title").expect("title");
                assert!(inside(previous), "title outside content");
                let body_count = match state {
                    "port-server" => 7,
                    "port-local" => 4,
                    "moonbot" | "key-mismatch" | "connecting" | "refused" => 2,
                    "refused-mismatch" => 3,
                    _ => 1,
                };
                let mut selectors = Vec::new();
                if !matches!(state, "connecting" | "fallback" | "done") {
                    selectors.push("cw-screen-sub");
                }
                selectors.extend(
                    [
                        "cw-key-body-0",
                        "cw-key-body-1",
                        "cw-key-body-2",
                        "cw-key-body-3",
                        "cw-key-body-4",
                        "cw-key-body-5",
                        "cw-key-body-6",
                    ]
                    .into_iter()
                    .take(body_count),
                );
                for selector in selectors {
                    let bounds = visual
                        .debug_bounds(selector)
                        .unwrap_or_else(|| panic!("{state}: missing {selector}"));
                    assert!(
                        inside(bounds),
                        "{language:?}/{width}/{state}/{selector}: column overflow {bounds:?}"
                    );
                    assert!(
                        previous.bottom() <= bounds.origin.y + px(0.5),
                        "{language:?}/{state}: body siblings overlap"
                    );
                    previous = bounds;
                }
                let mut controls = match state {
                    "port-server" | "port-local" => vec![
                        "cw-key-seg",
                        "cw-key-port",
                        "cw-key-linux",
                        "cw-key-linux-note",
                        "cw-key-port-hint",
                    ],
                    "moonbot" => vec!["cw-key-steps"],
                    "key" | "key-mismatch" => {
                        vec!["cw-key-name", "cw-key-box", "cw-key-paste", "cw-key-key"]
                    }
                    "noresp-silent" | "noresp-partial" => vec!["cw-key-steps"],
                    "refused" | "refused-mismatch" => {
                        vec!["cw-key-steps", "cw-key-box", "cw-key-paste", "cw-key-key"]
                    }
                    "noaddr" => vec!["cw-key-ip"],
                    _ => vec![],
                };
                if state == "port-server" || state.starts_with("noresp") {
                    controls.extend(["cw-key-code", "cw-key-copy", "cw-key-command-text"]);
                }
                for selector in controls {
                    let bounds = visual
                        .debug_bounds(selector)
                        .unwrap_or_else(|| panic!("{state}: missing {selector}"));
                    assert!(
                        inside(bounds),
                        "{language:?}/{width}/{state}/{selector}: outside content {bounds:?}"
                    );
                }
                if state.starts_with("refused") {
                    assert!(
                        visual.debug_bounds("cw-key-name").is_none(),
                        "replacement name is never applied"
                    );
                }
                if state.starts_with("port-") {
                    for (selector, key) in [
                        ("cw-key-linux-note", "connect_wizard.by_key.linux_note"),
                        ("cw-key-port-hint", "connect_wizard.by_key.port_hint"),
                    ] {
                        let bounds = visual.debug_bounds(selector).expect("required note");
                        text_fits(
                            &mut visual,
                            bounds,
                            t!(key).to_string(),
                            design::ui_font(),
                            FontWeight::NORMAL,
                            design::t_body,
                        );
                    }
                }
                if state == "port-server" || state == "port-local" {
                    assert!(visual.debug_bounds("cw-key-seg").is_some());
                }
                if state == "port-server" || state.starts_with("noresp") {
                    assert!(visual.debug_bounds("cw-key-code").is_some());
                    let bounds = visual.debug_bounds("cw-key-command-text").expect("command");
                    text_fits(
                        &mut visual,
                        bounds,
                        logic::firewall_command(3000),
                        design::mono(),
                        FontWeight::NORMAL,
                        design::t_caption,
                    );
                }
                let footer: &[&str] = match state {
                    "connecting" => &["cw-key-cancel"],
                    "noresp-silent" | "noresp-partial" | "fallback" => {
                        &["cw-key-drop", "cw-key-retry"]
                    }
                    "refused" => &["cw-key-drop", "cw-key-new-key"],
                    "refused-mismatch" => &["cw-key-drop", "cw-key-accept", "cw-key-repaste"],
                    "noaddr" => &["cw-key-drop", "cw-key-connect"],
                    "done" => &["cw-again", "cw-finish"],
                    "port-server" | "port-local" | "moonbot" => &["cw-key-back", "cw-key-next"],
                    "key-mismatch" => &["cw-key-back", "cw-key-accept", "cw-key-repaste"],
                    "key" => &["cw-key-back", "cw-key-connect"],
                    _ => unreachable!(),
                };
                let mut button_bounds: Vec<Bounds<Pixels>> = Vec::new();
                for selector in footer {
                    let bounds = visual
                        .debug_bounds(selector)
                        .unwrap_or_else(|| panic!("{state}: missing footer {selector}"));
                    assert!(
                        bounds.origin.x >= px(0.0) && bounds.right() <= px(width),
                        "{language:?}/{state}: footer outside window"
                    );
                    for other in &button_bounds {
                        assert!(
                            bounds.right() <= other.origin.x + px(0.5)
                                || other.right() <= bounds.origin.x + px(0.5)
                                || bounds.bottom() <= other.origin.y + px(0.5)
                                || other.bottom() <= bounds.origin.y + px(0.5),
                            "{language:?}/{width}/{state}: footer buttons overlap {bounds:?} {other:?}"
                        );
                    }
                    button_bounds.push(bounds);
                }
                let title = match state {
                    "port-server" | "port-local" => "connect_wizard.by_key.port_title",
                    "moonbot" => "connect_wizard.by_key.moonbot_title",
                    "key" | "key-mismatch" => "connect_wizard.by_key.key_title",
                    "connecting" => "connect_wizard.by_key.connecting_title",
                    "noresp-silent" | "noresp-partial" => "connect_wizard.by_key.noresp_title",
                    "refused" | "refused-mismatch" => "connect_wizard.by_key.refused_title",
                    "noaddr" => "connect_wizard.by_key.noaddr_title",
                    "fallback" => "connect_wizard.by_key.fail_title",
                    _ => "connect_wizard.done.title",
                };
                let bounds = visual.debug_bounds("cw-screen-title").unwrap();
                text_fits(
                    &mut visual,
                    bounds,
                    t!(title).to_string(),
                    design::ui_font(),
                    FontWeight::SEMIBOLD,
                    design::t_title,
                );
            }
        }
    }
}

/// Independently shape the visible prose so a fixed-height text wrapper cannot hide wrapped lines.
fn text_fits(
    visual: &mut VisualTestContext,
    bounds: Bounds<Pixels>,
    text: String,
    family: SharedString,
    weight: FontWeight,
    font_size: fn(&App) -> Pixels,
) {
    let height = visual.update(|window, app| {
        let runs = [TextRun {
            len: text.len(),
            font: Font {
                weight,
                ..font(family)
            },
            ..TextRun::default()
        }];
        window
            .text_system()
            .shape_text(
                text.into(),
                font_size(app),
                &runs,
                Some(bounds.size.width),
                None,
            )
            .unwrap()
            .iter()
            .map(|line| f32::from(line.size(design::line_px(app, 14.0)).height))
            .sum::<f32>()
    });
    assert!(
        height <= f32::from(bounds.size.height) + 0.5,
        "text clipped: needs {height}, has {:?}",
        bounds.size.height
    );
}

/// Cancellation must delete a new row only, retain failed deletions, and leave reused rows intact.
#[gpui::test]
fn attempt_ownership_and_failure_latch(cx: &mut TestAppContext) {
    let _locale = crate::test_locale::force("en");
    cx.update(|app| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Light, 1.0),
            app,
        )
    });
    let handle = cx.open_window(size(px(460.0), px(420.0)), |window, cx| Probe {
        state: "key",
        inputs: ByKeyState::new(window, cx),
    });
    handle
        .update(cx, |probe, _, _| {
            let state = &mut probe.inputs;
            for created in [false, true] {
                state.uid = Some(42);
                state.created = created;
                let mut deleted = Vec::new();
                state
                    .abandon(|uid| {
                        deleted.push(uid);
                        Ok(())
                    })
                    .unwrap();
                assert_eq!(deleted, if created { vec![42] } else { vec![] });
                assert_eq!(state.uid, None);
                assert_eq!(state.step, Step::Key);
            }
            state.uid = Some(42);
            state.created = true;
            assert!(
                state
                    .abandon(|_| Err(anyhow::anyhow!("disk failure")))
                    .is_err()
            );
            assert_eq!(state.uid, Some(42));
            let first = Failure {
                screen: FailScreen::KeyRefused,
                address: String::new(),
                port: 3000,
                seconds: 0,
                detail: String::new(),
            };
            state.retain_failure(false, Some(first));
            // Failed -> Connecting has no new observed failure and must keep the same screen.
            state.retain_failure(false, None);
            assert_eq!(
                state.failure.as_ref().unwrap().screen,
                FailScreen::KeyRefused
            );
            state.retain_failure(true, None);
            assert!(state.failure.is_none());
        })
        .unwrap();
}
