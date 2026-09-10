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
    MoonTabStrip, MoonTag, MoonText, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::clash::{Clash, Clashes, Severity};
use super::meta::{self, Origin, SlotMeta, key_slot_meta, mouse_slot_meta};
use super::pull::{PullRow, PullVerdict, apply_core_hotkeys, preview_core_hotkeys};
use super::pull_gestures::{self, GesturePullRow, apply_core_gestures, preview_core_gestures};
use super::{
    HotkeyGroup, HotkeySlot, MouseSlot, MoveKindSlot, all_mouse_slots, mouse_slot_id,
    mouse_slot_value, mouse_slot_wip, move_kind_slot_id, move_kind_slot_value, parse_hotkey,
    set_mouse_slot_value, set_mouse_slot_verbatim, set_move_kind_slot_value, set_slot_value,
    short_move_twin, slot_id, slot_label, slot_value,
};
use crate::design;
use crate::settings::SettingsView;

/// Logical width reserved for every hotkey row title.
const ROW_TITLE_WIDTH: f32 = 160.0;

/// Maximum readable width of a hotkey row description before its editor column begins.
const ROW_DESCRIPTION_MAX_WIDTH: f32 = 640.0;

/// Logical width reserved for the marks that say where a binding acts and where its binding came
/// from.
///
/// A column of its own rather than an inline pair, and reserved on the rows that carry no marks
/// too: the marks are read DOWN the page ("which of these is mine to keep"), which only works while
/// they start at the same x on every row.
const ROW_MARKS_WIDTH: f32 = 150.0;

/// Width of the surface half inside that column.
///
/// Fixed so the provenance tag beside it starts at the same x on every row — the tag is what the
/// page is read down, and a tag that moves with the length of the words before it is not a column.
const ROW_SCOPE_WIDTH: f32 = 104.0;

/// Wraps one trailing control so it keeps its own width instead of stretching into the row.
fn control_cell(child: impl IntoElement, cx: &App) -> gpui::Div {
    sized_cell(child, ROW_CONTROL_WIDTH, cx)
}

/// The same, at a stated width.
fn sized_cell(child: impl IntoElement, width: f32, cx: &App) -> gpui::Div {
    div().flex_none().w(design::ui_px(cx, width)).child(child)
}

/// One line of muted body text, the style this tab's descriptions, hints and marks all share.
fn muted_line(text: String, p: &MoonPalette) -> impl IntoElement {
    MoonText::new(text)
        .uppercase(false)
        .mono(false)
        .wrap()
        .line_height(12.0)
        .color(p.text_muted)
        .render()
}

/// The rendered width every row editor shares — the hotkey field and both dropdowns.
///
/// It has to be applied through `trigger_width` (rendered pixels) rather than
/// `trigger_width_scaled`, because the two components scale differently: `MoonHotkeyInput::width`
/// goes through the UI scale, while a scaled trigger width goes through the FONT scale. Left to
/// their own defaults they agree only at one setting of the font slider and drift apart at every
/// other, which is exactly how a field and the dropdown beside it ended up different widths.
const ROW_EDITOR_WIDTH: f32 = 176.0;

/// Extra width the "move kind" cell gets over the gesture cell beside it.
///
/// Its labels are phrases rather than a keystroke — "Параллельно к курсору" against "Alt+Middle" —
/// so the same width truncated them under the caret.
const ROW_KIND_EXTRA: f32 = 56.0;

/// Width of every trailing control cell.
///
/// Each control sits in a `flex_none` box of this width rather than straight in the row. Without
/// the box a trigger grows into whatever space is left, which is how one dropdown ended up twice
/// the width of the row above it with the description running underneath it.
const ROW_CONTROL_WIDTH: f32 = 184.0;

