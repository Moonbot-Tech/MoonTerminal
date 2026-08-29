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
use super::common::LayoutPopupHost as _;
use super::popup_slot::ChartPopup;
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
            b.select_fig_tool(tool);
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
        let (draw_mode, tool, sells_zone) = {
            let b = self.backend.read(cx);
            (b.fig_draw_mode, b.fig_tool, b.sells_zone_armed())
        };
        // Nothing is the current tool while drawing is off — that IS what "Cursor" means, and the
        // trigger has to say so rather than showing a tool that no click would use. The
        // Sells-to-zone mode borrows the Zone tool, so while it runs the check mark belongs to the
        // MODE's entry instead: two ticks would give two answers to "what does a click do", and
        // clicking a ticked Zone would silently end the mode.
        let current = (draw_mode && !sells_zone).then_some(tool);

        // Cursor first, then the mode, then the two groups split on `alertable`. Both groups are
        // built from the registry in its own order, so this function names no tool.
        let backend_off = self.backend.clone();
        let backend_zone = self.backend.clone();
        let mut items = vec![
            MoonMenuItem::with_key(
                SharedString::new_static("cursor"),
                SharedString::from(format!("↖  {}", t!("chart.fig.cursor"))),
            )
            .checked(current.is_none() && !sells_zone)
            .on_click(move |_, _, app| {
                backend_off.update(app, |b, bcx| {
                    // Cursor means "no tool places anything", which an armed Sells-to-zone mode
                    // would contradict on the next click. Restored first so the tool the mode interrupted
                    // is the one that comes back when drawing is switched on again.
                    b.disarm_sells_zone();
                    b.fig_draw_mode = false;
                    bcx.notify();
                });
            }),
            // The mode itself, so it can be seen and left with the mouse: the hotkey is not the
            // only way out, and a ticked entry is what says "this is what a click does now".
            MoonMenuItem::with_key(
                SharedString::new_static("sells-zone"),
                SharedString::from(format!("✎  {}", t!("chart.fig.sells_zone"))),
            )
            .checked(sells_zone)
            .on_click(move |_, _, app| {
                backend_zone.update(app, |b, bcx| {
                    b.toggle_sells_zone_arm();
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
        // The Sells-to-zone mode borrows the Zone tool, so the picker must say which of the two is
        // running: it is the only part of the UI that shows the mode when the pointer is not over a
        // chart, and the two do very different things with the next pair of Ctrl+clicks.
        let trigger = match (sells_zone, current) {
            (true, _) => format!("✎  {}", t!("chart.fig.sells_zone")),
            (false, Some(_)) => format!("{}  {}", def.glyph, t!(def.locale_key)),
            (false, None) => format!("↖  {}", t!("chart.fig.cursor")),
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

        // The settings button stays in the cluster. The hanging defaults panel is painted by
        // `render_fig_style_panel` after the chart-body dismiss layers so it hit-tests above them.
        let open = self.popup_shows(ChartPopup::FigStyle) && current.is_some();
        let settings = div()
            .relative()
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
                        this.toggle_chart_popup(ChartPopup::FigStyle, cx)
                    }))
                    .render(),
            );

        h_flex()
            .items_center()
            .gap(design::ui_px(cx, 2.0))
            .child(picker)
            .child(settings)
    }

    /// Hanging tool-defaults panel, painted after the chart-body dismiss layers.
    ///
    /// Disarming the tool closes it rather than leaving it parked over the chart.
    pub(super) fn render_fig_style_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (draw_mode, tool, sells_zone) = {
            let b = self.backend.read(cx);
            (b.fig_draw_mode, b.fig_tool, b.sells_zone_armed())
        };
        let current = (draw_mode && !sells_zone).then_some(tool);
        if !(self.popup_shows(ChartPopup::FigStyle) && current.is_some()) {
            return None;
        }
        crate::figstyle::render_tool_defaults(
            &self.backend,
            tool,
            crate::figstyle::WorkspaceAuthority::Unscoped,
            Some(self.custom_color_cell()),
            cx,
        )
    }
}
