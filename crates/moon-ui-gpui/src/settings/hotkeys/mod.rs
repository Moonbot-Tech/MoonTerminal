//! Вкладка «Хоткеи»: MoonBot-compatible набор горячих клавиш, но с нормальной
//! компоновкой по сценариям.
//!
//! Разбито по файлам: здесь — енумы слотов (`HotkeySlot`/`MouseSlot`), маппинг
//! «слот → поле `HotkeysConfig`» (геттеры/сеттеры/id) и `parse_hotkey`; [`tab`] —
//! сам `impl SettingsView` (сборка вкладки и строки-редакторы).

mod tab;

use gpui::*;
use moon_core::config::{HotkeysConfig, MouseGestureBinding};

#[derive(Clone, Copy)]
enum HotkeySlot {
    OrderSize(usize),
    SellPreset(usize),
    ManualStrategy(usize),
    CancelBuy,
    PanicSell,
    PanicSellOne,
    CancelAllBuys,
    JoinSells,
    SwitchCharts,
    ReloadBook,
    NewLong,
    NewShort,
    SplitOrder,
    SplitOrderX,
    ShiftBuyUp,
    ShiftBuyDown,
    ShiftSellUp,
    ShiftSellDown,
    MakeShot,
    MakeShotBot,
    ReloadChart,
    ScalePlus,
    ScaleMinus,
    SellPlus,
    SellMinus,
    SpyMode,
    ShowCharts,
    SwitchFigure,
    FitSells,
    Broadcast,
    DrawHline,
    DrawSegment,
    DrawTriangle,
    DrawChannel,
    FigDelete,
    FigAlert,
}

#[derive(Clone, Copy)]
enum MouseSlot {
    BuySet,
    ShortSet,
    PendingLong,
    PendingShort,
    BuyMove,
    SellMove,
    BuyMove2,
    SellMove2,
    ShortBuyMove,
    ShortSellMove,
    ShortBuyMove2,
    ShortSellMove2,
}

fn parse_hotkey(raw: &str) -> Option<Keystroke> {
    let raw = raw.trim();
    if raw.is_empty() {
        None
    } else {
        Keystroke::parse(raw).ok()
    }
}

/// Единый список «слот → поле `HotkeysConfig`»: геттер и сеттер раньше дублировали 30
/// строк маппинга (одну под `&`, другую под `&mut`). Макрос держит список в одном месте;
/// `$($brw)+` принимает `&` или `&mut`, исчерпывающий match по-прежнему проверяет компилятор.
macro_rules! hotkey_field {
    ($hotkeys:ident, $slot:expr, $($brw:tt)+) => {
        match $slot {
            HotkeySlot::OrderSize(i) => $($brw)+ $hotkeys.order_size[i],
            HotkeySlot::SellPreset(i) => $($brw)+ $hotkeys.sell_preset[i],
            HotkeySlot::ManualStrategy(i) => $($brw)+ $hotkeys.manual_strategy[i],
            HotkeySlot::CancelBuy => $($brw)+ $hotkeys.cancel_buy,
            HotkeySlot::PanicSell => $($brw)+ $hotkeys.panic_sell,
            HotkeySlot::PanicSellOne => $($brw)+ $hotkeys.panic_sell_one,
            HotkeySlot::CancelAllBuys => $($brw)+ $hotkeys.cancel_all_buys,
            HotkeySlot::JoinSells => $($brw)+ $hotkeys.join_sells,
            HotkeySlot::SwitchCharts => $($brw)+ $hotkeys.switch_charts,
            HotkeySlot::ReloadBook => $($brw)+ $hotkeys.reload_book,
            HotkeySlot::NewLong => $($brw)+ $hotkeys.new_long,
            HotkeySlot::NewShort => $($brw)+ $hotkeys.new_short,
            HotkeySlot::SplitOrder => $($brw)+ $hotkeys.split_order,
            HotkeySlot::SplitOrderX => $($brw)+ $hotkeys.split_order_x,
            HotkeySlot::ShiftBuyUp => $($brw)+ $hotkeys.shift_buy_up,
            HotkeySlot::ShiftBuyDown => $($brw)+ $hotkeys.shift_buy_down,
            HotkeySlot::ShiftSellUp => $($brw)+ $hotkeys.shift_sell_up,
            HotkeySlot::ShiftSellDown => $($brw)+ $hotkeys.shift_sell_down,
            HotkeySlot::MakeShot => $($brw)+ $hotkeys.make_shot,
            HotkeySlot::MakeShotBot => $($brw)+ $hotkeys.make_shot_bot,
            HotkeySlot::ReloadChart => $($brw)+ $hotkeys.reload_chart,
            HotkeySlot::ScalePlus => $($brw)+ $hotkeys.scale_plus,
            HotkeySlot::ScaleMinus => $($brw)+ $hotkeys.scale_minus,
            HotkeySlot::SellPlus => $($brw)+ $hotkeys.sell_plus,
            HotkeySlot::SellMinus => $($brw)+ $hotkeys.sell_minus,
            HotkeySlot::SpyMode => $($brw)+ $hotkeys.spy_mode,
            HotkeySlot::ShowCharts => $($brw)+ $hotkeys.show_charts,
            HotkeySlot::SwitchFigure => $($brw)+ $hotkeys.switch_figure,
            HotkeySlot::FitSells => $($brw)+ $hotkeys.fit_sells,
            HotkeySlot::Broadcast => $($brw)+ $hotkeys.broadcast,
            HotkeySlot::DrawHline => $($brw)+ $hotkeys.draw_hline,
            HotkeySlot::DrawSegment => $($brw)+ $hotkeys.draw_segment,
            HotkeySlot::DrawTriangle => $($brw)+ $hotkeys.draw_triangle,
            HotkeySlot::DrawChannel => $($brw)+ $hotkeys.draw_channel,
            HotkeySlot::FigDelete => $($brw)+ $hotkeys.fig_delete,
            HotkeySlot::FigAlert => $($brw)+ $hotkeys.fig_alert,
        }
    };
}

