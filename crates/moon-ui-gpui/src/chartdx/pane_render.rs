//! Per-pane retained GPU state and camera lifecycle.

use super::*;

/// GPU state for one panel's `gpu_canvas` callbacks, separate from `Container` logic and
/// synchronized in `prepare` by index plus `(core, market)` identity.
pub(in crate::chartdx) struct PaneRender {
    pub(in crate::chartdx) core: Option<CoreId>,
    pub(in crate::chartdx) market: String,
    /// Core name for the chart corner label, resolved from `SessionManager` during order sync.
    pub(in crate::chartdx) core_name: String,
    /// Admitted-core count for that same caption, or `None` when this pane does not name a
    /// widened set.
    ///
    /// `Some(n)` means this pane owns the history request and `n` cores were admitted. A foreign
    /// trade draws only on that owner pane (`pane_admits_record`); a follower keeps `None` and its
    /// own name. The caption string is built from this in `refresh_pane_labels`, not here.
    pub(in crate::chartdx) all_cores_count: Option<usize>,
    /// Ticker for that same caption (`BEAT-USDT`), resolved from the core's catalog in
    /// `sync_from_market_source` and cached here.
    ///
    /// Cached rather than derived while drawing: the caption is drawn every frame, and resolving
    /// it takes the market-source lock and reads a snapshot. Deriving it from `market` alone
    /// cannot name a Hyperliquid spot index (`@156`) or tell two COIN-M expiries apart.
    pub(in crate::chartdx) ticker: String,
    /// Provider+generation+meta key the cached `ticker` was resolved at; see the retry in
    /// `sync_from_market_source`.
    pub(in crate::chartdx) ticker_catalog_key: u64,
    /// Whether `ticker` has been resolved at all. An empty string cannot say this: a market with
    /// no label resolves to one, and the pane would take the source lock again every sync.
    pub(in crate::chartdx) ticker_resolved: bool,
    /// Current Y-scale badge to the left of the corner label, as a whole percentage of visible range
    /// relative to price. `None` while there is no price to measure against. Computed
    /// by `sync_from_market_source` from the panel's logical `ChartView`.
    pub(in crate::chartdx) scale_badge: Option<i32>,
    /// Current X-scale badge: the whole seconds the plot spans, from [`time_scale_secs`]. Computed
    /// by `sync_from_market_source` beside [`Self::scale_badge`].
    pub(in crate::chartdx) time_scale_s: Option<i64>,
    /// Finished translucent plate under the corner caption, in DEVICE pixels `[x, y, w, h]`.
    ///
    /// Computed by `prepare_text`, which owns the caption's geometry, and drawn verbatim by
    /// `sync_readout_params`. A zero height means no caption is drawn this frame. This is the ONE
    /// caption's plates, one per column: the coin's and, when the two lines split around the order
    /// book's left edge, the core name's. Measuring each drawn row separately is what let the plate
    /// drift away from the text it sits under; a single plate spanning both columns would instead
    /// darken the candles lying between them.
    pub(in crate::chartdx) caption_plates: [[f32; 4]; text::CAPTION_PLATES],
    /// Build buffer for the click boxes, taken and returned like the bars' scratch beside it: the
    /// boxes are grown per line and converted once, and a fresh vector per pane per frame is churn
    /// on the present path.
    pub(crate) volume_boxes: Vec<(usize, text::CaptionBox)>,
    /// Build buffer for the bars above, so a pass that changes nothing costs no allocation.
    ///
    /// A SECOND buffer rather than taking the published one: the published bars have to survive the
    /// pass to be compared against what it produced, and taking them left the comparison against an
    /// empty vector — which reported a change on every frame and re-published the readout batch
    /// forever.
    pub(crate) caption_bars_scratch: Vec<text::CaptionBar>,
    /// Where each volume module was drawn, in the pane's own LOGICAL pixels.
    ///
    /// Rebuilt every frame beside [`Self::arb_hits`], and for the same reason: a right-click has to
    /// hit what the last frame actually drew.
    pub(crate) volume_hits: Vec<VolumeHit>,
    /// Measured strategy-filter header targets from the last successful caption pass.
    pub(in crate::chartdx) filter_header_hits: Vec<FilterHeaderHit>,
    /// Where each scrollable label column was drawn, rebuilt with [`Self::filter_header_hits`].
    pub(in crate::chartdx) column_bands: Vec<ColumnBand>,
    /// The plot's left strip where the wheel scrolls the expanded strategy-filter column.
    pub(in crate::chartdx) filter_strip: Option<ColumnBand>,
    /// Where each pressable caption reserved its room, on the same terms as [`Self::volume_hits`]:
    /// rebuilt every frame, because the panel places a control at what the LAST frame laid out.
    pub(crate) action_rects: Vec<ActionPlacement>,
    /// Build buffer for that list, kept so a chart with buttons allocates nothing per frame — the
    /// same take-and-return the caption bars use.
    pub(crate) action_draws: Vec<text::ActionDraw>,
    /// Buy/sell proportion bars this pane's captions published, in DEVICE pixels.
    ///
    /// Beside [`Self::caption_plates`] and drawn from the same batch: `prepare_text` owns the
    /// geometry — a bar is placed from the same measurement as the figure beside it — and
    /// `sync_readout_params` turns each into two rectangles. Empty on a chart printing no volume.
    pub(crate) caption_bars: Vec<text::CaptionBar>,
    /// Captions this pane resolved from the configuration, and the inputs they were built from.
    pub(in crate::chartdx) labels: text::LabelState,
    /// Pristine caption rows; each draw mutates a separate working clone.
    pub(in crate::chartdx) caption_rows: Option<text::RowPlanCache>,
    /// Venue label for the pane's core, resolved during order sync beside `core_name`.
    pub(in crate::chartdx) venue: String,
    /// Quote currency of this pane's market, resolved with the ticker from the same label.
    pub(in crate::chartdx) quote: String,
    /// `market_currency` of this pane's market — the CORE's own name for the coin, resolved from
    /// the same label as the ticker beside it.
    ///
    /// Kept apart from that ticker because it is an IDENTITY rather than a caption: the core's own
    /// lists (its favourites, its permanent blacklist) are matched against this exact string, and
    /// deriving it by trimming a market name is what the core's team asked us not to do — a
    /// contract tail, a dex prefix or a multiplier makes that derivation wrong.
    pub(in crate::chartdx) coin: String,
    /// Whether [`Self::labels`] was last built with a shot's caption substitution in force.
    ///
    /// The shot's proof is about what a FRAME drew, and `refresh_pane_labels` runs on the sync
    /// paths rather than the frame path, so a presented frame can still be showing captions built
    /// before the substitution landed. This flag is what lets `prepare_text` tell those two frames
    /// apart; without it the proof would count a frame that still names the user's own core.
    pub(in crate::chartdx) labels_shot_substituted: bool,
    /// Strategy name of the newest open order on this market, from the same sync.
    pub(in crate::chartdx) label_strategy: String,
    /// Strategy and line of the newest detect this pane's core fired on this market, from the same
    /// sync. Replaced only by the next detect on that market — see `LabelInputs::detect_strategy`.
    pub(in crate::chartdx) label_detect_strategy: String,
    pub(in crate::chartdx) label_detect_msg: String,
    /// Core-built strategy-filter skip lines for this pane's market.
    pub(in crate::chartdx) filter_lines: Vec<String>,
    /// First visible line of each scrolled label column, as `(label row, first)`.
    ///
    /// Transient: never saved, and gone with the rest of this struct when the pane switches core or
    /// market (the reset in `data_state/orders.rs`).
    pub(in crate::chartdx) label_scroll: Vec<(usize, u32)>,
    /// Open-position figures per basis, from the same sync.
    pub(in crate::chartdx) label_basis: [text::BasisStats; 3],
    /// Signed one-hour and 24-hour changes, refreshed with the market snapshot.
    pub(in crate::chartdx) delta_1h: Option<f64>,
    pub(in crate::chartdx) delta_24h: Option<f64>,
    /// Exchange and BTC background movement plus funding, refreshed with the same snapshot and
    /// only while a caption asks for any of it.
    pub(in crate::chartdx) label_context: Option<moon_core::market::MarketContextReadout>,
    /// Quote side, venue caps, coin tags and the exchange's own position, on the same terms as
    /// [`Self::label_context`]: refreshed with the market snapshot, and only while a caption asks.
    pub(in crate::chartdx) label_figures: Option<moon_core::market::MarketFiguresReadout>,
    /// Retained-history movement per window, gated separately because it costs more.
    pub(in crate::chartdx) label_windows: Option<moon_core::market::MarketWindowsReadout>,
    /// What was liquidated over the same periods, for the captions that print it.
    pub(in crate::chartdx) label_liquidations: Vec<(
        (moon_core::market::VolumeSpan, moon_core::market::VolumeAt),
        moon_core::market::LiqSpanReadout,
    )>,
    /// The moment the pointer is on, QUANTIZED, or `None` while it is off this pane.
    ///
    /// Quantized where it is collected rather than where it is read: it is part of the caption
    /// cache key, and an unrounded value would make every pixel of mouse travel a new one.
    pub(in crate::chartdx) label_cursor_ms: Option<i64>,
    /// Traded amounts, one entry per distinct span this pane's captions ask for.
    ///
    /// Read on the same throttle as the arbitrage column beside it: a span longer than the
    /// protocol's own rolling buckets is answered by walking retained rows, and a volume figure is
    /// read by eye. Empty while no caption prints one.
    pub(in crate::chartdx) label_volumes: Vec<(
        (moon_core::market::VolumeSpan, moon_core::market::VolumeAt),
        moon_core::market::VolumeSpanReadout,
    )>,
    /// When they were last read, in Unix milliseconds. Zero means never.
    pub(in crate::chartdx) label_volume_read_ms: i64,
    /// Market those amounts were read for; a pane that just switched coins reads again at once.
    pub(in crate::chartdx) label_volume_market: String,
    /// Spans they were read for.
    ///
    /// Compared as well as the market, and for the same reason: the right-click menu changes the
    /// period, and waiting out the throttle for the new one would blank the block for a quarter of
    /// a second on every pick — the figures are addressed BY span, so the old set answers nothing.
    pub(in crate::chartdx) label_volume_spans:
        Vec<(moon_core::market::VolumeSpan, moon_core::market::VolumeAt)>,
    /// Venues this terminal is connected to, refreshed with the session sync that fills the order
    /// figures beside it. Empty until a caption asks for the column.
    pub(in crate::chartdx) label_arb_reachable: Vec<(u8, String)>,
    /// Where each arbitrage venue NAME was drawn, in the pane's own logical pixels.
    ///
    /// Rebuilt by the caption pass on every presented frame and reused in place, because a click
    /// has to hit what the LAST frame actually drew: the column moves with the pane, and a stale
    /// rectangle would open the wrong exchange.
    pub(crate) arb_hits: Vec<ArbHit>,
    /// Arbitrage quotes for this pane's market, refreshed on a throttle rather than per revision:
    /// the protocol only hands them over one venue at a time, each behind the market lock.
    pub(in crate::chartdx) label_arb: Vec<moon_core::market::ArbQuote>,
    /// When they were last read, in Unix milliseconds. Zero means never.
    pub(in crate::chartdx) label_arb_read_ms: i64,
    /// Market those quotes were read for.
    ///
    /// Carried because the THROTTLE outlives a retarget: a pane switched to another coin would
    /// otherwise keep printing the previous one's arbitrage prices until the quarter-second was up.
    /// Every other readout here follows the new market on the revision that changed it.
    pub(in crate::chartdx) label_arb_market: String,
    /// Wall clock the funding countdown is measured against, QUANTIZED TO THE MINUTE.
    ///
    /// Quantized because it is part of the caption cache key: the raw clock would differ on every
    /// revision and re-format a countdown that prints the same minute either way. Zero while no
    /// countdown is configured, so an unused clock cannot wake anything.
    pub(in crate::chartdx) label_now_ms: i64,
    /// What this pane's market buttons state, as the panel last pushed it.
    ///
    /// Pushed rather than read here: whether panic is armed and whether the workspace rail allows
    /// a command are the terminal's own answers, and the engine has neither the backend nor the
    /// window group to ask.
    pub(crate) label_actions: text::ActionInputs,
    pub(in crate::chartdx) view: ChartViewGpu,
    pub(in crate::chartdx) layers: PlatformLayers,
    pub(in crate::chartdx) background_params: BackgroundParams,
    pub(in crate::chartdx) grid_params: GridParams,
    pub(in crate::chartdx) cursor_params: CursorParams,
    pub(in crate::chartdx) readout_rects: Vec<ReadoutRect>,
    pub(in crate::chartdx) readout_time_width: f32,
    pub(in crate::chartdx) readout_time_line_h: f32,
    pub(in crate::chartdx) readout_price_width: f32,
    pub(in crate::chartdx) readout_price_line_h: f32,
    pub(in crate::chartdx) history_cursor: ChartHistoryCursor,
    pub(in crate::chartdx) history_buffers: ChartHistoryBuffers,
    /// Last source slice signature used to decide if retained chart history must be read.
    pub(in crate::chartdx) source_history_sig: u64,
    /// Last provider generation seen by this pane. Changed generation means source replacement.
    pub(in crate::chartdx) source_generation: u64,
    /// Last chart-archive revision seen by this pane.
    ///
    /// A change means the core's archive was merged into the retained rings, prepending rows OLDER
    /// than this pane's cursors. Those rows are unreachable by an incremental drain, so the pane
    /// answers with a full history reset rather than the usual wake.
    pub(in crate::chartdx) source_archive: u64,
    pub(in crate::chartdx) cross_upload: Vec<ChartCross>,
    /// LIQUIDATION trade-cross upload buffer using `side=2` in the same combo ring.
    pub(in crate::chartdx) liq_upload: Vec<ChartCross>,
    /// The full uploaded last-price line; the non-DX11 backends and a capacity change re-send it
    /// whole.
    pub(in crate::chartdx) last_line_rows: Vec<PriceLinePoint>,
    /// The full uploaded mark-price line, kept like `last_line_rows`.
    pub(in crate::chartdx) mark_line_rows: Vec<PriceLinePoint>,
    /// Epoch both line mirrors were converted against; NaN before the first.
    pub(in crate::chartdx) price_line_epoch: f64,
    /// The whole composed candle list, converted for the GPU and patched in place by a tail read.
    pub(in crate::chartdx) candle_rows: Vec<CandleGpu>,
    /// Epoch `candle_rows` was converted against; a tail patch against another epoch is rejected.
    pub(in crate::chartdx) candle_rows_epoch: f64,
    /// A tail patch could not be applied; the next frame re-reads the history for a full list.
    pub(in crate::chartdx) candle_resync: bool,
    /// Candle candidates retained with the uploaded series for drawing-tool snapping.
    pub(in crate::chartdx) figure_snap: figure_snap::FigureSnapData,
    /// Last candle-series revision delivered to the GPU; `u64::MAX` means never delivered.
    pub(in crate::chartdx) last_candle_rev: u64,
    /// Applied candle-view config, stored already reduced to `CandleViewCfg::history_inputs` — the
    /// fields the history read consumes: time frame, mode, the trade-candle boundary and price-line
    /// visibility. A change in any of them resets history. Style-only fields are neutralized there
    /// and reach the separately cached GPU style, which also carries theme colors and fill alpha.
    pub(in crate::chartdx) applied_candle_cfg: moon_core::market::CandleViewCfg,
    /// Current-time time-frame bucket at the previous sync; movement shifts the trade zone and resets.
    pub(in crate::chartdx) last_zone_bucket: i64,
    /// Last candle style sent to the layer, compared before `set_candle_style`.
    pub(in crate::chartdx) candle_style: CandleStyleGpu,
    /// Last price-line style sent to the layer, compared before `set_price_style`.
    pub(in crate::chartdx) price_style: PriceStyleGpu,
    /// Trade-tick style retained across resource recreation and compared before rebaking.
    pub(in crate::chartdx) tick_style: TickStyleGpu,
    /// Last bottom-volume style sent to the layer, compared before `set_volume_style`.
    pub(in crate::chartdx) volume_style: VolumeStyleGpu,
    /// Retained per-candle volume samples for the visible-range max/average.
    ///
    /// A COPY on purpose: `history_buffers.candles` is cleared at the start of every read and
    /// refilled only when the series revision moved, so during a plain pan it is empty while
    /// the uploaded candle layer is still resident. Scaling the band from it would blank the
    /// band on exactly the gesture that should rescale it.
    pub(in crate::chartdx) volume_samples: Vec<moon_chart::VolumeSample>,
    /// Conservative upper bound on `tf_ms` among `volume_samples`, not an exact maximum.
    ///
    /// A full replacement scans every sample. A patch keeps this bound when it is wider
    /// than the new suffix, so a removed maximum may stay high until the next full
    /// replacement. Sorted band lookups only use it to widen the candidate window; the
    /// exact candle intersection still decides the result.
    pub(in crate::chartdx) volume_samples_max_tf: f64,
    /// Visible-range volume max and average behind the band, kept as SEMANTIC values.
    ///
    /// The numeric labels read these rather than inverting `VolumeStyleGpu.m`, whose fields are
    /// normalisation reciprocals and are deliberately quantized for cache stability.
    pub(in crate::chartdx) volume_stats: Option<moon_chart::VolumeStats>,
    /// Retained bought/sold samples behind the sides half of the band (`candle_volume_sides`),
    /// for its visible-range maximum, the split boundary and the cursor readout; empty while
    /// the switch is off.
    ///
    /// Retained for the same reason `volume_samples` is: the series is re-read only when the
    /// history moved, while a plain pan must still rescale the band from what is resident.
    pub(in crate::chartdx) side_samples: Vec<moon_core::market::SideVolumeBucket>,
    /// Reusable sides-layer upload buffer.
    pub(in crate::chartdx) side_upload: Vec<SideVolumeGpu>,
    /// Spare bucket buffer a re-read lands in, compared against `side_samples` before anything
    /// is shipped; swapped in when it differs, so neither read allocates.
    pub(in crate::chartdx) side_scratch: Vec<moon_core::market::SideVolumeBucket>,
    /// Rolling window the resident sides samples were summed over, milliseconds; `0` while the
    /// sides style is off. A different effective window — a zoom under `Auto`, or a popup pick —
    /// is a re-read even when the history did not move. The cursor readout names this window.
    pub(in crate::chartdx) side_tf_ms: i64,
    /// Sampling step of the resident sides samples, milliseconds (each sample's `tf_ms`); `0`
    /// while off. Follows the zoom, and moving it is a re-read for the same reason.
    pub(in crate::chartdx) side_step_ms: i64,
    /// Absolute `[from, to]` the resident sides samples were read for, unix milliseconds. The
    /// visible window leaving it is a re-read; `(MAX, MIN)` — nothing resident — makes the first
    /// visible window leave it at once.
    pub(in crate::chartdx) side_range: (i64, i64),
    /// Retained horizontal-volume BINS (turnover by price over the profile's window, at the tick
    /// or finer), the source of the samples below; empty while the zone is off.
    ///
    /// Retained for the reason `side_samples` is: the profile is re-read on the source's slow
    /// clock, while a pan or a price zoom must still resample from what is resident.
    pub(in crate::chartdx) hvol_rows: Vec<moon_core::market::PriceProfileRow>,
    /// The rolling sums the zone draws — one per device pixel of its height, over
    /// `hvol_price_window` of price around that pixel — resampled from `hvol_rows` whenever the
    /// bins, the window or the Y camera moved. What the readout and the maximum read.
    pub(in crate::chartdx) hvol_samples: Vec<moon_core::market::PriceProfileRow>,
    /// The grid the resident samples were taken on; a different one is a resample.
    pub(in crate::chartdx) hvol_grid: Option<moon_chart::hvol::SampleGrid>,
    /// Reusable horizontal-volume upload buffer.
    pub(in crate::chartdx) hvol_upload: Vec<HvolRowGpu>,
    /// Revision of the resident profile as the source stamped it; `0` while nothing is resident.
    /// Handed back on the next read so an unchanged profile is not even copied.
    pub(in crate::chartdx) hvol_rev: u64,
    /// The price the resident rows' width was taken as a percentage of; `0` for none yet. Moves
    /// only past `moon_chart::hvol::REF_PRICE_BAND`, so the rows are not re-binned per tick.
    pub(in crate::chartdx) hvol_ref_price: f64,
    /// The market's tick as read with `hvol_ref_price`, `None` when the source does not know it.
    pub(in crate::chartdx) hvol_price_step: Option<f64>,
    /// Bin width in price units the resident bins were built at; `0` while off.
    pub(in crate::chartdx) hvol_row_width: f64,
    /// The rolling window in price units the samples are summed over; `0` while off.
    pub(in crate::chartdx) hvol_price_window: f64,
    /// Window the resident rows cover, `None` while off.
    pub(in crate::chartdx) hvol_window: Option<moon_core::market::ProfileWindow>,
    /// When the profile was last asked for, unix milliseconds: the window ends NOW, so a quiet
    /// market still needs a re-read as rows slide out of it.
    pub(in crate::chartdx) hvol_read_ms: i64,
    /// Last horizontal-volume style sent to the layer, compared before `set_hvol_style`. Its
    /// `zone` is what the text pass places the zone's captions in.
    pub(in crate::chartdx) hvol_style: HvolStyleGpu,
    /// Visible-range maximum behind the zone's rows, kept as a SEMANTIC value the way
    /// `volume_stats` is for the band's.
    pub(in crate::chartdx) hvol_stats: Option<f32>,
    /// What the zone's corner caption names: the window in seconds (`None` for `Max`) and the
    /// effective price window as a percentage of the reference price.
    pub(in crate::chartdx) hvol_caption: Option<(Option<u32>, f32)>,
    /// Whether the volume readout under the crosshair prints at the zone's LEFT edge (else its
    /// right one); read by the text pass. Moonbot's `Disp. vol`.
    pub(in crate::chartdx) hvol_readout_left: bool,
    /// Whether the zone's captions get backing plates — light text on the dense readout plate in
    /// every theme — rather than the theme's plain caption ink. The non-`transparent` half of
    /// Moonbot's `Disp. vol`; read by the text pass.
    pub(in crate::chartdx) hvol_plates: bool,
    /// Whether the band's scale labels sit at the plot's right edge; read by the text pass.
    pub(in crate::chartdx) volume_scale_right: bool,
    /// Whether the bottom band's captions print over the volume bars rather than above them;
    /// read by the text pass.
    pub(in crate::chartdx) labels_over_volume: bool,
    pub(in crate::chartdx) combo_cross_capacity: usize,
    pub(in crate::chartdx) combo_price_line_capacity: usize,
    pub(in crate::chartdx) orderbook_view: ChartViewGpu,
    pub(in crate::chartdx) pane_bounds: [f32; 4],
    pub(in crate::chartdx) book_style: BookStyle,
    pub(in crate::chartdx) resident_left_rel: f32,
    /// Relative time of the OLDEST trade cross actually resident in the combo ring.
    ///
    /// `NaN` while none are. Distinct from `resident_left_rel`, which records what the read ASKED
    /// for: the hide-candles zone needs what the ring actually HAS, so it never blanks a bucket
    /// with no crosses to draw in its place.
    pub(in crate::chartdx) combo_left_rel: f32,
    /// Camera position, in pixels, at this pane's last history reset.
    ///
    /// A pan is covered by the prefetch the last read already fetched, so the next reset is owed
    /// only once the camera has travelled further than that; see the use site. `i64::MIN` means
    /// "never reset", which makes the distance overflow into a reset on the first pass.
    pub(in crate::chartdx) pan_reset_cam_px: i64,
    /// Last observed combo device generation; device loss requires history reupload.
    pub(in crate::chartdx) last_device_gen: u64,
    /// Last order-book build: data revision plus visible price window.
    pub(in crate::chartdx) last_book_rev: u64,
    pub(in crate::chartdx) last_book_lo: f32,
    pub(in crate::chartdx) last_book_hi: f32,
    /// Price window the current order-book instances were emitted over, margin included, and
    /// the visible range their bar lengths were normalized to.
    pub(in crate::chartdx) last_book_emit: (f32, f32),
    pub(in crate::chartdx) last_book_range: f32,
    /// Reused order-book instance buffer.
    pub(in crate::chartdx) book_scratch: Vec<moon_core::data::LevelInstance>,
    /// Book revision the sell-line depth labels were measured against; `u64::MAX` means
    /// unmeasured, which is also how `sync_orders_from_session` asks for a re-measure after
    /// rebuilding them. Separate from `last_book_rev` because that one also tracks the visible
    /// window: the labels' figure spans price to the line and does not depend on the camera, so
    /// panning must not re-sum the book.
    pub(in crate::chartdx) last_label_book_rev: u64,
    /// Last order revision uploaded into the userdata buffer.
    pub(in crate::chartdx) last_order_lines_rev: u64,
    /// Last `archived_lines_rev` the userdata buffer was built with — the closed trades' Moonbot
    /// lines ride the same buffer as the live orders, so a new archive answer rebuilds it.
    pub(in crate::chartdx) last_archived_lines_rev: u64,
    /// Last `frozen_overlay_rev` the userdata buffer was built with: a frozen viewer's corridor
    /// and modelled trades ride the same buffer as its archived store.
    pub(in crate::chartdx) last_overlay_rev: u64,
    /// The archived store this pane last drew, and the `(archived_lines_rev, twins signature,
    /// graphics bits, closed-order cap)` it was built for. Reused across the forced syncs a drag
    /// or hover fires per frame; see the order pass.
    pub(in crate::chartdx) archived_store:
        Option<Rc<moon_core::session::order_lines::OrderLineStore>>,
    pub(in crate::chartdx) archived_store_key: Option<(u64, u64, u64, u64)>,
    /// Strategy snapshots can change order appearance without an order update.
    pub(in crate::chartdx) last_order_strategies_rev: u64,
    /// Sparse strategy snapshots inherit defaults from a separately arriving schema.
    pub(in crate::chartdx) last_order_schema_rev: u64,
    /// Last order-zone signature. Zones live in the base cache, drawn over the grid and under the
    /// candles, while lines and traces render as an overlay. Zone changes must invalidate base;
    /// line hover and drag must not.
    pub(in crate::chartdx) last_order_zone_sig: u64,
    /// Local time when the userdata buffer was rebuilt from `order_lines_rev`.
    pub(in crate::chartdx) last_order_lines_sync_ms: f64,
    /// Order-userdata revision waiting for the next GPU prepare.
    pub(in crate::chartdx) pending_order_gpu_rev: Option<u64>,
    /// Last order revision that reached GPU prepare.
    pub(in crate::chartdx) last_order_gpu_rev: u64,
    /// Local time of the GPU prepare associated with `last_order_gpu_rev`.
    pub(in crate::chartdx) last_order_gpu_ms: f64,
    /// Last order revision actually rendered by the own-pass draw.
    pub(in crate::chartdx) last_order_present_rev: u64,
    /// Local time of the first draw for `last_order_present_rev`.
    pub(in crate::chartdx) last_order_present_ms: f64,
    /// Last order UID highlighted while building userdata.
    pub(in crate::chartdx) last_order_highlight_uid: Option<u64>,
    /// Last drag preview encoded into userdata.
    pub(in crate::chartdx) last_order_drag_preview: Option<(u64, LineKind, u32)>,
    /// Figure signature from store and interaction encoded into userdata; `u64::MAX` means dirty.
    pub(in crate::chartdx) last_figures_sig: u64,
    /// News-mark signature encoded into userdata; `u64::MAX` means dirty.
    pub(in crate::chartdx) last_news_sig: u64,
    /// Durable closed-trade marker signature encoded into userdata.
    pub(in crate::chartdx) last_trade_history_sig: u64,
    /// What the currently uploaded trade arrows were built FROM: the clusters, and the map from
    /// their members back to the panel's own record list.
    ///
    /// Retained rather than recomputed because the signature above quantizes the view scale: within
    /// one bucket the view keeps moving while the buffers do not, so re-clustering from the live
    /// scale would answer about a picture that is not on screen. Hit-testing reads this.
    pub(in crate::chartdx) trade_geometry: trade_history_sync::TradeGeometry,
    /// The filtered marks and sorted actions `trade_geometry` is built from, reused across pans.
    pub(in crate::chartdx) trade_source: Option<trade_history_sync::TradeSource>,
    /// Userdata buffers the order sync builds into, retained so a rebuild allocates nothing.
    pub(in crate::chartdx) ud_scratch: UdScratch,
    /// Warning-badge signature encoded into userdata; `u64::MAX` means dirty.
    pub(in crate::chartdx) last_warn_sig: u64,
    /// Prepared order-line labels for size, percentage, and quantity, rebuilt when orders change.
    /// `prepare_text` draws them and maps Y through `view` each frame.
    pub(in crate::chartdx) order_labels: Vec<OrderLabel>,
    /// Figure readouts, rebuilt with figure userdata and drawn by `prepare_text`.
    ///
    /// Most tools fill this only for the figure under the cursor and the one being drawn, so an
    /// idle chart pays nothing. A ratio scale (Fibonacci) is the exception and always names its
    /// levels — a level whose price appears only under the cursor cannot be read at a glance.
    pub(in crate::chartdx) figure_labels: Vec<moon_chart::figures::FigureLabel>,
    /// Stable priority order for `order_labels`, rebuilt together with order labels.
    /// Cursor-only text frames must not allocate/sort it again.
    pub(in crate::chartdx) order_label_order: Vec<usize>,
    /// Reused CPU caption bands so row arbitration allocates only when the order count grows.
    pub(in crate::chartdx) order_caption_scratch: Vec<text::OrderCaption>,
    /// Order-book volume labels on sell lines, matching Moonbot `LastSellOrderPriceVol`: the order
    /// provides the target and the current CPU order-book copy provides actual volume.
    pub(in crate::chartdx) orderbook_labels: Vec<OrderBookLabel>,
    /// Prospective selected F1-F6 order size in USD rendered at the cursor crosshair. `None` means no
    /// active size or rate. `ChartPanel::render`, which has Backend access, computes and copies it here.
    pub(in crate::chartdx) prospective_usd: Option<f64>,
    /// Placed order and cursor labels for this frame. `prepare_text` lays them out with overlap
    /// avoidance, and `sync_readout_params` builds their backing plates.
    pub(in crate::chartdx) label_placed: Vec<PlacedLabel>,
    /// The other of `prepare_text`'s two layout buffers, reused so a frame allocates none.
    pub(in crate::chartdx) label_placed_spare: Vec<PlacedLabel>,
    /// CPU copy of visible order-book levels for quantity labels under the cursor and on sell lines.
    /// Filled during order-book upload in `prepare`; empty while the order book is disabled.
    pub(in crate::chartdx) orderbook_levels: Vec<moon_core::data::BookDepthPoint>,
    /// Live best `(bid, ask)` book prices defining the three-color order-book zone background.
    pub(in crate::chartdx) book_best: Option<(f32, f32)>,
    /// Own-pass X camera: time epoch, right-side future fraction, follow flag, and last QUANTIZED
    /// right-edge pixel position. The callback advances the camera from these fields on every
    /// whole-pixel vblank present, providing live scrolling without a separate timer.
    pub(in crate::chartdx) epoch_ms: f64,
    pub(in crate::chartdx) right_margin_frac: f32,
    pub(in crate::chartdx) follow: bool,
    pub(in crate::chartdx) last_edge_px: i64,
    /// Offset of this source's trade clock from the local one; the live edge sits at `now + offset`
    /// so the newest ticks stay inside the plot when the PC clock lags the core. Reset with the
    /// pane when its core or market changes.
    pub(in crate::chartdx) live_clock: moon_chart::live_clock::LiveClockOffset,
    /// Last fitted (visible left, duration, pixels/ms); resize and zoom invalidate independently
    /// of the history floor, while live motion is throttled to whole pixels.
    pub(in crate::chartdx) price_scan_window: Option<(f32, f32, f32)>,
    pub(in crate::chartdx) cached_tick_price: Option<(f32, f32)>,
    pub(in crate::chartdx) cached_last_price: Option<f32>,
    /// Whether this pane has ever had price data of its OWN in the window — trades, candles or an
    /// order line — as opposed to the last price and the order-book band, which are only kept on
    /// screen. Decides whether the price fit may fall back to those references; see `fit_band`.
    /// Monotone until the pane's market changes.
    pub(in crate::chartdx) saw_window_data: bool,
    /// Last live-order range for auto-Y. Full session sync updates it; market-only frame sync reads
    /// this cache without touching CoreStore from `frame()`.
    pub(in crate::chartdx) cached_order_price: Option<(f32, f32)>,
    /// Whether this pane is visible and rendered this frame, set by `prepare`.
    pub(in crate::chartdx) active: bool,
    /// Whether this panel enables its per-window order book; disabled hides the book and corner label.
    pub(in crate::chartdx) orderbook_enabled: bool,
    /// Whether the combo ring was last filled WITH liquidation crosses, compared against
    /// `ChartGraphicsCfg::liquidations` on every sync: a flip resets the ring for a re-upload with
    /// or without them.
    pub(in crate::chartdx) liquidations_enabled: bool,
    /// The `(last, mark)` price-line switches the pane last uploaded under, compared against
    /// `ChartGraphicsCfg` on every sync for the same reason as the flag above.
    pub(in crate::chartdx) applied_price_lines: (bool, bool),
    /// This panel's order-book-only mode, hiding chart and price axis and using the full width.
    pub(in crate::chartdx) orderbook_only: bool,
    /// Whether ANY corner button — pin, compare lock or broom — is drawn on this pane, so the caption
    /// pass can reserve the strip's height only where a button actually stands.
    pub(in crate::chartdx) corner_buttons: bool,
    /// Price-axis position (`Left`, `Right`, or `Hide`), controlling label side and reserved gutter.
    /// Applied to every engine panel.
    pub(in crate::chartdx) price_axis_pos: crate::persistence::chart_persist::PriceAxisPos,
    /// Whether the time axis and its bottom-label gutter are visible. Disabled lets the plot fill
    /// slot height. Applied to every engine panel.
    pub(in crate::chartdx) time_axis_visible: bool,
    /// CPU/base inputs changed and D3D prepare must upload/bake resident resources before draw.
    /// Cursor-only presents leave this false.
    pub(in crate::chartdx) gpu_prepare_dirty: bool,
}

