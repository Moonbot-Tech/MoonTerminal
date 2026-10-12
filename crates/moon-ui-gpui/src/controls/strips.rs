//! Order-size (F1-F6) and fixed-sell (S1-S6) preset strips with native click, double-click,
//! Ctrl+wheel, tooltip, and inline-edit behavior.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::feed::ClientSettingsEdit;
use moon_core::util::fmt::compact_order_size;
use moon_ui::{
    MoonAccent, MoonButton, MoonButtonIconSlot, MoonButtonVariant, MoonDisclosureDirection,
    MoonInput, MoonInputState, MoonMenuItem, MoonPopover, MoonPopoverPlacement, MoonPopupMenu,
    MoonSegmentDensity, MoonSegmentItem, MoonSegmentSelection, MoonSegmentedControl, MoonSize,
    h_flex,
};
use rust_i18n::t;

use super::fmt::{fmt_sell_pct, scroll_dy, wheel_step};
use crate::{Backend, design};

/// Base floor for a preset cell's fitted width.
const MIN_CELL_W: f32 = 34.0;
/// Base ceiling for a preset cell's fitted width.
///
/// An anomalous value cannot stretch the toolbar across the window; MoonUI truncates only the
/// visible label while the inline editor still exposes the complete value.
const MAX_CELL_W: f32 = 104.0;
/// Cell density of both preset strips: the mockup's tight pills (label plus 8px a side, no
/// hotkey gap). Fitting and rendering must read the same value.
const STRIP_DENSITY: MoonSegmentDensity = MoonSegmentDensity::Compact;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Action selected by an order-size preset click.
pub(super) enum SizeClickAction {
    Select(usize),
    Edit(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Action selected by a fixed-sell preset click.
pub(super) enum SellClickAction {
    SelectFixed(usize),
    EngageMain,
    Edit(usize),
}

/// Resolve an order-size click without coupling click-count semantics to backend mutation.
///
/// Args:
///     index: Zero-based preset index.
///     click_count: Native click count reported by GPUI.
///
/// Returns:
///     Selection for a single click or inline editing for a double-or-later click.
fn size_click_action(index: usize, click_count: usize) -> SizeClickAction {
    if click_count >= 2 {
        SizeClickAction::Edit(index)
    } else {
        SizeClickAction::Select(index)
    }
}

/// Resolve a fixed-sell click without coupling click-count semantics to backend mutation.
///
/// Args:
///     index: Zero-based preset index.
///     selected_slot: Currently engaged one-based fixed-sell slot.
///     click_count: Native click count reported by GPUI.
///
/// Returns:
///     Inline editing for a double-or-later click, main take profit when the active slot is clicked,
///     or the one-based fixed-sell slot to engage.
fn sell_click_action(
    index: usize,
    selected_slot: Option<usize>,
    click_count: usize,
) -> SellClickAction {
    if click_count >= 2 {
        SellClickAction::Edit(index)
    } else if selected_slot == Some(index + 1) {
        SellClickAction::EngageMain
    } else {
        SellClickAction::SelectFixed(index + 1)
    }
}

/// Resolve a click on the sell control in `form`.
///
/// The full strip resolves as [`sell_click_action`]. The collapsed form draws only the engaged
/// preset, so a single click there would only clear it and take the cell — and with it the
/// double-click's editor — off the row; it does nothing, and changing the selection is the caret
/// menu's job. A double click edits that preset, the slot kept, as on the full strip.
///
/// Returns:
///     The action, or `None` for a click that must not act.
pub(super) fn sell_click_action_in(
    form: StripForm,
    index: usize,
    selected_slot: Option<usize>,
    click_count: usize,
) -> Option<SellClickAction> {
    match form {
        StripForm::Full => Some(sell_click_action(index, selected_slot, click_count)),
        StripForm::Collapsed => (click_count >= 2).then_some(SellClickAction::Edit(index)),
    }
}

/// Return the direction of one preset step, or `None` when the gesture must not edit a value.
///
/// Ctrl prevents an ordinary toolbar scroll from rewriting trading parameters. A non-zero
/// vertical delta excludes horizontal trackpad gestures.
///
/// Args:
///     modifiers: Keyboard modifiers captured with the wheel event.
///     delta: Two-dimensional wheel or trackpad delta.
///
/// Returns:
///     `Some(true)` for an upward step, `Some(false)` for a downward step, or `None`.
fn wheel_step_dir(modifiers: Modifiers, delta: ScrollDelta) -> Option<bool> {
    if !modifiers.control {
        return None;
    }
    let dy = scroll_dy(delta);
    if dy == 0.0 {
        return None;
    }
    Some(dy > 0.0)
}

/// Six preset items fitted by MoonUI before the toolbar computes its row budget.
///
/// Rendering and budgeting consume the same resolved items, so Terminal does not mirror segment
/// padding, gaps, selected text weight, truncation, or pixel rounding.
pub(crate) struct FittedCells {
    items: [MoonSegmentItem; 6],
    labels: [String; 6],
    /// The cell the collapsed sell form draws while no S slot is engaged; empty for sizes.
    main: MoonSegmentItem,
}

impl FittedCells {
    /// Fit all preset labels through MoonUI at the current theme and font size.
    ///
    /// Args:
    ///     cx: Application context used by MoonUI for theme-aware measurement.
    ///     labels: The six complete preset values.
    ///
    /// Returns:
    ///     Six fitted items with stable, pre-render resolved widths.
    pub(super) fn fit(cx: &App, labels: [String; 6]) -> Self {
        Self {
            items: labels.clone().map(|label| fitted_cell(cx, label)),
            labels,
            main: MoonSegmentItem::new("", "").width(0.0),
        }
    }

    /// Also fit the main take-profit value the collapsed sell form shows with no slot engaged.
    pub(super) fn with_main(mut self, cx: &App, label: impl Into<SharedString>) -> Self {
        self.main = fitted_cell(cx, label);
        self
    }

    /// The six fitted cell widths, in rendered pixels.
    pub(super) fn widths(&self) -> [f32; 6] {
        std::array::from_fn(|index| self.items[index].resolved_width())
    }

    /// The complete label of preset `index`.
    pub(super) fn label(&self, index: usize) -> &str {
        self.labels.get(index).map_or("", String::as_str)
    }

    /// The fitted main cell, unselected.
    pub(super) fn main_item(&self) -> MoonSegmentItem {
        self.main.clone()
    }
}

/// Fit one hotkey-less preset cell at the strips' density and width bounds.
///
/// The width is rounded up to a whole pixel: a short label sits at the font-scaled floor, which is
/// fractional at most scales, and the layout snaps each cell to the pixel grid, so twelve
/// fractional cells draw wider than the toolbar budget counted them.
///
/// Args:
///     cx: Application context used for theme-aware text measurement.
///     label: Complete preset value.
///
/// Returns:
///     The fitted cell.
fn fitted_cell(cx: &App, label: impl Into<SharedString>) -> MoonSegmentItem {
    let item = MoonSegmentItem::new("", label)
        .density(STRIP_DENSITY)
        .fit_width(cx, MIN_CELL_W, MAX_CELL_W);
    let width = item.resolved_width().ceil();
    item.width(width)
}

/// Format six complete group-owned preset values.
///
/// Args:
///     values: Complete values for the group.
///     fmt: Value formatter shared by all six cells.
///
/// Returns:
///     Six display labels.
fn labels(values: [f64; 6], fmt: impl Fn(f64) -> String) -> [String; 6] {
    values.map(fmt)
}

/// Return the group's complete order-size labels.
///
/// Args:
///     values: Complete group-local USD-equivalent order-size values.
///
/// Returns:
///     Six formatted order-size labels.
pub(super) fn size_labels(values: [f64; 6]) -> [String; 6] {
    labels(values, compact_order_size)
}

/// Return the group's complete fixed-sell percentage labels.
///
/// Args:
///     pcts: Complete group-local fixed-sell percentages.
///
/// Returns:
///     Six formatted percentage labels.
pub(super) fn sell_labels(pcts: [f64; 6]) -> [String; 6] {
    labels(pcts, |pct| format!("{}%", fmt_sell_pct(pct)))
}

/// Which of a preset's two forms the toolbar draws.
///
/// The collapsed form is the selected cell alone — rendered by the same segmented-control path
/// with one item, so the double-click editor and the Ctrl+wheel step reach exactly the handlers the
/// full strip uses — plus a small caret beside it that opens the full list. Not a dropdown: a
/// `MoonPopover` opens on the first mouse-down and stops it there, which would swallow the
/// click-count gesture the cell needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StripForm {
    Full,
    Collapsed,
}

/// The rendered cell geometry of one preset strip, pure so the collapse arithmetic is testable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct StripGeometry {
    /// Fitted width of each of the six cells.
    pub cells: [f32; 6],
    /// Fitted width of the cell the collapsed sell form draws while no S slot is engaged.
    pub main: f32,
    /// The joined strip's hairline frame (both sides).
    pub frame: f32,
    /// The caret button beside a collapsed cell.
    pub caret: f32,
    /// Gap between a collapsed cell and its caret.
    pub caret_gap: f32,
}