fn slot_value(hotkeys: &HotkeysConfig, slot: HotkeySlot) -> &str {
    hotkey_field!(hotkeys, slot, &)
}

fn set_slot_value(hotkeys: &mut HotkeysConfig, slot: HotkeySlot, value: String) -> bool {
    let target = hotkey_field!(hotkeys, slot, &mut);
    if *target == value {
        false
    } else {
        *target = value;
        true
    }
}

fn mouse_slot_value(hotkeys: &HotkeysConfig, slot: MouseSlot) -> MouseGestureBinding {
    match slot {
        MouseSlot::BuySet => hotkeys.buy_set_click,
        MouseSlot::ShortSet => hotkeys.short_set_click,
        MouseSlot::PendingLong => hotkeys.pending_long_click,
        MouseSlot::PendingShort => hotkeys.pending_short_click,
        MouseSlot::BuyMove => hotkeys.buy_move_click,
        MouseSlot::SellMove => hotkeys.sell_move_click,
        MouseSlot::BuyMove2 => hotkeys.buy_move_click2,
        MouseSlot::SellMove2 => hotkeys.sell_move_click2,
        MouseSlot::ShortBuyMove => hotkeys.short_buy_move_click,
        MouseSlot::ShortSellMove => hotkeys.short_sell_move_click,
        MouseSlot::ShortBuyMove2 => hotkeys.short_buy_move_click2,
        MouseSlot::ShortSellMove2 => hotkeys.short_sell_move_click2,
    }
}

fn set_mouse_slot_value(
    hotkeys: &mut HotkeysConfig,
    slot: MouseSlot,
    value: MouseGestureBinding,
) -> bool {
    let mut changed = false;
    match slot {
        MouseSlot::BuySet => changed |= set_mouse_field(&mut hotkeys.buy_set_click, value),
        MouseSlot::ShortSet => changed |= set_mouse_field(&mut hotkeys.short_set_click, value),
        MouseSlot::PendingLong => {
            changed |= set_mouse_field(&mut hotkeys.pending_long_click, value)
        }
        MouseSlot::PendingShort => {
            changed |= set_mouse_field(&mut hotkeys.pending_short_click, value)
        }
        MouseSlot::BuyMove => {
            changed |= set_mouse_field(&mut hotkeys.buy_move_click, value);
            if hotkeys.same_hotkeys_for_move {
                changed |= set_mouse_field(&mut hotkeys.short_buy_move_click, value);
            }
        }
        MouseSlot::SellMove => {
            changed |= set_mouse_field(&mut hotkeys.sell_move_click, value);
            if hotkeys.same_hotkeys_for_move {
                changed |= set_mouse_field(&mut hotkeys.short_sell_move_click, value);
            }
        }
        MouseSlot::BuyMove2 => {
            changed |= set_mouse_field(&mut hotkeys.buy_move_click2, value);
            if hotkeys.same_hotkeys_for_move {
                changed |= set_mouse_field(&mut hotkeys.short_buy_move_click2, value);
            }
        }
        MouseSlot::SellMove2 => {
            changed |= set_mouse_field(&mut hotkeys.sell_move_click2, value);
            if hotkeys.same_hotkeys_for_move {
                changed |= set_mouse_field(&mut hotkeys.short_sell_move_click2, value);
            }
        }
        MouseSlot::ShortBuyMove => {
            changed |= set_mouse_field(&mut hotkeys.short_buy_move_click, value)
        }
        MouseSlot::ShortSellMove => {
            changed |= set_mouse_field(&mut hotkeys.short_sell_move_click, value)
        }
        MouseSlot::ShortBuyMove2 => {
            changed |= set_mouse_field(&mut hotkeys.short_buy_move_click2, value)
        }
        MouseSlot::ShortSellMove2 => {
            changed |= set_mouse_field(&mut hotkeys.short_sell_move_click2, value)
        }
    }
    changed
}

