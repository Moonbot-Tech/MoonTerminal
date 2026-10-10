//! Retained chart hit geometry, action state and label priorities.

use super::*;

#[derive(Clone, Copy, PartialEq)]
pub(in crate::chartdx) struct CursorState {
    pub(in crate::chartdx) pane: usize,
    pub(in crate::chartdx) local: [f32; 2],
}

/// A placed label after overlap avoidance stores logical position, alignment, and width so
/// `sync_readout_params` can build a translucent backing plate. `solid` selects a dense foreground
/// plate for cursor numbers instead of the light plate used by the market corner label.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct PlacedLabel {
    pub x: f32,
    pub y: f32,
    pub ax: f32,
    pub ay: f32,
    pub w: f32,
    pub h: f32,
    pub solid: bool,
}

/// One arbitrage venue name as it was drawn, and which venue it names.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ArbHit {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Protocol platform code, and the DEX name for a deployer.
    pub code: u8,
    pub dex: String,
    /// Whether a core is connected to this venue — whether the click has anywhere to go.
    pub reachable: bool,
}

impl ArbHit {
    /// Whether a point in the pane's own logical pixels lands on this name.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
}

/// Where one VOLUME module was drawn, and which module it is.
///
/// The whole block, not one caption: the right-click menu edits the module's period, and a reader
/// aiming at "the volumes" is aiming at the three lines together. Grown by the SAME box the backing
/// plate is grown with — see `CaptionBox` — so a click cannot answer for a rectangle the plate
/// never covered.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct VolumeHit {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Index of the module in the pane's caption configuration.
    pub row: usize,
}

impl VolumeHit {
    /// Whether a point in the pane's own logical pixels lands on this block.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
}

/// Where one pressable caption reserved its room, and what the control there is.
///
/// The seam between the two halves of a chart button. The caption pass owns WHERE it goes — the
/// band, the alignment, the order among the modules — and cannot draw a GPUI element; the panel
/// owns the control and cannot lay it out. So the pass publishes this rectangle, in the pane's own
/// logical pixels, and the panel puts the application's own button in it.
///
/// Per CAPTION, not per module: two buttons standing in one module is a shape the reader can build
/// — and the shipped pair does — and one rectangle for both would give them one control.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::chartdx) struct ActionPlacement {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// The caption's font size in logical pixels — what the rectangle was measured at.
    pub size: f32,
    /// What the button does and how long a ban it sets.
    pub mark: text::ActionMark,
    /// Which caption it was drawn from, by identity — see `ActionDraw`.
    pub row: usize,
    pub part: usize,
}

/// One chart button, ready for the panel to place.
///
/// [`ActionPlacement`] resolved: the caption's own label and the pane's market folded in, so the
/// panel builds a control out of this and reads nothing else.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartActionButton {
    /// Rectangle in the WINDOW's logical pixels — the space the caption layout reserved.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// What pressing it does. How long a ban runs is asked at the press.
    pub action: moon_core::config::ChartAction,
    /// Whether what it controls is currently ON: panic armed, a ban running.
    pub active: bool,
    /// Whether the workspace rail lets this window command the core.
    pub enabled: bool,
    /// The label the caption pass built and measured the rectangle against, and the size it was
    /// measured at: the control draws at that size, so the reader's caption-size step moves the
    /// words and the box together.
    pub label: String,
    pub size: f32,
    /// Which caption it came from, which is also what keeps its element id stable.
    pub row: usize,
    pub part: usize,
    /// The `(core, market)` the pane was DRAWN for.
    pub core: moon_core::session::CoreId,
    pub market: String,
    /// That market's `market_currency` — the identity the core's own favourites list is matched
    /// against. Empty until the catalogue has named the market.
    pub coin: String,
}

/// Which market buttons a chart's captions actually place.
///
/// Asked once per render and answered from the caption configuration, so each fact behind a button
/// is looked up only where one prints it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WantedActions {
    pub cancel_buy: bool,
    pub panic_sell: bool,
    pub temp_ban: bool,
    pub favorite: bool,
}

impl WantedActions {
    /// Whether this chart places any button at all.
    pub fn any(self) -> bool {
        self.cancel_buy || self.panic_sell || self.temp_ban || self.favorite
    }
}

