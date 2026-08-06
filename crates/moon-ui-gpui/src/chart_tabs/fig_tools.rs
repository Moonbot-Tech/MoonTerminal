//! Figure-drawing cluster in the tab strip: two buttons.
//!
//! The first PICKS the tool from a list — the ones Moonbot also draws first, in the order of the
//! core's own chart-object types, then a separator, then the ones only we have. Which group a tool
//! lands in is read from [`ToolDef::alertable`], the same flag that decides whether it can be armed
//! as an alert, so a tool moves between the groups by becoming sendable and never by being listed
//! somewhere by hand. The list opens with "Cursor", which is how drawing mode is left now that
//! there is no pencil: no tool selected means no drawing.
//!
//! The second opens that tool's SETTINGS — colour, thickness, opacity, line kind, fill and whatever
//! switches the tool itself declares. It is the same panel the right-click on a figure opens
//! (`crate::figstyle`), aimed at the tool's defaults instead of at one figure, so the two surfaces
//! cannot drift apart: a control added for a figure appears here by existing.

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonMenuItem, MoonMenuSize, MoonPalette,
    MoonPopover, MoonPopoverPlacement, MoonPopupMenu, MoonRect, MoonSelectorPill,
    MoonSelectorSegment, h_flex,
};

use moon_core::figures::{FigureTool, ToolDef};
use rust_i18n::t;

use super::ChartTabs;
use crate::design;

/// Width of the tool field, in font-scaled units. A fifth narrower than the row of six buttons it
/// replaced: the longest name it has to hold is a two-word one, and the field sat half empty.
const PICKER_W: f32 = 134.0;
/// Its height, matching the Micro buttons beside it.
const PICKER_H: f32 = 18.0;

/// One tool's entry in the picker: its glyph and its name, checked when it is the current one.
fn tool_item(
    def: &'static ToolDef,
    current: Option<FigureTool>,
    backend: Entity<crate::Backend>,
) -> MoonMenuItem {
    let tool = def.tool;
    MoonMenuItem::with_key(
        SharedString::new_static(def.key),
        SharedString::from(format!("{}  {}", def.glyph, t!(def.locale_key))),
    )
    .checked(current == Some(tool))
    .on_click(move |_, _, app| {
        backend.update(app, |b, bcx| {
            // Picking a tool is also how drawing is entered: choosing one and then being told to
            // turn something else on would be a step with no purpose.
            b.fig_tool = tool;
            b.fig_draw_mode = true;
            bcx.notify();
        });
    })
}

impl ChartTabs {
    /// The arbitrary-colour cell handed to the settings panel's swatch row.
    ///
    /// Lives here and not in `figstyle` because the wheel needs an `Entity` to hold its open state,
    /// and that belongs to the view that hosts the popup. The subscription in `ChartTabs::new`
    /// writes the chosen RGB into `fig_style`, keeping the opacity the stepper owns.
    fn custom_color_cell(&self) -> AnyElement {
        div()
            .id("fig-custom-color")
            .tooltip(|_window, cx| {
                cx.new(|_| moon_ui::MoonTooltipView::new(t!("chart.fig.custom_color").to_string()))
                    .into()
            })
            .child(
                moon_ui::MoonColorPicker::new(&self.fig_color_picker)
                    .colors(design::picker_palette()),
            )
            .into_any_element()
    }

