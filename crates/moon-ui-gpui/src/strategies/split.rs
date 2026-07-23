//! Panel splitter layout: drag state, widths persistence, and the resize handles.

use super::*;

/// Panel boundary being dragged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum PanelSplit {
    Tree,
    Versions,
    Sections,
}

/// Splitter drag payload.
#[derive(Clone)]
pub(super) struct PanelResizeDrag {
    pub(super) which: PanelSplit,
}

/// Empty splitter drag ghost; the panel itself follows the pointer.
struct SplitGhost;
impl Render for SplitGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Width of a panel splitter in logical pixels.
const PANEL_SPLIT_W: f32 = 5.0;
/// Width of the collapsed Versions strip.
const VERSIONS_COLLAPSED_W: f32 = 22.0;

impl StrategiesView {
    /// Persist panel widths and Versions collapse state in layout.
    /// The debounced save loop drains `layout_dirty`, as it does for window geometry.
    pub(super) fn save_panels(&mut self, cx: &mut Context<Self>) {
        let mut p = self.panels;
        p.versions_collapsed = self.versions.collapsed;
        self.panels = p;
        self.backend.update(cx, |b, _| {
            let cur = b.layout.strategies_panels;
            if (
                cur.tree_w,
                cur.versions_w,
                cur.sections_w,
                cur.versions_collapsed,
            ) != (p.tree_w, p.versions_w, p.sections_w, p.versions_collapsed)
            {
                b.layout.strategies_panels = p;
                b.layout_dirty = true;
            }
        });
    }

    /// Build a draggable vertical splitter between panels.
    pub(super) fn panel_splitter(&self, which: PanelSplit, cx: &Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        div()
            .id(SharedString::from(format!("strat-split-{which:?}")))
            .w(px(PANEL_SPLIT_W))
            .h_full()
            .flex_none()
            .cursor(CursorStyle::ResizeLeftRight)
            .hover(move |s| s.bg(moon_alpha(p.blue, 0.35)))
            .on_drag(PanelResizeDrag { which }, |_, _, _, cx| {
                cx.new(|_| SplitGhost)
            })
            .into_any_element()
    }

    /// Resize a panel from the pointer's window-relative position minus the panel's left edge.
    /// Absolute positioning remains stable when intermediate drag events are missed.
    pub(super) fn on_panel_split_drag(
        &mut self,
        x: f32,
        which: PanelSplit,
        cx: &mut Context<Self>,
    ) {
        match which {
            PanelSplit::Tree => {
                self.panels.tree_w = (x - PANEL_SPLIT_W / 2.0).clamp(240.0, 900.0);
            }
            PanelSplit::Versions => {
                self.panels.versions_w =
                    (x - self.panels.tree_w - PANEL_SPLIT_W * 1.5).clamp(90.0, 420.0);
            }
            PanelSplit::Sections => {
                let vers = if self.versions.collapsed {
                    VERSIONS_COLLAPSED_W
                } else {
                    self.panels.versions_w + PANEL_SPLIT_W
                };
                self.panels.sections_w =
                    (x - self.panels.tree_w - PANEL_SPLIT_W - vers - PANEL_SPLIT_W / 2.0)
                        .clamp(150.0, 520.0);
            }
        }
        self.save_panels(cx);
        cx.notify();
    }
}
