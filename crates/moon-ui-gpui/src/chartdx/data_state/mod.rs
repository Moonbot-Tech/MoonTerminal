//! Per-pane chart data preparation in `ChartDataState`: the main non-drawing preparation reads
//! history, order books, and orders; uploads GPU buffers; applies automatic Y scaling; and tracks
//! change signatures. Owns the shared data state and resolved closed-trade caption inputs.

use super::*;

pub(in crate::chartdx) struct ChartDataState {
    pub(in crate::chartdx) container: Rc<RefCell<Container>>,
    pub(in crate::chartdx) render: Rc<RefCell<RenderState>>,
    pub(in crate::chartdx) theme: ChartTheme,
    /// App-wide order-book width in physical pixels.
    pub(in crate::chartdx) order_book_width_px: f32,
    pub(in crate::chartdx) orders: OrdersStyle,
    pub(in crate::chartdx) follow: bool,
    pub(in crate::chartdx) present_rate_hz: f32,
    pub(in crate::chartdx) w: u32,
    pub(in crate::chartdx) h: u32,
    pub(in crate::chartdx) origin: (f32, f32),
    pub(in crate::chartdx) scene_visible: bool,
    /// Whether to show the per-window or panel order book. Disabled sets `glass_w=0`, skips level
    /// construction, and hides the label. Applied to every panel in this engine.
    pub(in crate::chartdx) orderbook_enabled: bool,
    /// Order-book-only mode from the comparison broom button: hide chart and price axis and use the
    /// full width for the order book. Applied to every panel in follower engines.
    pub(in crate::chartdx) orderbook_only: bool,
    /// Per-window price-axis position (`Left`, `Right`, or `Hide`) controlling gutter layout and
    /// label side. Defaults to Left, the historical left gutter.
    pub(in crate::chartdx) price_axis_pos: crate::persistence::chart_persist::PriceAxisPos,
    /// Whether the per-window time axis, bottom labels, and gutter are visible. Disabled lets the
    /// plot fill the full height. Enabled by default.
    pub(in crate::chartdx) time_axis_visible: bool,
    /// Whether this engine's panes may show the horizontal-volume zone at all, whatever the tab
    /// configured: a follower of an active comparison lock does not, and the panel decides that
    /// from its role. On by default.
    pub(in crate::chartdx) hvol_allowed: bool,
    /// Whether the panel draws the comparison lock on every pane of this engine — the panel's own
    /// `compare_eligible`. Panel-wide by construction, not per-pane. Read by the caption pass so the
    /// top-left captions clear the pin/lock/broom strip.
    pub(in crate::chartdx) compare_lock_shown: bool,
    /// Effective candle and trade rendering settings for time frame, mode, and zone, applied to all
    /// engine panels. They may be a per-tab override or the `layout.candle_view` fallback.
    pub(in crate::chartdx) candle_view: moon_core::market::CandleViewCfg,
    /// Effective chart graphics settings: trade-history arrow size, connector thickness, and which
    /// order lines are drawn. Like `candle_view` above, a per-tab override or the
    /// `layout.chart_graphics` fallback.
    pub(in crate::chartdx) chart_graphics: moon_core::config::ChartGraphicsCfg,
    /// Effective chart caption configuration: which figures print beside the plot, in which corner
    /// and style. A per-tab override or the `layout.chart_labels` fallback, like the two above.
    /// Shared with the render mirror through an `Rc`; see the field there.
    pub(in crate::chartdx) chart_labels: Rc<moon_core::config::ChartLabelsCfg>,
    /// The closed trade this engine draws, when it was handed one; see the render mirror.
    pub(in crate::chartdx) trade_labels: Option<Rc<TradeLabels>>,
    /// Whether this engine is a HISTORICAL viewer: its subject is a closed interval, not `now`.
    ///
    /// The ONE home of that fact — the caption gates read it here, and so does `set_follow`
    /// through the engine, because an engine is `Clone` over these shared handles and a second
    /// copy of the flag on the clone could disagree with this one.
    ///
    /// It answers from the moment the window is CONSTRUCTED, which is what the gates need: the
    /// replay lands seconds later, and a gate that waited for it would print a few seconds of live
    /// figures over an empty chart and then take them away.
    pub(in crate::chartdx) historical: bool,
    /// The GLOBAL arbitrage roster, shared with the render mirror the same way. Not a per-tab
    /// override: which venues matter and what colour they are is one answer for the whole terminal.
    pub(in crate::chartdx) arb_view: Rc<moon_core::config::ArbViewCfg>,
    /// Saved X scale in pixels per millisecond from Shift+middle-click sync. NEW panels start with it
    /// instead of the built-in time-window default; `None` uses that default.
    pub(in crate::chartdx) default_x_ppm: Option<f32>,
    /// Prospective selected F1-F6 manual order size in USD for the cursor crosshair label.
    /// `ChartPanel::render`, which has Backend access, sets it. `None` means no size or rate.
    pub(in crate::chartdx) prospective_usd: Option<f64>,
    /// Interactive order-line hover or drag highlight. It does not change market data and only
    /// triggers an infrequent userdata rebuild when the UID changes.
    pub(in crate::chartdx) order_highlight: Option<(CoreId, u64)>,
    /// Local line-price preview during drag; the command reaches the core only on mouse-up.
    pub(in crate::chartdx) order_drag_preview: Option<(CoreId, u64, LineKind, f32)>,
    /// Shared user-figure store from Backend `Rc`; see `figures_sync`.
    pub(in crate::chartdx) figures:
        Option<std::rc::Rc<std::cell::RefCell<moon_core::figures::FigureStore>>>,
    /// This panel's figure interaction state for drawing preview, hover, and selection plus its revision.
    pub(in crate::chartdx) figure_visual: figures_sync::FigureVisual,
    pub(in crate::chartdx) figure_visual_rev: u64,
    /// This panel's news marks (tag-coloured gems on the plot's bottom edge) plus their revision;
    /// see `news_sync`. Shared with the panel, which hit-tests the same list.
    pub(in crate::chartdx) news_marks: std::rc::Rc<Vec<moon_chart::news_marks::NewsMark>>,
    /// Index of the mark under the cursor, drawn grown from the axis.
    pub(in crate::chartdx) news_hovered: Option<usize>,
    /// Durable closed trades for this exact Main chart target.
    pub(in crate::chartdx) trade_history: std::rc::Rc<Vec<moon_core::db::ChartTradeRecord>>,
    /// Cores the panel admitted for `trade_history`, or `None` when no set was handed over.
    ///
    /// `None` draws own-core only: no history load has published a set, the target was cleared, or
    /// this is the frozen Trade window, which publishes records and never hands a set over. A chart
    /// load stores `Some` even when the flag is off and the set is only the owner.
    pub(in crate::chartdx) trade_history_cores:
        Option<std::rc::Rc<trade_history_sync::TradeHistoryCores>>,
    /// The time axis this engine's replicated closed-trade stamps are corrected on.
    ///
    /// The replica stores `buydate`/`closedate` on the CORE's own wall clock, while the chart
    /// epoch and its candles are true UTC, so every stamp is lifted through this axis before it
    /// becomes a chart millisecond. A core with no measurement converts as the identity. See
    /// `moon_core::db::report_axis`; this axis carries only the CURRENT segment per core
    /// (`Backend::report_axis`'s documented limitation, `backend/mod.rs:1803-1821`).
    pub(in crate::chartdx) report_axis: moon_core::db::ReportAxis,
    /// Revision incremented whenever the durable history set changes.
    pub(in crate::chartdx) trade_history_revision: u64,
    /// Archived order lines of the closed trades in `trade_history`, by `ReportUID`: what the
    /// "Moonbot lines" style draws in place of the arrows. Handed in by the panel from the trace
    /// resolver (`backend::traces`); this engine never asks for them itself. See `archived_lines`.
    pub(in crate::chartdx) archived_lines:
        Rc<HashMap<i64, std::sync::Arc<[moon_core::feed::ArchivedOrderTrace]>>>,
    /// Advances on every `set_archived_lines`; folded into the order signature and the per-pane
    /// gate so a new answer rebuilds the userdata buffer exactly like a live order change.
    pub(in crate::chartdx) archived_lines_rev: u64,
    /// The trade arrow under the cursor as `(pane, mark index in that pane, buy)`. It is drawn
    /// grown and fully opaque.
    ///
    /// Qualified by PANE because every pane draws only its own core's trades. Identified by an
    /// ACTION — mark plus direction — rather than by cluster, because clusters are renumbered by
    /// every rebuild and a bare mark names a whole trade rather than one of its two ends.
    pub(in crate::chartdx) trade_hovered: Option<(usize, usize, bool)>,
    /// This panel's warning badges (amber gems on the plot's bottom edge); see `warn_sync`. Shared
    /// with the panel, which hit-tests the same list.
    pub(in crate::chartdx) warn_marks: std::rc::Rc<Vec<moon_chart::news_marks::NewsMark>>,
    /// Index of the warning badge under the cursor.
    pub(in crate::chartdx) warn_hovered: Option<usize>,
    pub(in crate::chartdx) market_source: Option<MarketDataSource>,
    /// Frozen market history this engine draws INSTEAD of the live source, when it has one.
    ///
    /// Set only by the trade window, which owns its own engine. While it is `Some`, the history
    /// read below is answered from these rows and the live source is never consulted — so a replay
    /// cannot reach the user's main chart even by mistake: that engine's field is `None` and there
    /// is no shared key either could collide on. Contrast `moon_core::fixture`, whose bench state
    /// is process-wide by design.
    pub(in crate::chartdx) trade_replay:
        Option<Rc<moon_core::market::trade_replay::TradeReplaySeries>>,
    /// The archived order lines of the trade a frozen viewer shows, drawn INSTEAD of the session's
    /// live order store — which that viewer empties, since it holds what is open right now.
    ///
    /// Same ownership as `trade_replay`: only the trade window sets it, on its own engine, and a
    /// live chart's field stays `None`. Built by `OrderLineStore::archived`, so the order sync
    /// reads it exactly as it reads a live store and the chart draws it through the same geometry.
    pub(in crate::chartdx) frozen_orders:
        Option<Rc<moon_core::session::order_lines::OrderLineStore>>,
    /// The price band the trade window asks the auto-Y fit to include beside the visible prices
    /// — the trade's own lines, and its shown neighbours' — or `None` to fit the prices alone.
    ///
    /// Set with `frozen_orders`, by the same owner: a live chart takes this band from the
    /// session store's open orders (`auto_fit_range`), which an archived store never has.
    pub(in crate::chartdx) frozen_fit_range: Option<(f32, f32)>,
    /// What a frozen viewer draws beside its store — the entry corridor, modelled trades — and
    /// the revision each hand-over stamps, which the order sync is gated on. Same ownership as
    /// `frozen_orders`; drawn in either trade style, since neither is an order of the trade.
    pub(in crate::chartdx) frozen_overlay: Option<Rc<moon_chart::frozen_overlay::FrozenOverlay>>,
    pub(in crate::chartdx) frozen_overlay_rev: u64,
    pub(in crate::chartdx) last_frame_tick_at: Option<Instant>,
    pub(in crate::chartdx) present_rate_candidate_hz: f32,
    pub(in crate::chartdx) present_rate_candidate_hits: u8,
    /// Device pixels per chart-design pixel: the platform factor, excluding UI zoom. Every size
    /// the chart draws — line widths, candle outlines, axis gutters, caption text — goes through
    /// it, which is what keeps the chart at the monitor's density while the interface zooms.
    pub(in crate::chartdx) last_ppp: f32,
    /// The window's content zoom at the last frame: content pixels times this are chart-design
    /// pixels. Read by the overlays that place GPUI elements over chart geometry.
    pub(in crate::chartdx) content_zoom: f32,
    pub(in crate::chartdx) slot_bounds: Option<Bounds<Pixels>>,
    pub(in crate::chartdx) last_order_sig: u64,
    pub(in crate::chartdx) last_prepared_market_sig: u64,
    pub(in crate::chartdx) last_source_market_sig: u64,
    /// When the countdown clock was last consulted, for the throttle in `tick_countdown_captions`.
    ///
    /// Monotonic rather than the wall clock this feature is about: it paces a CHECK, and a check
    /// paced by a clock the user can move backwards would stop happening.
    pub(in crate::chartdx) last_countdown_check: Option<Instant>,
    pub(in crate::chartdx) view_dirty: bool,
}

/// What ONE closed trade was, in the form the captions print it.
///
/// STRINGS, already resolved, because the two halves of the answer live in different places: the
/// detect line and the exit reason come from the report replica, while the strategy has to be
/// NAMED through the session's strategy store — which this layer has no access to and no business
/// reaching into. The window resolves both and hands the result down, exactly as it hands down the
/// frozen series beside it.
///
/// Compared by value: it is part of the caption cache key, and the window replaces the whole handle
/// rather than mutating it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TradeLabels {
    /// Strategy that opened the trade, already named, or empty when it cannot be named.
    pub(crate) strategy: String,
    /// The detect line the trade fired on, with the core's diagnostic tail already dropped.
    pub(crate) detect: String,
    /// Why the position closed, as the core stated it.
    pub(crate) sell_reason: String,
}

// Responsibilities are split without logic changes: state handles lifecycle, signatures, and
// frames; orders synchronizes orders and line labels; market synchronizes history, order books,
// and automatic Y scaling.
mod market;
pub(crate) mod orders;
mod state;