impl SettingsView {
    /// Builds the Settings Hotkeys tab, including its lifted-contrast group strip.
    ///
    /// The strip needs `window` because it is rendered through a lifted palette rather than the
    /// active one: MoonUI keys an inactive tab label off `text_muted`, which sits under the body
    /// contrast floor in both stock themes, and `render_with_theme` is the only way to hand it a
    /// different palette.
    ///
    /// Args:
    ///     window: Window that owns the strip's persistent overflow state.
    ///     cx: Settings context used to read the hotkey draft and build callbacks.
    ///
    /// Returns:
    ///     The complete Hotkeys tab content.
    pub(in crate::settings) fn hotkeys_tab(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                    .mono(false)
                    .font_size(11.0)
                    .line_height(14.0)
                    .color(p.text)
                    .render(),
            )
            .child(muted_line(t!("hotkeys.group.builtin_hint").to_string(), &p))
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
        let strip_h = design::tab_strip_h(cx);
        let items: Vec<MoonTabItem> = HotkeyGroup::ALL
            .iter()
            .map(|g| MoonTabItem::new(g.title()).selected(self.hotkeys_group == *g))
            .collect();
        let strip = MoonTabStrip::new("hotkeys-group-strip")
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
            });
        let strip = design::chrome_tab_strip(strip, p, window, cx);
        let switcher = div().w_full().h(strip_h).child(strip);

        let body = v_flex()
            .w_full()
            .gap(design::ui_px(cx, 3.0))
            .child(muted_line(self.hotkeys_group.hint(), &p))
            // Said once for the whole page instead of in every row's own description: the marks
            // repeat on every line, so their meaning does not have to.
            .child(muted_line(t!("hotkeys.marks_legend").to_string(), &p))
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
        // Built once for the whole group rather than per row: it is an index over every slot, and
        // asking it forty-six times to rebuild itself forty-six times would be the same answer at
        // forty-six times the price.
        let clashes = Clashes::build(&hotkeys);
        match group {
            HotkeyGroup::Presets => (0..ORDER_SIZE_KEYS)
                .map(|i| {
                    let title = format!("F{}", i + 1);
                    let desc = t!("hotkeys.order_size", n = i + 1).to_string();
                    self.hotkey_row(
                        title,
                        desc,
                        HotkeySlot::OrderSize(i),
                        &hotkeys,
                        &clashes,
                        cx,
                    )
                })
                .chain((0..SELL_PRESET_KEYS).map(|i| {
                    let title = format!("S{}", i + 1);
                    let desc = t!("hotkeys.sell_preset", n = i + 1).to_string();
                    self.hotkey_row(
                        title,
                        desc,
                        HotkeySlot::SellPreset(i),
                        &hotkeys,
                        &clashes,
                        cx,
                    )
                }))
                .collect(),
            HotkeyGroup::Trading => vec![
                self.hotkey_row(
                    t!("hotkeys.cancel_buy").to_string(),
                    t!("hotkeys.cancel_buy_hint").to_string(),
                    HotkeySlot::CancelBuy,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.hotkey_row(
                    t!("hotkeys.panic_sell").to_string(),
                    t!("hotkeys.panic_sell_hint").to_string(),
                    HotkeySlot::PanicSell,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.hotkey_row(
                    t!("hotkeys.panic_sell_one").to_string(),
                    t!("hotkeys.panic_sell_one_hint").to_string(),
                    HotkeySlot::PanicSellOne,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.hotkey_row(
                    t!("hotkeys.cancel_all_buys").to_string(),
                    t!("hotkeys.cancel_all_buys_hint").to_string(),
                    HotkeySlot::CancelAllBuys,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.hotkey_row(
                    t!("hotkeys.join_sells").to_string(),
                    t!("hotkeys.join_sells_hint").to_string(),
                    HotkeySlot::JoinSells,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.hotkey_row(
                    t!("hotkeys.new_long").to_string(),
                    t!("hotkeys.new_long_hint").to_string(),
                    HotkeySlot::NewLong,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.hotkey_row(
                    t!("hotkeys.new_short").to_string(),
                    t!("hotkeys.new_short_hint").to_string(),
                    HotkeySlot::NewShort,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.hotkey_row(
                    t!("hotkeys.split_order").to_string(),
                    t!("hotkeys.split_order_hint", n = SPLIT_ORDER_PARTS).to_string(),
                    HotkeySlot::SplitOrder,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.hotkey_row(
                    t!("hotkeys.split_order_x").to_string(),
                    t!("hotkeys.split_order_x_hint").to_string(),
                    HotkeySlot::SplitOrderX,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.split_parts_row(&hotkeys, cx),
                self.hotkey_row(
                    t!("hotkeys.sells_to_rect").to_string(),
                    t!("hotkeys.sells_to_rect_hint").to_string(),
                    HotkeySlot::SellsToRect,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
            ],
            HotkeyGroup::Chart => vec![
                self.hotkey_row(
                    t!("hotkeys.switch_charts").to_string(),
                    t!("hotkeys.switch_charts_hint").to_string(),
                    HotkeySlot::SwitchCharts,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.hotkey_row(
                    t!("hotkeys.scale_plus").to_string(),
                    t!("hotkeys.scale_plus_hint").to_string(),
                    HotkeySlot::ScalePlus,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.hotkey_row(
                    t!("hotkeys.scale_minus").to_string(),
                    t!("hotkeys.scale_minus_hint").to_string(),
                    HotkeySlot::ScaleMinus,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.hotkey_row(
                    t!("hotkeys.chart_shot").to_string(),
                    t!("hotkeys.chart_shot_hint").to_string(),
                    HotkeySlot::ChartShot,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
            ],
            HotkeyGroup::Draw => vec![
                self.hotkey_row(
                    t!("hotkeys.switch_figure").to_string(),
                    t!("hotkeys.switch_figure_hint").to_string(),
                    HotkeySlot::SwitchFigure,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.hotkey_row(
                    t!("hotkeys.draw_hline").to_string(),
                    t!("hotkeys.draw_hline_hint").to_string(),
                    HotkeySlot::DrawHline,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.hotkey_row(
                    t!("hotkeys.draw_segment").to_string(),
                    t!("hotkeys.draw_segment_hint").to_string(),
                    HotkeySlot::DrawSegment,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.hotkey_row(
                    t!("hotkeys.draw_triangle").to_string(),
                    t!("hotkeys.draw_triangle_hint").to_string(),
                    HotkeySlot::DrawTriangle,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.hotkey_row(
                    t!("hotkeys.draw_channel").to_string(),
                    t!("hotkeys.draw_channel_hint").to_string(),
                    HotkeySlot::DrawChannel,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.hotkey_row(
                    t!("hotkeys.fig_delete").to_string(),
                    t!("hotkeys.fig_delete_hint").to_string(),
                    HotkeySlot::FigDelete,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.hotkey_row(
                    t!("hotkeys.fig_alert").to_string(),
                    t!("hotkeys.fig_alert_hint").to_string(),
                    HotkeySlot::FigAlert,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.hotkey_row(
                    t!("hotkeys.fig_undo").to_string(),
                    t!("hotkeys.fig_undo_hint").to_string(),
                    HotkeySlot::FigUndo,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.mouse_row(
                    t!("hotkeys.mouse.fig_delete").to_string(),
                    t!("hotkeys.mouse.fig_delete_hint").to_string(),
                    MouseSlot::FigDelete,
                    None,
                    &hotkeys,
                    &clashes,
                    false,
                    cx,
                ),
            ],
            HotkeyGroup::OrderMove => vec![
                self.hotkey_row(
                    t!("hotkeys.shift_buy_up").to_string(),
                    t!("hotkeys.shift_buy_up_hint").to_string(),
                    HotkeySlot::ShiftBuyUp,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.hotkey_row(
                    t!("hotkeys.shift_buy_down").to_string(),
                    t!("hotkeys.shift_buy_down_hint").to_string(),
                    HotkeySlot::ShiftBuyDown,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.hotkey_row(
                    t!("hotkeys.shift_sell_up").to_string(),
                    t!("hotkeys.shift_sell_up_hint").to_string(),
                    HotkeySlot::ShiftSellUp,
                    &hotkeys,
                    &clashes,
                    cx,
                ),
                self.hotkey_row(
                    t!("hotkeys.shift_sell_down").to_string(),
                    t!("hotkeys.shift_sell_down_hint").to_string(),
                    HotkeySlot::ShiftSellDown,
                    &hotkeys,
                    &clashes,
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
                    &clashes,
                    false,
                    cx,
                ),
                self.mouse_row(
                    t!("hotkeys.mouse.short_set").to_string(),
                    t!("hotkeys.mouse.short_set_hint").to_string(),
                    MouseSlot::ShortSet,
                    None,
                    &hotkeys,
                    &clashes,
                    false,
                    cx,
                ),
                self.mouse_row(
                    t!("hotkeys.mouse.pending_long").to_string(),
                    t!("hotkeys.mouse.pending_long_hint").to_string(),
                    MouseSlot::PendingLong,
                    None,
                    &hotkeys,
                    &clashes,
                    false,
                    cx,
                ),
                self.mouse_row(
                    t!("hotkeys.mouse.pending_short").to_string(),
                    t!("hotkeys.mouse.pending_short_hint").to_string(),
                    MouseSlot::PendingShort,
                    None,
                    &hotkeys,
                    &clashes,
                    false,
                    cx,
                ),
                self.mouse_row(
                    t!("hotkeys.mouse.buy_move").to_string(),
                    t!("hotkeys.mouse.buy_move_hint").to_string(),
                    MouseSlot::BuyMove,
                    Some(MoveKindSlot::BuyMove),
                    &hotkeys,
                    &clashes,
                    false,
                    cx,
                ),
                self.mouse_row(
                    t!("hotkeys.mouse.sell_move").to_string(),
                    t!("hotkeys.mouse.sell_move_hint").to_string(),
                    MouseSlot::SellMove,
                    Some(MoveKindSlot::SellMove),
                    &hotkeys,
                    &clashes,
                    false,
                    cx,
                ),
                self.mouse_row(
                    t!("hotkeys.mouse.buy_move2").to_string(),
                    t!("hotkeys.mouse.buy_move2_hint").to_string(),
                    MouseSlot::BuyMove2,
                    Some(MoveKindSlot::BuyMove2),
                    &hotkeys,
                    &clashes,
                    false,
                    cx,
                ),
                self.mouse_row(
                    t!("hotkeys.mouse.sell_move2").to_string(),
                    t!("hotkeys.mouse.sell_move2_hint").to_string(),
                    MouseSlot::SellMove2,
                    Some(MoveKindSlot::SellMove2),
                    &hotkeys,
                    &clashes,
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
                    &clashes,
                    hotkeys.same_hotkeys_for_move,
                    cx,
                ),
                self.mouse_row(
                    t!("hotkeys.mouse.short_sell_move").to_string(),
                    t!("hotkeys.mouse.short_sell_move_hint").to_string(),
                    MouseSlot::ShortSellMove,
                    None,
                    &hotkeys,
                    &clashes,
                    hotkeys.same_hotkeys_for_move,
                    cx,
                ),
                self.mouse_row(
                    t!("hotkeys.mouse.short_buy_move2").to_string(),
                    t!("hotkeys.mouse.short_buy_move2_hint").to_string(),
                    MouseSlot::ShortBuyMove2,
                    None,
                    &hotkeys,
                    &clashes,
                    hotkeys.same_hotkeys_for_move,
                    cx,
                ),
                self.mouse_row(
                    t!("hotkeys.mouse.short_sell_move2").to_string(),
                    t!("hotkeys.mouse.short_sell_move2_hint").to_string(),
                    MouseSlot::ShortSellMove2,
                    None,
                    &hotkeys,
                    &clashes,
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
                        &clashes,
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
                    .mono(false)
                    .wrap()
                    .font_size(11.0)
                    .line_height(14.0)
                    .color(p.text_muted)
                    .render(),
            )
            .into_any_element()
    }

    /// The caption a row shows when something else answers its binding.
    ///
    /// Under the description rather than beside the editor: it is a sentence, it names other rows,
    /// and it has to be able to wrap. Red when one of the two never fires, amber when both do and
    /// the user simply ought to know.
    fn clash_line(&self, clash: &Clash, p: &MoonPalette) -> AnyElement {
        let color = match clash.severity {
            Severity::Shadowed => p.red_text,
            Severity::Shares => p.amber,
        };
        MoonText::new(clash.text.clone())
            .uppercase(false)
            .mono(false)
            .wrap()
            .line_height(12.0)
            .color(color)
            .render()
            .into_any_element()
    }

    /// The marks column: WHERE this binding acts, and what Moonbot has of it.
    ///
    /// Two facts, because a user asks two different questions of this page — "why did my key do
    /// nothing over there" and "what will pasting a Moonbot configuration overwrite" — and neither
    /// was answerable from the row before. `None` renders the column empty, keeping a row that owns
    /// no slot (the part count, the mirroring checkbox) aligned with the rows that do.
    ///
    /// Args:
    ///     marks: The slot's two facts, or `None` for a row that is not a slot.
    ///     p: Active palette, already read by every caller.
    ///     cx: Settings context used for scaled geometry and the zone setting.
    ///
    /// Returns:
    ///     The fixed-width marks column.
    fn slot_marks(
        &self,
        marks: Option<SlotMeta>,
        p: &MoonPalette,
        cx: &Context<Self>,
    ) -> gpui::Div {
        let column = div().flex_none().w(design::ui_px(cx, ROW_MARKS_WIDTH));
        let Some(marks) = marks else {
            return column;
        };
        // The surface a row's two-way set resolves to right now, rather than the disjunction: the
        // reader wants the answer for the terminal in front of them.
        let separate_zones = {
            let b = self.backend.read(cx);
            b.preview
                .as_ref()
                .unwrap_or(&b.config)
                .separate_control_zones
        };
        column.child(
            h_flex()
                .gap(design::ui_px(cx, 6.0))
                .items_center()
                // The surface as plain muted text in a fixed sub-column, not a second pill and not
                // a wrapping pair. Two pills of the same tone would say nothing about which of them
                // is the one that changes; and letting the two share one wrapping box made the
                // whole point of the column collapse — a long surface pushed the tag onto its own
                // line, so no two rows started their tag at the same x.
                .child(
                    div()
                        .flex_none()
                        .w(design::ui_px(cx, ROW_SCOPE_WIDTH))
                        .child(muted_line(marks.scope.resolved(separate_zones).label(), p)),
                )
                .child(
                    match marks.origin {
                        // Two tones, because the two answers matter equally at a glance: blue for
                        // a binding a paste or a pull will overwrite, green for one nothing can.
                        Origin::Shared => MoonTag::info(),
                        Origin::Local => MoonTag::positive(),
                    }
                    .mono(false)
                    // `.child` rather than `.label`: the latter uppercases, and the legend that
                    // explains these marks quotes them as written.
                    .child(marks.origin.label())
                    .render(),
                ),
        )
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
        clashes: &Clashes,
        cx: &Context<Self>,
    ) -> AnyElement {
        // Most rows title themselves with a localized phrase, but the preset slots title
        // themselves with their own IDENTITY -- `F3`, `S2` -- which is a value, and `core_pull_row`
        // pins that same string mono. Read it off the slot the row already carries rather than
        // asking every call site to declare it: the two that pass an identity are exactly the two
        // preset variants.
        let title_is_identity =
            matches!(slot, HotkeySlot::OrderSize(_) | HotkeySlot::SellPreset(_));
        let raw = slot_value(hotkeys, slot);
        let parsed = parse_hotkey(raw);
        let invalid = !raw.trim().is_empty() && parsed.is_none();

        let id = format!("hotkey-{}", slot_id(slot));
        let clash = clashes.key(hotkeys, slot);

        self.row_head(
            title.into(),
            desc.into(),
            Some(key_slot_meta(slot)),
            title_is_identity,
            clash.clone().into_iter().collect(),
            cx,
        )
        .child(control_cell(
            MoonHotkeyInput::new(id)
                .value(parsed)
                .placeholder(t!("hotkeys.unassigned").to_string())
                .recording_placeholder(t!("hotkeys.recording").to_string())
                .invalid(invalid)
                // No `.conflict()`: the component paints that frame AMBER and stamps an
                // unlocalized "conflict" badge inside the field, and amber is this page's
                // "both work" tone. The caption below the description carries the whole
                // answer, in the right colour and in words.
                .compact()
                .width(ROW_EDITOR_WIDTH)
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
            cx,
        ))
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
        clashes: &Clashes,
        disabled: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let current = mouse_slot_value(hotkeys, slot);
        let id = format!("mouse-{}", mouse_slot_id(slot));
        let backend = self.backend.clone();
        let wip = mouse_slot_wip(slot);
        // Every note this row prints, in one place under the description: the "not wired yet"
        // status used to sit in a column of its own on the far right, which put it a screen away
        // from the sentence it belongs beside.
        let mut notes: Vec<Clash> = Vec::new();
        if wip {
            notes.push(Clash {
                severity: Severity::Shares,
                text: t!("hotkeys.todo").to_string(),
            });
        }
        // A greyed short row follows its long twin, so a clash reported on it would name a binding
        // the row does not own.
        if !disabled && let Some(clash) = clashes.mouse(hotkeys, slot) {
            notes.push(clash);
        }
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

        // A greyed short row follows its long twin, so a clash reported on it would name a binding
        // the row does not own.
        // One line, with the trailing controls in a block of their own.
        //
        // The block is what makes the columns line up. Before it, a gesture row's first dropdown
        // started wherever the row above happened to leave it: rows carry a different NUMBER of
        // trailing controls — a kind dropdown here, a status note there — and a wider tail pushed
        // everything left of it. Every cell inside the block is reserved on every row, empty ones
        // included, so the block is one width and the columns hold.
        self.row_head(
            title.into(),
            desc.into(),
            Some(mouse_slot_meta(slot)),
            false,
            notes,
            cx,
        )
        .child(
            h_flex()
                .flex_none()
                .gap(design::ui_px(cx, 10.0))
                .items_center()
                .child(control_cell(
                    Self::row_dropdown(id, current.label(), cx)
                        .trigger_variant(if current == MouseGestureBinding::None {
                            MoonButtonVariant::Neutral
                        } else {
                            MoonButtonVariant::Blue
                        })
                        .menu_width_scaled(228.0)
                        .disabled(disabled)
                        .items(items),
                    cx,
                ))
                .child(match kind_slot {
                    Some(kind_slot) => sized_cell(
                        self.move_kind_dropdown(kind_slot, hotkeys, disabled, cx),
                        ROW_CONTROL_WIDTH + ROW_KIND_EXTRA,
                        cx,
                    ),
                    None => div()
                        .flex_none()
                        .w(design::ui_px(cx, ROW_CONTROL_WIDTH + ROW_KIND_EXTRA)),
                }),
        )
        .into_any_element()
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
        cx: &App,
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
            cx,
        )
        // The trigger shows the chosen kind, so the menu carries the name of the setting — the
        // "Move kind" column heading Moonbot puts above the same list.
        .header(18.0, |_, cx| {
            let p = MoonPalette::active(cx);
            MoonText::new(t!("hotkeys.move_kind.title").to_string())
                .uppercase(false)
                .mono(false)
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
    ///     marks: The slot's two facts, or `None` for a row that owns no slot.
    ///     mono_title: Whether the title is an identity like `F3` rather than a phrase.
    ///     notes: Lines printed under the description — a clash, a "not wired yet" — in order.
    ///     cx: Settings context used for palette and scaled layout.
    ///
    /// Returns:
    ///     The row prefix to which callers append one or two controls.
    fn row_head(
        &self,
        title: String,
        desc: String,
        marks: Option<SlotMeta>,
        mono_title: bool,
        notes: Vec<Clash>,
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
                            .mono(mono_title)
                            .wrap()
                            .font_size(11.0)
                            .line_height(14.0)
                            .color(p.text)
                            .render(),
                    ),
            )
            .child(self.slot_marks(marks, &p, cx))
            .child(
                // Match title sizing, use muted text, and wrap within the window.
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(design::ui_px(cx, 2.0))
                    .max_w(design::ui_px(cx, ROW_DESCRIPTION_MAX_WIDTH))
                    .child(
                        MoonText::new(desc)
                            .uppercase(false)
                            .mono(false)
                            .wrap()
                            .font_size(11.0)
                            .line_height(14.0)
                            .color(p.text_muted)
                            .render(),
                    )
                    .children(notes.iter().map(|note| self.clash_line(note, &p))),
            )
    }

    /// Builds a row's trailing dropdown with the trigger geometry shared by this tab.
    fn row_dropdown(id: String, label: impl Into<SharedString>, cx: &App) -> MoonDropdown {
        MoonDropdown::new(SharedString::from(id))
            .label(label)
            .trigger_caret(true)
            .trigger_size(MoonButtonSize::Micro)
            .trigger_width(design::ui_value(cx, ROW_EDITOR_WIDTH))
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
            Some(meta::SPLIT_PARTS),
            false,
            Vec::new(),
            cx,
        )
        .child(control_cell(
            Self::row_dropdown("hotkey-split-parts".into(), current.to_string(), cx)
                .trigger_variant(MoonButtonVariant::Blue)
                .menu_width_scaled(120.0)
                .items(items),
            cx,
        ))
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
            .child(self.slot_marks(Some(meta::SAME_FOR_MOVE), &MoonPalette::active(cx), cx))
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
                                    // Mirrors only on the way ON, which is what Moonbot's own
                                    // dialog does. The pull re-aims in both directions, because
                                    // there a short row can go live with a value nothing wrote.
                                    for slot in all_mouse_slots() {
                                        if let Some(twin) = short_move_twin(slot) {
                                            let long = mouse_slot_value(&p.hotkeys, slot);
                                            set_mouse_slot_verbatim(&mut p.hotkeys, twin, long);
                                        }
                                    }
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
    /// render) and writes `hotkeys.toml` immediately.
    ///
    /// Rebuilding cannot disagree with what was drawn over the DRAFT — same computation, same
    /// values. It can over the CORE's block: that is re-read from the store here, so a
    /// configuration arriving between the frame the user read and the click they made is what gets
    /// applied. The window is an eye-to-click one and the alternative — applying rows captured at
    /// render time — would write a layout the core has already replaced, which is worse.
    ///
    /// This bypasses the tab's usual preview/Save cycle on purpose: `HotkeysConfig::save()` is a
    /// separate file with its own saver, no `config_dirty` involved. Writing both
    /// `config.hotkeys` and `preview.hotkeys` keeps them in sync so a LATER "Settings > Save"
    /// click (which starts from `preview`) cannot silently roll the pull back to what the draft
    /// looked like when the window opened.
    fn confirm_core_pull(&mut self, core: CoreId, cx: &mut Context<Self>) {
        let outcome = self.backend.update(cx, |b, bcx| {
            let (layout, manual_strategy_keys, gestures) = b
                .session
                .store()
                .core(core)
                .and_then(|d| d.core_config.as_ref())
                .map(|c| {
                    (
                        c.manual.core_hotkeys.clone(),
                        c.manual.strat_buttons.hot_keys,
                        c.gestures,
                    )
                })?;
            let base = b
                .preview
                .as_ref()
                .map(|p| p.hotkeys.clone())
                .unwrap_or_else(|| b.config.hotkeys.clone());
            let rows = preview_core_hotkeys(&base, &layout, &manual_strategy_keys);
            let gesture_rows = preview_core_gestures(&base, &gestures);
            let mut hotkeys = base;
            let mut changed = apply_core_hotkeys(&mut hotkeys, &rows);
            changed |= apply_core_gestures(&mut hotkeys, &gesture_rows);
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
                    // A slot identity (F1, S1) is a value, and the key columns it is compared
                    // against on this same row -- MoonHotkeyInput, the arrow, MoonKbd -- all stay
                    // mono. Without this pin it would be the only column of the comparison that
                    // changed face.
                    .font_family(design::mono())
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

    /// One gesture row of the pull preview.
    ///
    /// Text on both sides rather than the key row's `MoonHotkeyInput` and `MoonKbd`: a gesture is a
    /// phrase ("Ctrl+Left (CTRL_Click)"), not a keystroke, and those two controls draw keystrokes.
    fn core_pull_gesture_row(&self, row: &GesturePullRow, cx: &Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        let (verdict_text, verdict_color): (String, u32) = match row.verdict {
            // Not produced for gestures — a zero ordinal is a value there — but the verdict enum
            // is shared with the keyboard half, so the arm has to exist. It borrows that half's
            // wording rather than carrying three translations nothing can render.
            PullVerdict::Empty => (t!("hotkeys.pull.verdict.empty").to_string(), p.text_muted),
            PullVerdict::Unsupported => (
                t!("hotkeys.pull.verdict.gesture_unsupported").to_string(),
                p.amber,
            ),
            PullVerdict::Unchanged => (
                t!("hotkeys.pull.verdict.unchanged").to_string(),
                p.text_muted,
            ),
            PullVerdict::WillApply => (
                t!("hotkeys.pull.verdict.will_apply").to_string(),
                p.green_text,
            ),
            // Never produced for gestures — see `pull_gestures::preview_core_gestures`.
            PullVerdict::Conflict => (t!("hotkeys.pull.verdict.conflict").to_string(), p.red_text),
        };
        let dim = matches!(
            row.verdict,
            PullVerdict::Empty | PullVerdict::Unsupported | PullVerdict::Unchanged
        );

        h_flex()
            .w_full()
            .min_h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
            .gap(design::ui_px(cx, 10.0))
            .items_center()
            .child(
                div()
                    .flex_none()
                    .w(design::ui_px(cx, 200.0))
                    .text_size(design::t_caption(cx))
                    .text_color(rgba_from(p.text, 1.0))
                    .child(pull_gestures::target_label(row.target)),
            )
            .child(
                div()
                    .flex_none()
                    .w(design::ui_px(cx, 170.0))
                    .text_size(design::t_caption(cx))
                    .text_color(rgba_from(p.text_muted, 1.0))
                    .child(row.current.clone()),
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
                div()
                    .flex_none()
                    .w(design::ui_px(cx, 170.0))
                    .text_size(design::t_caption(cx))
                    .text_color(rgba_from(if dim { p.text_muted } else { p.text }, 1.0))
                    .child(row.incoming.clone()),
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
                .mono(false)
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
                c.gestures,
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

        let Some((layout, manual_strategy_keys, gestures)) = manual else {
            out.push(self.pull_hint(t!("hotkeys.pull.empty").to_string(), &p, cx));
            return out;
        };

        let rows = preview_core_hotkeys(hotkeys, &layout, &manual_strategy_keys);
        let gesture_rows = preview_core_gestures(hotkeys, &gestures);
        let any_will_apply = rows
            .iter()
            .map(|r| r.verdict)
            .chain(gesture_rows.iter().map(|r| r.verdict))
            .any(|v| v == PullVerdict::WillApply);
        for row in &rows {
            out.push(self.core_pull_row(row, cx));
        }
        out.push(
            MoonText::new(t!("hotkeys.pull.gestures").to_string())
                .uppercase(false)
                .mono(false)
                .font_size(11.0)
                .line_height(14.0)
                .color(p.text)
                .render()
                .into_any_element(),
        );
        for row in &gesture_rows {
            out.push(self.core_pull_gesture_row(row, cx));
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
