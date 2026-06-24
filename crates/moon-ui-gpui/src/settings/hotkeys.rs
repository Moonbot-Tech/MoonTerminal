//! Вкладка «Хоткеи»: MoonBot-compatible набор горячих клавиш, но с нормальной
//! компоновкой по сценариям.

use gpui::*;
use moon_core::config::{
    HotkeysConfig, MANUAL_STRATEGY_KEYS, MouseGestureBinding, ORDER_SIZE_KEYS, SELL_PRESET_KEYS,
};
use moon_ui::{
    MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonDropdown,
    MoonHotkeyInput, MoonMenuItem, MoonMenuSize, MoonPalette, MoonText, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::SettingsView;
use crate::design;

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

impl SettingsView {
    pub(super) fn hotkeys_tab(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let hotkeys = {
            let b = self.backend.read(cx);
            b.preview.as_ref().unwrap_or(&b.config).hotkeys.clone()
        };

        v_flex()
            .w_full()
            .gap(design::ui_px(cx, 12.0))
            .child(hotkey_group(
                &t!("hotkeys.group.presets"),
                &t!("hotkeys.group.presets_hint"),
                p,
                cx,
                (0..ORDER_SIZE_KEYS)
                    .map(|i| {
                        let title = format!("F{}", i + 1);
                        let desc = t!("hotkeys.order_size", n = i + 1).to_string();
                        self.hotkey_row(title, desc, HotkeySlot::OrderSize(i), &hotkeys, cx)
                    })
                    .chain((0..SELL_PRESET_KEYS).map(|i| {
                        let title = format!("S{}", i + 1);
                        let desc = t!("hotkeys.sell_preset", n = i + 1).to_string();
                        self.hotkey_row(title, desc, HotkeySlot::SellPreset(i), &hotkeys, cx)
                    })),
            ))
            .child(hotkey_group(
                &t!("hotkeys.group.trading"),
                &t!("hotkeys.group.trading_hint"),
                p,
                cx,
                [
                    self.hotkey_row(
                        t!("hotkeys.cancel_buy").to_string(),
                        t!("hotkeys.cancel_buy_hint").to_string(),
                        HotkeySlot::CancelBuy,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.panic_sell").to_string(),
                        t!("hotkeys.panic_sell_hint").to_string(),
                        HotkeySlot::PanicSell,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.panic_sell_one").to_string(),
                        t!("hotkeys.panic_sell_one_hint").to_string(),
                        HotkeySlot::PanicSellOne,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.cancel_all_buys").to_string(),
                        t!("hotkeys.cancel_all_buys_hint").to_string(),
                        HotkeySlot::CancelAllBuys,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.join_sells").to_string(),
                        t!("hotkeys.join_sells_hint").to_string(),
                        HotkeySlot::JoinSells,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.new_long").to_string(),
                        t!("hotkeys.new_long_hint").to_string(),
                        HotkeySlot::NewLong,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.new_short").to_string(),
                        t!("hotkeys.new_short_hint").to_string(),
                        HotkeySlot::NewShort,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.split_order").to_string(),
                        t!("hotkeys.split_order_hint").to_string(),
                        HotkeySlot::SplitOrder,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.split_order_x").to_string(),
                        t!("hotkeys.split_order_x_hint").to_string(),
                        HotkeySlot::SplitOrderX,
                        &hotkeys,
                        cx,
                    ),
                ],
            ))
            .child(hotkey_group(
                &t!("hotkeys.group.chart"),
                &t!("hotkeys.group.chart_hint"),
                p,
                cx,
                [
                    self.hotkey_row(
                        t!("hotkeys.reload_chart").to_string(),
                        t!("hotkeys.reload_chart_hint").to_string(),
                        HotkeySlot::ReloadChart,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.reload_book").to_string(),
                        t!("hotkeys.reload_book_hint").to_string(),
                        HotkeySlot::ReloadBook,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.switch_charts").to_string(),
                        t!("hotkeys.switch_charts_hint").to_string(),
                        HotkeySlot::SwitchCharts,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.show_charts").to_string(),
                        t!("hotkeys.show_charts_hint").to_string(),
                        HotkeySlot::ShowCharts,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.switch_figure").to_string(),
                        t!("hotkeys.switch_figure_hint").to_string(),
                        HotkeySlot::SwitchFigure,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.fit_sells").to_string(),
                        t!("hotkeys.fit_sells_hint").to_string(),
                        HotkeySlot::FitSells,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.scale_plus").to_string(),
                        t!("hotkeys.scale_plus_hint").to_string(),
                        HotkeySlot::ScalePlus,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.scale_minus").to_string(),
                        t!("hotkeys.scale_minus_hint").to_string(),
                        HotkeySlot::ScaleMinus,
                        &hotkeys,
                        cx,
                    ),
                ],
            ))
            .child(hotkey_group(
                &t!("hotkeys.group.order_move"),
                &t!("hotkeys.group.order_move_hint"),
                p,
                cx,
                [
                    self.hotkey_row(
                        t!("hotkeys.shift_buy_up").to_string(),
                        t!("hotkeys.shift_buy_up_hint").to_string(),
                        HotkeySlot::ShiftBuyUp,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.shift_buy_down").to_string(),
                        t!("hotkeys.shift_buy_down_hint").to_string(),
                        HotkeySlot::ShiftBuyDown,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.shift_sell_up").to_string(),
                        t!("hotkeys.shift_sell_up_hint").to_string(),
                        HotkeySlot::ShiftSellUp,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.shift_sell_down").to_string(),
                        t!("hotkeys.shift_sell_down_hint").to_string(),
                        HotkeySlot::ShiftSellDown,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.sell_plus").to_string(),
                        t!("hotkeys.sell_plus_hint").to_string(),
                        HotkeySlot::SellPlus,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.sell_minus").to_string(),
                        t!("hotkeys.sell_minus_hint").to_string(),
                        HotkeySlot::SellMinus,
                        &hotkeys,
                        cx,
                    ),
                ],
            ))
            .child(hotkey_group(
                &t!("hotkeys.group.mouse"),
                &t!("hotkeys.group.mouse_hint"),
                p,
                cx,
                [
                    self.mouse_row(
                        t!("hotkeys.mouse.buy_set").to_string(),
                        t!("hotkeys.mouse.buy_set_hint").to_string(),
                        MouseSlot::BuySet,
                        &hotkeys,
                        false,
                        cx,
                    ),
                    self.mouse_row(
                        t!("hotkeys.mouse.short_set").to_string(),
                        t!("hotkeys.mouse.short_set_hint").to_string(),
                        MouseSlot::ShortSet,
                        &hotkeys,
                        false,
                        cx,
                    ),
                    self.mouse_row(
                        t!("hotkeys.mouse.pending_long").to_string(),
                        t!("hotkeys.mouse.pending_long_hint").to_string(),
                        MouseSlot::PendingLong,
                        &hotkeys,
                        false,
                        cx,
                    ),
                    self.mouse_row(
                        t!("hotkeys.mouse.pending_short").to_string(),
                        t!("hotkeys.mouse.pending_short_hint").to_string(),
                        MouseSlot::PendingShort,
                        &hotkeys,
                        false,
                        cx,
                    ),
                    self.mouse_row(
                        t!("hotkeys.mouse.buy_move").to_string(),
                        t!("hotkeys.mouse.buy_move_hint").to_string(),
                        MouseSlot::BuyMove,
                        &hotkeys,
                        false,
                        cx,
                    ),
                    self.mouse_row(
                        t!("hotkeys.mouse.sell_move").to_string(),
                        t!("hotkeys.mouse.sell_move_hint").to_string(),
                        MouseSlot::SellMove,
                        &hotkeys,
                        false,
                        cx,
                    ),
                    self.mouse_row(
                        t!("hotkeys.mouse.buy_move2").to_string(),
                        t!("hotkeys.mouse.buy_move2_hint").to_string(),
                        MouseSlot::BuyMove2,
                        &hotkeys,
                        false,
                        cx,
                    ),
                    self.mouse_row(
                        t!("hotkeys.mouse.sell_move2").to_string(),
                        t!("hotkeys.mouse.sell_move2_hint").to_string(),
                        MouseSlot::SellMove2,
                        &hotkeys,
                        false,
                        cx,
                    ),
                    self.same_move_checkbox(&hotkeys, cx),
                    self.mouse_row(
                        t!("hotkeys.mouse.short_buy_move").to_string(),
                        t!("hotkeys.mouse.short_buy_move_hint").to_string(),
                        MouseSlot::ShortBuyMove,
                        &hotkeys,
                        hotkeys.same_hotkeys_for_move,
                        cx,
                    ),
                    self.mouse_row(
                        t!("hotkeys.mouse.short_sell_move").to_string(),
                        t!("hotkeys.mouse.short_sell_move_hint").to_string(),
                        MouseSlot::ShortSellMove,
                        &hotkeys,
                        hotkeys.same_hotkeys_for_move,
                        cx,
                    ),
                    self.mouse_row(
                        t!("hotkeys.mouse.short_buy_move2").to_string(),
                        t!("hotkeys.mouse.short_buy_move2_hint").to_string(),
                        MouseSlot::ShortBuyMove2,
                        &hotkeys,
                        hotkeys.same_hotkeys_for_move,
                        cx,
                    ),
                    self.mouse_row(
                        t!("hotkeys.mouse.short_sell_move2").to_string(),
                        t!("hotkeys.mouse.short_sell_move2_hint").to_string(),
                        MouseSlot::ShortSellMove2,
                        &hotkeys,
                        hotkeys.same_hotkeys_for_move,
                        cx,
                    ),
                ],
            ))
            .child(hotkey_group(
                &t!("hotkeys.group.tools"),
                &t!("hotkeys.group.tools_hint"),
                p,
                cx,
                [
                    self.hotkey_row(
                        t!("hotkeys.make_shot").to_string(),
                        t!("hotkeys.make_shot_hint").to_string(),
                        HotkeySlot::MakeShot,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.make_shot_bot").to_string(),
                        t!("hotkeys.make_shot_bot_hint").to_string(),
                        HotkeySlot::MakeShotBot,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.spy_mode").to_string(),
                        t!("hotkeys.spy_mode_hint").to_string(),
                        HotkeySlot::SpyMode,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.broadcast").to_string(),
                        t!("hotkeys.broadcast_hint").to_string(),
                        HotkeySlot::Broadcast,
                        &hotkeys,
                        cx,
                    ),
                ],
            ))
            .child(hotkey_group(
                &t!("hotkeys.group.manual_strategy"),
                &t!("hotkeys.group.manual_strategy_hint"),
                p,
                cx,
                (0..MANUAL_STRATEGY_KEYS).map(|i| {
                    self.hotkey_row(
                        t!("hotkeys.manual_strategy", n = i + 1).to_string(),
                        t!("hotkeys.manual_strategy_hint", n = i + 1).to_string(),
                        HotkeySlot::ManualStrategy(i),
                        &hotkeys,
                        cx,
                    )
                }),
            ))
    }

    fn hotkey_row(
        &self,
        title: impl Into<String>,
        desc: impl Into<String>,
        slot: HotkeySlot,
        hotkeys: &HotkeysConfig,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let raw = slot_value(hotkeys, slot);
        let parsed = parse_hotkey(raw);
        let invalid = !raw.trim().is_empty() && parsed.is_none();
        let id = format!("hotkey-{}", slot_id(slot));

        h_flex()
            .w_full()
            .min_h(design::fit_h_px(cx, 38.0, 13.0, 7.0))
            .gap(design::ui_px(cx, 12.0))
            .items_center()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(1.0))
                    .child(
                        MoonText::new(title.into())
                            .mono(true)
                            .font_size(design::font_value(cx, 11.0))
                            .line_height(design::line_value(cx, 14.0))
                            .color(p.text)
                            .render(),
                    )
                    .child(
                        MoonText::new(desc.into())
                            .mono(true)
                            .font_size(design::font_value(cx, 9.0))
                            .line_height(design::line_value(cx, 12.0))
                            .color(p.text_muted)
                            .render(),
                    ),
            )
            .child(
                MoonHotkeyInput::new(id)
                    .value(parsed)
                    .placeholder(t!("hotkeys.unassigned").to_string())
                    .recording_placeholder(t!("hotkeys.recording").to_string())
                    .invalid(invalid)
                    .conflict(false)
                    .compact()
                    .width(176.0)
                    .on_change(
                        cx.processor(move |this, value: Option<Keystroke>, _window, cx| {
                            let value = value.map(|k| k.unparse()).unwrap_or_default();
                            this.set_hotkey(slot, value, cx);
                        }),
                    ),
            )
            .into_any_element()
    }

    fn mouse_row(
        &self,
        title: impl Into<String>,
        desc: impl Into<String>,
        slot: MouseSlot,
        hotkeys: &HotkeysConfig,
        disabled: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let current = mouse_slot_value(hotkeys, slot);
        let id = format!("mouse-{}", mouse_slot_id(slot));
        let backend = self.backend.clone();
        let items = MouseGestureBinding::ALL.into_iter().map(move |gesture| {
            let backend = backend.clone();
            MoonMenuItem::with_key(
                gesture.config_value(),
                format!("{} ({})", gesture.label(), gesture.moonbot_name()),
            )
            .checked(gesture == current)
            .on_click(move |_, _, cx| {
                backend.update(cx, |b, bcx| {
                    if let Some(p) = b.preview.as_mut() {
                        if set_mouse_slot_value(&mut p.hotkeys, slot, gesture) {
                            bcx.notify();
                        }
                    }
                });
            })
        });

        h_flex()
            .w_full()
            .min_h(design::fit_h_px(cx, 34.0, 12.0, 6.0))
            .gap(design::ui_px(cx, 12.0))
            .items_center()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(1.0))
                    .child(
                        MoonText::new(title.into())
                            .mono(true)
                            .font_size(design::font_value(cx, 11.0))
                            .line_height(design::line_value(cx, 14.0))
                            .color(if disabled { p.text_muted } else { p.text })
                            .render(),
                    )
                    .child(
                        MoonText::new(desc.into())
                            .mono(true)
                            .font_size(design::font_value(cx, 9.0))
                            .line_height(design::line_value(cx, 12.0))
                            .color(p.text_muted)
                            .render(),
                    ),
            )
            .child(
                MoonDropdown::new(SharedString::from(id))
                    .label(current.label())
                    .trigger_size(MoonButtonSize::Micro)
                    .trigger_variant(if current == MouseGestureBinding::None {
                        MoonButtonVariant::Neutral
                    } else {
                        MoonButtonVariant::Blue
                    })
                    .trigger_width(176.0)
                    .menu_width(228.0)
                    .menu_size(MoonMenuSize::Compact)
                    .disabled(disabled)
                    .items(items),
            )
            .into_any_element()
    }

    fn same_move_checkbox(&self, hotkeys: &HotkeysConfig, cx: &Context<Self>) -> AnyElement {
        let backend = self.backend.clone();

        h_flex()
            .w_full()
            .min_h(design::fit_h_px(cx, 30.0, 12.0, 6.0))
            .items_center()
            .child(
                MoonCheckbox::new("same-hotkeys-for-move")
                    .checked(hotkeys.same_hotkeys_for_move)
                    .size(MoonCheckboxSize::Compact)
                    .label(t!("hotkeys.mouse.same_move").to_string())
                    .on_change(move |value, _window, cx| {
                        backend.update(cx, |b, bcx| {
                            if let Some(p) = b.preview.as_mut() {
                                let changed = p.hotkeys.same_hotkeys_for_move != *value;
                                p.hotkeys.same_hotkeys_for_move = *value;
                                if *value {
                                    p.hotkeys.short_buy_move_click = p.hotkeys.buy_move_click;
                                    p.hotkeys.short_sell_move_click = p.hotkeys.sell_move_click;
                                    p.hotkeys.short_buy_move_click2 = p.hotkeys.buy_move_click2;
                                    p.hotkeys.short_sell_move_click2 = p.hotkeys.sell_move_click2;
                                }
                                if changed {
                                    bcx.notify();
                                }
                            }
                        });
                    }),
            )
            .into_any_element()
    }

    fn set_hotkey(&mut self, slot: HotkeySlot, value: String, cx: &mut Context<Self>) {
        let changed = self.backend.update(cx, |b, bcx| {
            let mut changed = false;
            if let Some(p) = b.preview.as_mut() {
                changed = set_slot_value(&mut p.hotkeys, slot, value);
                if changed {
                    bcx.notify();
                }
            }
            changed
        });
        if changed {
            cx.notify();
        }
    }
}