impl PaneRender {
    /// Creates a pane with no retained caption geometry or uploaded market state.
    ///
    /// The caption plate starts empty because `prepare_text` is its sole publisher; seeding a
    /// guessed rectangle here would briefly draw stale backing geometry before the first prepare.
    pub(in crate::chartdx) fn new() -> Self {
        Self {
            core: None,
            market: String::new(),
            core_name: String::new(),
            all_cores_count: None,
            ticker: String::new(),
            ticker_catalog_key: 0,
            ticker_resolved: false,
            scale_badge: None,
            time_scale_s: None,
            caption_plates: [[0.0; 4]; text::CAPTION_PLATES],
            caption_bars: Vec::new(),
            caption_bars_scratch: Vec::new(),
            volume_boxes: Vec::new(),
            volume_hits: Vec::new(),
            filter_header_hits: Vec::new(),
            column_bands: Vec::new(),
            filter_strip: None,
            action_rects: Vec::new(),
            action_draws: Vec::new(),
            labels: text::LabelState::default(),
            caption_rows: None,
            venue: String::new(),
            quote: String::new(),
            coin: String::new(),
            labels_shot_substituted: false,
            label_strategy: String::new(),
            label_basis: [text::BasisStats::default(); 3],
            delta_1h: None,
            delta_24h: None,
            label_detect_strategy: String::new(),
            label_detect_msg: String::new(),
            filter_lines: Vec::new(),
            label_scroll: Vec::new(),
            label_context: None,
            label_figures: None,
            label_windows: None,
            label_volumes: Vec::new(),
            label_liquidations: Vec::new(),
            label_cursor_ms: None,
            label_volume_read_ms: 0,
            label_volume_market: String::new(),
            label_volume_spans: Vec::new(),
            arb_hits: Vec::new(),
            label_arb_reachable: Vec::new(),
            label_arb: Vec::new(),
            label_arb_read_ms: 0,
            label_arb_market: String::new(),
            label_now_ms: 0,
            label_actions: text::ActionInputs::default(),
            view: ChartViewGpu::default(),
            layers: PlatformLayers::new(),
            background_params: BackgroundParams::default(),
            grid_params: GridParams::default(),
            cursor_params: CursorParams::default(),
            readout_rects: Vec::new(),
            readout_time_width: 0.0,
            readout_time_line_h: 0.0,
            readout_price_width: 0.0,
            readout_price_line_h: 0.0,
            history_cursor: ChartHistoryCursor::default(),
            history_buffers: ChartHistoryBuffers::default(),
            source_history_sig: u64::MAX,
            source_generation: u64::MAX,
            source_archive: u64::MAX,
            cross_upload: Vec::new(),
            liq_upload: Vec::new(),
            last_line_rows: Vec::new(),
            mark_line_rows: Vec::new(),
            price_line_epoch: f64::NAN,
            candle_rows: Vec::new(),
            candle_rows_epoch: f64::NAN,
            candle_resync: false,
            figure_snap: figure_snap::FigureSnapData::default(),
            last_candle_rev: u64::MAX,
            applied_candle_cfg: moon_core::market::CandleViewCfg::default().history_inputs(),
            last_zone_bucket: i64::MIN,
            candle_style: CandleStyleGpu::default(),
            price_style: PriceStyleGpu::default(),
            tick_style: TickStyleGpu::default(),
            volume_style: VolumeStyleGpu::default(),
            volume_samples: Vec::new(),
            volume_samples_max_tf: 0.0,
            volume_stats: None,
            side_samples: Vec::new(),
            side_upload: Vec::new(),
            side_scratch: Vec::new(),
            side_tf_ms: 0,
            side_step_ms: 0,
            side_range: (i64::MAX, i64::MIN),
            hvol_rows: Vec::new(),
            hvol_samples: Vec::new(),
            hvol_grid: None,
            hvol_upload: Vec::new(),
            hvol_rev: 0,
            hvol_ref_price: 0.0,
            hvol_price_step: None,
            hvol_row_width: 0.0,
            hvol_price_window: 0.0,
            hvol_window: None,
            hvol_read_ms: i64::MIN,
            hvol_style: HvolStyleGpu::default(),
            hvol_stats: None,
            hvol_caption: None,
            hvol_readout_left: false,
            hvol_plates: true,
            volume_scale_right: false,
            labels_over_volume: false,
            combo_cross_capacity: 0,
            combo_price_line_capacity: 0,
            orderbook_view: ChartViewGpu::default(),
            pane_bounds: [0.0, 0.0, 1.0, 1.0],
            book_style: BookStyle::default(),
            resident_left_rel: f32::NAN,
            combo_left_rel: f32::NAN,
            pan_reset_cam_px: i64::MIN,
            last_device_gen: 0,
            last_book_rev: u64::MAX,
            last_label_book_rev: u64::MAX,
            last_book_lo: f32::NAN,
            last_book_hi: f32::NAN,
            last_book_emit: (f32::NAN, f32::NAN),
            last_book_range: f32::NAN,
            book_scratch: Vec::new(),
            last_order_lines_rev: u64::MAX,
            last_archived_lines_rev: u64::MAX,
            last_overlay_rev: u64::MAX,
            archived_store: None,
            archived_store_key: None,
            last_order_strategies_rev: u64::MAX,
            last_order_schema_rev: u64::MAX,
            last_order_zone_sig: 0,
            last_order_lines_sync_ms: 0.0,
            pending_order_gpu_rev: None,
            last_order_gpu_rev: u64::MAX,
            last_order_gpu_ms: 0.0,
            last_order_present_rev: u64::MAX,
            last_order_present_ms: 0.0,
            last_order_highlight_uid: None,
            last_order_drag_preview: None,
            last_figures_sig: u64::MAX,
            last_news_sig: u64::MAX,
            last_trade_history_sig: u64::MAX,
            trade_geometry: trade_history_sync::TradeGeometry::default(),
            trade_source: None,
            ud_scratch: UdScratch::default(),
            last_warn_sig: u64::MAX,
            order_labels: Vec::new(),
            figure_labels: Vec::new(),
            order_label_order: Vec::new(),
            order_caption_scratch: Vec::new(),
            orderbook_labels: Vec::new(),
            prospective_usd: None,
            label_placed: Vec::new(),
            label_placed_spare: Vec::new(),
            orderbook_levels: Vec::new(),
            book_best: None,
            epoch_ms: 0.0,
            right_margin_frac: 0.10,
            follow: false,
            last_edge_px: i64::MIN,
            live_clock: moon_chart::live_clock::LiveClockOffset::default(),
            price_scan_window: None,
            cached_tick_price: None,
            cached_last_price: None,
            saw_window_data: false,
            cached_order_price: None,
            active: false,
            orderbook_enabled: true,
            liquidations_enabled: true,
            applied_price_lines: (true, true),
            orderbook_only: false,
            corner_buttons: false,
            price_axis_pos: crate::persistence::chart_persist::PriceAxisPos::Left,
            time_axis_visible: true,
            gpu_prepare_dirty: true,
        }
    }