    pub(super) fn render_fig_tools(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (draw_mode, tool) = {
            let b = self.backend.read(cx);
            (b.fig_draw_mode, b.fig_tool)
        };
        // Nothing is the current tool while drawing is off — that IS what "Cursor" means, and the
        // trigger has to say so rather than showing a tool that no click would use.
        let current = draw_mode.then_some(tool);

        // Cursor first, then the two groups split on `alertable`. Both groups are built from the
        // registry in its own order, so this function names no tool.
        let backend_off = self.backend.clone();
        let mut items = vec![
            MoonMenuItem::with_key(
                SharedString::new_static("cursor"),
                SharedString::from(format!("↖  {}", t!("chart.fig.cursor"))),
            )
            .checked(current.is_none())
            .on_click(move |_, _, app| {
                backend_off.update(app, |b, bcx| {
                    b.fig_draw_mode = false;
                    bcx.notify();
                });
            }),
        ];
        for group in [true, false] {
            let mut group_items = moon_core::figures::tools::REGISTRY
                .iter()
                .filter(|d| d.alertable == group)
                .map(|d| tool_item(d, current, self.backend.clone()))
                .peekable();
            if group_items.peek().is_some() {
                // A separator between what precedes and this group — never a leading one, and never
                // two in a row when a group turns out to be empty.
                items.push(MoonMenuItem::separator());
                items.extend(group_items);
            }
        }

        let def = tool.def();
        let label = match current {
            Some(_) => format!("{}  {}", def.glyph, t!(def.locale_key)),
            None => format!("↖  {}", t!("chart.fig.cursor")),
        };
        // A pill and a popover rather than `MoonDropdown`: the dropdown bakes its caret into the
        // label string and centres the result, so a tool's name sits in the middle of the field
        // with air on both sides. `MoonSelectorPill` is the product's own field-with-a-value — it
        // lays its segments out from the LEFT and draws the caret at the right edge — and it takes
        // the same `MoonMenuItem` list through `MoonPopupMenu`, so the menu itself is unchanged.
        let p = MoonPalette::active(cx);
        let picker_open = self.fig_tool_popup_open;
        let trigger_w = f32::from(design::font_w_px(cx, PICKER_W));
        let trigger_h = f32::from(design::ui_px(cx, PICKER_H));
        let picker_view = cx.entity();
        let picker = MoonPopover::new("fig-tool-popover")
            .placement(MoonPopoverPlacement::BottomStart)
            // No `content_width`: the content is a MENU, and a menu measures its own rows. That
            // constant is for panels, whose content box a popover has to be told the size of.
            .open(picker_open)
            .on_open_change(move |open, _window, app| {
                picker_view.update(app, |this, cx| {
                    this.fig_tool_popup_open = open;
                    cx.notify();
                });
            })
            .trigger(
                // `MoonSelectorPill::bounds` is absolute, so an explicit in-flow box owns the
                // geometry the popover measures and anchors to — the same shape the header's core
                // selector uses.
                div()
                    .id("fig-tool-tip")
                    .relative()
                    .flex_none()
                    .w(px(trigger_w))
                    .h(px(trigger_h))
                    .tooltip(|_window, cx| {
                        cx.new(|_| {
                            moon_ui::MoonTooltipView::new(t!("chart.fig.draw_tip").to_string())
                        })
                        .into()
                    })
                    .child(
                        MoonSelectorPill::new("fig-tool")
                            .bounds(MoonRect::new(0.0, 0.0, trigger_w, trigger_h))
                            .height(trigger_h)
                            .radius(f32::from(design::r_button(cx)))
                            .caret(true)
                            .segment(
                                MoonSelectorSegment::new(label)
                                    // Blue while a tool is armed, so the strip says at a glance
                                    // that the next click on the chart draws.
                                    .color(if draw_mode { p.accent } else { p.text })
                                    .font_size(f32::from(design::t_caption(cx))),
                            )
                            .render(),
                    ),
            )
            .content(
                MoonPopupMenu::new("fig-tool-menu")
                    .fit_width(160.0, 320.0)
                    .size(MoonMenuSize::Compact)
                    .items(items)
                    .render(),
            );

        // The settings button and, hanging under it, the shared panel for this tool's defaults.
        // Disarming the tool closes it rather than leaving it parked over the chart.
        let open = self.fig_style_popup_open && current.is_some();
        let settings = div()
            .relative()
            // The dismiss layer under the cluster closes the panel on mouse-DOWN, and this button
            // toggles on mouse-UP: without stopping the press here, pressing ⚙ while open would
            // close and immediately reopen it, reading as a dead control. The panel itself already
            // stops its own input inside `figstyle::shell`.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                MoonButton::new("fig-settings")
                    .label("⚙")
                    .size(MoonButtonSize::Micro)
                    .variant(if open {
                        MoonButtonVariant::Blue
                    } else {
                        MoonButtonVariant::Ghost
                    })
                    .selected(open)
                    // Nothing to aim it at with no tool armed: the panel would edit the defaults of
                    // a tool the trigger says is not selected.
                    .disabled(current.is_none())
                    .tooltip(t!("chart.fig.tool_settings").to_string())
                    .on_click(cx.listener(|this, _, _w, cx| {
                        this.fig_style_popup_open = !this.fig_style_popup_open;
                        cx.notify();
                    }))
                    .render(),
            )
            .children(
                open.then(|| {
                    crate::figstyle::render_tool_defaults(
                        &self.backend,
                        tool,
                        Some(self.custom_color_cell()),
                        cx,
                    )
                })
                .flatten(),
            );

        h_flex()
            .items_center()
            .gap(design::ui_px(cx, 2.0))
            .child(picker)
            .child(settings)
    }
}