impl StripGeometry {
    /// Width of the full six-cell strip.
    pub(super) fn full(&self) -> f32 {
        self.cells.iter().sum::<f32>() + self.frame
    }

    /// Width the collapsed form draws its one cell at: the widest of the six cells and the main
    /// cell, so picking another preset never changes the row's width or its level.
    pub(super) fn collapsed_cell(&self) -> f32 {
        self.cells.iter().copied().fold(self.main, f32::max)
    }

    /// Width of the collapsed form: its one cell at [`Self::collapsed_cell`], its frame, and the
    /// caret beside it.
    pub(super) fn collapsed(&self) -> f32 {
        self.collapsed_cell() + self.frame + self.caret_gap + self.caret
    }
}

/// The zero-based S cell the collapsed sell form shows: the engaged slot, or `None` while the main
/// take profit is engaged (or a manual strategy owns the sell target), which shows the main value.
pub(super) fn collapsed_sell_cell(sel_slot: Option<usize>) -> Option<usize> {
    sel_slot
        .and_then(|slot| slot.checked_sub(1))
        .filter(|&ix| ix < 6)
}

/// Label of the collapsed cell: the shown preset's own label, or `main` when none is shown.
#[cfg(test)]
pub(super) fn collapsed_label(labels: &[String; 6], shown: Option<usize>, main: &str) -> String {
    shown
        .and_then(|index| labels.get(index).cloned())
        .unwrap_or_else(|| main.to_string())
}