/// What the TERMINAL knows about one pane's market buttons.
///
/// The panel-facing half of the button state: everything here is an answer the engine cannot give
/// itself — the workspace rail belongs to the window, the armed flag mixes the core's snapshot with
/// this terminal's optimistic override, and the temporary blacklist lives in the session store.
/// Whether the chart is live at all is NOT here: that one the engine answers, so no caller can
/// hand a finished trade a pressable button by forgetting to ask.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MarketActionState {
    /// Whether this window's workspace rail lets it command the pane's core right now.
    pub allowed: bool,
    /// Whether panic selling is armed on the pane's market.
    pub panic_armed: bool,
    /// When the core's temporary ban on that market runs out, Unix ms, or `None` for no ban.
    pub ban_until_ms: Option<i64>,
    /// Whether that market is marked on the core, or `None` while the core has not reported its
    /// configuration — see `ActionInputs::favorite` on why that is a third state.
    pub favorite: Option<bool>,
}

pub(crate) const ORDER_LABEL_NEUTRAL: u32 = u32::MAX;

// STATIC grid matches Moonbot's 60x10 divisions: it stays fixed while labels move.
// Ten horizontal bands make a percent ruler: a 20% scale gives 2% per band.
pub(crate) const GRID_N_VERT: f32 = 60.0;
pub(crate) const GRID_N_HORIZ: f32 = 10.0;

// Caption categories also determine the legacy pinned-edge stack order. On-screen secondary
// captions are arbitrated by interaction and distance; percentages and order numbers survive.
pub(crate) const PRIO_BUY: u8 = 10;
pub(crate) const PRIO_SELL_SIZE: u8 = 20;
pub(crate) const PRIO_SELL_PCT: u8 = 30;
pub(crate) const PRIO_STOP_PCT: u8 = 40;

/// Prepared order-line label (reference category E): text, line price on Y, placement above or below
/// the line, and line color. It is built while orders synchronize in `sync_orders_from_session`,
/// where `session` is available, and drawn by `prepare_text`.
#[derive(Clone)]
pub(crate) struct OrderLabel {
    /// Order identity shared by its chart and book captions.
    pub uid: u64,
    /// Line price converted to Y through `view` each frame.
    pub price: f32,
    pub text: String,
    /// `true` places the label above the line; `false` places it below.
    pub above: bool,
    /// Line color as `0xRRGGBB`, also used for the label.
    pub color: u32,
    /// Caption category and stable ordering for the existing pinned-edge stack.
    pub priority: u8,
    /// Whether a DRAG label must render on top without overlap suppression. Hover does not set it —
    /// hovering feeds `order_highlight`, and [`OrderLabel::highlighted`] is that half.
    pub force: bool,
    /// On-screen row arbitration preserves every caption of the hovered or dragged order.
    pub interacting: bool,
    /// Whether this label belongs to the order under the pointer.
    ///
    /// The pinned column keeps the caption: several exits pinned to one
    /// edge are thinned down to the nearest one's captions, and the painter puts the HIGHLIGHTED
    /// order's line on top of that pile — thinning its caption away would leave the highlighted
    /// line labelled with a stranger's numbers.
    pub highlighted: bool,
    /// Whether the label follows a line that is PINNED to the plot's edge when its price leaves the
    /// visible band, and must therefore be clamped the same way instead of dropped off screen.
    ///
    /// The text is unaffected: it states the order's real price, percentage and size wherever the
    /// line ended up, which is the whole point of pinning the drawing and nothing else.
    pub pinned: bool,
}

/// Prepared sell-depth caption with the same interaction identity as its chart captions.
#[derive(Clone)]
pub(crate) struct OrderBookLabel {
    /// Order identity used to preserve its own pair across the book edge.
    pub uid: u64,
    /// Dragging or hovering keeps this order's depth caption visible.
    pub interacting: bool,
    /// Sell-line price; the label is drawn in the orderbook zone at this Y.
    pub price: f32,
    pub short: bool,
    /// Cached whole-book notional for this sell-line depth label, recomputed when the order
    /// labels or the book revision change; text frames only format and draw it. `None` means the
    /// figure was never measured — no book for this market yet, or the book is switched off — and
    /// the label is not drawn at all, because a drawn `0` claims "no glass to clear".
    pub notional: Option<f32>,
}
