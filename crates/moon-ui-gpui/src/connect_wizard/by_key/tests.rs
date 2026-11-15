//! Fixture-only layout and attempt lifecycle regressions; vertical body overflow is allowed.

use gpui::{Bounds, Context, Font, FontWeight, InteractiveElement, IntoElement, ParentElement,
    Pixels, Render, SharedString, TestAppContext, TextRun, VisualTestContext, Window,
    div, font, px, size};
use moon_core::config::{Language, UiThemeMode};
use moon_ui::{MoonButton, MoonTheme};
use rust_i18n::t;
use super::{ByKeyState, FailScreen, Failure, Host, Step, failure, logic, steps};
use crate::connect_wizard::render::{Screen, note};
use crate::design;
use crate::connect_wizard::{MIN_SIZE, render};

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
        let mut screen = match state {
            "port-server" | "port-local" => steps::port_screen(steps::PortOptions {
                host: if state == "port-local" { Host::ThisComputer } else { Host::WindowsServer },
                command_port: 3000, copied: false,
            }, &input.port, |_, _, _, _| {}, |_, _, _| {}, |_, _, _| {}, cx),
            "moonbot" => steps::moonbot_screen(3000, cx),
            "key" | "key-mismatch" => steps::key_screen(&input.name, &input.key,
                (state == "key-mismatch").then_some(5010), 3000, |_, _, _| {}, cx),
            "connecting" => steps::connecting_screen("203.0.113.24:3000", cx),
            "noresp-silent" | "noresp-partial" => failure::no_response_screen(failure::NoResponse {
                port: 3000, address: "203.0.113.24:3000", seconds: 15,
                silent: state == "noresp-silent", fault: t!("connect_wizard.by_key.connecting_note").to_string(), copied: false,
            }, |_, _, _| {}, cx),
            "refused" => failure::key_refused_screen(&input.name, &input.key, |_, _, _| {}, cx),
            "noaddr" => failure::no_address_screen(&input.ip, 3000, cx),
            // The session-owned done_body cannot be constructed without a live Backend.
            "done" => Screen::new(t!("connect_wizard.done.title"))
                .child(note(t!("connect_wizard.done.license_pending"), cx)),
            _ => unreachable!(),
        };
        let (left, right) = match state {
            "connecting" => (None, "connect_wizard.cancel"),
            "noresp-silent" | "noresp-partial" => (Some("connect_wizard.by_key.drop"), "connect_wizard.check_again"),
            "refused" => (Some("connect_wizard.by_key.drop"), "connect_wizard.by_key.refused_connect"),
            "noaddr" => (Some("connect_wizard.by_key.drop"), "connect_wizard.by_key.connect"),
            "done" => (Some("connect_wizard.again"), "connect_wizard.finish"),
            "moonbot" => (Some("connect_wizard.back"), "connect_wizard.by_key.key_copied"),
            "port-server" | "port-local" => (Some("connect_wizard.back"), "connect_wizard.next"),
            "key-mismatch" => (Some("connect_wizard.back"), "connect_wizard.by_key.paste_again"),
            _ => (Some("connect_wizard.back"), "connect_wizard.by_key.connect"),
        };
        if let Some(left) = left {
            screen = screen.left(div().id("cw-test-left").debug_selector(|| "cw-test-left".into()).child(MoonButton::new("cw-test-left-action")
                .size(design::CONTROL_TIER).label(t!(left).to_string()).render()));
        }
        if state == "key-mismatch" {
            screen = screen.right(div().id("cw-test-accept").debug_selector(|| "cw-test-accept".into()).child(MoonButton::new("cw-test-accept-action")
                .size(design::CONTROL_TIER).label(t!("connect_wizard.by_key.connect_with_port", port = 5010).to_string()).render()));
        }
        screen = screen.right(div().id("cw-test-right").debug_selector(|| "cw-test-right".into()).child(MoonButton::new("cw-test-right-action")
            .primary().size(design::CONTROL_TIER).label(t!(right).to_string()).render()));
        let main = render::screen_body(screen.title, screen.sub, screen.body, true, cx);
        render::frame(main, screen.left, screen.right,
            ["connect_wizard.rail.way", "connect_wizard.rail.port", "connect_wizard.rail.moonbot", "connect_wizard.rail.key", "connect_wizard.rail.done"]
                .into_iter().map(|key| t!(key).to_string()).collect(),
            if state == "done" { 4 } else { 3 }, f32::from(window.viewport_size().width), cx)
    }
}

