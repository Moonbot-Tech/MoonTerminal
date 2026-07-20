//! Order-size (F1-F6) and fixed-sell (S1-S6) preset strips with click, double-click, and
//! Ctrl+wheel interaction, plus their shared `strip_with_overlay` frame.

use gpui::*;

use moon_ui::{MoonAccent, MoonInput, MoonInputState, MoonSegmentItem, MoonSegmentedControl};

use moon_core::feed::ClientSettingsEdit;
use moon_core::session::CoreId;
use rust_i18n::t;

use super::fmt::{fmt_adaptive, fmt_sell_pct, scroll_dy, wheel_step};
use crate::{Backend, design};

/// Direction of one preset step for a wheel gesture, or `None` for "leave the value alone".
///
/// Two conditions, both required.
///
/// **Ctrl**, because the strips sit in a dense toolbar where a bare wheel would silently rewrite
/// trading parameters: the order size goes into the core's config, the sell percentage straight
/// into the core. An accidental scroll over the toolbar must not do that, and it has no undo.
///
/// **Non-zero Y**, because `ScrollDelta` is two-dimensional: a horizontal gesture (trackpad,
/// shift-scroll) carries `y == 0`, and a naive `y > 0.0` would read it as "down" and shrink the
/// parameter from sideways movement.
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

/// Base horizontal padding on each side of a `MoonSegmentedControl` cell.
///
/// `HOTKEY_GAP` counts even though our hotkey text is empty: flex puts a gap between children
/// whether or not the first one is empty, so the cell spends those pixels either way.
///
/// Interim (FORK_BUGS), same class and same removal trigger as `design::popover_outer_width` and
/// its siblings: `MoonSegmentItem` exposes only `.width(f32)`, so from outside the library there is
/// no way to ask a cell what it needs. Delete these four and measure through the widget once
/// `MoonSegmentedControl` grows a fit-to-content mode upstream. Until then a padding change in
/// `segment.rs` mis-sizes every toolbar cell with nothing here to catch it.
const CELL_PAD_X: f32 = 11.0;
/// Base gap reserved between the cell's empty hotkey slot and its label.
const CELL_HOTKEY_GAP: f32 = 5.0;
/// Base font size used by a preset-cell label.
const CELL_LABEL_SIZE: f32 = 11.0;
/// Weight of the SELECTED cell, which draws semibold and is therefore wider than an unselected
/// one. Measuring by the heavier of the two is what stops a cell from growing at the moment it is
/// clicked, which would nudge the whole strip on every selection.
const CELL_LABEL_WEIGHT: f32 = 500.0;
/// Base floor for a cell's width — it is a mouse target. A short value ("1%") would otherwise collapse
/// into a slit that is awkward to hit.
const MIN_CELL_W: f32 = 34.0;
/// Base ceiling for a cell's width before its visible label is truncated and pixel-rounded.
///
/// The width follows the content, and the content comes from outside: an order-size preset is
/// hand-edited and accepts any positive `f64`, the sell percentages come from the core. With no
/// ceiling a single value like 1e308 formats to hundreds of digits and stretches both the strip and
/// its interaction layer across thousands of pixels — no amount of caption collapsing rescues that
/// row. A label past the ceiling is truncated with an ellipsis; the ceiling is wide enough that
/// only an anomalous value reaches it. Double-clicking the cell still exposes the full value in the
/// inline editor.
const MAX_CELL_W: f32 = 104.0;

/// A strip's six cells, each fitted to the width ceiling, with the widths they were fitted to.
///
/// Labels and widths travel TOGETHER because they are one result: the strip and its interaction
/// layer must be handed the same truncated label and the same width, or they disagree about what a
/// cell contains and where it sits — the exact drift [`strip_with_overlay`] spends a paragraph
/// guarding against. Passing the pair as one value puts that coupling in the type instead of in
/// prose.
pub(super) struct FittedCells {
    labels: [String; 6],
    widths: [f32; 6],
}