/// What picking S row `index` from the collapsed form's menu does: exactly what a single click on
/// that cell of the full strip does, through the same resolver — the engaged row restores the main
/// take profit.
pub(super) fn sell_menu_action(index: usize, sel_slot: Option<usize>) -> SellClickAction {
    sell_click_action(index, sel_slot, 1)
}

/// Pick S row `index` from a menu, the caret's or the overflow's: the same resolver and the same
/// edit a single click on that cell of the full strip makes.
pub(crate) fn sell_menu_pick(
    backend: &Entity<Backend>,
    group: &str,
    index: usize,
    sel_slot: Option<usize>,
    app: &mut App,
) {
    backend.update(app, |backend, cx| {
        apply_sell_action(backend, group, sell_menu_action(index, sel_slot));
        cx.notify();
    });
}

/// Pick B row `index` from a menu, the caret's or the overflow's: selects and persists that preset.
pub(crate) fn size_menu_pick(backend: &Entity<Backend>, group: &str, index: usize, app: &mut App) {
    backend.update(app, |backend, cx| {
        backend.set_order_size_sel(group, index);
        backend.order_size_rev = backend.order_size_rev.wrapping_add(1);
        cx.notify();
    });
}

/// Gap between a collapsed cell and its caret, before the UI scale.
const CARET_GAP: f32 = 2.0;

/// The one size click handler both forms install. `indices` maps a control cell to its preset.
fn size_on_click(
    backend: Entity<Backend>,
    group: Option<String>,
    indices: Vec<usize>,
) -> impl Fn(usize, &ClickEvent, &mut Window, &mut App) + 'static {
    move |cell, event, _, app| {
        let (Some(group), Some(&index)) = (group.as_deref(), indices.get(cell)) else {
            return;
        };
        backend.update(app, |backend, cx| {
            match size_click_action(index, event.click_count()) {
                SizeClickAction::Select(index) => {
                    backend.set_order_size_sel(group, index);
                }
                SizeClickAction::Edit(index) => {
                    backend.order_size_edit_req = Some((group.to_string(), index));
                }
            }
            backend.order_size_rev = backend.order_size_rev.wrapping_add(1);
            cx.notify();
        });
    }
}

