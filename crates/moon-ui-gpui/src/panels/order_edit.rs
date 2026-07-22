//! Active Order editor, ported from the Moonbot dialog. It opens from the side/type cell in the
//! Orders table or the chart order-line context menu. Market, side, status, size, strategy, and the
//! pending entry condition are read-only because the protocol does not edit them here. The active
//! leg price is submitted through `move_order`; changed SL, TS, TP, and VStop groups are submitted
//! through `update_order_stops` and `moon-core::feed::order_edit`. Cancel, the close button, and
//! overlay dismissal discard the dialog state without submitting it.

use gpui::*;
use moon_core::feed::{OrderRow, OrderStopsForm, StopGroupEdit, TakeProfitEdit, VStopEdit};
use moon_core::session::CoreId;
use moon_core::symbol;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonInput,
    MoonInputState, MoonNotification, MoonPalette, MoonTone, MoonWindowExt as _, h_flex, v_flex,
};
use rust_i18n::t;

use crate::Backend;
use crate::design::{self, moon};

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
            let token = symbol::coin_of_market(&s.row.market_display).to_string();
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

/// Builds one label-value row in the read-only information block.
fn info_kv(p: MoonPalette, label: String, value: impl IntoElement) -> impl IntoElement {
    h_flex()
        .gap_1()
        .items_center()
        .child(div().text_color(moon(p.text_muted)).child(label))
        .child(value)
}

