use super::*;

/// Everything the configured captions can read, in the form they are read in.
///
/// Numbers rather than pre-rendered strings: the comparison that decides whether to re-format has
/// to be cheap and exact, and formatting first would make it neither.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::chartdx) struct LabelInputs {
    /// Coin ticker as the pane resolved it.
    pub ticker: String,
    /// Name of the core that owns the pane's market.
    pub core_name: String,
    /// Venue label for that core; empty when this build cannot name it.
    pub venue: String,
    /// Quote currency of the pane's market, uppercase; empty when the catalog carries none.
    pub quote: String,
    /// User-assigned strategy name of the newest OPEN order on this market.
    pub strategy: String,
    /// Strategy and line of the newest detect THIS core fired on this market.
    ///
    /// Kept until the NEXT detect on the same market replaces it — there is no expiry: the caption
    /// answers "what last fired here", and a line that vanished on a timer would leave the reader
    /// unable to tell "nothing fired" from "it fired a while ago".
    pub detect_strategy: String,
    pub detect_msg: String,
    /// Core-built skip-reason lines for this market, one per enabled strategy.
    ///
    /// Empty until the chart has asked for them and the core has answered. Compared by value in
    /// the caption cache, so a new snapshot re-formats the column and an identical repeat does not.
    pub filter_lines: Vec<String>,
    /// What the CLOSED TRADE this chart was handed was: the strategy that opened it, the line it
    /// fired on, why it closed. `None` on every live chart — the trade-detail window is the only
    /// one that is handed a trade, and the captions reading this print nothing anywhere else.
    ///
    /// Carried behind an `Rc` because it is fixed for the window's life and shared across panes:
    /// replacing a pane's held inputs must not duplicate the three strings owned by the window.
    pub trade: Option<Rc<crate::chartdx::TradeLabels>>,
    /// Last traded price the chart itself is drawing.
    pub last_price: Option<f32>,
    /// Y-scale badge as a whole percentage; `None` while it is hidden.
    pub scale_badge: Option<i32>,
    /// Time-scale badge: the whole seconds the plot spans; `None` while there is no plot to measure.
    pub time_scale_s: Option<i64>,
    /// Comparison-mode difference from the locked anchor, in percent.
    pub compare_pct: Option<f32>,
    /// Signed one-hour and 24-hour changes, from the readout the header ticker uses.
    pub delta_1h: Option<f64>,
    pub delta_24h: Option<f64>,
    /// Market-wide background: the exchange's own average movement and BTC's, plus funding.
    ///
    /// `None` until a caption asks for any of it — the sync path does not read the snapshot for a
    /// figure nobody prints.
    pub context: Option<moon_core::market::MarketContextReadout>,
    /// Wall clock the countdown captions are measured against, in Unix milliseconds.
    ///
    /// Carried as an INPUT rather than read at format time so the cache key sees it: a countdown
    /// re-formats when the figure it prints changes, and not on every revision in between. Already
    /// QUANTIZED by the sync to the coarsest step the drawn countdowns can live with — a minute
    /// while every one of them is far out, a second once one is inside its last hour.
    pub now_ms: i64,
    /// The chart's own candle timeframe in milliseconds, for a countdown caption set to `Авто`.
    ///
    /// A caption parameter resolves against it rather than the caption reading the candle
    /// configuration itself: the caption pass is handed everything it prints, and this keeps the
    /// timeframe in the cache key, so switching the chart's timeframe re-formats the countdown.
    pub chart_tf_ms: i64,
    /// Quote side, venue caps, coin tags and the EXCHANGE's own position on this market.
    ///
    /// `None` until a caption asks for any of it, like [`Self::context`]: the readout takes the
    /// source lock and two versioned snapshots, and a pane that prints none of these figures must
    /// not pay for them on every market revision.
    pub figures: Option<MarketFiguresReadout>,
    /// Retained-history MOVEMENT, per window. `None` on the same terms, and gated separately
    /// because it costs more — it walks the trade buckets and the candle ring.
    pub windows: Option<MarketWindowsReadout>,
    /// Traded amounts, one entry per distinct period the configuration asks for.
    ///
    /// A list rather than a per-window array, because a period is no longer one of eight: two volume
    /// modules on one chart can read "the last minute" and "the last 500 trades", and one of them
    /// can be measuring around the pointer instead of at the live edge. Short — a chart prints one
    /// or two — so it is searched linearly rather than hashed.
    pub volumes: Vec<((VolumeSpan, VolumeAt), VolumeSpanReadout)>,
    /// What was liquidated over those same periods, for the captions that print it.
    ///
    /// Its own list because it is its own ring with its own depth, and because a chart that never
    /// prints `L` must not order the read at all.
    pub liquidations: Vec<((VolumeSpan, VolumeAt), LiqSpanReadout)>,
    /// The moment the pointer is on, in unix milliseconds, or `None` while it is off this pane.
    ///
    /// Carried as an INPUT so the caption cache sees it: a measuring caption re-formats when the
    /// pointer moves to a different moment, and not on the revisions in between. Already QUANTIZED
    /// by the sync — see `CURSOR_QUANTUM_MS` — so a pixel of mouse travel is not a new value.
    pub cursor_ms: Option<i64>,
    /// Venues this terminal has a core connected to, as `(platform code, dex name)`.
    ///
    /// Collected on the SESSION sync, where the core list is in hand — the caption pass has neither
    /// the session nor the right to walk it. An ordinary exchange carries an empty dex.
    pub arb_reachable: Vec<(u8, String)>,
    /// Arbitrage quotes for this market, as the core last reported them.
    ///
    /// Refreshed on a THROTTLE rather than every revision — see the sync — because reading them
    /// costs one market-lock round trip per venue, and a column of prices is read by eye.
    pub arb: Vec<ArbQuote>,
    /// Open-position figures, one entry per [`PnlBasis`] at [`basis_index`].
    pub basis: [BasisStats; 3],
    /// What the pane's market BUTTONS state right now.
    pub actions: ActionInputs,
    /// First visible line of each scrolled label column, as `(label row, first)`.
    ///
    /// Absent means unscrolled. An input like the rest so a scroll re-formats the column through
    /// the same comparison every other change goes through.
    pub column_scroll: Vec<(usize, u32)>,
}

