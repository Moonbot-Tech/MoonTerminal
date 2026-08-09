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
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuItem, MoonMenuSize, h_flex,
};

use moon_core::figures::{FigureTool, ToolDef};
use rust_i18n::t;

use super::ChartTabs;
use crate::design;

/// Width of the tool field, in font-scaled units. A fifth narrower than it first shipped: the
/// longest name it has to hold is two short words, and the field sat half empty.
const PICKER_W: f32 = 134.0;

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

    /// Render the figure-tool selector and explicitly unscoped tool-default settings surface.
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
        let trigger = match current {
            Some(_) => format!("{}  {}", def.glyph, t!(def.locale_key)),
            None => format!("↖  {}", t!("chart.fig.cursor")),
        };
        // `MoonDropdown` and not a pill in a popover: a popover always draws its own border and
        // padding AROUND its content, so a menu inside one reads as two frames, the outer one
        // larger than the inner. The dropdown is one frame and the product's own menu chrome.
        //
        // Its label is centred, which is not what this field wants, and that cannot be fixed from
        // here: the dropdown bakes its caret into the label string and the button centres the
        // whole result. Logged in `docs-internal/FORK_BUGS.md`; a `trigger_align` on MoonUI's side
        // is the fix.
        let picker = MoonDropdown::new("fig-tool")
            .label(trigger)
            .trigger_caret(true)
            .trigger_variant(if draw_mode {
                MoonButtonVariant::Blue
            } else {
                MoonButtonVariant::Soft
            })
            .trigger_size(MoonButtonSize::Micro)
            .trigger_width_scaled(PICKER_W)
            .menu_width_scaled(180.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items);
        // `MoonDropdown` carries no tooltip of its own, so the hint hangs on a wrapper. It is the
        // only place the Ctrl gesture is written down now that the pencil's tooltip is gone.
        let picker = div()
            .id("fig-tool-tip")
            .tooltip(|_window, cx| {
                cx.new(|_| moon_ui::MoonTooltipView::new(t!("chart.fig.draw_tip").to_string()))
                    .into()
            })
            .child(picker);

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
                        crate::figstyle::WorkspaceAuthority::Unscoped,
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
