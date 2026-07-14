//! Сборка вкладки «Хоткеи» — раскладка как в Moonbot: сверху всегда видимый блок
//! встроенных (захардкоженных) хоткеев, под ним переключатель групп-саб-вкладок
//! (`SettingsView.hotkeys_group`), ниже — строки активной группы. Однострочные
//! редакторы (`hotkey_row`/`mouse_row`/`same_move_checkbox`) пишут в draft.

use gpui::*;
use moon_core::config::{
    HotkeysConfig, MANUAL_STRATEGY_KEYS, MouseGestureBinding, ORDER_SIZE_KEYS, SELL_PRESET_KEYS,
};
use moon_ui::{
    MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonDropdown,
    MoonHotkeyInput, MoonMenuItem, MoonMenuSize, MoonPalette, MoonRect, MoonTabItem, MoonTabStrip,
    MoonText, h_flex, v_flex,
};
use rust_i18n::t;

use super::{
    HotkeyGroup, HotkeySlot, MouseSlot, mouse_slot_id, mouse_slot_value, mouse_slot_wip,
    parse_hotkey, set_mouse_slot_value, set_slot_value, slot_id, slot_value,
};
use crate::design;
use crate::settings::SettingsView;

impl SettingsView {
    pub(in crate::settings) fn hotkeys_tab(&self, cx: &Context<Self>) -> impl IntoElement {
        let hotkeys = {
            let b = self.backend.read(cx);
            b.preview.as_ref().unwrap_or(&b.config).hotkeys.clone()
        };
        let p = MoonPalette::active(cx);

        // Как в Moonbot: сверху — фиксированные встроенные хоткеи (всегда видны), под
        // ними переключатель групп (саб-вкладки), ниже — строки только активной группы.
        let builtin = v_flex()
            .w_full()
            .gap(design::ui_px(cx, 3.0))
            .child(
                MoonText::new(t!("hotkeys.group.builtin").to_string())
                    .uppercase(false)
                    .mono(true)
                    .font_size(design::font_value(cx, 11.0))
                    .line_height(design::line_value(cx, 14.0))
                    .color(p.text)
                    .render(),
            )
            .child(
                MoonText::new(t!("hotkeys.group.builtin_hint").to_string())
                    .uppercase(false)
                    .mono(true)
                    .wrap()
                    .font_size(design::font_value(cx, 9.0))
                    .line_height(design::line_value(cx, 12.0))
                    .color(p.text_muted)
                    .render(),
            )
            .children([
                    self.builtin_row(t!("hotkeys.builtin.wheel_zoom").to_string(), cx),
                    self.builtin_row(t!("hotkeys.builtin.wheel_pan").to_string(), cx),
                    self.builtin_row(t!("hotkeys.builtin.cancel_hover").to_string(), cx),
                    self.builtin_row(t!("hotkeys.builtin.esc_close").to_string(), cx),
                    self.builtin_row(t!("hotkeys.builtin.close_all").to_string(), cx),
                    self.builtin_row(t!("hotkeys.builtin.reset_windows").to_string(), cx),
            ]);

        // Переключатель групп — тот же контрол, что полоска чарт-вкладок главного окна
        // («Main»/Add: MoonTabStrip + MoonTabItem — обычный шрифт, без капса кнопок).
        // Стрип рисует табы абсолютно и без явных bounds схлопывается в 0×0 (см.
        // chart_tabs/strip.rs) — ширину даём с запасом, контейнер ниже режет по своей.
        let entity = cx.entity();
        let strip_h = design::fit_h_px(cx, 28.0, 13.0, 7.5);
        let items: Vec<MoonTabItem> = HotkeyGroup::ALL
            .iter()
            .map(|g| {
                let label = g.title();
                // Ширина по содержимому, как у чарт-вкладок (~7px/символ + паддинги).
                let width = (label.chars().count() as f32 * 7.0 + 28.0).clamp(64.0, 168.0);
                MoonTabItem::new(label)
                    .width(width)
                    .selected(self.hotkeys_group == *g)
            })
            .collect();
        let switcher = div()
            .relative()
            .w_full()
            .h(strip_h)
            .overflow_hidden()
            .child(
                MoonTabStrip::new("hotkeys-group-strip")
                    .gap(4.0)
                    .bounds(MoonRect::new(0.0, 0.0, 2000.0, f32::from(strip_h)))
                    .items(items)
                    .on_click(move |ix, _event, _window, app| {
                        let Some(g) = HotkeyGroup::ALL.get(ix).copied() else {
                            return;
                        };
                        entity.update(app, |this, c| {
                            if this.hotkeys_group != g {
                                this.hotkeys_group = g;
                                c.notify();
                            }
                        });
                    })
                    .render(),
            );

        let body = v_flex()
            .w_full()
            .gap(design::ui_px(cx, 3.0))
            .child(
                MoonText::new(self.hotkeys_group.hint())
                    .uppercase(false)
                    .mono(true)
                    .wrap()
                    .font_size(design::font_value(cx, 9.0))
                    .line_height(design::line_value(cx, 12.0))
                    .color(p.text_muted)
                    .render(),
            )
            .children(self.group_rows(self.hotkeys_group, &hotkeys, cx));

        v_flex()
            .w_full()
            .gap(design::ui_px(cx, 10.0))
            .child(builtin)
            .child(switcher)
            .child(body)
    }