/// What a pressable caption prints, and whether pressing it does anything.
///
/// Carried as INPUTS like every other caption's figures, and for the same reason: a button whose
/// text is built anywhere else would reshape its run on a frame where nothing about it moved. The
/// three facts here are the only ones its text depends on.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(in crate::chartdx) struct ActionInputs {
    /// Whether this chart can act on a market at all.
    ///
    /// False on the trade-detail window, which draws a picture of a trade that already closed:
    /// every action caption prints NOTHING there rather than a disabled button, because a button
    /// over a finished trade reads as an offer to act on it.
    pub live: bool,
    /// Whether the workspace rail lets this window command the pane's core right now. A button that
    /// cannot be pressed is drawn faded, the way an unreachable arbitrage venue is.
    pub allowed: bool,
    /// Whether panic selling is already armed on this market, which is what turns `Panic Sell` into
    /// `Stop Panic`.
    pub panic_armed: bool,
    /// When the core's temporary ban on this market runs out, Unix ms, or `None` while it holds
    /// none. A DEADLINE rather than a remainder so the figure counts down against the caption
    /// clock without the panel pushing a new value per frame.
    pub ban_until_ms: Option<i64>,
    /// Whether this market is one of the core's marked ones, or `None` while the core has not
    /// reported its configuration at all.
    ///
    /// Three states rather than two, and the third is not "no": a star drawn hollow on an unknown
    /// list invites a press that would write a list nobody has read. The caption prints the hollow
    /// star either way — there is nothing else it could draw — and the button beside it is disabled
    /// on `None`; see `Backend::fav_market`.
    pub favorite: Option<bool>,
}

/// Open-position figures for ONE basis.
///
/// Kept per basis rather than filtered at format time because the filter is the expensive half:
/// an order walk per configured label would repeat the same pass up to three times.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(in crate::chartdx) struct BasisStats {
    /// How many orders are open on this market.
    pub open_orders: u32,
    /// Open position in the base coin, negative while short.
    pub pos_size: f64,
    /// Quote money the open positions are carrying, at their entry prices.
    pub spent: f64,
    /// Unrealized result in the market's quote money.
    pub pnl_quote: f64,
    /// Current notional of what is open: position size times mark, in the quote currency.
    ///
    /// Counted over a WIDER set than [`Self::pnl_quote`], deliberately: a row whose entry price has
    /// not arrived still has a size and a mark, and withholding it would understate what is at
    /// risk. This is the rule the chart overlay used before these captions replaced it.
    pub exposure: f64,
    /// Whether anything contributed to [`Self::exposure`].
    pub has_exposure: bool,
    /// Whether any order contributed a result at all. Distinguishes "flat" from "nothing open",
    /// which a bare zero cannot.
    pub has_position: bool,
}

impl BasisStats {
    /// Unrealized result as a percentage of what the open positions spent.
    ///
    /// `None` when nothing is open, which is NOT the same as zero: a chart with no position must
    /// print no percentage rather than a confident `0.00%`.
    pub(super) fn pnl_pct(&self) -> Option<f64> {
        (self.spent > 0.0).then(|| self.pnl_quote / self.spent * 100.0)
    }
}

/// Index of a basis in [`LabelInputs::basis`].
pub(in crate::chartdx) fn basis_index(basis: PnlBasis) -> usize {
    match basis {
        PnlBasis::All => 0,
        PnlBasis::Real => 1,
        PnlBasis::Emulator => 2,
    }
}
