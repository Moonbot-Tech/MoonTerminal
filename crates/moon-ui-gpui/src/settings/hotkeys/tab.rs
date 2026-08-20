//! Builds the Hotkeys tab in a Moonbot-style layout: an always-visible block of hard-coded
//! built-in hotkeys, a group sub-tab switcher (`SettingsView.hotkeys_group`), and the active
//! group's rows. Single-row editors (`hotkey_row`, `mouse_row`, and `same_move_checkbox`) update
//! the draft.

use gpui::*;
use moon_core::config::{
    HotkeysConfig, MANUAL_STRATEGY_KEYS, MouseGestureBinding, MoveKind, ORDER_SIZE_KEYS,
    SELL_PRESET_KEYS, SPLIT_ORDER_PARTS, SPLIT_PARTS_MAX, SPLIT_PARTS_MIN,
};
use moon_ui::{
    MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonDropdown,
    MoonHotkeyInput, MoonMenuItem, MoonMenuSize, MoonPalette, MoonRect, MoonTabItem, MoonTabStrip,
    MoonText, h_flex, v_flex,
};
use rust_i18n::t;

use super::{
    HotkeyGroup, HotkeySlot, MouseSlot, MoveKindSlot, mouse_slot_id, mouse_slot_value,
    mouse_slot_wip, move_kind_slot_id, move_kind_slot_value, parse_hotkey, set_mouse_slot_value,
    set_move_kind_slot_value, set_slot_value, slot_id, slot_value,
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

        // Match Moonbot: fixed built-ins stay at the top, the group sub-tabs follow, and only the
        // active group's rows appear below.
        let builtin = v_flex()
            .w_full()
            .gap(design::ui_px(cx, 3.0))
            .child(
                MoonText::new(t!("hotkeys.group.builtin").to_string())
                    .uppercase(false)
                    .mono(true)
                    .font_size(11.0)
                    .line_height(14.0)
                    .color(p.text)
                    .render(),
            )
            .child(
                MoonText::new(t!("hotkeys.group.builtin_hint").to_string())
                    .uppercase(false)
                    .mono(true)
                    .wrap()
                    .line_height(12.0)
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

        // Reuse the main window's chart-tab control (`MoonTabStrip` + `MoonTabItem`) for
        // normal-case labels. Its tabs use absolute positioning and collapse to zero without
        // explicit bounds; provide ample width and let the enclosing container clip it.
        let entity = cx.entity();
        let strip_h = design::fit_h_px(cx, 28.0, 13.0, 7.5);
        let items: Vec<MoonTabItem> = HotkeyGroup::ALL
            .iter()
            .map(|g| {
                let label = g.title();
                // Size from label content like chart tabs, using roughly seven pixels per
                // character plus padding.
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
                    .line_height(12.0)
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

    /// Builds the active group's sub-tab rows.
    ///
    /// The supplied hotkey snapshot is cloned locally before its values are passed to row builders.
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
                    t!("hotkeys.split_order_hint", n = SPLIT_ORDER_PARTS).to_string(),
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
                self.split_parts_row(&hotkeys, cx),
                self.hotkey_row(
                    t!("hotkeys.sells_to_rect").to_string(),
                    t!("hotkeys.sells_to_rect_hint").to_string(),
                    HotkeySlot::SellsToRect,
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
                self.hotkey_row(
                    t!("hotkeys.chart_shot").to_string(),
                    t!("hotkeys.chart_shot_hint").to_string(),
                    HotkeySlot::ChartShot,
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
                    None,
                    &hotkeys,
                    false,
                    cx,
                ),
                self.mouse_row(
                    t!("hotkeys.mouse.short_set").to_string(),
                    t!("hotkeys.mouse.short_set_hint").to_string(),
                    MouseSlot::ShortSet,
                    None,
                    &hotkeys,
                    false,
                    cx,
                ),
                self.mouse_row(
                    t!("hotkeys.mouse.pending_long").to_string(),
                    t!("hotkeys.mouse.pending_long_hint").to_string(),
                    MouseSlot::PendingLong,
                    None,
                    &hotkeys,
                    false,
                    cx,
                ),
                self.mouse_row(
                    t!("hotkeys.mouse.pending_short").to_string(),
                    t!("hotkeys.mouse.pending_short_hint").to_string(),
                    MouseSlot::PendingShort,
                    None,
                    &hotkeys,
                    false,
                    cx,
                ),
                self.mouse_row(
                    t!("hotkeys.mouse.buy_move").to_string(),
                    t!("hotkeys.mouse.buy_move_hint").to_string(),
                    MouseSlot::BuyMove,
                    Some(MoveKindSlot::BuyMove),
                    &hotkeys,
                    false,
                    cx,
                ),
                self.mouse_row(
                    t!("hotkeys.mouse.sell_move").to_string(),
                    t!("hotkeys.mouse.sell_move_hint").to_string(),
                    MouseSlot::SellMove,
                    Some(MoveKindSlot::SellMove),
                    &hotkeys,
                    false,
                    cx,
                ),
                self.mouse_row(
                    t!("hotkeys.mouse.buy_move2").to_string(),
                    t!("hotkeys.mouse.buy_move2_hint").to_string(),
                    MouseSlot::BuyMove2,
                    Some(MoveKindSlot::BuyMove2),
                    &hotkeys,
                    false,
                    cx,
                ),
                self.mouse_row(
                    t!("hotkeys.mouse.sell_move2").to_string(),
                    t!("hotkeys.mouse.sell_move2_hint").to_string(),
                    MouseSlot::SellMove2,
                    Some(MoveKindSlot::SellMove2),
                    &hotkeys,
                    false,
                    cx,
                ),
                self.same_move_checkbox(&hotkeys, cx),
                self.mouse_row(
                    t!("hotkeys.mouse.short_buy_move").to_string(),
                    t!("hotkeys.mouse.short_buy_move_hint").to_string(),
                    MouseSlot::ShortBuyMove,
                    None,
                    &hotkeys,
                    hotkeys.same_hotkeys_for_move,
                    cx,
                ),
                self.mouse_row(
                    t!("hotkeys.mouse.short_sell_move").to_string(),
                    t!("hotkeys.mouse.short_sell_move_hint").to_string(),
                    MouseSlot::ShortSellMove,
                    None,
                    &hotkeys,
                    hotkeys.same_hotkeys_for_move,
                    cx,
                ),
                self.mouse_row(
                    t!("hotkeys.mouse.short_buy_move2").to_string(),
                    t!("hotkeys.mouse.short_buy_move2_hint").to_string(),
                    MouseSlot::ShortBuyMove2,
                    None,
                    &hotkeys,
                    hotkeys.same_hotkeys_for_move,
                    cx,
                ),
                self.mouse_row(
                    t!("hotkeys.mouse.short_sell_move2").to_string(),
                    t!("hotkeys.mouse.short_sell_move2_hint").to_string(),
                    MouseSlot::ShortSellMove2,
                    None,
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

    /// Builds a text-only row for a hard-coded, non-configurable hotkey, matching Moonbot's
    /// built-in hotkey reference page.
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
                    .font_size(11.0)
                    .line_height(14.0)
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
        // A key held by two slots resolves by branch order in the dispatcher, so one of the two
        // silently never fires. Show it here rather than letting the user hunt for it.
        let conflict = !raw.trim().is_empty()
            && hotkeys
                .bound_keys()
                .iter()
                .filter(|held| held.as_str() == raw.trim())
                .count()
                > 1;
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
                    .font_size(11.0)
                    .line_height(14.0)
                    .color(p.text)
                    .render(),
            )
            .child(
                // Match title sizing, use muted text, and wrap within the window.
                div().flex_1().min_w_0().child(
                    MoonText::new(desc.into())
                        .uppercase(false)
                        .mono(true)
                        .wrap()
                        .font_size(11.0)
                        .line_height(14.0)
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
                    .conflict(conflict)
                    .compact()
                    .width(176.0)
                    .on_change(
                        cx.processor(move |this, value: Option<Keystroke>, _window, cx| {
                            // Store the PHYSICAL key: a letter recorded under a Cyrillic layout
                            // would otherwise be saved as that layout's character.
                            let value = value
                                .map(|k| crate::hotkeys::recorded_keystroke(k).unparse())
                                .unwrap_or_default();
                            this.set_hotkey(slot, value, cx);
                        }),
                    ),
            )
            .into_any_element()
    }

    /// One mouse-gesture row: the binding, and for a move row the "Move kind" beside it.
    ///
    /// Args:
    ///     title: Row label.
    ///     desc: Row description.
    ///     slot: Gesture slot the first dropdown edits.
    ///     kind_slot: Move-kind slot for a move row, or `None` for a row that has no kind — the
    ///         placement rows, and the short rows, which share the long row's kind exactly as
    ///         Moonbot's single kind column does.
    ///     hotkeys: Configuration being edited.
    ///     disabled: Whether the row is inert because the mirror flag owns it.
    ///     cx: Settings context.
    ///
    /// Returns:
    ///     The rendered row.
    #[allow(clippy::too_many_arguments)]
    fn mouse_row(
        &self,
        title: impl Into<String>,
        desc: impl Into<String>,
        slot: MouseSlot,
        kind_slot: Option<MoveKindSlot>,
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

        let mut row = self.row_head(title.into(), desc.into(), disabled, cx);
        if wip {
            row = row.child(self.wip_tag(&p, cx));
        }
        row = row.child(
            Self::row_dropdown(id, current.label())
                .trigger_variant(if current == MouseGestureBinding::None {
                    MoonButtonVariant::Neutral
                } else {
                    MoonButtonVariant::Blue
                })
                .menu_width_scaled(228.0)
                .disabled(disabled)
                .items(items),
        );
        if let Some(kind_slot) = kind_slot {
            row = row.child(self.move_kind_dropdown(kind_slot, hotkeys, disabled));
        }
        row.into_any_element()
    }

    /// The "Move kind" selector of one move row — Moonbot's column of the same name.
    ///
    /// The gesture says WHERE (the clicked price); this says which orders go there and how the core
    /// arranges them. `None` leaves the gesture recognised and inert, which is Moonbot's own way of
    /// switching one off without clearing the binding.
    fn move_kind_dropdown(
        &self,
        slot: MoveKindSlot,
        hotkeys: &HotkeysConfig,
        disabled: bool,
    ) -> impl IntoElement {
        let current = move_kind_slot_value(hotkeys, slot);
        let backend = self.backend.clone();
        let items = MoveKind::ALL.into_iter().map(move |kind| {
            let backend = backend.clone();
            let label_key = format!("hotkeys.move_kind.{}", kind.id());
            MoonMenuItem::with_key(kind.id(), t!(&label_key).to_string())
                .checked(kind == current)
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        if let Some(p) = b.preview.as_mut()
                            && set_move_kind_slot_value(&mut p.hotkeys, slot, kind)
                        {
                            bcx.notify();
                        }
                    });
                })
        });
        let current_key = format!("hotkeys.move_kind.{}", current.id());
        Self::row_dropdown(
            format!("move-kind-{}", move_kind_slot_id(slot)),
            t!(&current_key).to_string(),
        )
        // The trigger shows the chosen kind, so the menu carries the name of the setting — the
        // "Move kind" column heading Moonbot puts above the same list.
        .header(18.0, |_, cx| {
            let p = MoonPalette::active(cx);
            MoonText::new(t!("hotkeys.move_kind.title").to_string())
                .uppercase(false)
                .mono(true)
                .font_size(9.0)
                .line_height(12.0)
                .color(p.text_muted)
                .render()
                .into_any_element()
        })
        .trigger_variant(if current == MoveKind::None {
            MoonButtonVariant::Neutral
        } else {
            MoonButtonVariant::Blue
        })
        .menu_width_scaled(228.0)
        .disabled(disabled)
        .items(items)
    }

    /// Builds the shared leading half of an editor row: title, then the wrapping description.
    ///
    /// Every row on this tab is that pair plus one control, and the sizes are deliberately equal —
    /// a description one step smaller was tried and read as a different font.
    fn row_head(
        &self,
        title: String,
        desc: String,
        disabled: bool,
        cx: &Context<Self>,
    ) -> gpui::Div {
        let p = MoonPalette::active(cx);
        h_flex()
            .w_full()
            .min_h(design::fit_h_px(cx, 24.0, 12.0, 6.0))
            .gap(design::ui_px(cx, 10.0))
            .items_center()
            .child(
                MoonText::new(title)
                    .uppercase(false)
                    .mono(true)
                    .font_size(11.0)
                    .line_height(14.0)
                    .color(if disabled { p.text_muted } else { p.text })
                    .render(),
            )
            .child(
                // Match title sizing, use muted text, and wrap within the window.
                div().flex_1().min_w_0().child(
                    MoonText::new(desc)
                        .uppercase(false)
                        .mono(true)
                        .wrap()
                        .font_size(11.0)
                        .line_height(14.0)
                        .color(p.text_muted)
                        .render(),
                ),
            )
    }

    /// Builds a row's trailing dropdown with the trigger geometry shared by this tab.
    fn row_dropdown(id: String, label: impl Into<SharedString>) -> MoonDropdown {
        MoonDropdown::new(SharedString::from(id))
            .label(label)
            .trigger_caret(true)
            .trigger_size(MoonButtonSize::Micro)
            .trigger_width_scaled(176.0)
            .menu_size(MoonMenuSize::Compact)
    }

    /// Builds the part-count selector for `Split N` (Moonbot `Hotkeys.SplitParts`).
    ///
    /// A dropdown over the allowed range rather than a text field: the value goes straight into a
    /// live split command, and a picker cannot leave a half-typed number in the draft.
    fn split_parts_row(&self, hotkeys: &HotkeysConfig, cx: &Context<Self>) -> AnyElement {
        let current = hotkeys.split_n_parts();
        let items = (SPLIT_PARTS_MIN..=SPLIT_PARTS_MAX).map(|parts| {
            let backend = self.backend.clone();
            MoonMenuItem::with_key(format!("split-parts-{parts}"), parts.to_string())
                .checked(i32::from(parts) == current)
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        if let Some(preview) = b.preview.as_mut()
                            && preview.hotkeys.split_parts != parts
                        {
                            preview.hotkeys.split_parts = parts;
                            bcx.notify();
                        }
                    });
                })
        });

        self.row_head(
            t!("hotkeys.split_parts").to_string(),
            t!("hotkeys.split_parts_hint").to_string(),
            false,
            cx,
        )
        .child(
            Self::row_dropdown("hotkey-split-parts".into(), current.to_string())
                .trigger_variant(MoonButtonVariant::Blue)
                .menu_width_scaled(120.0)
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

    /// Builds the amber "not connected" badge for mouse gestures without a runtime consumer.
    ///
    /// The selection is saved to configuration, but no action executes it yet.
    fn wip_tag(&self, p: &MoonPalette, _cx: &Context<Self>) -> AnyElement {
        MoonText::new(t!("hotkeys.todo").to_string())
            .uppercase(false)
            .mono(true)
            .line_height(12.0)
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
