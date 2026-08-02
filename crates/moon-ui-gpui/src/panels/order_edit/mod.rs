//! Active Order editor, ported from the Moonbot dialog. It opens from the side/type cell in the
//! Orders table or the chart order-line context menu. Market, side, status, size, strategy, and the
//! pending entry condition are read-only because the protocol does not edit them here. The active
//! leg price is submitted through `move_order`; changed SL, TS, TP, and VStop groups are submitted
//! through `update_order_stops` and `moon-core::feed::order_edit`. Cancel, the close button, and
//! overlay dismissal discard the dialog state without submitting it.

use gpui::*;
use moon_core::feed::{OrderRow, OrderStopsForm, StopGroupEdit, TakeProfitEdit, VStopEdit};
use moon_core::session::CoreId;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonInput,
    MoonInputState, MoonNotification, MoonPalette, MoonTone, MoonWindowExt as _, h_flex, v_flex,
};
use rust_i18n::t;

use crate::Backend;
use crate::design::{self, moon};

mod body;
mod submit;

use body::{dialog_body, dialog_footer};
use submit::apply;

/// Initial form values used to omit unchanged fields when OK builds its edit commands.
#[derive(Clone, Copy)]
struct InitVals {
    price: f64,
    sl_on: bool,
    sl_fixed: bool,
    sl_price: f64,
    ts_on: bool,
    ts_fixed: bool,
    ts_price: f64,
    tp_on: bool,
    tp_price: f64,
    vstop_on: bool,
    vstop_fixed: bool,
    vstop_level: f64,
    vstop_vol: f64,
}

/// Toggle and input state captured by the open dialog. Closing or replacing the unique dialog drops
/// its captured entity after the dialog closures are released.
pub struct OrderEditState {
    backend: Entity<Backend>,
    core: CoreId,
    uid: u64,
    row: OrderRow,
    core_name: String,
    /// Whether the worker status says entry execution completed. Executed orders seed the editable
    /// price from `row.sell_price`; pending orders seed it from `row.buy_price`.
    executed: bool,
    sl_on: bool,
    sl_fixed: bool,
    ts_on: bool,
    ts_fixed: bool,
    tp_on: bool,
    vstop_on: bool,
    vstop_fixed: bool,
    init: InitVals,
    price_input: Entity<MoonInputState>,
    sl_input: Entity<MoonInputState>,
    ts_input: Entity<MoonInputState>,
    tp_input: Entity<MoonInputState>,
    vstop_input: Entity<MoonInputState>,
    vstop_vol_input: Entity<MoonInputState>,
}

/// Formats a price with up to eight fixed decimal places and removes trailing zeroes, avoiding
/// scientific notation. Zero and non-finite values produce an empty string.
fn fmt_edit(v: f64) -> String {
    if !v.is_finite() || v == 0.0 {
        return String::new();
    }
    format!("{v:.8}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

/// Parses an input as `f64` after accepting commas as decimal points. Invalid syntax returns `None`;
/// callers apply any required positivity and finiteness checks.
fn parse_num(s: &str) -> Option<f64> {
    s.trim().replace(',', ".").parse::<f64>().ok()
}

/// Compares numeric edits with a relative epsilon suitable for both large and micro prices.
fn differs(a: f64, b: f64) -> bool {
    (a - b).abs() > a.abs().max(b.abs()) * 1e-9 + 1e-12
}

/// Returns the same long/short and entry/exit side label and tone used by the Orders table.
fn side_label(r: &OrderRow, executed: bool) -> (&'static str, MoonTone) {
    match (r.is_short, executed) {
        (false, false) => ("BUY", MoonTone::Negative),
        (false, true) => ("SELL", MoonTone::Info),
        (true, false) => ("Short-S", MoonTone::Negative),
        (true, true) => ("Short-B", MoonTone::Info),
    }
}

/// Opens the unique order editor for `uid` from `core` in the current window. The initial draft is
/// copied from the current store snapshot; a missing or already closed order logs a warning and no
/// dialog is opened.
pub(crate) fn open_order_edit(
    backend: Entity<Backend>,
    core: CoreId,
    uid: u64,
    window: &mut Window,
    cx: &mut App,
) {
    let (row, core_name) = {
        let b = backend.read(cx);
        let store = b.session.store();
        let Some(row) = store
            .core(core)
            .and_then(|d| d.orders.iter().find(|o| o.uid == uid).cloned())
        else {
            log::warn!("order edit: order core={core} uid={uid} not found");
            return;
        };
        let core_name = b
            .session
            .sessions()
            .iter()
            .find(|s| s.id == core)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        (row, core_name)
    };
    let executed = super::orders::executed(&row);

    let init = InitVals {
        price: if executed {
            row.sell_price
        } else {
            row.buy_price
        },
        sl_on: row.sl_on,
        sl_fixed: row.sl_fixed,
        sl_price: row.stop_loss.unwrap_or(0.0),
        ts_on: row.ts_on,
        ts_fixed: row.ts_fixed,
        ts_price: row.trailing.unwrap_or(0.0),
        tp_on: row.take_profit.is_some(),
        tp_price: row.take_profit.unwrap_or(0.0),
        vstop_on: row.vstop_on,
        vstop_fixed: row.vstop_fixed,
        vstop_level: row.vstop_level,
        vstop_vol: row.vstop_vol,
    };
    let input = |window: &mut Window, cx: &mut App, v: f64| {
        cx.new(|cx| MoonInputState::new(window, cx).default_value(fmt_edit(v)))
    };
    let price_input = input(window, cx, init.price);
    let sl_input = input(window, cx, init.sl_price);
    let ts_input = input(window, cx, init.ts_price);
    let tp_input = input(window, cx, init.tp_price);
    let vstop_input = input(window, cx, init.vstop_level);
    let vstop_vol_input = input(window, cx, init.vstop_vol);

    let state = cx.new(|_| OrderEditState {
        backend,
        core,
        uid,
        row,
        core_name,
        executed,
        sl_on: init.sl_on,
        sl_fixed: init.sl_fixed,
        ts_on: init.ts_on,
        ts_fixed: init.ts_fixed,
        tp_on: init.tp_on,
        vstop_on: init.vstop_on,
        vstop_fixed: init.vstop_fixed,
        init,
        price_input,
        sl_input,
        ts_input,
        tp_input,
        vstop_input,
        vstop_vol_input,
    });

    window.open_unique_moon_dialog("order-edit-dialog", cx, move |dialog, _window, cx| {
        let p = MoonPalette::active(cx);
        let content_state = state.clone();
        let footer_state = state.clone();
        let title = {
            let s = state.read(cx);
            let token = s.row.coin.clone();
            let (side, _) = side_label(&s.row, s.executed);
            format!("{} — {token} ({side})", t!("orders.edit.title"))
        };
        dialog
            .w(px(470.0))
            .close_button(true)
            .overlay(true)
            .overlay_closable(true)
            .bg(moon(p.shell_high))
            .border_color(moon(p.border))
            .rounded(design::r_container(cx))
            .text_color(moon(p.text))
            .header(
                div()
                    .w_full()
                    .py_2()
                    .border_b_1()
                    .border_color(moon(p.border))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .on_cancel(|_, _, _| true)
            .content(move |content, _window, cx| content.child(dialog_body(&content_state, cx)))
            .footer(dialog_footer(footer_state, p))
    });
}
