//! The "Crosshair labels" frame of the chart-graphics popup (#1118), spanning the popup's top: one
//! line per crosshair value — the armed order size, the book depth, the percent — each with a
//! dropdown of places and two arrows that move it nearer to or further from the crosshair line.
//!
//! The rows are listed in the stored stacking order, so the list itself shows which value sits
//! nearest the line when several share a place. Every edit goes through [`super::write_cfg`] into
//! the tab's `ChartGraphicsCfg::cursor_labels`, and travels with ⧉ like every switch beside it.

use gpui::*;
use moon_core::config::{CursorLabel, CursorLabelSpot, CursorLabelsCfg};
use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonPalette,
    h_flex, v_flex,
};
use rust_i18n::t;

use super::{GraphicsPopupHost, write_cfg};
use crate::design;

/// Gap between a row's name, its dropdown and its arrows, before UI scaling.
const ROW_GAP: f32 = 4.0;

/// One width for all three dropdown triggers, before UI scaling, so the rows line up.
///
/// Sized to hold the longest place name of every shipped language whole: the trigger draws in
/// MoonUI's monospaced face (`DROPDOWN_TRIGGER_MONO`) at the `MoonButtonSize::density` tier — 14 px
/// at the default density — so a name costs 0.6 × 14 px per cell (two for a CJK character), plus
/// the two-cell caret and MoonUI's 14 px of padding. The longest today is Spanish "Cruz a la
/// izquierda, arriba": 29 cells → 258; the rest is headroom for the padding scaling apart from
/// the font. `theme_contract` re-derives this from the dictionaries, so a longer translation
/// fails a test instead of an ellipsis showing up in one language only.
const SPOT_DD_W: f32 = 272.0;

/// The dictionary key naming one crosshair value.
fn label_key(label: CursorLabel) -> &'static str {
    match label {
        CursorLabel::OrderSize => "chart.graphics.cursor_order_size",
        CursorLabel::Depth => "chart.graphics.cursor_depth",
        CursorLabel::Percent => "chart.graphics.cursor_percent",
    }
}

/// A short id fragment for one crosshair value, unique within the frame.
fn label_id(label: CursorLabel) -> &'static str {
    match label {
        CursorLabel::OrderSize => "size",
        CursorLabel::Depth => "depth",
        CursorLabel::Percent => "pct",
    }
}

/// One value's row on one line: its name, a dropdown of places, and the two order arrows.
///
/// Args:
///     id: Per-host element identity prefix.
///     entity: Popup host, written on every pick.
///     cfg: The target's stored places, read fresh on every render.
///     ix: The row's position in the stacking order.
///     label: The value this row places.
///     p: Active palette.
///     cx: App context, for the type steps and the control tier.
fn cursor_row<T: GraphicsPopupHost>(
    id: &str,
    entity: &Entity<T>,
    cfg: &CursorLabelsCfg,
    ix: usize,
    label: CursorLabel,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let current = cfg.spot(label);
    let items = crate::panels::radio_items(
        CursorLabelSpot::ALL.iter().map(|spot| {
            (
                *spot,
                SharedString::from(format!("{id}-cursor-{}-{spot:?}", label_id(label))),
                SharedString::from(t!(spot.locale_key()).to_string()),
            )
        }),
        current,
        crate::panels::RadioMark::Check,
        {
            let entity = entity.clone();
            move |app, spot: CursorLabelSpot| {
                write_cfg(&entity, app, |c| c.cursor_labels.set_spot(label, spot));
            }
        },
    );
    let dropdown = MoonDropdown::new(SharedString::from(format!(
        "{id}-cursor-{}",
        label_id(label)
    )))
    .label(t!(current.locale_key()).to_string())
    .trigger_caret(true)
    .trigger_variant(MoonButtonVariant::Soft)
    .trigger_size(MoonButtonSize::density(cx))
    // The same width on every row; a scaled width also lets MoonUI cut a name with an ellipsis
    // rather than spill it onto the arrows, should a future translation outgrow it.
    .trigger_width_scaled(SPOT_DD_W)
    .fit_menu_width(SPOT_DD_W, 360.0)
    .items(items);
    let last = ix + 1 == cfg.order.len();
    let arrow = |toward_line: bool| {
        let (dir, icon, tip) = if toward_line {
            (
                "up",
                "icons/arrow-up.svg",
                t!("chart.graphics.cursor_nearer"),
            )
        } else {
            (
                "down",
                "icons/arrow-down.svg",
                t!("chart.graphics.cursor_further"),
            )
        };
        let entity = entity.clone();
        MoonButton::new(SharedString::from(format!(
            "{id}-cursor-{}-{dir}",
            label_id(label)
        )))
        .outline()
        .width(design::glyph_btn_w(cx))
        .leading_icon(MoonButtonIconSlot::new(icon))
        .tooltip(tip.to_string())
        .disabled(if toward_line { ix == 0 } else { last })
        .on_click(move |_, _w, app| {
            write_cfg(&entity, app, |c| c.cursor_labels.shift(label, toward_line));
        })
        .render()
    };
    // One line: the name takes what is left, the dropdown and the arrows sit at the right.
    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, ROW_GAP))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text))
                .child(t!(label_key(label)).to_string()),
        )
        .child(dropdown)
        .child(arrow(true))
        .child(arrow(false))
        .into_any_element()
}

/// The frame's rows in stacking order, under a caption saying what the order means.
///
/// Args:
///     id: Per-host element identity prefix.
///     entity: Popup host.
///     cfg: The target's stored places.
///     width: The frame's inner width, which the caller computes for a frame spanning the
///         whole popup.
///     p: Active palette.
///     cx: App context.
///
/// Returns:
///     The frame's content, to go inside its `popup_group`.
pub(super) fn cursor_rows<T: GraphicsPopupHost>(
    id: &str,
    entity: &Entity<T>,
    cfg: &CursorLabelsCfg,
    width: Pixels,
    p: MoonPalette,
    cx: &App,
) -> Div {
    let mut col = v_flex().w(width).gap(design::ui_px(cx, 6.0));
    for (ix, label) in cfg.order.iter().enumerate() {
        col = col.child(cursor_row(id, entity, cfg, ix, *label, p, cx));
    }
    col.child(
        div()
            .text_size(design::t_caption(cx))
            .text_color(rgb(p.text_muted))
            .child(t!("chart.graphics.cursor_order_hint").to_string()),
    )
}