fn dialog_body(state: &Entity<OrderEditState>, cx: &mut App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let s = state.read(cx);
    let r = &s.row;
    let (side, side_tone) = side_label(r, s.executed);
    let side_text = if r.emulator {
        format!("{side}(E)")
    } else {
        side.to_string()
    };
    let strat = if r.strat.is_empty() {
        "—".to_string()
    } else {
        r.strat.clone()
    };
    let status = if r.status.is_empty() {
        "—".to_string()
    } else {
        r.status.clone()
    };

    // Build the read-only order summary.
    let info = v_flex()
        .w_full()
        .gap_1()
        .child(
            h_flex()
                .w_full()
                .gap_3()
                .child(info_kv(
                    p,
                    t!("orders.edit.coin").to_string(),
                    div()
                        .text_color(moon(p.accent))
                        .font_weight(FontWeight::SEMIBOLD)
                        // Match chart pair formatting, for example `BTC_USDT` to `BTC-USDT`.
                        .child(symbol::display_pair(&r.market_display)),
                ))
                .child(info_kv(
                    p,
                    t!("orders.edit.side").to_string(),
                    div()
                        .text_color(moon(side_tone.color(p)))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(side_text),
                ))
                .child(info_kv(
                    p,
                    t!("orders.edit.status").to_string(),
                    div().child(status),
                )),
        )
        .child(
            h_flex()
                .w_full()
                .gap_3()
                .child(info_kv(
                    p,
                    t!("orders.edit.size").to_string(),
                    div().child(format!(
                        "{} ({:.0}%)",
                        crate::panels::num(r.size),
                        r.fill_pct
                    )),
                ))
                .child(info_kv(
                    p,
                    t!("orders.edit.strategy").to_string(),
                    div().child(strat),
                ))
                .child(info_kv(
                    p,
                    t!("orders.edit.core").to_string(),
                    div().child(s.core_name.clone()),
                )),
        );

    // Show the pending entry condition as read-only protocol data when present.
    let cond = r.pending_cond.map(|c| {
        div()
            .w_full()
            .text_color(moon(p.text_muted))
            .child(t!("orders.edit.cond", p = crate::panels::num(c)).to_string())
    });

    // Build the active entry- or exit-leg price editor.
    let price_title = if s.executed {
        t!("orders.edit.sell_price").to_string()
    } else {
        t!("orders.edit.buy_price").to_string()
    };
    let price_group = moon_ui::MoonGroupBox::new("oe-price")
        .title(price_title)
        .padding(8.0)
        .gap(6.0)
        .child(
            h_flex()
                .w_full()
                .gap_2()
                .items_center()
                .child(
                    div().w(px(150.0)).child(
                        MoonInput::new("oe-price-input")
                            .state(&s.price_input)
                            .small(),
                    ),
                )
                .child(
                    div().text_color(moon(p.text_muted)).child(
                        t!(
                            "orders.edit.current",
                            p = crate::panels::num(r.price as f64)
                        )
                        .to_string(),
                    ),
                ),
        );

    // Build stop toggles and the numeric inputs enabled by their current modes.
    let toggle = |id: &'static str,
                  label: String,
                  checked: bool,
                  w: f32,
                  apply: fn(&mut OrderEditState, bool)|
     -> AnyElement {
        let st = state.clone();
        div()
            .w(px(w))
            .flex_none()
            .child(
                MoonCheckbox::new(SharedString::from(id))
                    .label(label)
                    .checked(checked)
                    .size(MoonCheckboxSize::Compact)
                    .on_change(move |ch: &bool, _w, app| {
                        let on = *ch;
                        st.update(app, |s, cx| {
                            apply(s, on);
                            cx.notify();
                        });
                    }),
            )
            .into_any_element()
    };
    // A zero width makes the numeric input fill the remaining row; otherwise it stays fixed-width.
    let num_input = |id: &'static str, input: &Entity<MoonInputState>, enabled: bool, w: f32| {
        let host = if w > 0.0 {
            div().w(px(w)).flex_none()
        } else {
            div().flex_1().min_w(px(70.0))
        };
        host.child(MoonInput::new(id).state(input).small().disabled(!enabled))
            .into_any_element()
    };
    let fixed_label = t!("orders.edit.fixed").to_string();

    let sl_row = h_flex()
        .w_full()
        .gap_2()
        .items_center()
        .child(toggle("oe-sl-on", "SL".into(), s.sl_on, 78.0, |s, on| {
            s.sl_on = on;
        }))
        .child(toggle(
            "oe-sl-fixed",
            fixed_label.clone(),
            s.sl_fixed,
            72.0,
            |s, on| s.sl_fixed = on,
        ))
        .child(num_input(
            "oe-sl-price",
            &s.sl_input,
            s.sl_on && s.sl_fixed,
            0.0,
        ));
    let ts_row = h_flex()
        .w_full()
        .gap_2()
        .items_center()
        .child(toggle("oe-ts-on", "TS".into(), s.ts_on, 78.0, |s, on| {
            s.ts_on = on;
        }))
        .child(toggle(
            "oe-ts-fixed",
            fixed_label.clone(),
            s.ts_fixed,
            72.0,
            |s, on| s.ts_fixed = on,
        ))
        .child(num_input(
            "oe-ts-price",
            &s.ts_input,
            s.ts_on && s.ts_fixed,
            0.0,
        ));
    let tp_row = h_flex()
        .w_full()
        .gap_2()
        .items_center()
        .child(toggle("oe-tp-on", "TP".into(), s.tp_on, 78.0, |s, on| {
            s.tp_on = on;
        }))
        .child(div().w(px(72.0)).flex_none())
        .child(num_input("oe-tp-price", &s.tp_input, s.tp_on, 0.0));
    let vstop_row = h_flex()
        .w_full()
        .gap_2()
        .items_center()
        .child(toggle(
            "oe-vs-on",
            "VStop".into(),
            s.vstop_on,
            78.0,
            |s, on| s.vstop_on = on,
        ))
        .child(toggle(
            "oe-vs-fixed",
            fixed_label,
            s.vstop_fixed,
            72.0,
            |s, on| s.vstop_fixed = on,
        ))
        .child(num_input("oe-vs-level", &s.vstop_input, s.vstop_on, 0.0))
        .child(
            div()
                .text_color(moon(p.text_muted))
                .flex_none()
                .child(t!("orders.edit.vol_lt").to_string()),
        )
        .child(num_input("oe-vs-vol", &s.vstop_vol_input, s.vstop_on, 76.0));

    let stops_group = moon_ui::MoonGroupBox::new("oe-stops")
        .title(t!("orders.edit.stops").to_string())
        .padding(8.0)
        .gap(6.0)
        .child(sl_row)
        .child(ts_row)
        .child(tp_row)
        .child(vstop_row);

    let mut body = v_flex()
        .w_full()
        .gap_2()
        .font_family(design::mono())
        .text_size(design::t_body(cx))
        .text_color(moon(p.text))
        .child(info);
    if let Some(cond) = cond {
        body = body.child(cond);
    }
    body.child(price_group)
        .child(stops_group)
        .into_any_element()
}