/// The one size Ctrl+wheel handler both forms install. `indices` maps a control cell to its preset.
fn size_on_scroll(
    backend: Entity<Backend>,
    group: Option<String>,
    indices: Vec<usize>,
) -> impl Fn(usize, &ScrollWheelEvent, &mut Window, &mut App) + 'static {
    move |cell, event, _, app| {
        let Some(up) = wheel_step_dir(event.modifiers, event.delta) else {
            return;
        };
        let (Some(group), Some(&index)) = (group.as_deref(), indices.get(cell)) else {
            return;
        };
        backend.update(app, |backend, cx| {
            // Seed from the SAME resolution the write targets, not the group-local getter: a
            // relative Ctrl+wheel step must be computed against the value about to be
            // overwritten, or it silently replaces a core preset with a value derived from the
            // group's generation (goal A2 FIX-1).
            let (sizes, _) = backend.write_aligned_order_sizes(group);
            let current = sizes[index];
            let next = wheel_step(current, up, 1.0);
            if next != current {
                backend.set_order_size_value(group, index, next);
                backend.order_size_rev = backend.order_size_rev.wrapping_add(1);
                cx.notify();
            }
        });
    }
}

/// Apply one resolved sell action to the group's exits.
fn apply_sell_action(backend: &mut Backend, group: &str, action: SellClickAction) {
    match action {
        SellClickAction::Edit(index) => {
            backend.sell_edit_req = Some((group.to_string(), index));
        }
        SellClickAction::EngageMain => {
            backend.edit_group_exit(group, ClientSettingsEdit::EngageMainTakeProfit);
            backend.order_size_rev = backend.order_size_rev.wrapping_add(1);
        }
        SellClickAction::SelectFixed(slot) => {
            backend.edit_group_exit(group, ClientSettingsEdit::SelectFixedSellSlot(slot));
            backend.order_size_rev = backend.order_size_rev.wrapping_add(1);
        }
    }
}

/// The one sell click handler both forms install. `indices` maps a control cell to its S preset;
/// the collapsed main cell maps to none and so does nothing.
fn sell_on_click(
    backend: Entity<Backend>,
    group: Option<String>,
    sel_slot: Option<usize>,
    indices: Vec<usize>,
    form: StripForm,
) -> impl Fn(usize, &ClickEvent, &mut Window, &mut App) + 'static {
    move |cell, event, _, app| {
        let (Some(group), Some(&index)) = (group.as_deref(), indices.get(cell)) else {
            return;
        };
        let Some(action) = sell_click_action_in(form, index, sel_slot, event.click_count()) else {
            return;
        };
        backend.update(app, |backend, cx| {
            apply_sell_action(backend, group, action);
            cx.notify();
        });
    }
}

/// The one sell Ctrl+wheel handler both forms install.
fn sell_on_scroll(
    backend: Entity<Backend>,
    group: Option<String>,
    indices: Vec<usize>,
) -> impl Fn(usize, &ScrollWheelEvent, &mut Window, &mut App) + 'static {
    move |cell, event, _, app| {
        let Some(up) = wheel_step_dir(event.modifiers, event.delta) else {
            return;
        };
        let (Some(group), Some(&index)) = (group.as_deref(), indices.get(cell)) else {
            return;
        };
        backend.update(app, |backend, cx| {
            // Seed from the SAME resolution the write targets, not the group-local getter: a
            // relative Ctrl+wheel step must be computed against the S-slot value about to be
            // overwritten, not the group's generation while the write lands on the core's
            // (goal A2 FIX-2).
            let current = backend.write_aligned_group_exit(group).fixed_sell_pcts[index];
            let next = wheel_step(current, up, 0.5);
            if next != current {
                backend.edit_group_exit(
                    group,
                    ClientSettingsEdit::SetFixedSellPct {
                        slot: index + 1,
                        pct: next,
                    },
                );
                backend.order_size_rev = backend.order_size_rev.wrapping_add(1);
                cx.notify();
            }
        });
    }
}

