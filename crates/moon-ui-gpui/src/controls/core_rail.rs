//! Shared selection and hover chrome for the Auto and expert core rails.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::MoonPalette;

use crate::design;

/// Build rail chrome without navigation or trading actions.
///
/// Args:
///     id: Stable row identity.
///     selected: Whether the owning surface selected the row.
///     selectable: Whether the row accepts a selection click.
///     horizontal_padding: Logical horizontal inset.
///     gap: Logical child spacing.
///     p: Active palette.
///     cx: Context supplying UI scale.
///
/// Returns:
///     A full-size row ready for surface-specific content and actions.
pub(crate) fn rail_row_base(
    id: impl Into<ElementId>,
    selected: bool,
    selectable: bool,
    horizontal_padding: f32,
    gap: f32,
    p: MoonPalette,
    cx: &App,
) -> Stateful<Div> {
    div()
        .id(id)
        .size_full()
        .flex()
        .items_center()
        .min_w_0()
        .gap(design::ui_px(cx, gap))
        .px(design::ui_px(cx, horizontal_padding))
        .text_size(design::t_body(cx))
        .text_color(rgb(if selectable { p.text } else { p.text_muted }))
        .when(selected, |row| {
            row.bg(design::moon_alpha(
                p.accent,
                design::RAIL_ROW_SELECTED_ALPHA,
            ))
        })
        .when(selectable, |row| {
            row.cursor_pointer().hover(move |row| {
                row.bg(design::moon_alpha(p.accent, design::RAIL_ROW_HOVER_ALPHA))
            })
        })
}