fn set_mouse_field(field: &mut MouseGestureBinding, value: MouseGestureBinding) -> bool {
    if *field == value {
        false
    } else {
        *field = value;
        true
    }
}

fn mouse_slot_id(slot: MouseSlot) -> &'static str {
    match slot {
        MouseSlot::BuySet => "buy-set",
        MouseSlot::ShortSet => "short-set",
        MouseSlot::PendingLong => "pending-long",
        MouseSlot::PendingShort => "pending-short",
        MouseSlot::BuyMove => "buy-move",
        MouseSlot::SellMove => "sell-move",
        MouseSlot::BuyMove2 => "buy-move2",
        MouseSlot::SellMove2 => "sell-move2",
        MouseSlot::ShortBuyMove => "short-buy-move",
        MouseSlot::ShortSellMove => "short-sell-move",
        MouseSlot::ShortBuyMove2 => "short-buy-move2",
        MouseSlot::ShortSellMove2 => "short-sell-move2",
    }
}

fn slot_id(slot: HotkeySlot) -> String {
    match slot {
        HotkeySlot::OrderSize(i) => format!("order-size-{i}"),
        HotkeySlot::SellPreset(i) => format!("sell-preset-{i}"),
        HotkeySlot::ManualStrategy(i) => format!("manual-strategy-{i}"),
        HotkeySlot::CancelBuy => "cancel-buy".into(),
        HotkeySlot::PanicSell => "panic-sell".into(),
        HotkeySlot::PanicSellOne => "panic-sell-one".into(),
        HotkeySlot::CancelAllBuys => "cancel-all-buys".into(),
        HotkeySlot::JoinSells => "join-sells".into(),
        HotkeySlot::SwitchCharts => "switch-charts".into(),
        HotkeySlot::ReloadBook => "reload-book".into(),
        HotkeySlot::NewLong => "new-long".into(),
        HotkeySlot::NewShort => "new-short".into(),
        HotkeySlot::SplitOrder => "split-order".into(),
        HotkeySlot::SplitOrderX => "split-order-x".into(),
        HotkeySlot::ShiftBuyUp => "shift-buy-up".into(),
        HotkeySlot::ShiftBuyDown => "shift-buy-down".into(),
        HotkeySlot::ShiftSellUp => "shift-sell-up".into(),
        HotkeySlot::ShiftSellDown => "shift-sell-down".into(),
        HotkeySlot::MakeShot => "make-shot".into(),
        HotkeySlot::MakeShotBot => "make-shot-bot".into(),
        HotkeySlot::ReloadChart => "reload-chart".into(),
        HotkeySlot::ScalePlus => "scale-plus".into(),
        HotkeySlot::ScaleMinus => "scale-minus".into(),
        HotkeySlot::SellPlus => "sell-plus".into(),
        HotkeySlot::SellMinus => "sell-minus".into(),
        HotkeySlot::SpyMode => "spy-mode".into(),
        HotkeySlot::ShowCharts => "show-charts".into(),
        HotkeySlot::SwitchFigure => "switch-figure".into(),
        HotkeySlot::FitSells => "fit-sells".into(),
        HotkeySlot::Broadcast => "broadcast".into(),
        HotkeySlot::DrawHline => "draw-hline".into(),
        HotkeySlot::DrawSegment => "draw-segment".into(),
        HotkeySlot::DrawTriangle => "draw-triangle".into(),
        HotkeySlot::DrawChannel => "draw-channel".into(),
        HotkeySlot::FigDelete => "fig-delete".into(),
        HotkeySlot::FigAlert => "fig-alert".into(),
    }
}