    /// Drops everything derived from a book this pane no longer has: the order book was switched
    /// off for the window, or the market view went away with its core.
    ///
    /// Both are figures about a live book, so neither may outlive it — a frozen bid/ask would keep
    /// answering the cursor's percentage, and a stale sell-line volume would keep describing glass
    /// that is no longer drawn. `u64::MAX` also asks the book path to re-measure once one returns.
    pub(in crate::chartdx) fn forget_book_figures(&mut self) {
        if self.book_best.is_none() && self.last_label_book_rev == u64::MAX {
            return;
        }
        self.book_best = None;
        self.last_label_book_rev = u64::MAX;
        crate::chartdx::data_state::orders::clear_orderbook_label_notionals(
            &mut self.orderbook_labels,
        );
    }

    pub(in crate::chartdx) fn finish_order_gpu_prepare(&mut self, now_ms: f64) {
        if let Some(rev) = self.pending_order_gpu_rev.take() {
            self.last_order_gpu_rev = rev;
            self.last_order_gpu_ms = now_ms;
        }
    }

    pub(in crate::chartdx) fn finish_order_present(&mut self, now_ms: f64) {
        if self.last_order_present_rev != self.last_order_gpu_rev {
            self.last_order_present_rev = self.last_order_gpu_rev;
            self.last_order_present_ms = now_ms;
        }
    }

