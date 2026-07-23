//! The active-order editor dialog body and footer render.

use super::*;

/// Builds one label-value row in the read-only information block.
fn info_kv(p: MoonPalette, label: String, value: impl IntoElement) -> impl IntoElement {
    h_flex()
        .gap_1()
        .items_center()
        .child(div().text_color(moon(p.text_muted)).child(label))
        .child(value)
}

pub(super) fn dialog_body(state: &Entity<OrderEditState>, cx: &mut App) -> AnyElement {
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

pub(super) fn dialog_footer(state: Entity<OrderEditState>, p: MoonPalette) -> AnyElement {
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