impl FittedCells {
    /// Fit `labels` to the cell ceiling at the current theme and font size.
    ///
    /// The width follows the label, which is what keeps the text from being squeezed inside a box
    /// that did not grow with it: a cell's padding goes through `ui()` and its label through
    /// `font()`, so only a measured width tracks the Font slider on both. Widths are rounded up to
    /// whole pixels — a fractional width is one more way for the strip and the overlay to round
    /// differently.
    ///
    /// Consequence accepted, not fixed: a step that changes a value's DIGIT COUNT (900 -> 1000)
    /// rewidths its cell and shifts the ones after it, under a stationary cursor during Ctrl+wheel.
    /// Quantizing the width to hide that would hand back the slack the measurement exists to
    /// reclaim, and the cell being edited keeps the cursor either way — the cells to its right move.
    pub(super) fn fit(cx: &App, labels: [String; 6]) -> Self {
        let pad = design::ui_value(cx, CELL_PAD_X) * 2.0 + design::ui_value(cx, CELL_HOTKEY_GAP);
        let min = design::font_w(cx, MIN_CELL_W);
        let max_text = (design::font_w(cx, MAX_CELL_W) - pad).max(0.0);
        // Measured at the selected cell's size and weight, and once per label: measuring costs an
        // uncached glyph layout per character, and this runs every frame for twelve cells.
        let measure =
            |s: &str| design::ui_text_width(cx, s, CELL_LABEL_SIZE, CELL_LABEL_WEIGHT, true);
        let mut fitted: [String; 6] = std::array::from_fn(|_| String::new());
        let mut widths = [0.0f32; 6];
        for (i, label) in labels.into_iter().enumerate() {
            let (text, text_w) = design::fit_text(&label, max_text, &measure);
            fitted[i] = text;
            widths[i] = cell_width(text_w, pad, min);
        }
        Self {
            labels: fitted,
            widths,
        }
    }

    /// The strip's total width — for the toolbar row's budget.
    pub(super) fn total_width(&self) -> f32 {
        self.widths.iter().sum()
    }
}

/// One cell's width from its measured label, the cell's own padding, and the floor.
///
/// Split off from [`FittedCells::fit`] deliberately: everything there hangs off `App` (theme, Font
/// slider, text system) while this is pure arithmetic a test can pin.
fn cell_width(text_w: f32, pad: f32, min: f32) -> f32 {
    (text_w + pad).max(min).ceil()
}

/// Preset cell labels, or dashes when there is no core.
///
/// One function for both strips: the no-core placeholder is a single decision, and written twice it
/// can be changed on one strip and forgotten on the other.
fn labels(values: Option<[f64; 6]>, fmt: impl Fn(f64) -> String) -> [String; 6] {
    std::array::from_fn(|i| match values {
        Some(v) => fmt(v[i]),
        None => "—".to_string(),
    })
}

/// Order-size cell labels.
///
/// A dash rather than the config default: with no core there is no account whose presets these
/// would be, and six plausible round numbers standing where a value belongs read as that account's
/// settings. Every other figure on the row already degrades to a dash in the same state.
pub(super) fn size_labels(values: Option<[f64; 6]>) -> [String; 6] {
    labels(values, fmt_adaptive)
}

/// Fixed-sell cell labels.
pub(super) fn sell_labels(pcts: Option<[f64; 6]>) -> [String; 6] {
    labels(pcts, |p| format!("{}%", fmt_sell_pct(p)))
}

/// Order-size preset strip (values, no F1-F6 captions). Values come from the core's config, the
/// selection is stored per core in `Backend::order_size_sel`; with no core there is neither, so
/// `sel` is `None` and nothing is lit. Interaction rides a transparent overlay over each button
/// (`MoonSegmentedControl` does not tell click, double-click and wheel apart itself): single click
/// = select; double click = inline edit (`order_size_edit_req`); CTRL+WHEEL = step the value by its
/// order of magnitude (see [`wheel_step_dir`] — a bare wheel leaves it alone). `core=None` → no
/// interaction.
pub(super) fn size_strip(
    cells: &FittedCells,
    sel: Option<usize>,
    edit_ix: Option<usize>,
    input: &Entity<MoonInputState>,
    backend: Entity<Backend>,
    core: Option<CoreId>,
    unit: Option<&str>,
) -> impl IntoElement {
    // The unit goes in the cell's tooltip: on a narrow window the group caption "Size, USDT"
    // collapses, while the tooltip is reachable at every width. No core, no invented unit.
    let unit = unit.map(str::to_string);
    let items: Vec<MoonSegmentItem> = (0..6)
        .map(|i| {
            let mut it = MoonSegmentItem::new("", cells.labels[i].clone()).width(cells.widths[i]);
            if sel == Some(i) {
                it = it.selected(true);
            }
            it
        })
        .collect();
    let seg = MoonSegmentedControl::new("toolbar-size-presets")
        .accent(MoonAccent::Amber)
        .items(items)
        .render();

    let backend_click = backend.clone();
    strip_with_overlay(
        seg,
        "size",
        &cells.widths,
        edit_ix,
        "toolbar-size-edit",
        input,
        core.is_some(),
        move |i| match unit.as_deref() {
            Some(u) => t!("toolbar.size_hint", n = i + 1, unit = u).to_string(),
            None => t!("toolbar.size_hint_nounit", n = i + 1).to_string(),
        },
        // Одиночный клик = выбор пресета; дабл = инлайн-правка (`order_size_edit_req`).
        move |i, dbl, cx| {
            let Some(core) = core else { return };
            backend_click.update(cx, |b, bcx| {
                if dbl {
                    b.order_size_edit_req = Some((core, i));
                } else {
                    // Выбор + персист в конфиг (восстановление после перезапуска).
                    b.set_order_size_sel(core, i);
                }
                b.order_size_rev = b.order_size_rev.wrapping_add(1);
                bcx.notify();
            });
        },
        // Колесо = ±значение с шагом по порядку величины (frac 1.0).
        move |i, up, cx| {
            let Some(core) = core else { return };
            backend.update(cx, |b, bcx| {
                let cur = b.order_size_value(core, i);
                let next = wheel_step(cur, up, 1.0);
                if next != cur {
                    b.set_order_size_value(core, i, next);
                    b.order_size_rev = b.order_size_rev.wrapping_add(1);
                    bcx.notify();
                }
            });
        },
    )
}