    /// Advance the X-follow camera only when `now_ms` moves by at least one WHOLE pixel, matching
    /// Moonbot `round(Now/FdtScale)`. Between pixel crossings the frame is pixel-identical, so present
    /// can reuse it without work. Whole-pixel steps remove subpixel jitter, while calling on every
    /// present keeps vblank motion smooth. Returns `true` when the camera actually moved for the
    /// productive-frame counter.
    pub(in crate::chartdx) fn advance_camera(&mut self, now_ms: f64) -> bool {
        if !self.follow || !(self.view.time_to_px > 0.0) {
            return false;
        }
        // Use ONE ppm guard for forward and inverse conversion. Previously `target_px` used raw ppm
        // while `inv_ppm` used a 1e-6 floor; at deep zoom-out below 1e-6 for a 365-day window,
        // `right_rel` collapsed near zero and shifted the chart left of the order book.
        let ppm = self.view.time_to_px.max(moon_chart::view::MIN_PX_PER_MS);
        let target_px = ((now_ms - self.epoch_ms) * ppm as f64).round() as i64;
        if target_px == self.last_edge_px {
            return false;
        }
        self.last_edge_px = target_px;
        let inv_ppm = 1.0 / ppm;
        let area_w = self.view.bounds[2];
        let glass_w = self.orderbook_view.bounds[2];
        let window_ms = area_w * inv_ppm;
        let right_rel = target_px as f32 * inv_ppm;
        self.view.view_time0 = right_rel + window_ms * self.right_margin_frac - window_ms;
        self.view.pad = self.view.view_time0 + (area_w + glass_w) * inv_ppm;
        self.gpu_prepare_dirty = true;
        true
    }
}
