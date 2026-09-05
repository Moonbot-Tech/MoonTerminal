//! Builds the Hotkeys tab in a Moonbot-style layout: an always-visible block of hard-coded
//! built-in hotkeys, a group sub-tab switcher (`SettingsView.hotkeys_group`), and the active
//! group's rows. Single-row editors (`hotkey_row`, `mouse_row`, and `same_move_checkbox`) update
//! the draft.

use gpui::*;
use moon_core::config::moonbot_import::shortcut;
use moon_core::config::{
    HotkeysConfig, MANUAL_STRATEGY_KEYS, MouseGestureBinding, MoveKind, ORDER_SIZE_KEYS,
    SELL_PRESET_KEYS, SPLIT_ORDER_PARTS, SPLIT_PARTS_MAX, SPLIT_PARTS_MIN,
};
use moon_core::feed::CoreConfigState;
use moon_core::session::CoreId;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonDropdown,
    MoonHotkeyInput, MoonKbd, MoonKbdSize, MoonMenuItem, MoonMenuSize, MoonPalette, MoonTabItem,
    MoonTabStrip, MoonText, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::pull::{PullRow, PullVerdict, apply_core_hotkeys, preview_core_hotkeys};
use super::{
    HotkeyGroup, HotkeySlot, MouseSlot, MoveKindSlot, mouse_slot_id, mouse_slot_value,
    mouse_slot_wip, move_kind_slot_id, move_kind_slot_value, parse_hotkey, set_mouse_slot_value,
    set_move_kind_slot_value, set_slot_value, slot_id, slot_label, slot_value,
};
use crate::design;
use crate::settings::SettingsView;

/// Logical width reserved for every hotkey row title.
const ROW_TITLE_WIDTH: f32 = 160.0;

