//! User-defined chart figures (a drawing layer similar to Moonbot's pencil tool):
//! horizontal lines, segments, triangles, and horizontal channels. Figures drawn in the
//! terminal are local and persist in `figures.json`; only figures with the "Alert" checkbox
//! enabled are sent to the core as upserted `TChartObject` blobs. Server-originated alert
//! figures are merged into the same store but are not persisted locally. Each set is keyed by
//! `(CoreId, market)`: as in Moonbot, a drawing belongs to a specific bot's chart.
//!
//! The model lives here in `moon-core` so the geometry builder in `moon-chart` and the UI layer
//! share the same types without depending on each other.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::config::{paths, write_file_atomic};
use crate::session::CoreId;

/// Figure node represented by a `(time, price)` point. Time is Unix milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FigNode {
    pub time_ms: f64,
    pub price: f64,
}

/// Figure type corresponding to Moonbot's `TChartObject` blob types: HLine=1, Segment=2,
/// Triangle=4, Channel=5 (Fibo=3 is not yet supported). A triangle has three vertices;
/// a Moonbot channel consists of TWO HORIZONTAL prices (a price corridor) without time values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FigureKind {
    /// Horizontal line at a price, extending indefinitely in time.
    HLine { price: f64 },
    /// Segment between two nodes.
    Segment { a: FigNode, b: FigNode },
    /// Triangle defined by three vertices (like Moonbot's triangle, type 4).
    Triangle { a: FigNode, b: FigNode, c: FigNode },
    /// Horizontal channel defined by two prices (like Moonbot's channel, type 5).
    Channel { price1: f64, price2: f64 },
}

impl FigureKind {
    /// Human-readable type name for alert lists and tooltips.
    pub fn label(&self) -> &'static str {
        match self {
            FigureKind::HLine { .. } => "Горизонталь",
            FigureKind::Segment { .. } => "Линия",
            FigureKind::Triangle { .. } => "Треугольник",
            FigureKind::Channel { .. } => "Канал",
        }
    }

    /// Reference price used by the alert list's Price column and sorting.
    pub fn anchor_price(&self) -> f64 {
        match self {
            FigureKind::HLine { price } => *price,
            FigureKind::Channel { price1, .. } => *price1,
            FigureKind::Segment { a, .. } | FigureKind::Triangle { a, .. } => a.price,
        }
    }
}

/// A single chart figure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Figure {
    /// Local ID, monotonically increasing within the store. For alerts, the same ID becomes
    /// `obj_uid` when upserted to the core.
    pub id: u64,
    pub kind: FigureKind,
    /// Line color in RGBA format.
    pub color: [u8; 4],
    /// Thickness in pixels before pixels-per-point scaling.
    pub thickness: f32,
    /// Line style (Solid/Dash/Dot/DashDot/DashDotDot), stored at blob offset 13.
    #[serde(default)]
    pub line_kind: LineKind,
    /// Creation time in Unix milliseconds, shown in the alert list's Time column.
    pub created_ms: i64,
    /// Whether the "Alert" checkbox is enabled and the figure is sent to the core as a chart alert.
    pub alert: bool,
    /// Associated strategy ID of the "Alerts" type; 0 means no strategy. Stored at blob offset 32.
    #[serde(default)]
    pub strategy_id: u64,
    /// Whether the figure CAME FROM THE CORE (an alert drawn in Moonbot) and was decoded from
    /// a server blob rather than drawn locally. Such figures are NOT persisted because the
    /// server owns them, and they disappear when the server removes them. They can still be
    /// selected and moved, with edits re-upserted to the core. This field is not serialized,
    /// so figures loaded from disk are always local.
    #[serde(default, skip)]
    pub from_server: bool,
}

/// Drawing-mode tool that determines which figure the pencil creates. This application-wide
/// selection is configured in the pencil popup and stored in the UI backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FigureTool {
    HLine,
    Segment,
    Triangle,
    Channel,
}

impl FigureTool {
    /// All tools in the order used by the figure-switching hotkey.
    pub const ALL: [FigureTool; 4] = [
        FigureTool::HLine,
        FigureTool::Segment,
        FigureTool::Triangle,
        FigureTool::Channel,
    ];

