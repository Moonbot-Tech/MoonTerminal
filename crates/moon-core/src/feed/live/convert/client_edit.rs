//! Targeted edits to retained compact client settings.

use super::*;

/// Applies a targeted toolbar edit to the retained settings snapshot THROUGH command helpers.
/// Raw fields `s_price`/`sb_num` are `pub(crate)` in production; `price_drop_level` is public.
pub(in crate::feed::live) fn apply_client_settings_edit(
    s: &mut moonproto::ClientSettingsCommand,
    edit: ClientSettingsEdit,
) {
    match edit {
        ClientSettingsEdit::TakeProfit { pct, extended } => {
            // x_tmode/"s9": on -> x_sell stores pct/10 (displayed as 100..900%); off -> x_sell
            // stores pct directly (1..100%). The core clamps TP to 100 without this flag, so set
            // both fields.
            let mode = if extended {
                TakeProfitMode::Extended
            } else {
                TakeProfitMode::Normal
            };
            let Some(pct) = mode.canonical_take_profit_pct(pct) else {
                return;
            };
            s.fixed_sell_mode = false;
            if extended {
                s.x_tmode = true;
                s.x_sell = (pct / 10.0) as i32;
            } else {
                s.x_tmode = false;
                s.x_sell = pct as i32;
            }
        }
        ClientSettingsEdit::StopLossPct(pct) => {
            // Snapped to the core's own 0.1 grid: sending a finer value writes a generation the
            // core cannot echo back, and everything gated on that echo waits for nothing.
            let Some(pct) = crate::config::GroupExitSettings::wire_stop_loss_pct(pct) else {
                return;
            };
            s.price_drop_level = pct;
        }
        ClientSettingsEdit::ScalpTakeProfit(pct) => {
            let Some(pct) = TakeProfitMode::Scalp.canonical_take_profit_pct(pct) else {
                return;
            };
            // Fixed-sell preset encoding also reads x_tmode, so scalp must clear a previous
            // extended generation before the group serializer writes S1-S6.
            s.x_tmode = false;
            s.set_scalp_take_profit_percent(pct);
        }
        ClientSettingsEdit::SelectFixedSellSlot(slot) => {
            // Enable fixed-sell mode; otherwise the effective TP remains on x_sell and does not
            // change. This makes effective_take_profit_percent() equal the selected preset, while
            // the toolbar deliberately continues to show the main TP and the S-button shows the
            // selected fixed-sell value.
            s.fixed_sell_mode = true;
            s.set_selected_fixed_sell_slot(slot);
        }
        ClientSettingsEdit::EngageMainTakeProfit => {
            // Return to the main TP by disabling fixed-sell without changing the TP value (x_sell/scalp).
            s.fixed_sell_mode = false;
        }
        ClientSettingsEdit::SetFixedSellPct { slot, pct } => {
            // Displayed percentage = s_price * (x_tmode ? 10 : 1); derive s_price inversely.
            let mode = if s.x_tmode {
                TakeProfitMode::Extended
            } else {
                TakeProfitMode::Normal
            };
            let Some(pct) = mode.canonical_fixed_sell_pct(pct) else {
                return;
            };
            let price = if s.x_tmode {
                (pct / 10.0) as f32
            } else {
                pct as f32
            };
            s.set_fixed_sell_preset_price(slot, price);
        }
        // Core behavior defaults still owned by the compact channel. The exit rules, icebergs and
        // blacklist moved to the safe-share channel with the settings popup's General tab, which is
        // the only place that edited them; `emu_mode` stayed because that channel is the ONLY one
        // carrying it (see `shared_config::absorb`).
        ClientSettingsEdit::UseStopMarket(on) => s.use_stop_market = on,
        ClientSettingsEdit::PanicIfPriceDrop(on) => s.panic_if_price_drop = on,
        ClientSettingsEdit::SignOrders(on) => s.sign_orders = on,
        ClientSettingsEdit::EmuMode(on) => s.emu_mode = on,
    }
}