/// Полоса fixed-sell пресетов (S1-S6) рядом с кнопкой TP (без подписи). Значения — из
/// `ClientSettings` активного ядра (видимые проценты). Слот подсвечен ТОЛЬКО когда задействован
/// (`sel_slot = Some` лишь при `fixed_sell_mode`); по умолчанию все S погашены, горит TP.
/// Клик — задействовать слот (гасит TP); повторный клик по активному — вернуть TP. Нет ядра — прочерки.
pub(super) fn sell_strip(
    cells: &FittedCells,
    sel_slot: Option<usize>,
    edit_ix: Option<usize>,
    input: &Entity<MoonInputState>,
    backend: Entity<Backend>,
    core: Option<CoreId>,
) -> impl IntoElement {
    let items: Vec<MoonSegmentItem> = (0..6)
        .map(|i| {
            let mut it = MoonSegmentItem::new("", cells.labels[i].clone()).width(cells.widths[i]);
            if sel_slot == Some(i + 1) {
                it = it.selected(true);
            }
            it
        })
        .collect();
    let seg = MoonSegmentedControl::new("toolbar-sell-presets")
        .accent(MoonAccent::Blue)
        .items(items)
        .render();

    let backend_click = backend.clone();
    strip_with_overlay(
        seg,
        "sell",
        &cells.widths,
        edit_ix,
        "toolbar-sell-edit",
        input,
        core.is_some(),
        |i| t!("toolbar.sell_hint", n = i + 1).to_string(),
        // Одиночный клик = задействовать слот (гасит TP); повторный клик по активному слоту
        // = вернуть TP (гасит S, не трогая значение TP); дабл = инлайн-правка %.
        move |i, dbl, cx| {
            let Some(core) = core else { return };
            backend_click.update(cx, |b, bcx| {
                if dbl {
                    b.sell_edit_req = Some((core, i));
                } else {
                    // Повторный клик по уже горящему слоту → возврат к главному TP.
                    let (edit, local_slot) = if sel_slot == Some(i + 1) {
                        (ClientSettingsEdit::EngageMainTakeProfit, None)
                    } else {
                        (ClientSettingsEdit::SelectFixedSellSlot(i + 1), Some(i + 1))
                    };
                    b.set_fixed_sell_slot_local(core, local_slot);
                    b.order_size_rev = b.order_size_rev.wrapping_add(1);
                    if let Err(error) = b.session.edit_client_settings(core, edit) {
                        log::warn!("toggle fixed-sell slot failed: {error}");
                    }
                }
                bcx.notify();
            });
        },
        // Колесо = ±% полразрядом (frac 0.5). Значение % — на ядре, читаем из снимка ClientSettings.
        move |i, up, cx| {
            let Some(core) = core else { return };
            backend.update(cx, |b, bcx| {
                let cur = b.fixed_sell_pct(core, i);
                let next = wheel_step(cur, up, 0.5);
                if next != cur {
                    // Оптимистично: локальный кэш + перерисовка СРАЗУ; в ядро — тоже.
                    b.set_fixed_sell_pct_local(core, i, next);
                    b.order_size_rev = b.order_size_rev.wrapping_add(1);
                    bcx.notify();
                    if let Err(error) = b.session.edit_client_settings(
                        core,
                        ClientSettingsEdit::SetFixedSellPct {
                            slot: i + 1,
                            pct: next,
                        },
                    ) {
                        log::warn!("set fixed-sell pct (wheel) failed: {error}");
                    }
                }
            });
        },
    )
}