/// Which preset strip a collapsed caret menu belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StripCaret {
    Size,
    Sell,
}

impl StripCaret {
    /// The shallowest toolbar level that draws this strip's caret: the level keeps it while its
    /// menu is open.
    pub(crate) fn min_level(self) -> crate::design::ToolbarLevel {
        crate::design::ToolbarLevel(match self {
            StripCaret::Sell => 1,
            StripCaret::Size => 4,
        })
    }

    /// The deepest toolbar level that draws this strip's caret: the next one folds the control
    /// into "More".
    pub(crate) fn max_level(self) -> crate::design::ToolbarLevel {
        use crate::design::ToolbarLevel;
        ToolbarLevel(match self {
            StripCaret::Sell => ToolbarLevel::TAKE_SELL_FOLD - 1,
            StripCaret::Size => ToolbarLevel::SIZE_FOLD - 1,
        })
    }

    /// Whether this caret's open menu has lost its trigger or its purpose: the strip stopped
    /// taking edits (`enabled` false — a lock, or a manual strategy owning the sell price), or the
    /// level the window fits (`fitting`) folded the control into "More". A wider window keeps it:
    /// the level holds at [`Self::min_level`], which always fits.
    pub(crate) fn menu_gone(self, fitting: crate::design::ToolbarLevel, enabled: bool) -> bool {
        !enabled || fitting > self.max_level()
    }
}

/// Report a caret menu's open state to its owner.
pub(crate) type CaretChange = std::rc::Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// What a strip acts through. `None` at [`size_strip`] / [`sell_strip`] draws the same geometry
/// with no handlers — the inert path layout tests render.
pub(crate) struct StripIo<'a> {
    /// Application state receiving edits.
    pub backend: Entity<Backend>,
    /// Shared inline-editor state.
    pub input: &'a Entity<MoonInputState>,
    /// Whether the collapsed form's caret menu is open.
    pub caret_open: bool,
    /// Report the caret menu's open state.
    pub on_caret: CaretChange,
}

/// The caret beside a collapsed cell: a popover listing all six presets, the shown one checked.
fn caret_menu(
    id: &'static str,
    items: Vec<MoonMenuItem>,
    enabled: bool,
    caret: Option<(bool, CaretChange)>,
    cx: &App,
) -> impl IntoElement {
    let (open, on_change) = match caret {
        Some((open, on_change)) => (open, Some(on_change)),
        None => (false, None),
    };
    MoonPopover::new(SharedString::from(format!("{id}-popover")))
        .placement(MoonPopoverPlacement::BottomStart)
        .fit_content()
        .close_on_content_click(true)
        .disabled(!enabled)
        // Controlled from `Shell`, so the toolbar level can keep this caret drawn while its menu
        // is open (`ToolbarWidths::level_at_least`).
        .open(open)
        .when_some(on_change, |popover, on_change| {
            popover.on_open_change(move |open, window, cx| on_change(open, window, cx))
        })
        .trigger(
            MoonButton::new(SharedString::from(format!("{id}-caret")))
                .variant(MoonButtonVariant::Ghost)
                .width(design::glyph_btn_w(cx))
                .disabled(!enabled)
                .leading_icon(MoonButtonIconSlot::caret(
                    MoonDisclosureDirection::DownUp,
                    false,
                ))
                .render(),
        )
        .content(
            MoonPopupMenu::new(SharedString::from(format!("{id}-menu")))
                .fit_width(120.0, 240.0)
                .size(MoonSize::Xs)
                .items(items)
                .render(),
        )
}