    /// Next tool in the cycle; the `switch_figure` hotkey advances HLine -> Segment -> ... -> HLine.
    pub fn next(self) -> FigureTool {
        let i = Self::ALL.iter().position(|&t| t == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// Line style corresponding to Moonbot's "Kind" and Delphi's `TPenStyle` at blob offset 13:
/// Solid=0, Dash=1, Dot=2, DashDot=3, DashDotDot=4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LineKind {
    Solid,
    #[default]
    Dash,
    Dot,
    DashDot,
    DashDotDot,
}

impl LineKind {
    pub const ALL: [LineKind; 5] = [
        LineKind::Solid,
        LineKind::Dash,
        LineKind::Dot,
        LineKind::DashDot,
        LineKind::DashDotDot,
    ];

    /// `TPenStyle` value stored at blob offset 13.
    pub fn to_pen(self) -> u32 {
        match self {
            LineKind::Solid => 0,
            LineKind::Dash => 1,
            LineKind::Dot => 2,
            LineKind::DashDot => 3,
            LineKind::DashDotDot => 4,
        }
    }

    pub fn from_pen(v: u32) -> Self {
        match v {
            0 => LineKind::Solid,
            2 => LineKind::Dot,
            3 => LineKind::DashDot,
            4 => LineKind::DashDotDot,
            _ => LineKind::Dash,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LineKind::Solid => "Solid",
            LineKind::Dash => "Dash",
            LineKind::Dot => "Dot",
            LineKind::DashDot => "DashDot",
            LineKind::DashDotDot => "DashDotDot",
        }
    }

    /// Whether the line is solid, used to map horizontal lines to `LineInstance.style` 0/1.
    pub fn is_solid(self) -> bool {
        self == LineKind::Solid
    }
}

/// Current drawing style (color, thickness, and line style), applied to NEW figures and edited
/// in the pencil popup. Stored in the UI backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawStyle {
    /// RGBA color; `a` is the "Opacity" value from the popup.
    pub color: [u8; 4],
    pub thickness: f32,
    pub kind: LineKind,
}

impl Default for DrawStyle {
    fn default() -> Self {
        // Light blue to distinguish figures from order lines, 1 px, dashed.
        Self {
            color: [64, 196, 255, 255],
            thickness: 1.0,
            kind: LineKind::Dash,
        }
    }
}

/// Figure-set key identifying the chart for a specific core and market.
pub type FigureKey = (CoreId, String);

/// Store for all chart figures and their persistence. It lives in the UI backend; interactive
/// edits go through store methods that increment `rev` to gate redraws and set `dirty` for a
/// debounced save by the coordination tick, like config and dock state.
#[derive(Debug, Default)]
pub struct FigureStore {
    /// All chart figures: persisted local figures (`from_server=false`) and server-managed
    /// Moonbot alerts (`from_server=true`). Both are selected, moved, and deleted identically.
    by_key: HashMap<FigureKey, Vec<Figure>>,
    next_id: u64,
    /// Incremented on every change so the chart data state knows to reload the set.
    rev: u64,
    /// Whether edits are awaiting a debounced save.
    pub dirty: bool,
}

impl FigureStore {
    pub fn rev(&self) -> u64 {
        self.rev
    }

    pub fn figures(&self, core: CoreId, market: &str) -> &[Figure] {
        self.by_key
            .get(&(core, market.to_string()))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Reconciles server alert figures created in the core or Moonbot into the shared set.
    /// `server` is a fresh complete set of `from_server` figures decoded from blobs. Local
    /// figures (`from_server=false`) are preserved, and an armed local figure is not duplicated
    /// when a server figure has the same ID. This is called only when the server set changes,
    /// as gated by its activity counter, so `rev` is incremented unconditionally.
    pub fn set_server_figures(&mut self, server: HashMap<FigureKey, Vec<Figure>>) {
        for figs in self.by_key.values_mut() {
            figs.retain(|f| !f.from_server);
        }
        for (key, figs) in server {
            let entry = self.by_key.entry(key).or_default();
            for f in figs {
                if entry.iter().any(|e| e.id == f.id && !e.from_server) {
                    continue;
                }
                entry.push(f);
            }
        }
        self.by_key.retain(|_, v| !v.is_empty());
        self.rev = self.rev.wrapping_add(1);
    }

    /// All alert figures for the Alerts window as `(core, market, &Figure)` tuples.
    pub fn all_alerts(&self) -> impl Iterator<Item = (CoreId, &str, &Figure)> + '_ {
        self.by_key.iter().flat_map(|((c, m), v)| {
            v.iter()
                .filter(|f| f.alert)
                .map(move |f| (*c, m.as_str(), f))
        })
    }

