//! User-defined chart figures (the drawing layer): the model, the per-tool registry, the store
//! and the primitives each tool emits.
//!
//! **One tool = one module** under [`tools`]. A tool owns its own data struct, its geometry, its
//! hit test and its handles. Everything generic about a figure — style, store, persistence,
//! dragging, handle picking — lives here and is shared by every tool, so adding a tool is a new
//! module, one [`FigureKind`] arm and one [`tools::REGISTRY`] row: no edit to the chart panel,
//! the renderer or the toolbar. Only two lists outside the layer still name tools one by one, and
//! both are meant to: the per-tool hotkey fields in `hotkeys.toml`, and the core's chart-object
//! types in [`crate::alert_blob`], which a tool the core does not know simply does not join.
//!
//! Rendering and input stay OUT of this crate, in both directions:
//! - a tool emits primitives into a [`sink::GeomSink`], which `moon-chart` implements over its
//!   GPU instance buffers, so a tool never names a GPU type;
//! - a tool hit-tests through [`proj::Proj`], which the UI implements over its pane mapping, so
//!   a tool never names a GPUI type and its math is unit-testable without a window.
//!
//! Both trait objects are called from mouse events and from the geometry rebuild that
//! `figures_sig` gates — never per frame per figure.
//!
//! Figures drawn in the terminal are local and persist in `figures.json`; only figures with the
//! "Alert" checkbox enabled are sent to the core as upserted `TChartObject` blobs, and only for
//! the tools the core knows ([`tools::ToolDef::alertable`]). Server-originated alert figures are
//! merged into the same store but are not persisted locally.

mod kind;
pub mod levels;
mod node;
pub mod proj;
pub mod sink;
mod store;
mod style;
pub mod tools;

pub use kind::FigureKind;
pub use node::FigNode;
pub use proj::Proj;
pub use sink::{BuildCtx, GeomSink, LabelPlace, LabelText, Stroke};
pub use store::{FigureKey, FigureStore};
pub use style::{DrawStyle, LineKind, DEFAULT_FILL_ALPHA};
pub use tools::{
    apply_settings, build_figure, drag_figure, pick_figure, pick_handle, settings_of, FigureTool,
    Grab, GrabMode, ToolDef, ToolSetting, ToolSettings, ToolShape,
};

use serde::{Deserialize, Serialize};

/// A single chart figure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Figure {
    /// Local ID, monotonically increasing within the store. For alerts, the same ID becomes
    /// `obj_uid` when upserted to the core.
    pub id: u64,
    pub kind: FigureKind,
    /// Line color in RGBA format.
    pub color: [u8; 4],
    /// Fill color in RGBA format; **zero alpha means the figure is not filled**.
    ///
    /// A figure saved before fills existed loads with none, and keeps looking exactly as it did.
    /// Giving it one would be a silent one-way rewrite of a file the user cannot edit back: there
    /// is no per-figure style editor yet, so a fill applied behind their back could not be taken
    /// off. A redrawn figure picks up the current style's fill like any other.
    #[serde(default)]
    pub fill: [u8; 4],
    /// Thickness in pixels before pixels-per-point scaling.
    pub thickness: f32,
    /// Line style (Solid/Dash/Dot/DashDot/DashDotDot), stored at blob offset 13.
    #[serde(default)]
    pub line_kind: LineKind,
    /// Creation time in Unix milliseconds, shown in the alert list's Time column.
    ///
    /// Fractional on purpose. The wire carries a Delphi `TDateTime`, whose resolution is finer than
    /// a millisecond, and this field is what a re-upsert writes back into it. Rounding it to whole
    /// milliseconds rewrote the creation instant of an object drawn in Moonbot on every drag —
    /// measured, byte for byte at `@22` — and Moonbot then DELETED the object about 200 ms later.
    /// A figure drawn here has no fraction to lose, so nothing is paid for keeping it.
    pub created_ms: f64,
    /// Whether the "Alert" checkbox is enabled and the figure is sent to the core as a chart alert.
    pub alert: bool,
    /// Associated strategy ID of the "Alerts" type; 0 means no strategy. Stored at blob offset 32.
    #[serde(default)]
    pub strategy_id: u64,
    /// Whether the figure is shown on this market for EVERY core rather than only for the core it
    /// was drawn on.
    ///
    /// The figure still belongs to its owning core's set — sharing only widens who sees it, so
    /// un-sharing cannot lose it. A shared figure cannot be an alert: an alert is upserted to one
    /// specific core, and there is no core to upsert to when the figure belongs to all of them.
    #[serde(default)]
    pub shared: bool,
    /// When this session last sent an upsert for this figure, in Unix milliseconds; zero when it
    /// sent none.
    ///
    /// Not persisted, and not a fact about the figure — a fact about a round trip in flight. The
    /// reconcile below clears an `alert` the core turns out not to hold, and between our upsert and
    /// the core's echo the core legitimately does not hold it yet. Measured on a live core, that
    /// window is ~1.3 s.
    #[serde(default, skip)]
    pub alert_sent_ms: f64,
    /// Whether the figure CAME FROM THE CORE (an alert drawn in Moonbot) and was decoded from
    /// a server blob rather than drawn locally. Such figures are NOT persisted because the
    /// server owns them, and they disappear when the server removes them. They can still be
    /// selected and moved, with edits re-upserted to the core. This field is not serialized,
    /// so figures loaded from disk are always local.
    #[serde(default, skip)]
    pub from_server: bool,
}

impl Figure {
    /// Builds a local figure of `kind` in the current drawing style.
    pub fn new(kind: FigureKind, style: DrawStyle, created_ms: f64) -> Self {
        Self {
            id: 0,
            kind,
            color: style.color,
            fill: style.fill,
            thickness: style.thickness,
            line_kind: style.kind,
            created_ms,
            alert: false,
            strategy_id: 0,
            shared: false,
            alert_sent_ms: 0.0,
            from_server: false,
        }
    }

    /// The tool that draws this figure.
    pub fn tool(&self) -> FigureTool {
        self.kind.tool()
    }

    /// This figure's style as one value.
    ///
    /// A figure stores the four style fields flat, because that is what it is persisted and drawn
    /// as; a surface that edits "a style" should not have to know which of a figure's fields carry
    /// it. Paired with [`Self::set_style`] so the mapping is written once and in one place.
    pub fn style(&self) -> DrawStyle {
        DrawStyle {
            color: self.color,
            thickness: self.thickness,
            kind: self.line_kind,
            fill: self.fill,
        }
    }

    /// Applies a style, returning whether anything changed. The return value is what the callers'
    /// edit paths use to decide whether to persist and re-upsert, so an unchanged style costs
    /// neither a save nor a round trip to the core.
    pub fn set_style(&mut self, style: DrawStyle) -> bool {
        if self.style() == style {
            return false;
        }
        self.color = style.color;
        self.thickness = style.thickness;
        self.line_kind = style.kind;
        self.fill = style.fill;
        true
    }

    /// Whether this figure can be armed as a core alert: the core must know the figure type, and a
    /// shared figure has no single core to arm it on.
    pub fn can_alert(&self) -> bool {
        !self.shared && self.tool().def().alertable
    }

    /// Whether this figure can be shared across cores: an armed alert belongs to its core, and a
    /// server-owned figure is not ours to widen.
    pub fn can_share(&self) -> bool {
        !self.alert && !self.from_server
    }
}

#[cfg(test)]
mod tests;