/// Build the order-size preset control in its `form`.
///
/// Single click selects and persists a preset, double click requests inline editing, and
/// Ctrl+wheel changes the value by its order of magnitude — on every cell either form draws. The
/// collapsed form's caret lists B1–B6.
///
/// Args:
///     cells: MoonUI-fitted preset cells used by both rendering and toolbar budgeting.
///     sel: Selected zero-based preset index.
///     edit_ix: Zero-based cell to replace with the inline editor.
///     io: Backend, editor state and caret state; `None` renders the control inert.
///     group: Window group that owns the local USD presets; `None` disables the control — used
///         when the displayed core and the write target disagree (goal A2 FIX-3), so a click,
///         double-click, or wheel step cannot mutate a source other than the one on screen.
///     unit: USDT-equivalent unit shown in tooltips.
///     form: Full strip or the collapsed selected cell with its caret.
///     cx: Application context used to size the caret.
///
/// Returns:
///     The configured control.
#[allow(clippy::too_many_arguments)]
pub(super) fn size_strip(
    cells: &FittedCells,
    sel: usize,
    edit_ix: Option<usize>,
    io: Option<StripIo>,
    group: Option<String>,
    unit: &str,
    form: StripForm,
    cx: &App,
) -> AnyElement {
    let interactive = group.is_some();
    let indices: Vec<usize> = match form {
        StripForm::Full => (0..6).collect(),
        StripForm::Collapsed => vec![sel.min(5)],
    };
    let collapsed_w = strip_geometry(cells, cx).collapsed_cell();
    let item = |index: usize| {
        let item = cells.items[index].clone();
        let item = match form {
            StripForm::Full => item,
            StripForm::Collapsed => item.width(collapsed_w),
        };
        item.selected(sel == index)
            .disabled(!interactive)
            .tooltip(t!("toolbar.size_hint", n = index + 1, unit = unit).to_string())
    };
    let mut segment = MoonSegmentedControl::new("toolbar-size-presets")
        .selection(MoonSegmentSelection::Filled)
        .density(STRIP_DENSITY)
        .accent(MoonAccent::Blue)
        .item_gap(0.0)
        .items(indices.iter().map(|&index| item(index)));
    if let Some(io) = io.as_ref() {
        segment = segment
            .on_click(size_on_click(
                io.backend.clone(),
                group.clone(),
                indices.clone(),
            ))
            .on_scroll(size_on_scroll(
                io.backend.clone(),
                group.clone(),
                indices.clone(),
            ));
        if let Some(cell) = edit_ix.and_then(|ix| indices.iter().position(|&index| index == ix)) {
            segment = segment.replace_item(
                cell,
                MoonInput::new("toolbar-size-edit")
                    .state(io.input)
                    .size(design::INPUT_SIZE),
            );
        }
    }
    if form == StripForm::Full {
        return segment.render().into_any_element();
    }
    let menu_items = (0..6)
        .map(|index| {
            let row = MoonMenuItem::with_key(
                format!("toolbar-size-row-{index}"),
                format!("B{}  {}", index + 1, cells.label(index)),
            )
            .selected(sel == index)
            .checked(sel == index);
            let (Some(io), Some(group)) = (io.as_ref(), group.clone()) else {
                return row;
            };
            let backend = io.backend.clone();
            row.on_click(move |_, _, app| size_menu_pick(&backend, &group, index, app))
        })
        .collect();
    h_flex()
        .flex_none()
        .items_center()
        .gap(design::ui_px(cx, CARET_GAP))
        .child(segment.render())
        .child(caret_menu(
            "toolbar-size",
            menu_items,
            interactive,
            io.map(|io| (io.caret_open, io.on_caret)),
            cx,
        ))
        .into_any_element()
}