    pub fn get(&self, core: CoreId, market: &str, id: u64) -> Option<&Figure> {
        self.figures(core, market).iter().find(|f| f.id == id)
    }

    /// Adds a LOCAL figure and returns its ID.
    pub fn add(&mut self, core: CoreId, market: &str, mut fig: Figure) -> u64 {
        self.next_id += 1;
        fig.id = self.next_id;
        fig.from_server = false;
        let id = fig.id;
        self.by_key
            .entry((core, market.to_string()))
            .or_default()
            .push(fig);
        self.bump();
        id
    }

    /// Edits a figure in place while dragging a node or body. `edit` returns true if it changed anything.
    pub fn edit(
        &mut self,
        core: CoreId,
        market: &str,
        id: u64,
        edit: impl FnOnce(&mut Figure) -> bool,
    ) -> bool {
        let Some(fig) = self
            .by_key
            .get_mut(&(core, market.to_string()))
            .and_then(|v| v.iter_mut().find(|f| f.id == id))
        else {
            return false;
        };
        if edit(fig) {
            self.bump();
            true
        } else {
            false
        }
    }

    pub fn remove(&mut self, core: CoreId, market: &str, id: u64) -> Option<Figure> {
        let list = self.by_key.get_mut(&(core, market.to_string()))?;
        let idx = list.iter().position(|f| f.id == id)?;
        let fig = list.remove(idx);
        if list.is_empty() {
            self.by_key.remove(&(core, market.to_string()));
        }
        self.bump();
        Some(fig)
    }

    /// Removes all figures from a chart for the tool's Clear All action.
    pub fn clear(&mut self, core: CoreId, market: &str) -> usize {
        let n = self
            .by_key
            .remove(&(core, market.to_string()))
            .map(|v| v.len())
            .unwrap_or(0);
        if n > 0 {
            self.bump();
        }
        n
    }

    fn bump(&mut self) {
        self.rev = self.rev.wrapping_add(1);
        self.dirty = true;
    }

    // Persistence

    /// Highest core uid this store still holds figures for.
    ///
    /// Feeds the durable uid high-water mark: figures are keyed by `(CoreId, market)`, so a
    /// reissued uid would surface a deleted core's figures on the new core — and any edit or
    /// alert toggle made there would then target the new core.
    pub fn max_core_uid(&self) -> Option<u64> {
        self.by_key.keys().map(|(core, _)| *core).max()
    }

    /// Loads from `figures.json`; a missing or malformed file produces an empty store.
    pub fn load() -> Self {
        let path = paths::figures_path();
        let by_key: Vec<PersistEntry> = match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                log::warn!("figures.json битый ({e}) → без фигур");
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        let mut store = Self::default();
        for e in by_key {
            let max_id = e.figures.iter().map(|f| f.id).max().unwrap_or(0);
            store.next_id = store.next_id.max(max_id);
            store.by_key.insert((e.core, e.market), e.figures);
        }
        store
    }

    /// Saves to `figures.json` non-fatally and clears `dirty`. Server alerts (`from_server`)
    /// are NOT persisted because they are managed by the core.
    pub fn save(&mut self) {
        let list: Vec<PersistEntry> = self
            .by_key
            .iter()
            .filter_map(|((core, market), figures)| {
                let figures: Vec<Figure> =
                    figures.iter().filter(|f| !f.from_server).cloned().collect();
                (!figures.is_empty()).then(|| PersistEntry {
                    core: *core,
                    market: market.clone(),
                    figures,
                })
            })
            .collect();
        match serde_json::to_string_pretty(&list) {
            Ok(s) => {
                if let Err(e) =
                    write_file_atomic(&paths::figures_path(), s.as_bytes(), "figures.json")
                {
                    log::warn!("не записал figures.json: {e}");
                }
            }
            Err(e) => log::warn!("не сериализовал figures.json: {e}"),
        }
        self.dirty = false;
    }
}

/// Flat serialization entry used because JSON cannot represent a `HashMap` with tuple keys.
#[derive(Serialize, Deserialize)]
struct PersistEntry {
    core: CoreId,
    market: String,
    figures: Vec<Figure>,
}