/// Shared frame for both preset strips (size/sell): the segmented control plus a transparent
/// interaction layer over it (`MoonSegmentedControl` does not tell click, double-click and wheel
/// apart itself), in which the cell being edited is replaced by an inline input. Size and sell
/// differ only in `on_click`/`on_wheel`; the wheel gate ([`wheel_step_dir`]) itself is shared, so
/// Ctrl is required on both strips. `overlay=false` (no core) → no interaction.
///
/// **The interaction layer is THE SAME flex row as the cells, not absolute offsets summed from the
/// preceding widths.** GPUI runs every length through `round_to_device_pixel` (`moon-gpui/src/
/// taffy.rs`, `impl ToTaffy<f32> for AbsoluteLength`): each cell's width is rounded on its own,
/// while an absolute prefix sum is rounded once as a whole — so the two roundings diverge and the
/// error accumulates left to right (on fractional widths and a fractional device scale, up to
/// several pixels by the sixth slot). Sharing the width array does NOT fix it: same numbers,
/// different rounding order. The only dependable guarantee is the same layout — same widths, same
/// siblings, same container — so taffy rounds both rows identically.
///
/// Consequence for edits: the layer's root must never gain padding, a border or a `gap`; any of
/// them shifts every hit target off its cell. `MoonSegmentedControl::item_gap` is left at its zero
/// default for the same reason — a non-zero one would have to be duplicated here.
///
/// `tooltip` supplies the hint BY SLOT NUMBER: the overlay covers the cell and swallows hover, so
/// `MoonSegmentedControl` cannot carry its own hint — and has no tooltip in its API anyway. It is
/// also the only place the user learns that Ctrl+wheel exists.
#[allow(clippy::too_many_arguments)]
fn strip_with_overlay(
    seg: impl IntoElement,
    id_prefix: &'static str,
    widths: &[f32; 6],
    edit_ix: Option<usize>,
    edit_input_id: &'static str,
    input: &Entity<MoonInputState>,
    overlay: bool,
    tooltip: impl Fn(usize) -> String + Clone + 'static,
    on_click: impl Fn(usize, bool, &mut App) + Clone + 'static,
    on_wheel: impl Fn(usize, bool, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let mut root = div().relative().flex().items_center().child(seg);
    // Bound the index at the boundary rather than trusting it: it arrives from an edit REQUEST
    // (a hotkey, a double click) that this function does not own.
    let edit_ix = edit_ix.filter(|i| *i < 6);

    // The layer is needed without interaction too — while a cell is being edited inline. A core
    // always exists when an edit was requested, but the two conditions stay separate so the input
    // cannot disappear under `overlay=false`.
    if overlay || edit_ix.is_some() {
        let mut layer = div().absolute().inset_0().flex().items_center();
        for i in 0..6 {
            let cell = div().w(px(widths[i])).h_full();
            layer = layer.child(if edit_ix == Some(i) {
                // This is the cell being edited: the input REPLACES the hit target, or the same
                // click would land both in the field and in the preset-selection handler.
                cell.child(MoonInput::new(edit_input_id).state(input).small())
                    .into_any_element()
            } else if overlay {
                let on_click = on_click.clone();
                let on_wheel = on_wheel.clone();
                let hint = tooltip(i);
                cell.id(SharedString::from(format!("{id_prefix}-hit-{i}")))
                    // The cell is clickable, but the overlay swallows the segment's hover — the
                    // cursor is the only sign of interactivity until the tooltip appears.
                    .cursor_pointer()
                    .tooltip(move |_window, cx| {
                        cx.new(|_| moon_ui::MoonTooltipView::new(hint.clone()))
                            .into()
                    })
                    .on_mouse_down(MouseButton::Left, move |ev, _w, cx| {
                        on_click(i, ev.click_count >= 2, cx);
                    })
                    .on_scroll_wheel(move |ev, _w, cx| {
                        if let Some(up) = wheel_step_dir(ev.modifiers, ev.delta) {
                            on_wheel(i, up, cx);
                        }
                    })
                    .into_any_element()
            } else {
                cell.into_any_element()
            });
        }
        root = root.child(layer);
    }
    root
}

#[cfg(test)]
mod tests {
    // NOT `use super::*`: the glob would pull in the `gpui::test` macro, and `#[test]` would
    // expand into itself (recursion limit).
    use super::{cell_width, wheel_step_dir};
    use gpui::{Modifiers, Point, ScrollDelta};

    fn lines(y: f32) -> ScrollDelta {
        ScrollDelta::Lines(Point { x: 0.0, y })
    }

    fn ctrl() -> Modifiers {
        Modifiers {
            control: true,
            ..Modifiers::default()
        }
    }

    #[test]
    fn bare_wheel_never_changes_a_preset() {
        // Removing the modifier gate would let scrolling over the toolbar silently rewrite the
        // order size in the config and the sell percentage in the core.
        assert_eq!(wheel_step_dir(Modifiers::default(), lines(1.0)), None);
        assert_eq!(wheel_step_dir(Modifiers::default(), lines(-1.0)), None);
    }

    #[test]
    fn ctrl_wheel_reports_direction() {
        // Reversing the Y comparison or rejecting Ctrl-modified input makes one of these assertions
        // fail before a preset is adjusted in the wrong direction or not adjusted at all.
        assert_eq!(wheel_step_dir(ctrl(), lines(1.0)), Some(true));
        assert_eq!(wheel_step_dir(ctrl(), lines(-1.0)), Some(false));
    }

    #[test]
    fn horizontal_gesture_is_not_a_downward_step() {
        // ScrollDelta is two-dimensional: a sideways gesture carries y == 0. A naive `y > 0.0`
        // would return "down" and SHRINK a trading parameter from horizontal scrolling.
        assert_eq!(wheel_step_dir(ctrl(), lines(0.0)), None);
    }

    /// Regression: dropping cell padding from `cell_width` squeezes the rendered preset label.
    #[test]
    fn a_cell_leaves_room_for_its_own_label() {
        // Plausible future edit: `controls::strips::cell_width` loses its `pad` term — someone
        // decides the measured text width is enough and drops the cell's own padding
        // (`CELL_PAD_X` + `CELL_HOTKEY_GAP`) as slack. Visible consequence: the label is squeezed
        // inside a box that was never sized for it again — exactly the crowding at a larger font
        // that made the width content-measured in the first place.
        //
        // The oracle is independent of the code: the test supplies both quantities, and "a cell is
        // never narrower than its content plus its padding" comes from the contract, not from the
        // implementation.
        let text = 40.0;
        let pad = 27.0;
        assert!(
            cell_width(text, pad, 34.0) >= text + pad,
            "a cell must fit its label together with its own padding"
        );
    }

    /// Regression: removing the minimum width makes short preset cells difficult to click.
    #[test]
    fn a_short_label_still_gets_a_clickable_cell() {
        // The floor is a mouse target: "1%" is narrower than the padding on its own, and without
        // the clamp the cell would collapse into a slit that is awkward to hit.
        assert_eq!(cell_width(6.0, 4.0, 34.0), 34.0);
    }

    /// Regression: removing pixel rounding can desynchronize cells from their hit targets.
    #[test]
    fn a_cell_width_is_whole_pixels() {
        // A second plausible edit to the same function, likelier than losing `pad`: `.ceil()` looks
        // like cosmetic rounding and gets removed as redundant. But a fractional width is precisely
        // the source of rounding divergence between the strip and its interaction layer that the
        // layer was rewritten as a flex row to avoid (see `strip_with_overlay`) — GPUI rounds every
        // length to a device pixel separately.
        //
        // The input is fractional ON PURPOSE: on the whole-number inputs of the two tests above,
        // losing `.ceil()` would go unnoticed.
        assert_eq!(cell_width(40.5, 26.0, 34.0), 67.0);
    }

    #[test]
    fn wheel_handler_consults_the_gate_rather_than_the_raw_delta() {
        // Guards the CALL SITE, which the three tests above cannot reach: they all exercise the
        // pure `wheel_step_dir` helper, so reading the raw delta directly in `on_scroll_wheel`
        // would defeat the Ctrl gate entirely and leave every one of them green. The gate is only
        // effective if the handler actually calls it.
        let source = include_str!("strips.rs");
        let implementation = source.split("#[cfg(test)]").next().unwrap_or(source);
        let handler = implementation
            .split(".on_scroll_wheel(")
            .nth(1)
            .expect("the strip overlay must still install a scroll-wheel handler");

        assert!(
            handler.contains("wheel_step_dir(ev.modifiers, ev.delta)"),
            "the wheel handler must route through wheel_step_dir, not read the delta directly"
        );
    }
}