fn dialog_footer(state: Entity<OrderEditState>, p: MoonPalette) -> AnyElement {
    let ok_state = state;
    h_flex()
        .w_full()
        .justify_end()
        .gap_2()
        .child(
            MoonButton::new("oe-cancel")
                .ghost()
                .size(MoonButtonSize::Micro)
                .label(t!("dialogs.cancel").to_string())
                .on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                })
                .render(),
        )
        .child(
            MoonButton::new("oe-ok")
                .size(MoonButtonSize::Micro)
                .variant(MoonButtonVariant::Blue)
                .label("OK")
                .on_click(move |_, window, cx| match apply(&ok_state, cx) {
                    Ok(()) => window.close_dialog(cx),
                    Err(error) => {
                        log::warn!("order edit apply failed: {error:#}");
                        window.push_notification(MoonNotification::error(error.to_string()), cx);
                    }
                })
                .render(),
        )
        .text_color(moon(p.text))
        .into_any_element()
}

/// Builds and submits edits from the dialog draft. A changed, positive, finite active-leg price is
/// sent through `move_order`; each changed stop group is included in `OrderStopsForm`, while an
/// untouched group remains `None`. `update_order_stops` treats an entirely empty form as a no-op.
/// The move command is queued before the stop form; submission is not atomic, and the first error is
/// returned so the OK handler can keep the dialog open and show a notification.
fn apply(state: &Entity<OrderEditState>, cx: &mut App) -> anyhow::Result<()> {
    let (backend, core, uid, form, new_price) = {
        let s = state.read(cx);
        let init = s.init;
        let val = |input: &Entity<MoonInputState>| parse_num(&input.read(cx).value());

        // Submit the active-leg price only when it is positive, finite, and meaningfully changed.
        let new_price =
            val(&s.price_input).filter(|p| *p > 0.0 && p.is_finite() && differs(*p, init.price));

        let mut form = OrderStopsForm::default();
        let sl_price = val(&s.sl_input).unwrap_or(0.0);
        if s.sl_on != init.sl_on
            || s.sl_fixed != init.sl_fixed
            || (s.sl_on && s.sl_fixed && differs(sl_price, init.sl_price))
        {
            form.sl = Some(StopGroupEdit {
                on: s.sl_on,
                fixed: s.sl_fixed,
                price: sl_price,
            });
        }
        let ts_price = val(&s.ts_input).unwrap_or(0.0);
        if s.ts_on != init.ts_on
            || s.ts_fixed != init.ts_fixed
            || (s.ts_on && s.ts_fixed && differs(ts_price, init.ts_price))
        {
            form.ts = Some(StopGroupEdit {
                on: s.ts_on,
                fixed: s.ts_fixed,
                price: ts_price,
            });
        }
        let tp_price = val(&s.tp_input).unwrap_or(0.0);
        if s.tp_on != init.tp_on || (s.tp_on && differs(tp_price, init.tp_price)) {
            form.tp = Some(TakeProfitEdit {
                on: s.tp_on,
                price: tp_price,
            });
        }
        let vs_level = val(&s.vstop_input).unwrap_or(0.0);
        let vs_vol = val(&s.vstop_vol_input).unwrap_or(0.0);
        if s.vstop_on != init.vstop_on
            || s.vstop_fixed != init.vstop_fixed
            || (s.vstop_on
                && (differs(vs_level, init.vstop_level) || differs(vs_vol, init.vstop_vol)))
        {
            form.vstop = Some(VStopEdit {
                on: s.vstop_on,
                fixed: s.vstop_fixed,
                level: vs_level,
                vol: vs_vol,
            });
        }
        (s.backend.clone(), s.core, s.uid, form, new_price)
    };

    backend.update(cx, |b, _| -> anyhow::Result<()> {
        if let Some(price) = new_price {
            b.session.move_order(core, uid, price)?;
        }
        b.session.update_order_stops(core, uid, form)?;
        Ok(())
    })
}
