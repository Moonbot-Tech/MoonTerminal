//! The figure being drawn: the nodes placed so far, the cursor preview, and completion.
//!
//! Tool-agnostic. How many clicks a tool takes, what it builds from them and what it shows before
//! the last one are read from its registry row (`ToolDef::clicks` / `make` / `preview`), so a new
//! tool draws here without a line of code.

use moon_core::figures::proj::PxPoint;
use moon_core::figures::{DrawStyle, FigNode, Figure, FigureKind, FigureTool, ToolSettings};
use moon_core::session::CoreId;

/// Figure currently being drawn on this panel.
pub(crate) struct FigDraft {
    pub pane: usize,
    pub core: CoreId,
    pub market: String,
    pub tool: FigureTool,
    /// Style snapshot captured from `Backend::fig_style` at draw start, so editing the style
    /// mid-draw does not change the figure already being placed.
    pub style: DrawStyle,
    /// The tool's switch defaults, snapshotted with the style and for the same reason. Applied to
    /// the preview as well as to the finished figure, so what is drawn under the cursor is what
    /// lands — a Fibonacci with levels switched off must not preview all eleven of them.
    switches: ToolSettings,
    /// Nodes placed so far, in click order. Always shorter than the tool's click count: the click
    /// that would complete it creates the figure instead.
    pub nodes: Vec<FigNode>,
    /// Current cursor position in data coordinates.
    pub cursor: FigNode,
    /// Pixel of the press the figure layer accepted and the pointer is still holding, if any.
    ///
    /// The press-drag-release gesture measures its travel from here, and the preview trusts it to
    /// decide that a pointer under a held button belongs to THIS draft rather than to a chart pan
    /// or an order-line drag that took the press instead. It lives on the draft so that dropping
    /// the draft drops the press with it: a press outliving what it was placed for is what lets an
    /// unrelated release be measured as a gesture, and every path that abandons a draft would
    /// otherwise have to remember to clear it by hand.
    ///
    /// Pixels, not `(time, price)`: it answers "how far has the hand moved", which is a question
    /// about the screen and must not change because the chart scrolled underneath.
    pub down: Option<PxPoint>,
    /// Nodes the tool derives from a live press-drag gesture (`ToolDef::drag_rest`), already
    /// projected back to `(time, price)`.
    ///
    /// Empty for every tool but the ones drawn by dragging a part of themselves, and empty for
    /// those too until a press actually travels. While it is not empty the preview is the FINISHED
    /// figure rather than the piece under the cursor — a dragged triangle previews the triangle the
    /// release will leave behind, not the base edge — so what is dragged is what lands.
    drag_rest: Vec<FigNode>,
    /// Whether this draft was started with the Sells-to-zone mode armed, snapshotted for the same
    /// reason the style is.
    ///
    /// What the finishing click does is decided by THIS, never by the mode's current state: the
    /// mode and the tool are the same `Channel` either way, so a mode armed — or dropped —
    /// mid-draw would otherwise turn a figure the user was drawing into a live bulk command, or
    /// the reverse. `sync_fig_visual` abandons a draft whose flag stops matching, exactly as it
    /// does for a changed tool.
    pub sells_zone: bool,
}

impl FigDraft {
    /// Starts a draft with nothing placed yet; the click that opened it goes through
    /// [`Self::place`] like every later one, so a one-click tool needs no special case.
    pub(super) fn new(
        pane: usize,
        core: CoreId,
        market: String,
        tool: FigureTool,
        style: DrawStyle,
        switches: ToolSettings,
        cursor: FigNode,
    ) -> Self {
        Self {
            pane,
            core,
            market,
            tool,
            style,
            switches,
            nodes: Vec::new(),
            cursor,
            down: None,
            drag_rest: Vec::new(),
            sells_zone: false,
        }
    }