/// Long translations must stay within the 416/386 px column and retain their measured text height.
/// The production body scrolls vertically, so its bottom may extend below the viewport.
#[gpui::test]
fn all_by_key_states_fit_both_minimum_columns(cx: &mut TestAppContext) {
    crate::test_locale::warm();
    cx.update(|app| MoonTheme::install_config(
        crate::startup::moon_theme_config_for_presentation(UiThemeMode::Light, 1.0), app));
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for width in [MIN_SIZE.0, render::RAIL_MIN_WINDOW_W] {
            for state in ["port-server", "port-local", "moonbot", "key", "key-mismatch", "connecting",
                "noresp-silent", "noresp-partial", "refused", "noaddr", "done"] {
                let handle = cx.open_window(size(px(width), px(MIN_SIZE.1)), |window, cx| Probe {
                    state, inputs: ByKeyState::new(window, cx),
                });
                let mut visual = VisualTestContext::from_window(handle.into(), cx);
                for _ in 0..3 { visual.update(|window, app| { let _ = window.draw(app); }); }
                let x = if width >= render::RAIL_MIN_WINDOW_W { render::RAIL_W } else { 0.0 } + 22.0;
                let inside = |bounds: Bounds<Pixels>| bounds.origin.x >= px(x - 0.5) && bounds.right() <= px(width - 21.5);
                let mut previous = visual.debug_bounds("cw-screen-title").expect("title");
                for selector in ["cw-screen-sub", "cw-key-body-0", "cw-key-body-1", "cw-key-body-2", "cw-key-body-3",
                    "cw-key-body-4", "cw-key-body-5", "cw-key-body-6", "cw-key-body-7", "cw-key-body-8", "cw-key-body-9"] {
                    if let Some(bounds) = visual.debug_bounds(selector) {
                        assert!(inside(bounds), "{language:?}/{width}/{state}/{selector}: column overflow {bounds:?}");
                        assert!(previous.bottom() <= bounds.origin.y + px(0.5), "{language:?}/{state}: body siblings overlap");
                        previous = bounds;
                    }
                }
                for selector in ["cw-key-seg", "cw-key-code", "cw-key-copy", "cw-key-port", "cw-key-linux",
                    "cw-key-steps", "cw-key-box", "cw-key-paste", "cw-key-ip", "cw-key-name", "cw-key-key"] {
                    if let Some(bounds) = visual.debug_bounds(selector) {
                        assert!(inside(bounds), "{language:?}/{width}/{state}/{selector}: outside content {bounds:?}");
                    }
                }
                if state == "port-server" || state == "port-local" { assert!(visual.debug_bounds("cw-key-seg").is_some()); }
                if state == "port-server" || state.starts_with("noresp") {
                    assert!(visual.debug_bounds("cw-key-code").is_some());
                    let bounds = visual.debug_bounds("cw-key-command-text").expect("command");
                    text_fits(&mut visual, bounds, logic::firewall_command(3000), design::mono(), FontWeight::NORMAL, true);
                }
                for selector in ["cw-test-left", "cw-test-accept", "cw-test-right"] {
                    if let Some(bounds) = visual.debug_bounds(selector) {
                        assert!(bounds.origin.x >= px(0.0) && bounds.right() <= px(width), "{language:?}/{state}: footer outside window");
                    }
                }
                let title = match state {
                    "port-server" | "port-local" => "connect_wizard.by_key.port_title",
                    "moonbot" => "connect_wizard.by_key.moonbot_title",
                    "key" | "key-mismatch" => "connect_wizard.by_key.key_title",
                    "connecting" => "connect_wizard.by_key.connecting_title",
                    "noresp-silent" | "noresp-partial" => "connect_wizard.by_key.noresp_title",
                    "refused" => "connect_wizard.by_key.refused_title",
                    "noaddr" => "connect_wizard.by_key.noaddr_title",
                    _ => "connect_wizard.done.title",
                };
                let bounds = visual.debug_bounds("cw-screen-title").unwrap();
                text_fits(&mut visual, bounds, t!(title).to_string(), design::ui_font(), FontWeight::SEMIBOLD, false);
            }
        }
    }
}

/// Independently shape the visible prose so a fixed-height text wrapper cannot hide wrapped lines.
fn text_fits(visual: &mut VisualTestContext, bounds: Bounds<Pixels>, text: String, family: SharedString, weight: FontWeight, caption: bool) {
    let height = visual.update(|window, app| {
        let runs = [TextRun { len: text.len(), font: Font { weight, ..font(family) }, ..TextRun::default() }];
        window.text_system().shape_text(text.into(), if caption { design::t_caption(app) } else { design::t_title(app) },
            &runs, Some(bounds.size.width), None).unwrap().iter()
            .map(|line| f32::from(line.size(design::line_px(app, 14.0)).height)).sum::<f32>()
    });
    assert!(height <= f32::from(bounds.size.height) + 0.5, "text clipped: needs {height}, has {:?}", bounds.size.height);
}

/// Cancellation must delete a new row only, retain failed deletions, and leave reused rows intact.
#[gpui::test]
fn attempt_ownership_and_failure_latch(cx: &mut TestAppContext) {
    let _locale = crate::test_locale::force("en");
    cx.update(|app| MoonTheme::install_config(
        crate::startup::moon_theme_config_for_presentation(UiThemeMode::Light, 1.0), app));
    let handle = cx.open_window(size(px(460.0), px(420.0)), |window, cx| Probe { state: "key", inputs: ByKeyState::new(window, cx) });
    handle.update(cx, |probe, _, _| {
        let state = &mut probe.inputs;
        for created in [false, true] {
            state.uid = Some(42); state.created = created;
            let mut deleted = Vec::new();
            state.abandon(|uid| { deleted.push(uid); Ok(()) }).unwrap();
            assert_eq!(deleted, if created { vec![42] } else { vec![] });
            assert_eq!(state.uid, None);
            assert_eq!(state.step, Step::Key);
        }
        state.uid = Some(42); state.created = true;
        assert!(state.abandon(|_| Err(anyhow::anyhow!("disk failure"))).is_err());
        assert_eq!(state.uid, Some(42));
        let first = Failure { screen: FailScreen::KeyRefused, address: String::new(), port: 3000, seconds: 0, detail: String::new() };
        state.retain_failure(false, Some(first));
        // Failed -> Connecting has no new observed failure and must keep the same screen.
        state.retain_failure(false, None);
        assert_eq!(state.failure.as_ref().unwrap().screen, FailScreen::KeyRefused);
        state.retain_failure(true, None);
        assert!(state.failure.is_none());
    }).unwrap();
}
