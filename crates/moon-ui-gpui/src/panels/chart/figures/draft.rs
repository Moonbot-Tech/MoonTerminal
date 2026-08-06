//! The figure being drawn: the nodes placed so far, the cursor preview, and completion.
//!
//! Tool-agnostic. How many clicks a tool takes, what it builds from them and what it shows before
//! the last one are read from its registry row (`ToolDef::clicks` / `make` / `preview`), so a new
//! tool draws here without a line of code.

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
        }
    }

    /// Whether the draft belongs to this pane, chart and tool. A click anywhere else — another
    /// pane, another market, or after switching tools — abandons it.
    pub(super) fn belongs_to(
        &self,
        pane: usize,
        core: CoreId,
        market: &str,
        tool: FigureTool,
    ) -> bool {
        self.pane == pane && self.core == core && self.market == market && self.tool == tool
    }

    /// Adds a node. Returns the finished figure kind once the tool's click count is reached, in
    /// which case the caller drops the draft.
    pub(super) fn place(&mut self, node: FigNode) -> Option<FigureKind> {
        self.nodes.push(node);
        self.cursor = node;
        let def = self.tool.def();
        if self.nodes.len() < def.clicks as usize {
            return None;
        }
        let mut kind = (def.make)(&self.nodes)?;
        moon_core::figures::apply_settings(&mut kind, &self.switches);
        Some(kind)
    }

    /// Transient preview figure drawn under the cursor.
    pub(super) fn preview(&self) -> Option<Figure> {
        let mut kind = (self.tool.def().preview)(&self.nodes, self.cursor)?;
        moon_core::figures::apply_settings(&mut kind, &self.switches);
        Some(Figure::new(kind, self.style, 0.0))
    }
}