    /// The tool's rule for deriving the rest of itself from a drag, when THIS draft is eligible to
    /// use it.
    ///
    /// Eligibility is a property of the draft, so it is stated here beside the draft's other rules
    /// rather than in the panel: only a draft holding exactly the press node derives anything — a
    /// gesture continuing a draft that already has nodes would add the derived ones on top of the
    /// clicked ones and overshoot the figure — and a Sells-to-zone band never does, because its
    /// finishing node sends a live bulk move and no price the hand did not point at may be one of
    /// the two it is spread over. The band arms a tool with no rule of its own today; this guards
    /// the day one of them grows one.
    pub(super) fn drag_rest_rule(&self) -> Option<fn(PxPoint, PxPoint) -> Vec<PxPoint>> {
        if self.sells_zone || self.nodes.len() != 1 {
            return None;
        }
        self.tool.def().drag_rest
    }

    /// Replaces the gesture-derived nodes, reporting whether they changed.
    ///
    /// Called on every accepted pointer move, with an empty set whenever the gesture is not live —
    /// the button came up, the press has not travelled far enough to be a drag, or the tool derives
    /// nothing. That keeps a stale apex from outliving the drag that produced it and reaching the
    /// preview between two ordinary clicks.
    pub(super) fn set_drag_rest(&mut self, rest: Vec<FigNode>) -> bool {
        if self.drag_rest == rest {
            return false;
        }
        self.drag_rest = rest;
        true
    }

    /// Marks the draft as a Sells-to-zone band, taken at the moment it starts.
    pub(super) fn for_sells_zone(mut self, armed: bool) -> Self {
        self.sells_zone = armed;
        self
    }

    /// Whether the draft belongs to this pane, chart, tool and drawing mode. A click anywhere else
    /// — another pane, another market, after switching tools, or after the Sells-to-zone mode was
    /// armed or dropped mid-draw — abandons it.
    ///
    /// `sells_zone` is the mode as it stands NOW: the tool is `Channel` on both sides of that
    /// switch, so without it a half-drawn figure would be finished as a live command, or a
    /// half-drawn command stored as a figure.
    pub(super) fn belongs_to(
        &self,
        pane: usize,
        core: CoreId,
        market: &str,
        tool: FigureTool,
        sells_zone: bool,
    ) -> bool {
        self.pane == pane
            && self.core == core
            && self.market == market
            && self.tool == tool
            && self.sells_zone == sells_zone
    }

    /// Whether this draft's later clicks still require the secondary modifier.
    ///
    /// Only the Sells-to-zone band does: its finishing click sends a live bulk move, while an
    /// unmodified left click on a chart is the trading/navigation gesture. The single statement of
    /// that rule, read by both the press and the release path.
    pub(crate) fn needs_modifier(&self) -> bool {
        self.sells_zone
    }

    /// Adds a node. Returns the finished figure kind once the tool's click count is reached, in
    /// which case the caller drops the draft.
    pub(super) fn place(&mut self, node: FigNode) -> Option<FigureKind> {
        self.nodes.push(node);
        self.cursor = node;
        // The gesture's derived nodes are placed by the caller as ordinary nodes; keeping them here
        // as well would preview them a second time on top of the ones already placed.
        self.drag_rest.clear();
        let def = self.tool.def();
        if self.nodes.len() < def.clicks as usize {
            return None;
        }
        let mut kind = (def.make)(&self.nodes)?;
        moon_core::figures::apply_settings(&mut kind, &self.switches);
        Some(kind)
    }

    /// Transient preview figure drawn under the cursor.
    ///
    /// A live gesture previews through `make` rather than through `preview`: it already knows every
    /// node the figure will have, so showing the tool's partial shape would draw one thing and
    /// leave another behind.
    pub(super) fn preview(&self) -> Option<Figure> {
        let def = self.tool.def();
        let mut kind = if self.drag_rest.is_empty() {
            (def.preview)(&self.nodes, self.cursor)?
        } else {
            let mut all = Vec::with_capacity(self.nodes.len() + 1 + self.drag_rest.len());
            all.extend_from_slice(&self.nodes);
            all.push(self.cursor);
            all.extend_from_slice(&self.drag_rest);
            (def.make)(&all)?
        };
        moon_core::figures::apply_settings(&mut kind, &self.switches);
        Some(Figure::new(kind, self.style, 0.0))
    }
}

#[cfg(test)]
mod tests;