/// Build the fixed-sell preset control in its `form`.
///
/// Single click engages a slot (or restores main TP when clicking the active slot), double click
/// requests inline editing, and Ctrl+wheel changes the group-owned percentage. The collapsed form
/// shows the engaged slot, or the main take-profit value unselected while no slot is engaged; its
/// caret lists S1–S6, where the engaged row restores the main take profit. Manual-strategy mode
/// disables everything because the toolbar passes `None`.
///
/// Args:
///     cells: MoonUI-fitted preset cells used by both rendering and toolbar budgeting.
///     sel_slot: Selected one-based fixed-sell slot.
///     edit_ix: Zero-based cell to replace with the inline editor.
///     io: Backend, editor state and caret state; `None` renders the control inert.
///     group: Interactive group; absence disables the control — also used when the displayed core
///         and the write target disagree (goal A2 FIX-3).
///     form: Full strip or the collapsed engaged cell with its caret.
///     cx: Application context used to size the caret.
///
/// Returns:
///     The configured control.
#[allow(clippy::too_many_arguments)]
pub(super) fn sell_strip(
    cells: &FittedCells,
    sel_slot: Option<usize>,
    edit_ix: Option<usize>,
    io: Option<StripIo>,
    group: Option<String>,
    form: StripForm,
    cx: &App,
) -> AnyElement {
    let interactive = group.is_some();
    let collapsed_w = strip_geometry(cells, cx).collapsed_cell();
    let item = |index: usize| {
        let item = cells.items[index].clone();
        let item = match form {
            StripForm::Full => item,
            StripForm::Collapsed => item.width(collapsed_w),
        };
        let mut item = item
            .selected(sel_slot == Some(index + 1))
            .disabled(!interactive);
        if interactive {
            item = item.tooltip(t!("toolbar.sell_hint", n = index + 1).to_string());
        }
        item
    };
    let (indices, items): (Vec<usize>, Vec<MoonSegmentItem>) = match form {
        StripForm::Full => ((0..6).collect(), (0..6).map(item).collect()),
        StripForm::Collapsed => match collapsed_sell_cell(sel_slot) {
            Some(index) => (vec![index], vec![item(index)]),
            // No slot engaged: the main take-profit value, unselected, bound to no preset.
            None => (
                Vec::new(),
                vec![cells.main_item().width(collapsed_w).disabled(!interactive)],
            ),
        },
    };
    let mut segment = MoonSegmentedControl::new("toolbar-sell-presets")
        .selection(MoonSegmentSelection::Filled)
        .density(STRIP_DENSITY)
        .accent(MoonAccent::Amber)
        .item_gap(0.0)
        .items(items);
    if let Some(io) = io.as_ref() {
        segment = segment
            .on_click(sell_on_click(
                io.backend.clone(),
                group.clone(),
                sel_slot,
                indices.clone(),
                form,
            ))
            .on_scroll(sell_on_scroll(
                io.backend.clone(),
                group.clone(),
                indices.clone(),
            ));
        if let Some(cell) = edit_ix
            .filter(|_| interactive)
            .and_then(|ix| indices.iter().position(|&index| index == ix))
        {
            segment = segment.replace_item(
                cell,
                MoonInput::new("toolbar-sell-edit")
                    .state(io.input)
                    .size(design::INPUT_SIZE),
            );
        }
    }
    if form == StripForm::Full {
        return segment.render().into_any_element();
    }
    let menu_items = (0..6)
        .map(|index| {
            let engaged = sel_slot == Some(index + 1);
            let row = MoonMenuItem::with_key(
                format!("toolbar-sell-row-{index}"),
                format!("S{}  {}", index + 1, cells.label(index)),
            )
            .selected(engaged)
            .checked(engaged);
            let (Some(io), Some(group)) = (io.as_ref(), group.clone()) else {
                return row;
            };
            let backend = io.backend.clone();
            row.on_click(move |_, _, app| sell_menu_pick(&backend, &group, index, sel_slot, app))
        })
        .collect();
    h_flex()
        .flex_none()
        .items_center()
        .gap(design::ui_px(cx, CARET_GAP))
        .child(segment.render())
        .child(caret_menu(
            "toolbar-sell",
            menu_items,
            interactive,
            io.map(|io| (io.caret_open, io.on_caret)),
            cx,
        ))
        .into_any_element()
}

/// The geometry the toolbar budget reads for one fitted strip.
///
/// Args:
///     cells: The fitted cells, the main cell included.
///     cx: Application context supplying the UI scale.
///
/// Returns:
///     The strip's pure geometry.
pub(super) fn strip_geometry(cells: &FittedCells, cx: &App) -> StripGeometry {
    StripGeometry {
        cells: cells.widths(),
        main: cells.main_item().resolved_width(),
        frame: design::joined_strip_width(cx, 0.0),
        caret: design::glyph_btn_w(cx),
        caret_gap: design::ui_value(cx, CARET_GAP),
    }
}

#[cfg(test)]
mod tests;