/// Maximum readable width of a hotkey row description before its editor column begins.
const ROW_DESCRIPTION_MAX_WIDTH: f32 = 640.0;

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
        // normal-case labels. Overflow-menu defaults off, so a short group list stays chevron-free.
        let entity = cx.entity();
        let strip_h = design::fit_h_px(cx, 28.0, 13.0, 7.5);
        let items: Vec<MoonTabItem> = HotkeyGroup::ALL
            .iter()
            .map(|g| MoonTabItem::new(g.title()).selected(self.hotkeys_group == *g))
            .collect();
        let switcher = div().w_full().h(strip_h).child(
            MoonTabStrip::new("hotkeys-group-strip")
                .gap(4.0)
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
                self.hotkey_row(
                    t!("hotkeys.fig_undo").to_string(),
                    t!("hotkeys.fig_undo_hint").to_string(),
                    HotkeySlot::FigUndo,
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
                .chain(self.core_pull_section(&hotkeys, cx))
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

    /// Build one keyboard shortcut row with the editor in the tab's shared control column.
    ///
    /// Args:
    ///     title: Shortcut label shown in the fixed title column.
    ///     desc: Localized explanation that wraps within its description column.
    ///     slot: Hotkey configuration slot edited by the input.
    ///     hotkeys: Draft configuration used to show the current binding and conflicts.
    ///     cx: Settings context used for palette, scaling, and input events.
    ///
    /// Returns:
    ///     The rendered shortcut row.
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
                div()
                    .flex_none()
                    .w(design::ui_px(cx, ROW_TITLE_WIDTH))
                    .child(
                        MoonText::new(title.into())
                            .uppercase(false)
                            .mono(true)
                            .wrap()
                            .font_size(11.0)
                            .line_height(14.0)
                            .color(p.text)
                            .render(),
                    ),
            )
            .child(
                // Match title sizing, use muted text, and wrap within the window.
                div()
                    .flex_1()
                    .min_w_0()
                    .max_w(design::ui_px(cx, ROW_DESCRIPTION_MAX_WIDTH))
                    .child(
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

    /// Build one mouse-gesture row with a binding and, for move rows, a "Move kind" selector.
    /// The trailing controls wrap at narrow widths rather than clipping.
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
            MoonMenuItem::with_key(gesture.config_value(), gesture.menu_label())
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

        let mut row = self
            .row_head(title.into(), desc.into(), disabled, cx)
            .child(
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
        if wip {
            row = row.child(self.wip_tag(&p, cx));
        }
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
            let label_key = kind.locale_key();
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
        let current_key = current.locale_key();
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
    /// Every row on this tab is that pair plus one or two controls. The row wraps trailing controls
    /// at narrow widths instead of clipping them, and the text sizes are deliberately equal — a
    /// description one step smaller was tried and read as a different font.
    ///
    /// Args:
    ///     title: Label displayed in the shared fixed-width title column.
    ///     desc: Muted description that may wrap within its capped column.
    ///     disabled: Whether the title uses muted styling.
    ///     cx: Settings context used for palette and scaled layout.
    ///
    /// Returns:
    ///     The row prefix to which callers append one or two controls.
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
            .flex_wrap()
            .min_h(design::fit_h_px(cx, 24.0, 12.0, 6.0))
            .gap(design::ui_px(cx, 10.0))
            .items_center()
            .child(
                div()
                    .flex_none()
                    .w(design::ui_px(cx, ROW_TITLE_WIDTH))
                    .child(
                        MoonText::new(title)
                            .uppercase(false)
                            .mono(true)
                            .wrap()
                            .font_size(11.0)
                            .line_height(14.0)
                            .color(if disabled { p.text_muted } else { p.text })
                            .render(),
                    ),
            )
            .child(
                // Match title sizing, use muted text, and wrap within the window.
                div()
                    .flex_1()
                    .min_w_0()
                    .max_w(design::ui_px(cx, ROW_DESCRIPTION_MAX_WIDTH))
                    .child(
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

    /// Builds the move-mirroring checkbox in the same control column as the gesture editors.
    ///
    /// Args:
    ///     hotkeys: Draft configuration that supplies the checkbox state.
    ///     cx: Settings context used for scaled layout and change events.
    ///
    /// Returns:
    ///     The aligned move-mirroring checkbox row.
    fn same_move_checkbox(&self, hotkeys: &HotkeysConfig, cx: &Context<Self>) -> AnyElement {
        let backend = self.backend.clone();

        h_flex()
            .w_full()
            .min_h(design::fit_h_px(cx, 30.0, 12.0, 6.0))
            .gap(design::ui_px(cx, 10.0))
            .items_center()
            .child(div().flex_none().w(design::ui_px(cx, ROW_TITLE_WIDTH)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .max_w(design::ui_px(cx, ROW_DESCRIPTION_MAX_WIDTH)),
            )
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

    /// Resolves the core whose layout the "pull" button addresses: the group's active trade
    /// core, the same resolution the header's manual-strategy cluster already uses.
    ///
    /// The Hotkeys tab has no owning window group of its own (unlike the toolbar or header, which
    /// render inside one group's window) — Settings is one shared window. `Backend::
    /// singleton_workspace` is the existing resolver for exactly this situation: it is the same
    /// "last focused live Auto group" the Strategies and Analytics windows already use to answer
    /// group-shaped questions from an unscoped window (`strategies/window.rs`,
    /// `analytics/tuner/mod.rs`).
    fn core_pull_target(&self, cx: &Context<Self>) -> Option<CoreId> {
        let b = self.backend.read(cx);
        let group = b.singleton_workspace()?.group;
        b.active_trade_core(&group)
    }

    /// Requests an on-purpose refresh and arms the `Pending` state for `core`.
    ///
    /// Fire-and-forget: completion arrives as a `SharedConfigUpdated` -> `FeedMsg::CoreConfig`,
    /// bumping `core_config_recv_rev` unconditionally even when the arriving config is
    /// byte-identical to what is already retained — which is exactly why `Pending` polls
    /// `core_config_recv_rev` here rather than the compare-then-bump `core_config_rev`; the
    /// latter would never clear on an identical echo.
    fn request_core_pull(&mut self, core: CoreId, cx: &mut Context<Self>) {
        let baseline = self
            .backend
            .read(cx)
            .session
            .store()
            .core(core)
            .map(|d| d.core_config_recv_rev)
            .unwrap_or(0);
        if let Err(error) = self.backend.read(cx).session.refresh_shared_config(core) {
            self.status = Some((crate::settings::StatusMsg::Text(error.to_string()), true));
        }
        self.core_pull = Some((core, baseline));
        cx.notify();
    }

    /// Applies every `WillApply` row of the CURRENT preview (rebuilt fresh here, not reused from
    /// render — the two are the same computation over the same draft, so they cannot disagree)
    /// and writes `hotkeys.toml` immediately.
    ///
    /// This bypasses the tab's usual preview/Save cycle on purpose: `HotkeysConfig::save()` is a
    /// separate file with its own saver, no `config_dirty` involved. Writing both
    /// `config.hotkeys` and `preview.hotkeys` keeps them in sync so a LATER "Settings > Save"
    /// click (which starts from `preview`) cannot silently roll the pull back to what the draft
    /// looked like when the window opened.
    fn confirm_core_pull(&mut self, core: CoreId, cx: &mut Context<Self>) {
        let outcome = self.backend.update(cx, |b, bcx| {
            let (layout, manual_strategy_keys) = b
                .session
                .store()
                .core(core)
                .and_then(|d| d.core_config.as_ref())
                .map(|c| {
                    (
                        c.manual.core_hotkeys.clone(),
                        c.manual.strat_buttons.hot_keys,
                    )
                })?;
            let base = b
                .preview
                .as_ref()
                .map(|p| p.hotkeys.clone())
                .unwrap_or_else(|| b.config.hotkeys.clone());
            let rows = preview_core_hotkeys(&base, &layout, &manual_strategy_keys);
            let mut hotkeys = base;
            let changed = apply_core_hotkeys(&mut hotkeys, &rows);
            if changed {
                b.config.hotkeys = hotkeys.clone();
                if let Some(p) = b.preview.as_mut() {
                    p.hotkeys = hotkeys.clone();
                }
                bcx.notify();
            }
            Some((changed, hotkeys))
        });
        match outcome {
            Some((true, hotkeys)) => match hotkeys.save() {
                Ok(()) => {
                    self.status = Some((
                        crate::settings::StatusMsg::Key("hotkeys.pull.applied"),
                        false,
                    ))
                }
                Err(e) => {
                    self.status = Some((crate::settings::StatusMsg::Text(e.to_string()), true))
                }
            },
            Some((false, _)) => {
                self.status = Some((
                    crate::settings::StatusMsg::Key("hotkeys.pull.nothing_to_apply"),
                    false,
                ))
            }
            None => {}
        }
        self.core_pull = None;
        cx.notify();
    }

    /// One preview row: the slot's own identity label (without it, two visually identical `F1 ->
    /// F2 will apply` rows give no indication of what they each change), the terminal's current
    /// key (`MoonHotkeyInput`, read-only), the core's incoming key (`MoonKbd`), and the verdict.
    fn core_pull_row(&self, row: &PullRow, cx: &Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        let id = format!("core-pull-{}", slot_id(row.slot));
        let (verdict_text, verdict_color): (String, u32) = match row.verdict {
            PullVerdict::Empty => (t!("hotkeys.pull.verdict.empty").to_string(), p.text_muted),
            PullVerdict::Unsupported => {
                (t!("hotkeys.pull.verdict.unsupported").to_string(), p.amber)
            }
            PullVerdict::Unchanged => (
                t!("hotkeys.pull.verdict.unchanged").to_string(),
                p.text_muted,
            ),
            PullVerdict::WillApply => (
                t!("hotkeys.pull.verdict.will_apply").to_string(),
                p.green_text,
            ),
            PullVerdict::Conflict => (t!("hotkeys.pull.verdict.conflict").to_string(), p.red_text),
        };

        h_flex()
            .w_full()
            .min_h(design::fit_h_px(cx, 24.0, 12.0, 6.0))
            .gap(design::ui_px(cx, 10.0))
            .items_center()
            .child(
                div()
                    .flex_none()
                    .w(design::ui_px(cx, 96.0))
                    .text_size(design::t_caption(cx))
                    .text_color(rgba_from(p.text, 1.0))
                    .child(slot_label(row.slot)),
            )
            .child(
                MoonHotkeyInput::new(format!("{id}-current"))
                    .value(parse_hotkey(&row.current))
                    .placeholder(t!("hotkeys.unassigned").to_string())
                    .disabled(true)
                    .compact()
                    .width(140.0),
            )
            .child(
                MoonText::new("->")
                    .uppercase(false)
                    .mono(true)
                    .font_size(11.0)
                    .line_height(14.0)
                    .color(p.text_muted)
                    .render(),
            )
            .child(
                MoonKbd::new(shortcut::display(row.core_decoded))
                    .size(MoonKbdSize::Compact)
                    .outline(matches!(
                        row.verdict,
                        PullVerdict::Empty | PullVerdict::Unsupported
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(design::t_caption(cx))
                    .text_color(rgba_from(verdict_color, 1.0))
                    .child(verdict_text),
            )
            .into_any_element()
    }

    /// The "pull layout from core" button and, once a layout has arrived, its preview diff.
    /// Placed after the ManualStrategy rows and gated on nothing else: it is always visible on
    /// this sub-tab, which is what lets a resolved core's Live/Stale/Awaiting state stay legible
    /// without the user having to click anything first.
    fn core_pull_section(&self, hotkeys: &HotkeysConfig, cx: &Context<Self>) -> Vec<AnyElement> {
        let p = MoonPalette::active(cx);
        let mut out: Vec<AnyElement> = vec![
            div()
                .w_full()
                .h(design::ui_px(cx, 1.0))
                .bg(rgba_from(p.border, 1.0))
                .into_any_element(),
            MoonText::new(t!("hotkeys.pull.title").to_string())
                .uppercase(false)
                .mono(true)
                .font_size(11.0)
                .line_height(14.0)
                .color(p.text)
                .render()
                .into_any_element(),
        ];

        let Some(core) = self.core_pull_target(cx) else {
            out.push(self.pull_hint(t!("hotkeys.pull.no_core").to_string(), &p, cx));
            return out;
        };

        let b = self.backend.read(cx);
        let core_data = b.session.store().core(core);
        let state = core_data.map(|d| d.core_config_state());
        let manual = core_data.and_then(|d| d.core_config.as_ref()).map(|c| {
            (
                c.manual.core_hotkeys.clone(),
                c.manual.strat_buttons.hot_keys,
            )
        });

        let pending = self.core_pull.is_some_and(|(pending_core, baseline)| {
            pending_core == core
                && self
                    .backend
                    .read(cx)
                    .session
                    .store()
                    .core(core)
                    .map(|d| d.core_config_recv_rev)
                    == Some(baseline)
        });

        let freshness = match state {
            Some(CoreConfigState::Live) => {
                Some((t!("hotkeys.pull.live").to_string(), p.green_text))
            }
            Some(CoreConfigState::Stale) => Some((t!("hotkeys.pull.stale").to_string(), p.amber)),
            _ => None,
        };

        let mut header = h_flex()
            .w_full()
            .items_center()
            .gap(design::ui_px(cx, 10.0))
            .child(
                MoonButton::new("hotkeys-pull-request")
                    .outline()
                    .small()
                    .width(180.0)
                    .loading(pending)
                    .label(t!("hotkeys.pull.button").to_string())
                    .on_click(cx.listener(move |this, _, _, cx| this.request_core_pull(core, cx))),
            );
        if let Some((text, color)) = freshness {
            header = header.child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgba_from(color, 1.0))
                    .child(text),
            );
        }
        out.push(header.into_any_element());

        if pending {
            out.push(self.pull_hint(t!("hotkeys.pull.pending").to_string(), &p, cx));
            return out;
        }

        let Some((layout, manual_strategy_keys)) = manual else {
            out.push(self.pull_hint(t!("hotkeys.pull.empty").to_string(), &p, cx));
            return out;
        };

        let rows = preview_core_hotkeys(hotkeys, &layout, &manual_strategy_keys);
        let any_will_apply = rows.iter().any(|r| r.verdict == PullVerdict::WillApply);
        for row in &rows {
            out.push(self.core_pull_row(row, cx));
        }
        out.push(
            h_flex()
                .w_full()
                .gap(design::ui_px(cx, 8.0))
                .child(
                    MoonButton::new("hotkeys-pull-confirm")
                        .primary()
                        .small()
                        .width(130.0)
                        .disabled(!any_will_apply)
                        .label(t!("hotkeys.pull.confirm").to_string())
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.confirm_core_pull(core, cx)),
                        ),
                )
                .child(
                    MoonButton::new("hotkeys-pull-cancel")
                        .outline()
                        .small()
                        .width(110.0)
                        .label(t!("hotkeys.pull.cancel").to_string())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.core_pull = None;
                            cx.notify();
                        })),
                )
                .into_any_element(),
        );
        out
    }

    /// Small muted status line shared by the "no core" / "empty" / "pending" states.
    fn pull_hint(&self, text: String, p: &MoonPalette, cx: &Context<Self>) -> AnyElement {
        div()
            .text_size(design::t_caption(cx))
            .text_color(rgba_from(p.text_muted, 1.0))
            .child(text)
            .into_any_element()
    }
}