    /// Строки активной группы (содержимое саб-вкладки). `hotkeys` клонируем локально —
    /// строки-редакторы берут `&hotkeys` (перенесены из аккордеон-версии без правок).
    fn group_rows(
        &self,
        group: HotkeyGroup,
        hotkeys: &HotkeysConfig,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let hotkeys = hotkeys.clone();
        match group {
            HotkeyGroup::Presets => (0..ORDER_SIZE_KEYS)
                    .map(|i| {
                        let title = format!("F{}", i + 1);
                        let desc = t!("hotkeys.order_size", n = i + 1).to_string();
                        self.hotkey_row(title, desc, HotkeySlot::OrderSize(i), &hotkeys, cx)
                    })
                    .chain((0..SELL_PRESET_KEYS).map(|i| {
                        let title = format!("S{}", i + 1);
                        let desc = t!("hotkeys.sell_preset", n = i + 1).to_string();
                        self.hotkey_row(title, desc, HotkeySlot::SellPreset(i), &hotkeys, cx)
                    }))
                    .collect(),
            HotkeyGroup::Trading => vec![
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
            HotkeyGroup::Chart => vec![
                    self.hotkey_row(
                        t!("hotkeys.switch_charts").to_string(),
                        t!("hotkeys.switch_charts_hint").to_string(),
                        HotkeySlot::SwitchCharts,
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
            HotkeyGroup::Draw => vec![
                    self.hotkey_row(
                        t!("hotkeys.draw_hline").to_string(),
                        t!("hotkeys.draw_hline_hint").to_string(),
                        HotkeySlot::DrawHline,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.draw_segment").to_string(),
                        t!("hotkeys.draw_segment_hint").to_string(),
                        HotkeySlot::DrawSegment,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.draw_triangle").to_string(),
                        t!("hotkeys.draw_triangle_hint").to_string(),
                        HotkeySlot::DrawTriangle,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.draw_channel").to_string(),
                        t!("hotkeys.draw_channel_hint").to_string(),
                        HotkeySlot::DrawChannel,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.fig_delete").to_string(),
                        t!("hotkeys.fig_delete_hint").to_string(),
                        HotkeySlot::FigDelete,
                        &hotkeys,
                        cx,
                    ),
                    self.hotkey_row(
                        t!("hotkeys.fig_alert").to_string(),
                        t!("hotkeys.fig_alert_hint").to_string(),
                        HotkeySlot::FigAlert,
                        &hotkeys,
                        cx,
                    ),
                ],
            HotkeyGroup::OrderMove => vec![
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
                ],
            HotkeyGroup::Mouse => vec![
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
            HotkeyGroup::ManualStrategy => (0..MANUAL_STRATEGY_KEYS)
                .map(|i| {
                    self.hotkey_row(
                        t!("hotkeys.manual_strategy", n = i + 1).to_string(),
                        t!("hotkeys.manual_strategy_hint", n = i + 1).to_string(),
                        HotkeySlot::ManualStrategy(i),
                        &hotkeys,
                        cx,
                    )
                })
                .collect(),
        }
    }

    /// Строка-описание встроенного (не конфигурируемого) хоткея: только текст, без
    /// редактора. Как справочная вкладка «Встроенные хоткеи» в Moonbot.
    fn builtin_row(&self, line: impl Into<String>, cx: &Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        h_flex()
            .w_full()
            .min_h(design::fit_h_px(cx, 22.0, 11.0, 5.0))
            .items_center()
            .child(
                MoonText::new(line.into())
                    .uppercase(false)
                    .mono(true)
                    .wrap()
                    .font_size(design::font_value(cx, 11.0))
                    .line_height(design::line_value(cx, 14.0))
                    .color(p.text_muted)
                    .render(),
            )
            .into_any_element()
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
            .min_h(design::fit_h_px(cx, 24.0, 12.0, 6.0))
            .gap(design::ui_px(cx, 10.0))
            .items_center()
            .child(
                MoonText::new(title.into())
                    .uppercase(false)
                    .mono(true)
                    .font_size(design::font_value(cx, 11.0))
                    .line_height(design::line_value(cx, 14.0))
                    .color(p.text)
                    .render(),
            )
            .child(
                // Описание — тем же размером, серым, с переносом (не уходит за окно).
                div().flex_1().min_w_0().child(
                    MoonText::new(desc.into())
                        .uppercase(false)
                        .mono(true)
                        .wrap()
                        .font_size(design::font_value(cx, 11.0))
                        .line_height(design::line_value(cx, 14.0))
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
        let wip = mouse_slot_wip(slot);
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

        let mut row = h_flex()
            .w_full()
            .min_h(design::fit_h_px(cx, 24.0, 12.0, 6.0))
            .gap(design::ui_px(cx, 10.0))
            .items_center()
            .child(
                MoonText::new(title.into())
                    .uppercase(false)
                    .mono(true)
                    .font_size(design::font_value(cx, 11.0))
                    .line_height(design::line_value(cx, 14.0))
                    .color(if disabled { p.text_muted } else { p.text })
                    .render(),
            )
            .child(
                // Описание — тем же размером, серым, с переносом (не уходит за окно).
                div().flex_1().min_w_0().child(
                    MoonText::new(desc.into())
                        .uppercase(false)
                        .mono(true)
                        .wrap()
                        .font_size(design::font_value(cx, 11.0))
                        .line_height(design::line_value(cx, 14.0))
                        .color(p.text_muted)
                        .render(),
                ),
            );
        if wip {
            row = row.child(self.wip_tag(&p, cx));
        }
        row
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

    /// Янтарный бейдж «не подключено» для строк, чей рантайм-путь ещё не существует
    /// ([`slot_wip`]/[`mouse_slot_wip`]): клавиша/жест сохраняются в конфиг, но действия нет.
    fn wip_tag(&self, p: &MoonPalette, cx: &Context<Self>) -> AnyElement {
        MoonText::new(t!("hotkeys.todo").to_string())
            .uppercase(false)
            .mono(true)
            .font_size(design::font_value(cx, 9.0))
            .line_height(design::line_value(cx, 12.0))
            .color(p.amber)
            .render()
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
