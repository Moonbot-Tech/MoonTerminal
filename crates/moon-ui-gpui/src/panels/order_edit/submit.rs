//! Applying the edited stop groups back to the core through the trade API.

use super::*;

/// Builds and submits edits from the dialog draft. A changed, positive, finite active-leg price is
/// sent through `move_order`; each changed stop group is included in `OrderStopsForm`, while an
/// untouched group remains `None`. `update_order_stops` treats an entirely empty form as a no-op.
/// The move command is queued before the stop form; submission is not atomic, and the first error is
/// returned so the OK handler can keep the dialog open and show a notification.
pub(super) fn apply(state: &Entity<OrderEditState>, cx: &mut App) -> anyhow::Result<()> {
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
