//! Instruction screen bodies, ready to receive the wizard's action callbacks.

use std::rc::Rc;

use gpui::*;
use moon_ui::{MoonInput, MoonInputState, MoonLink, MoonSegmentItem, MoonSegmentedControl, MoonSpinner, h_flex, v_flex};
use rust_i18n::t;

use super::logic::Host;
use super::parts::{self, Callout};
use crate::connect_wizard::render::{Screen, note};
use crate::design;

/// Display facts for the port instructions, independent of the input's pending text.
pub(super) struct PortOptions {
    pub(super) host: Host,
    pub(super) command_port: u16,
    pub(super) copied: bool,
}

/// Port instructions adapt to a remote Windows server or Moonbot on this computer.
pub(super) fn port_screen(
    options: PortOptions,
    port: &Entity<MoonInputState>,
    on_host: impl Fn(Host, &ClickEvent, &mut Window, &mut App) + 'static,
    on_linux: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_copy: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Screen {
    let PortOptions { host, command_port, copied } = options;
    let on_host = Rc::new(on_host);
    // Separate fitted cells can wrap as whole controls in long translations.
    let segments = h_flex().id("cw-key-seg").w_full().min_w(px(0.0)).flex_wrap()
        .gap(design::ui_px(cx, 4.0))
        .children([
            (Host::WindowsServer, "cw-key-server", t!("connect_wizard.by_key.where_server").to_string()),
            (Host::ThisComputer, "cw-key-local", t!("connect_wizard.by_key.where_local").to_string()),
        ].into_iter().map(|(value, id, title)| {
            let on_host = on_host.clone();
            MoonSegmentedControl::new(id)
                .item(MoonSegmentItem::new("", title).fit_width(cx, 0.0, f32::MAX).selected(host == value))
                .on_click(move |_, event, window, cx| on_host(value, event, window, cx))
                .into_any_element()
        }));
    let mut screen = Screen::new(t!("connect_wizard.by_key.port_title"))
        .sub(t!("connect_wizard.by_key.port_sub"))
        .child(v_flex().w_full().min_w(px(0.0)).gap(design::ui_px(cx, 4.0))
            .child(note(t!("connect_wizard.by_key.where"), cx)).child(segments))
        .child(h_flex().w_full().min_w(px(0.0)).flex_wrap().gap(design::ui_px(cx, 4.0))
            .child(note(t!("connect_wizard.by_key.linux_note"), cx))
            .child(MoonLink::new("cw-key-linux", t!("connect_wizard.start.new_title").to_string()).on_click(on_linux)))
        .child(v_flex().w_full().min_w(px(0.0)).gap(design::ui_px(cx, 4.0))
            .child(note(t!("connect_wizard.server.port"), cx))
            .child(h_flex().w_full().flex_wrap().items_center().gap(design::ui_px(cx, 10.0))
                .child(div().w(design::ui_px(cx, 90.0)).flex_none()
                    .child(MoonInput::new("cw-key-port").state(port).size(design::INPUT_SIZE)))
                .child(note(t!("connect_wizard.by_key.port_hint"), cx))));
    screen = match host {
        Host::ThisComputer => screen.child(parts::callout(Callout::Ok,
            t!("connect_wizard.by_key.local_title").to_string(),
            t!("connect_wizard.by_key.local_body").to_string(), cx)),
        Host::WindowsServer => screen
            .child(note(t!("connect_wizard.by_key.command"), cx))
            .child(parts::code_box(command_port, copied, on_copy, cx))
            .child(note(t!("connect_wizard.by_key.command_where"), cx))
            .child(parts::callout(Callout::Info,
                t!("connect_wizard.by_key.hoster_title").to_string(),
                t!("connect_wizard.by_key.hoster_body", port = command_port).to_string(), cx)),
    };
    screen
}

/// Ordered Moonbot actions avoid repeating the navigation path in the checklist.
pub(super) fn moonbot_screen(port: u16, cx: &App) -> Screen {
    let items = vec![
        parts::numbered_item(1, t!("connect_wizard.by_key.step1", port = port).to_string(), vec![], cx).into_any_element(),
        parts::numbered_item(2, t!("connect_wizard.by_key.step2").to_string(), vec![note(t!("connect_wizard.by_key.step2_note"), cx).into_any_element()], cx).into_any_element(),
        parts::numbered_item(3, t!("connect_wizard.by_key.step3").to_string(), vec![
            note(t!("connect_wizard.by_key.step3_note"), cx).into_any_element(),
            parts::status_line(t!("connect_wizard.by_key.step3_ok", port = port).to_string(), cx).into_any_element(),
        ], cx).into_any_element(),
        parts::numbered_item(4, t!("connect_wizard.by_key.step4").to_string(), vec![
            note(t!("connect_wizard.by_key.step4_need"), cx).into_any_element(),
            parts::status_line(t!("connect_wizard.by_key.step4_ok", port = port).to_string(), cx).into_any_element(),
            note(t!("connect_wizard.by_key.step4_warn"), cx).into_any_element(),
        ], cx).into_any_element(),
        parts::numbered_item(5, t!("connect_wizard.by_key.step5").to_string(), vec![note(t!("connect_wizard.by_key.step5_note"), cx).into_any_element()], cx).into_any_element(),
    ];
    Screen::new(t!("connect_wizard.by_key.moonbot_title"))
        .sub(t!("connect_wizard.by_key.moonbot_sub"))
        .child(div().font_weight(FontWeight::MEDIUM).child(t!("connect_wizard.by_key.path").to_string()))
        .child(parts::numbered_list(items, cx))
}

/// The same name/key form is reused when a core refuses the exported key.
pub(super) fn key_form(
    name: &Entity<MoonInputState>,
    key: &Entity<MoonInputState>,
    on_paste: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    v_flex().w_full().min_w(px(0.0)).gap(design::ui_px(cx, 12.0))
        .child(parts::input_field("cw-key-name", t!("connect_wizard.by_key.name").to_string(), name, String::new(), cx))
        .child(parts::key_row(key, on_paste, cx))
}

/// Warn about an old key port before the caller offers its explicit override action.
pub(super) fn key_screen(
    name: &Entity<MoonInputState>,
    key: &Entity<MoonInputState>,
    mismatch: Option<u16>,
    chosen: u16,
    on_paste: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Screen {
    let mut screen = Screen::new(t!("connect_wizard.by_key.key_title"))
        .sub(t!("connect_wizard.by_key.key_sub")).child(key_form(name, key, on_paste, cx));
    if let Some(key) = mismatch {
        screen = screen.child(parts::callout(Callout::Hold,
            t!("connect_wizard.by_key.mismatch_title", key = key, chosen = chosen).to_string(),
            t!("connect_wizard.by_key.mismatch_body", chosen = chosen).to_string(), cx));
    }
    screen
}

/// Waiting names the actual target without claiming a failure before it is observed.
pub(super) fn connecting_screen(address: &str, cx: &App) -> Screen {
    Screen::new(t!("connect_wizard.by_key.connecting_title"))
        .child(h_flex().w_full().min_w(px(0.0)).items_center().gap(design::ui_px(cx, 8.0))
            .child(MoonSpinner::new())
            .child(div().flex_1().min_w(px(0.0)).child(t!("connect_wizard.by_key.connecting_at", addr = address).to_string())))
        .child(note(t!("connect_wizard.by_key.connecting_note"), cx))
}