fn hotkey_group(
    title: &str,
    hint: &str,
    p: MoonPalette,
    cx: &App,
    rows: impl IntoIterator<Item = AnyElement>,
) -> AnyElement {
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 7.0))
        .p(design::ui_px(cx, 10.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(rgba_from(p.border, 0.9))
        .bg(rgba_from(p.panel, 0.58))
        .child(
            v_flex()
                .gap(px(1.0))
                .child(
                    MoonText::new(title.to_string())
                        .mono(true)
                        .font_size(design::font_value(cx, 11.0))
                        .line_height(design::line_value(cx, 14.0))
                        .color(p.text)
                        .render(),
                )
                .child(
                    MoonText::new(hint.to_string())
                        .mono(true)
                        .font_size(design::font_value(cx, 9.0))
                        .line_height(design::line_value(cx, 12.0))
                        .color(p.text_muted)
                        .render(),
                ),
        )
        .children(rows)
        .into_any_element()
}

fn parse_hotkey(raw: &str) -> Option<Keystroke> {
    let raw = raw.trim();
    if raw.is_empty() {
        None
    } else {
        Keystroke::parse(raw).ok()
    }
}

fn slot_value(hotkeys: &HotkeysConfig, slot: HotkeySlot) -> &str {
    match slot {
        HotkeySlot::OrderSize(i) => &hotkeys.order_size[i],
        HotkeySlot::SellPreset(i) => &hotkeys.sell_preset[i],
        HotkeySlot::ManualStrategy(i) => &hotkeys.manual_strategy[i],
        HotkeySlot::CancelBuy => &hotkeys.cancel_buy,
        HotkeySlot::PanicSell => &hotkeys.panic_sell,
        HotkeySlot::PanicSellOne => &hotkeys.panic_sell_one,
        HotkeySlot::CancelAllBuys => &hotkeys.cancel_all_buys,
        HotkeySlot::JoinSells => &hotkeys.join_sells,
        HotkeySlot::SwitchCharts => &hotkeys.switch_charts,
        HotkeySlot::ReloadBook => &hotkeys.reload_book,
        HotkeySlot::NewLong => &hotkeys.new_long,
        HotkeySlot::NewShort => &hotkeys.new_short,
        HotkeySlot::SplitOrder => &hotkeys.split_order,
        HotkeySlot::SplitOrderX => &hotkeys.split_order_x,
        HotkeySlot::ShiftBuyUp => &hotkeys.shift_buy_up,
        HotkeySlot::ShiftBuyDown => &hotkeys.shift_buy_down,
        HotkeySlot::ShiftSellUp => &hotkeys.shift_sell_up,
        HotkeySlot::ShiftSellDown => &hotkeys.shift_sell_down,
        HotkeySlot::MakeShot => &hotkeys.make_shot,
        HotkeySlot::MakeShotBot => &hotkeys.make_shot_bot,
        HotkeySlot::ReloadChart => &hotkeys.reload_chart,
        HotkeySlot::ScalePlus => &hotkeys.scale_plus,
        HotkeySlot::ScaleMinus => &hotkeys.scale_minus,
        HotkeySlot::SellPlus => &hotkeys.sell_plus,
        HotkeySlot::SellMinus => &hotkeys.sell_minus,
        HotkeySlot::SpyMode => &hotkeys.spy_mode,
        HotkeySlot::ShowCharts => &hotkeys.show_charts,
        HotkeySlot::SwitchFigure => &hotkeys.switch_figure,
        HotkeySlot::FitSells => &hotkeys.fit_sells,
        HotkeySlot::Broadcast => &hotkeys.broadcast,
    }
}

fn set_slot_value(hotkeys: &mut HotkeysConfig, slot: HotkeySlot, value: String) -> bool {
    let target = match slot {
        HotkeySlot::OrderSize(i) => &mut hotkeys.order_size[i],
        HotkeySlot::SellPreset(i) => &mut hotkeys.sell_preset[i],
        HotkeySlot::ManualStrategy(i) => &mut hotkeys.manual_strategy[i],
        HotkeySlot::CancelBuy => &mut hotkeys.cancel_buy,
        HotkeySlot::PanicSell => &mut hotkeys.panic_sell,
        HotkeySlot::PanicSellOne => &mut hotkeys.panic_sell_one,
        HotkeySlot::CancelAllBuys => &mut hotkeys.cancel_all_buys,
        HotkeySlot::JoinSells => &mut hotkeys.join_sells,
        HotkeySlot::SwitchCharts => &mut hotkeys.switch_charts,
        HotkeySlot::ReloadBook => &mut hotkeys.reload_book,
        HotkeySlot::NewLong => &mut hotkeys.new_long,
        HotkeySlot::NewShort => &mut hotkeys.new_short,
        HotkeySlot::SplitOrder => &mut hotkeys.split_order,
        HotkeySlot::SplitOrderX => &mut hotkeys.split_order_x,
        HotkeySlot::ShiftBuyUp => &mut hotkeys.shift_buy_up,
        HotkeySlot::ShiftBuyDown => &mut hotkeys.shift_buy_down,
        HotkeySlot::ShiftSellUp => &mut hotkeys.shift_sell_up,
        HotkeySlot::ShiftSellDown => &mut hotkeys.shift_sell_down,
        HotkeySlot::MakeShot => &mut hotkeys.make_shot,
        HotkeySlot::MakeShotBot => &mut hotkeys.make_shot_bot,
        HotkeySlot::ReloadChart => &mut hotkeys.reload_chart,
        HotkeySlot::ScalePlus => &mut hotkeys.scale_plus,
        HotkeySlot::ScaleMinus => &mut hotkeys.scale_minus,
        HotkeySlot::SellPlus => &mut hotkeys.sell_plus,
        HotkeySlot::SellMinus => &mut hotkeys.sell_minus,
        HotkeySlot::SpyMode => &mut hotkeys.spy_mode,
        HotkeySlot::ShowCharts => &mut hotkeys.show_charts,
        HotkeySlot::SwitchFigure => &mut hotkeys.switch_figure,
        HotkeySlot::FitSells => &mut hotkeys.fit_sells,
        HotkeySlot::Broadcast => &mut hotkeys.broadcast,
    };
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
    }
}
