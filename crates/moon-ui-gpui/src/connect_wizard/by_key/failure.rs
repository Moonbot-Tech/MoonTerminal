//! Recovery bodies selected from the connection diagnosis, with actions supplied by the caller.

use gpui::*;
use moon_ui::{MoonInputState, h_flex};
use rust_i18n::t;

use super::parts;
use crate::connect_wizard::render::{Screen, note};
use crate::design;

/// Retained facts for the timeout screen; `fault` is the caller's diagnosis wording.
pub(super) struct NoResponse<'a> {
    pub(super) port: u16,
    pub(super) address: &'a str,
    pub(super) seconds: u64,
    pub(super) silent: bool,
    pub(super) fault: String,
    pub(super) copied: bool,
}

/// Show a firewall checklist; partial inbound traffic retains the honest diagnosis wording.
pub(super) fn no_response_screen(
    facts: NoResponse<'_>,
    on_copy: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Screen {
    let NoResponse {
        port,
        address,
        seconds,
        silent,
        fault,
        copied,
    } = facts;
    let sub = if silent {
        t!(
            "connect_wizard.by_key.noresp_sub",
            t = seconds,
            addr = address
        )
        .to_string()
    } else {
        fault
    };
    let items = vec![
        parts::item(
            1,
            "connect_wizard.by_key.noresp_1",
            Some("connect_wizard.by_key.noresp_1_note"),
            cx,
        ),
        parts::numbered_item(
            2,
            t!("connect_wizard.by_key.noresp_2"),
            vec![parts::code_box(port, copied, on_copy, cx).into_any_element()],
            cx,
        ),
        parts::numbered_item(
            3,
            t!("connect_wizard.by_key.noresp_3"),
            vec![
                note(t!("connect_wizard.by_key.noresp_3_note", port = port), cx).into_any_element(),
            ],
            cx,
        ),
        parts::item(4, "connect_wizard.by_key.noresp_4", None, cx),
    ];
    Screen::new(t!("connect_wizard.by_key.noresp_title"))
        .sub(sub)
        .child(parts::numbered_list(items, cx))
}

/// A reachable core refusing access needs a fresh key and the Kernel prerequisites.
pub(super) fn key_refused_screen(
    key: &Entity<MoonInputState>,
    mismatch: Option<u16>,
    chosen: u16,
    on_paste: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Screen {
    let items = vec![
        parts::item(
            1,
            "connect_wizard.by_key.refused_1",
            Some("connect_wizard.by_key.refused_1_note"),
            cx,
        ),
        parts::item(2, "connect_wizard.by_key.refused_2", None, cx),
        parts::item(3, "connect_wizard.by_key.refused_3", None, cx),
    ];
    let mut screen = Screen::new(t!("connect_wizard.by_key.refused_title"))
        .sub(t!("connect_wizard.by_key.refused_sub"))
        .child(parts::numbered_list(items, cx))
        .child(parts::key_row(key, on_paste, cx));
    if let Some(port) = mismatch {
        screen = screen.child(parts::mismatch_callout(port, chosen, cx));
    }
    screen
}

/// Supply only the missing public address, keeping the port carried by the key visible.
pub(super) fn no_address_screen(ip: &Entity<MoonInputState>, port: u16, cx: &App) -> Screen {
    Screen::new(t!("connect_wizard.by_key.noaddr_title"))
        .sub(t!("connect_wizard.by_key.noaddr_sub"))
        .child(
            h_flex()
                .w_full()
                .min_w(px(0.0))
                .flex_wrap()
                .items_end()
                .gap(design::ui_px(cx, 10.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(design::ui_px(cx, 180.0))
                        .child(parts::input_field(
                            "cw-key-ip",
                            t!("connect_wizard.by_key.noaddr_ip").to_string(),
                            ip,
                            t!("connect_wizard.by_key.noaddr_placeholder").to_string(),
                            cx,
                        )),
                )
                .child(note(
                    t!("connect_wizard.by_key.noaddr_port", port = port),
                    cx,
                )),
        )
}
