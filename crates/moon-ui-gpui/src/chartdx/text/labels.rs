//! Turning the caption CONFIGURATION into the strings one pane draws.
//!
//! The split this module exists for: `prepare_text` runs on every presented frame, so it must not
//! format numbers, take a market-source lock or walk an order store. Everything a caption needs is
//! collected into [`LabelInputs`] while the pane synchronizes — which happens on a data REVISION,
//! not on a frame — and formatted into [`LabelState::texts`] only when those inputs actually differ
//! from the ones already formatted.
//!
//! That guard is not an optimization detail, it is the contract with the retained GPU text runs: a
//! run reshapes when its string changes, so a caption rebuilt with an identical value on every
//! revision would reshape a chart's whole corner several times a second for nothing.

use std::rc::Rc;

use moon_core::config::{
    ChartLabelField, ChartLabelPart, ChartLabelsCfg, PnlBasis, ROW_NAME_PART,
};
use moon_core::config::{ARB_PART_BASE, ArbViewCfg};
use moon_core::market::{ArbQuote, CoinTag, MarketFiguresReadout, MarketWindowsReadout};
use moon_core::util::fmt::{self, DeltaSign};
use rust_i18n::t;

use crate::order_math::{MONEY_DECIMALS, order_pnl, position_qty};

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
    /// Last traded price the chart itself is drawing.
    pub last_price: Option<f32>,
    /// Y-scale badge as a whole percentage; `None` while it is hidden.
    pub scale_badge: Option<i32>,
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
    /// Wall clock the funding countdown is measured against, in Unix milliseconds.
    ///
    /// Carried as an INPUT rather than read at format time so the cache key sees it: a countdown
    /// re-formats when the minute it prints changes, and not on every revision in between.
    pub now_ms: i64,
    /// Quote side, venue caps, coin tags and the EXCHANGE's own position on this market.
    ///
    /// `None` until a caption asks for any of it, like [`Self::context`]: the readout takes the
    /// source lock and two versioned snapshots, and a pane that prints none of these figures must
    /// not pay for them on every market revision.
    pub figures: Option<MarketFiguresReadout>,
    /// Retained-history movement and volume, per window. `None` on the same terms, and gated
    /// separately because it costs more — it walks the trade buckets and the candle ring.
    pub windows: Option<MarketWindowsReadout>,
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
    fn pnl_pct(&self) -> Option<f64> {
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

/// One caption ready to draw.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::chartdx) struct LabelText {
    /// Row this caption belongs to. The row decides the band, the alignment and the stacking
    /// order; the caption itself decides nothing about where it lands.
    pub row: usize,
    /// Index inside that row — [`ROW_NAME_PART`] for the row's own name — which with the row
    /// ADDRESSES this caption's retained text run.
    ///
    /// Carried rather than implied by position: unresolved captions are skipped, so a position in
    /// the list is not an index, and addressing runs by position would hand a run a different
    /// string whenever a neighbour appeared or vanished — reshaping both of them.
    pub part: usize,
    pub text: String,
    /// The caption's own prefix — "PnL", "Фандинг1ч" — kept APART from the value rather than glued
    /// to the front of it.
    ///
    /// Separate because the two are coloured separately: a by-sign caption paints the figure, and
    /// painting the word with it turns the row into a block of green the eye has to re-parse. Empty
    /// when the caption prints no prefix, which is the common case and costs nothing.
    pub prefix: String,
    /// Sign of the value behind the text, for a caption that colors by it. `None` means the value
    /// has no meaningful sign and the caption keeps the theme color.
    pub sign: Option<DeltaSign>,
    /// Whether the venue this line names has a core behind it — a chart the click can actually
    /// open. A venue with none is DIMMED: the column still states its price, since that is what the
    /// column is for, but nothing there responds to a click and the eye should know.
    pub reachable: bool,
    /// The venue this line names, for the click that opens the coin there.
    ///
    /// Only an arbitrage line carries one. The DEX name rides along because a Hyperliquid deployer
    /// is not identified by its code — every deployer shares the futures platform ordinal — and the
    /// core that trades it is found by that name.
    pub venue: Option<(u8, String)>,
    /// Colour this ONE line is drawn in, overriding the caption's own style.
    ///
    /// Only an arbitrage line uses it: its colour belongs to the VENUE, which the caption's style
    /// cannot express — one caption prints a dozen lines and they are not all Gate. `None`
    /// everywhere else, where the style answers.
    pub color: Option<u32>,
}

/// A pane's caption state: what it read, and what it formatted from it.
#[derive(Clone, Debug, Default)]
pub(in crate::chartdx) struct LabelState {
    /// Inputs the current [`Self::texts`] were formatted from.
    inputs: LabelInputs,
    /// Arbitrage roster the current [`Self::texts`] were arranged by.
    ///
    /// Held beside the configuration and compared the same way — by POINTER — because it is the
    /// same kind of value: global, replaced wholesale when the settings window writes it, and two
    /// dozen venues wide. Comparing it by value would walk those venues, names and all, on every
    /// revision of every pane, which is exactly what keeping it out of the by-value inputs avoids.
    arb_view: Option<Rc<ArbViewCfg>>,
    /// Configuration the current [`Self::texts`] were formatted under.
    ///
    /// The HANDLE, not a copy: a configuration is replaced wholesale whenever it changes, so
    /// pointer identity answers "same configuration" exactly — and answering it by value would deep-
    /// copy sixteen rows per pane on every revision that moved a price.
    cfg: Option<Rc<ChartLabelsCfg>>,
    /// Locale the current [`Self::texts`] were formatted in.
    ///
    /// Part of the cache key because a caption's short prefix comes from the dictionary and is
    /// BAKED into the stored string: without this a live language switch would leave the previous
    /// language on the chart until some unrelated input happened to move.
    locale: String,
    /// Formatted captions in draw order — row by row, caption by caption — skipping everything
    /// that resolved to nothing.
    pub texts: Vec<LabelText>,
    /// Reusable build buffer, so a re-format that changes nothing costs no allocation.
    scratch: Vec<LabelText>,
}

impl LabelState {
    /// Re-format the captions when anything they read has changed.
    ///
    /// Args:
    ///     cfg: The pane's effective caption configuration.
    ///     inputs: Freshly collected values.
    ///
    /// Returns:
    ///     Whether the drawn captions actually changed, and the pane therefore has to repaint.
    pub(in crate::chartdx) fn update(
        &mut self,
        cfg: &Rc<ChartLabelsCfg>,
        arb_view: &Rc<ArbViewCfg>,
        inputs: LabelInputs,
    ) -> bool {
        let locale = rust_i18n::locale().to_string();
        let same_cfg = self.cfg.as_ref().is_some_and(|held| Rc::ptr_eq(held, cfg));
        let same_view = self
            .arb_view
            .as_ref()
            .is_some_and(|held| Rc::ptr_eq(held, arb_view));
        if same_cfg && same_view && self.inputs == inputs && self.locale == locale {
            return false;
        }
        self.locale = locale;
        self.cfg = Some(cfg.clone());
        self.arb_view = Some(arb_view.clone());
        self.inputs = inputs;
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.clear();
        for (row_ix, row) in cfg.rows.iter().enumerate() {
            // THE gate for a whole module: its own switch, and the question of whether anything on
            // it would print. Asked here rather than in the layout pass because a row that resolves
            // to nothing must also cost nothing downstream — no rows collected, no runs addressed.
            if !row.is_drawn() {
                continue;
            }
            // The row's own name leads its captions, which is where a reader looks for what the
            // row IS before reading the figures on it.
            if let (true, Some(title)) = (row.show_name, crate::controls::row_title(row)) {
                scratch.push(LabelText {
                    row: row_ix,
                    part: ROW_NAME_PART,
                    text: title,
                    prefix: String::new(),
                    reachable: false,
                    venue: None,
                    sign: None,
                    color: None,
                });
            }
            let mut column_drawn = false;
            for (part_ix, part) in row.parts.iter().enumerate() {
                if !part.is_drawn() {
                    continue;
                }
                // A column caption is not one value: it expands into its own run range, one line
                // per venue, and never occupies its part index. Expanded HERE rather than in the
                // drawing pass so the cache below still compares finished strings.
                if part.field.is_column() {
                    // The FIRST column of a module owns its line range; a second one would emit the
                    // same `(row, part)` pairs and the two would reshape each other's runs every
                    // frame. The drawing pass resolves a column's style the same way — first one
                    // wins — so this is the same rule stated once on each side.
                    if !column_drawn {
                        push_arb_rows(
                            &mut scratch,
                            row_ix,
                            &self.inputs,
                            self.arb_view.as_deref(),
                            part.resolved_style(),
                        );
                        column_drawn = true;
                    }
                    continue;
                }
                // A caption with nothing to say draws nothing and takes no place on its row. This
                // is what lets the comparison delta and the scale badge sit in the DEFAULT
                // configuration without leaving two blank rows on an ordinary chart.
                let Some((text, sign)) = resolve(part, &self.inputs) else {
                    continue;
                };
                scratch.push(LabelText {
                    row: row_ix,
                    part: part_ix,
                    text,
                    prefix: caption_prefix(part, part.resolved_style().caption),
                    reachable: false,
                    venue: None,
                    sign,
                    color: None,
                });
            }
        }
        // The inputs moved, but the drawn result often does not: a last price that ticked inside
        // its rounding, an order revision that changed a line nothing prints. Comparing the
        // FORMATTED result is what keeps those from repainting the pane.
        let changed = scratch != self.texts;
        if changed {
            std::mem::swap(&mut self.texts, &mut scratch);
        }
        self.scratch = scratch;
        changed
    }
}

/// Figures for one basis.
fn stats_for(inputs: &LabelInputs, basis: PnlBasis) -> &BasisStats {
    &inputs.basis[basis_index(basis)]
}

/// Format one caption's VALUE, or report that it has nothing to print.
///
/// The value alone: the prefix that names it is built by [`caption_prefix`] and kept apart, so the
/// two can be coloured separately. Every arm here therefore answers with the figure and nothing
/// else, which is also why they read as a table.
fn resolve(part: &ChartLabelPart, inputs: &LabelInputs) -> Option<(String, Option<DeltaSign>)> {
    let stats = stats_for(inputs, part.pnl_basis);
    match part.field {
        ChartLabelField::None => None,
        ChartLabelField::Coin => non_empty(&inputs.ticker).map(|t| (t, None)),
        ChartLabelField::Core => non_empty(&inputs.core_name).map(|t| (t, None)),
        ChartLabelField::Venue => non_empty(&inputs.venue).map(|t| (t, None)),
        ChartLabelField::Quote => non_empty(&inputs.quote).map(|t| (t, None)),
        ChartLabelField::OrderStrategy => non_empty(&inputs.strategy).map(|t| (t, None)),
        ChartLabelField::DetectStrategy => {
            non_empty(&inputs.detect_strategy).map(|t| (t, None))
        }
        // The line is the core's own text and can be a sentence. It is cut to what a caption can be
        // read as at all; the chart's own width budget truncates whatever still does not fit, but
        // that budget measures a string it has already been handed, so an unbounded one would be
        // shaped in full first.
        // Stripping the strategy tail can leave a line with nothing in it — a detect whose text
        // was ONLY that tail — and an empty caption is NOT an empty string: it still opens its
        // module's line and reserves its plate. So the RESULT is checked, not just the input.
        ChartLabelField::DetectMsg => non_empty(&inputs.detect_msg)
            .and_then(|t| non_empty(&detect_line(&t)))
            .map(|t| (cut(&t), None)),
        ChartLabelField::ScaleBadge => inputs.scale_badge.map(|pct| {
            // A range below a whole percent in a quiet Auto market reads as "<1%", never as zero:
            // zero would claim the chart has no vertical span at all.
            let text = if pct == 0 {
                "<1%".to_string()
            } else {
                format!("{pct}%")
            };
            (text, None)
        }),
        // Deliberately the chart's own percentage formatter and not `fmt::signed_pct`: this figure
        // sits at the price the reader is comparing against, where a deviation that rounds to zero
        // still carries the DIRECTION, and the shared formatter drops the sign there on purpose.
        ChartLabelField::CompareDelta => inputs.compare_pct.map(|pct| {
            let sign = if pct >= 0.0 {
                DeltaSign::Positive
            } else {
                DeltaSign::Negative
            };
            let min = part.resolved_style().color_min_pct;
            (
                super::fmt_pct(pct),
                colored_sign(min, f64::from(pct), sign),
            )
        }),
        ChartLabelField::LastPrice => inputs
            .last_price
            .filter(|p| p.is_finite() && *p > 0.0)
            .map(|p| (fmt::adaptive(f64::from(p)), None)),
        ChartLabelField::Delta1h => inputs
            .delta_1h
            .and_then(|v| signed_pct_label(part, v)),
        ChartLabelField::Delta24h => inputs
            .delta_24h
            .and_then(|v| signed_pct_label(part, v)),
        ChartLabelField::OpenPnlPct => stats
            .pnl_pct()
            .and_then(|v| signed_pct_label(part, v)),
        ChartLabelField::OpenPnlMoney => stats.has_position.then(|| {
            let (text, sign) = fmt::signed_amount(stats.pnl_quote, MONEY_DECIMALS);
            (text, Some(sign))
        }),
        // Zero prints nothing rather than "0": the caption reports a position, and an empty corner
        // already says there is none.
        ChartLabelField::ExchangeDelta1h => inputs
            .context
            .and_then(|c| signed_pct_label(part, c.exchange_1h_pct)),
        ChartLabelField::ExchangeDelta24h => inputs
            .context
            .and_then(|c| signed_pct_label(part, c.exchange_24h_pct)),
        ChartLabelField::BtcDelta1h => inputs
            .context
            .and_then(|c| signed_pct_label(part, c.btc_1h_pct)),
        ChartLabelField::BtcDelta24h => inputs
            .context
            .and_then(|c| signed_pct_label(part, c.btc_24h_pct)),
        ChartLabelField::BtcDelta72h => inputs
            .context
            .and_then(|c| signed_pct_label(part, c.btc_72h_pct)),
        // A market that charges no funding prints nothing: a zero there would read as "free",
        // which is a different claim from "this venue has no funding at all".
        ChartLabelField::Funding => inputs
            .context
            .and_then(|c| c.funding_pct)
            .and_then(|v| signed_pct_label(part, v)),
        ChartLabelField::FundingIn => inputs
            .context
            .and_then(|c| c.funding_at_ms)
            .and_then(|at| fmt_countdown(at - inputs.now_ms))
            .map(|text| (text, None)),
        ChartLabelField::OpenOrders => (stats.open_orders > 0).then(|| {
            (
                stats.open_orders.to_string(),
                None,
            )
        }),
        ChartLabelField::Exposure => stats.has_exposure.then(|| {
            (
                plain(&fmt::compact_si(stats.exposure)),
                None,
            )
        }),
        // Expanded before this point, into its own run range: one caption, a dozen lines. Reaching
        // here would mean the expansion was skipped, and a single line saying "arbitrage" is not
        // what the caption is for.
        ChartLabelField::ArbColumn => None,
        ChartLabelField::CoinTags => inputs
            .figures
            .as_ref()
            .filter(|f| !f.tags.is_empty())
            .map(|f| (tags_text(&f.tags), None)),
        ChartLabelField::Bid => figure(inputs, |f| f.bid)
            .map(price_label),
        ChartLabelField::Ask => figure(inputs, |f| f.ask)
            .map(price_label),
        // The spread needs BOTH sides and a sane pair: a crossed book — which a stale snapshot can
        // show for a moment — would otherwise print a negative spread as if it were an arbitrage.
        ChartLabelField::Spread => inputs.figures.as_ref().and_then(|f| {
            let (bid, ask) = (f.bid?, f.ask?);
            let pct = (ask > bid).then(|| (ask - bid) / ask * 100.0)?;
            let (text, _) = fmt::pct(pct, 2)?;
            Some((text, None))
        }),
        ChartLabelField::MarkPrice => figure(inputs, |f| f.mark)
            .map(price_label),
        // The deviation is read against the price the CHART is drawing, so the caption cannot
        // disagree with the candle beside it.
        ChartLabelField::MarkDelta => {
            let mark = figure(inputs, |f| f.mark)?;
            let last = f64::from(inputs.last_price.filter(|p| p.is_finite() && *p > 0.0)?);
            signed_pct_label(part, (mark - last) / last * 100.0)
        }
        ChartLabelField::PriceStep => figure(inputs, |f| f.price_step)
            .map(price_label),
        ChartLabelField::Volume24h => figure(inputs, |f| f.vol_24h)
            .map(|v| (plain(&fmt::compact_si(v)), None)),
        ChartLabelField::WindowDelta => window(inputs, part)
            .and_then(|w| w.delta_pct)
            .and_then(|v| fmt::pct(v, 2))
            .map(|(text, _)| (text, None)),
        ChartLabelField::WindowVolume => window(inputs, part)
            .and_then(|w| w.volume_quote)
            .map(|v| (fmt::compact_si(v), None)),
        ChartLabelField::WindowBuyShare => window(inputs, part)
            .and_then(|w| w.buy_share_pct)
            .and_then(|v| fmt::pct(v, 1))
            .map(|(text, _)| (text, None)),
        ChartLabelField::MaxLeverage => inputs
            .figures
            .as_ref()
            .and_then(|f| f.max_leverage)
            .map(|v| (format!("x{v}"), None)),
        // A cap the venue never stated prints nothing; one it stated but that cannot be converted
        // yet also prints nothing, rather than "0" — see `MaxOrderSource`.
        ChartLabelField::MaxOrder => inputs
            .figures
            .as_ref()
            .map(|f| f.max_order)
            .filter(|m| m.value.is_finite() && m.value > 0.0)
            .map(|m| (plain(&fmt::compact_si(m.value)), None)),
        ChartLabelField::ExchPosSize => inputs
            .figures
            .as_ref()
            .and_then(|f| f.pos_size)
            .map(|v| {
                let sign = if v >= 0.0 {
                    DeltaSign::Positive
                } else {
                    DeltaSign::Negative
                };
                (plain(&fmt::compact_si(v)), Some(sign))
            }),
        ChartLabelField::ExchPosPrice => figure(inputs, |f| f.pos_price)
            .map(price_label),
        ChartLabelField::LiqPrice => figure(inputs, |f| f.liq_price)
            .map(price_label),
        ChartLabelField::Leverage => inputs
            .figures
            .as_ref()
            .and_then(|f| f.leverage_x)
            .map(|v| (format!("x{v}"), None)),
        ChartLabelField::MarginMode => inputs.figures.as_ref().and_then(|f| f.isolated).map(|iso| {
            let key = if iso {
                "chart_labels.margin.isolated"
            } else {
                "chart_labels.margin.cross"
            };
            (t!(key).to_string(), None)
        }),
        // Unlike the open-position figures, a session profit of exactly zero is a RESULT — the coin
        // was traded to break even — so it prints, with the neutral sign the formatter picks.
        ChartLabelField::SessionPnl => inputs.figures.as_ref().and_then(|f| f.session_pnl).map(|v| {
            let (text, sign) = fmt::signed_amount(v, MONEY_DECIMALS);
            (text, Some(sign))
        }),
        ChartLabelField::CoinBalance => figure(inputs, |f| f.coin_balance)
            .map(|v| (plain(&fmt::compact_si(v)), None)),
        ChartLabelField::PosSize => (stats.pos_size != 0.0).then(|| {
            let sign = if stats.pos_size >= 0.0 {
                DeltaSign::Positive
            } else {
                DeltaSign::Negative
            };
            (
                plain(&fmt::compact_si(stats.pos_size)),
                Some(sign),
            )
        }),
    }
}

/// Build the arbitrage column's lines for one module.
///
/// One line per venue the roster shows, in the roster's order, addressed from [`ARB_PART_BASE`] so
/// a venue that stops reporting cannot hand its retained run to the venue below it — which would
/// reshape every line under the gap on every frame.
fn push_arb_rows(
    out: &mut Vec<LabelText>,
    row_ix: usize,
    inputs: &LabelInputs,
    view: Option<&ArbViewCfg>,
    style: moon_core::config::ResolvedLabelStyle,
) {
    let Some(view) = view else {
        return;
    };
    // Every line of the column shares the caption's style, so the threshold is read once.
    let min_pct = style.color_min_pct;
    // Formatted for the WHOLE column before anything is padded: a column is aligned against its
    // own widest cell, which cannot be known one line at a time.
    let cells: Vec<ArbCell> = view
        .arrange(&inputs.arb)
        .into_iter()
        .map(|row| {
            // Formatted ONCE and read twice: the text carries it, and the sign it rounded to picks
            // the colour, so the two cannot disagree about a spread that rounds away.
            let spread = fmt::signed_pct(row.quote.spread_pct, 2);
            ArbCell {
                code: row.quote.venue.code(),
                dex: row.quote.dex_name.clone(),
                sign: spread
                    .as_ref()
                    .and_then(|(_, sign)| colored_sign(min_pct, row.quote.spread_pct, *sign)),
                price: match view.show.shows_price() {
                    true => fmt::adaptive(row.quote.price),
                    false => String::new(),
                },
                pct: match view.show.shows_spread() {
                    true => spread.map(|(pct, _)| pct).unwrap_or_default(),
                    false => String::new(),
                },
                // A venue that cannot be deposited to or withdrawn from is marked, not hidden: the
                // spread is real, the settlement is not, and a reader must not take one for the
                // other.
                blocked: view.mark_blocked
                    && (row.quote.deposit_blocked || row.quote.withdraw_blocked),
                label: row.label,
                color: row.color,
            }
        })
        .collect();
    // Column widths, in CHARACTERS. The chart draws its captions in a monospaced face — see
    // `design::mono` — so padding with spaces aligns them exactly, and it does so inside the two
    // runs the line already has instead of adding a run per column. That is what makes the prices
    // line up under each other the way the reference terminal's column does.
    let name_w = cells.iter().map(|c| c.label.chars().count()).max().unwrap_or(0);
    let price_w = cells.iter().map(|c| c.price.chars().count()).max().unwrap_or(0);
    let pct_w = cells.iter().map(|c| c.pct.chars().count()).max().unwrap_or(0);
    for (n, cell) in cells.into_iter().enumerate() {
        // The venue's NAME is this line's prefix: it is the word, the rest is the figure, and a
        // value-only colour then paints the price and the spread while the venue stays readable.
        let prefix = format!("{:<name_w$} ", cell.label);
        let mut text = String::new();
        if !cell.price.is_empty() {
            // Prices right-align, so their decimal points stand in one line; a name left-aligns,
            // because a word read left to right does.
            text.push_str(&format!("{:>price_w$}", cell.price));
        }
        if !cell.pct.is_empty() {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&format!("{:>pct_w$}", cell.pct));
        }
        if cell.blocked {
            text.push_str(" ⛔");
        }
        out.push(LabelText {
            row: row_ix,
            part: ARB_PART_BASE + n,
            text,
            prefix,
            reachable: inputs.arb_reachable.iter().any(|(code, dex)| match cell.dex.is_empty() {
                // The same rule the click uses to find a core: an ordinary exchange matches by
                // platform code and must not match a core that has a dex; a deployer matches by
                // its dex name alone, since every deployer shares one code.
                true => *code == cell.code && dex.is_empty(),
                false => *dex == cell.dex,
            }),
            venue: Some((cell.code, cell.dex)),
            // The SPREAD is what carries a direction here; the venue's own colour, when it has one,
            // overrides whatever the sign would have picked.
            sign: cell.sign,
            color: cell.color,
        });
    }
}

/// One arbitrage line before it is padded into a column.
struct ArbCell {
    /// Protocol platform code, and the DEX name when the venue is a deployer.
    code: u8,
    dex: String,
    label: String,
    price: String,
    pct: String,
    blocked: bool,
    sign: Option<DeltaSign>,
    color: Option<u32>,
}

/// A PRICE caption: the shared adaptive formatter, with the field's prefix when it asks for one.
///
/// Six fields print a price this way — bid, ask, mark, step, entry, liquidation — and spelling the
/// same pair of calls six times is how one of them ends up on a different formatter later.
fn price_label(v: f64) -> (String, Option<DeltaSign>) {
    (fmt::adaptive(v), None)
}

/// A value with no sign to state.
fn plain(text: &str) -> String {
    text.to_string()
}

/// One figure off the market readout, absent when the readout itself is.
fn figure(inputs: &LabelInputs, pick: impl Fn(&MarketFiguresReadout) -> Option<f64>) -> Option<f64> {
    inputs.figures.as_ref().and_then(pick)
}

/// The window figures this caption is configured to read.
fn window(
    inputs: &LabelInputs,
    part: &ChartLabelPart,
) -> Option<moon_core::market::WindowFigures> {
    let windows = inputs.windows.as_ref()?;
    windows.windows.get(part.window.index()).copied()
}

/// The coin's tags as one caption: `Seed · Alpha`.
///
/// One caption rather than one per tag, because the set is what is read — a coin is "a seed listing
/// that is also alpha" — and because a tag list that grew a column would push every figure beside
/// it off the pane.
fn tags_text(tags: &[CoinTag]) -> String {
    tags.iter()
        .map(|t| t.name())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Format a signed percentage, dropping a non-finite value.
///
/// The caption's colour THRESHOLD is applied here, where the sign is born: below it the caption
/// keeps the theme colour and still prints its figure. Doing it here rather than at draw time is
/// what keeps one rule for every by-sign percentage — the deltas, funding, the arbitrage spreads —
/// instead of a check per drawing site.
fn signed_pct_label(part: &ChartLabelPart, v: f64) -> Option<(String, Option<DeltaSign>)> {
    let (text, sign) = fmt::signed_pct(v, 2)?;
    Some((text, colored_sign(part.resolved_style().color_min_pct, v, sign)))
}

/// The sign a percentage is coloured by, or `None` when it is too small to be worth painting.
///
/// `None` is not "no sign": it is what [`super::RenderState::caption_color`] already reads as "keep
/// the theme colour", which is exactly what a figure below the threshold should do.
fn colored_sign(min_pct: f32, v: f64, sign: DeltaSign) -> Option<DeltaSign> {
    (v.abs() >= f64::from(min_pct)).then_some(sign)
}

/// Format a countdown as `2ч 05м`, `47м` or `<1м`, dropping one that has already elapsed.
///
/// A funding time in the past is not printed: the core republishes the next one within seconds, and
/// a negative countdown on screen reads as a stuck chart rather than as a stale field. Hours are
/// not carried past a day — funding intervals are hours, so a day-long remainder means the field is
/// wrong, and printing `27ч` says that more honestly than `1д 3ч`.
fn fmt_countdown(remaining_ms: i64) -> Option<String> {
    if remaining_ms < 0 {
        return None;
    }
    let total_min = remaining_ms / 60_000;
    let (hours, minutes) = (total_min / 60, total_min % 60);
    Some(match (hours, minutes) {
        (0, 0) => t!("chart_labels.funding_soon").to_string(),
        (0, m) => format!("{m}{}", t!("chart_labels.unit_minute")),
        (h, m) => format!(
            "{h}{} {m:02}{}",
            t!("chart_labels.unit_hour"),
            t!("chart_labels.unit_minute")
        ),
    })
}

/// Longest detect line kept, in characters.
///
/// Not a layout figure — the chart lays this caption out by WIDTH, and wraps it — but a bound on
/// what is shaped at all: a core is free to send a paragraph, and the text pass would measure every
/// glyph of it before deciding what fits.
///
/// The feed's own bound rather than a second literal: this caption WRAPS, so cutting it at one
/// line's worth would throw away exactly what the second and third lines exist to show, and there
/// is nothing left to cut below what the ring already keeps.
const DETECT_MSG_MAX: usize = moon_core::feed::DETECT_MSG_KEEP;

/// A detect line without the `(strategy <NAME>)` the core ends every one of them with.
///
/// The core writes that tail for its own log, where nothing else says which strategy fired. On a
/// chart it is the widest part of the line and says the least: the strategy has its own caption
/// beside this one, and what a reader wants from THIS caption is the numbers the detect fired on.
///
/// The tail is recognised only where it actually sits, so a line that MENTIONS a strategy
/// mid-sentence keeps every word of it. The format arrives on `moon_core::feed::DetectRow::msg`.
fn detect_line(s: &str) -> String {
    let body = s.trim_end();
    let Some(at) = strategy_tail_start(body) else {
        return body.to_string();
    };
    // What is left of a line that carried nothing else is its own opening — "MoonStrike:" — and
    // that colon introduced a value which never existed on the wire. Only here: a line that keeps
    // its whole text keeps its own punctuation with it.
    body[..at]
        .trim_end()
        .trim_end_matches(':')
        .trim_end()
        .to_string()
}

/// Where a trailing `(strategy <NAME>)` begins, or `None` when the line does not end with one.
///
/// Anchored at BOTH ends: the group has to close the line, and what stands between the angle
/// brackets has to be a name and only a name. Matching the opening alone would cut a line off at
/// the first place it happened to say the word. Round brackets are NOT excluded — a user is free to
/// call a strategy `SP (long)`, and rejecting that would leave the tail on exactly those lines.
fn strategy_tail_start(body: &str) -> Option<usize> {
    const OPEN: &str = "(strategy <";
    let at = body.rfind(OPEN)?;
    let inner = body.get(at + OPEN.len()..)?.strip_suffix(">)")?;
    (!inner.contains(['<', '>'])).then_some(at)
}

/// Cut a core-supplied line to something a caption can carry.
fn cut(s: &str) -> String {
    if s.chars().count() <= DETECT_MSG_MAX {
        return s.to_string();
    }
    let kept: String = s.chars().take(DETECT_MSG_MAX).collect();
    format!("{}…", kept.trim_end())
}

fn non_empty(s: &str) -> Option<String> {
    (!s.trim().is_empty()).then(|| s.to_string())
}

/// The caption's own prefix — `"PnL: "`, `"Δ24ч: "` — or nothing when it prints none.
///
/// Built beside the value rather than glued onto it, because the two are COLOURED separately: a
/// by-sign caption paints the figure and leaves the word in the theme's colour. The words come from
/// the dictionary, like everything else this feature prints.
///
/// A window figure names itself with its window — two "Δ" captions on one line are unreadable
/// unless each says whether it is the minute or the day — which is the only variation in the rule.
fn caption_prefix(part: &ChartLabelPart, on: bool) -> String {
    let Some(key) = part.field.caption_key().filter(|_| on) else {
        return String::new();
    };
    let tail = match part.field.uses_window() {
        true => t!(part.window.locale_key()).to_string(),
        false => String::new(),
    };
    format!("{}{tail}: ", t!(key))
}

/// Collect the open-position figures for every basis in ONE pass over a market's orders.
///
/// The arithmetic is [`crate::order_math`]'s, not this module's: the Orders table, the Assets panel
/// and the chart's own overlay all state this number, and a second formula here would be a fourth
/// answer to the same question. In particular the entry price is the one the feed RESOLVED — the
/// raw `buy_price` is a break-even including round-trip commission — and every price flows through
/// a NaN-rejecting gate.
///
/// Args:
///     rows: The core's order rows, unfiltered.
///     market: Market key the pane is showing.
///
/// Returns:
///     Figures indexed by [`basis_index`], and the strategy name of the newest open order.
pub(in crate::chartdx) fn collect_open_stats(
    rows: &[moon_core::feed::OrderRow],
    market: &str,
) -> ([BasisStats; 3], String) {
    let mut out = [BasisStats::default(); 3];
    // Newest wins: uid increases with creation, so the caption names the strategy that acted last.
    let mut newest: Option<(u64, &str)> = None;
    for row in rows.iter().filter(|r| r.market == market && !r.job_is_done) {
        if !row.strat_name.is_empty() && newest.is_none_or(|(uid, _)| row.uid > uid) {
            newest = Some((row.uid, row.strat_name.as_str()));
        }
        // Every live row counts as an OPEN ORDER, whether or not it holds a position yet: that
        // figure is about orders, and a working entry is one.
        for basis in PnlBasis::ALL {
            if basis.accepts(row.emulator) {
                out[basis_index(basis)].open_orders += 1;
            }
        }
        // Exposure asks less than the PnL does: a size and a mark, no entry price. Each figure
        // degrades on its own inputs rather than dragging the others down with it — the rule the
        // chart overlay these captions replaced was careful about.
        let mark = f64::from(row.price);
        if let Some(qty) = position_qty(row).filter(|_| mark.is_finite() && mark > 0.0) {
            for basis in PnlBasis::ALL {
                if basis.accepts(row.emulator) {
                    let s = &mut out[basis_index(basis)];
                    s.exposure += qty * mark;
                    s.has_exposure = true;
                }
            }
        }
        let (Some(qty), Some(pnl)) = (position_qty(row), order_pnl(row)) else {
            continue;
        };
        let spent = row.buy_price * qty;
        for basis in PnlBasis::ALL {
            if !basis.accepts(row.emulator) {
                continue;
            }
            let s = &mut out[basis_index(basis)];
            s.pos_size += if row.is_short { -qty } else { qty };
            s.spent += spent;
            s.pnl_quote += pnl;
            s.has_position = true;
        }
    }
    let strategy = newest.map(|(_, name)| name.to_string()).unwrap_or_default();
    (out, strategy)
}

/// One caption of a preview line: what it prints and how it is styled.
///
/// The editor draws these itself — it is a dialog, not a chart — but it must not FORMAT them
/// itself: a preview built from its own spelling of "how a percentage looks" stops being a preview
/// the first time the real formatter changes.
pub(crate) struct PreviewCaption {
    /// Whether this line belongs to a COLUMN — an arbitrage venue — which stacks whatever the
    /// module's flow says. The preview has to know, or it shows a row of venues the chart will
    /// never draw.
    pub column: bool,
    /// The caption's prefix, drawn in the theme's colour beside the value — the same split the
    /// chart draws, so a preview cannot claim a caption prints something the chart does not.
    pub prefix: String,
    pub text: String,
    pub sign: Option<DeltaSign>,
    pub style: moon_core::config::ResolvedLabelStyle,
}

/// Format one row against SAMPLE values, for the editor's "how it will look" line.
///
/// Sample rather than live values, deliberately: the editor is where a caption is CHOSEN, and a
/// figure the current market happens not to have — no position, no funding on a spot market —
/// would print nothing and read as "this label is broken". Every field answers here.
pub(crate) fn preview_row(row: &moon_core::config::ChartLabelRow) -> Vec<PreviewCaption> {
    // A module switched off prints nothing, and the sample says so rather than showing what it
    // WOULD print: the editor's line answers "what will the chart show".
    if !row.is_drawn() {
        return Vec::new();
    }
    let inputs = sample_inputs();
    let preview_roster = preview_roster();
    let mut out = Vec::new();
    if row.show_name {
        if let Some(title) = crate::controls::row_title(row) {
            out.push(PreviewCaption {
                column: false,
                prefix: String::new(),
                text: title,
                sign: None,
                style: moon_core::config::ChartLabelRow::name_style(),
            });
        }
    }
    for part in &row.parts[..row.used_parts()] {
        if !part.visible {
            continue;
        }
        // A column caption previews as the COLUMN it prints — same expansion the chart uses, same
        // sample data — because "what will this print" is a list of venues, not one line saying
        // "arbitrage".
        if part.field.is_column() {
            let base = part.resolved_style();
            let mut lines = Vec::new();
            push_arb_rows(&mut lines, 0, &inputs, Some(&preview_roster), base);
            out.extend(lines.into_iter().map(|line| PreviewCaption {
                column: true,
                prefix: line.prefix,
                text: line.text,
                sign: line.sign,
                style: moon_core::config::ResolvedLabelStyle {
                    color: match line.color {
                        Some(rgb) => moon_core::config::LabelColor::Fixed(rgb),
                        None => base.color,
                    },
                    ..base
                },
            }));
            continue;
        }
        if let Some((text, sign)) = resolve(part, &inputs) {
            let style = part.resolved_style();
            out.push(PreviewCaption {
                column: false,
                prefix: caption_prefix(part, style.caption),
                text,
                sign,
                style,
            });
        }
    }
    out
}

/// The market the preview describes: one coin, in profit on the hour and down on the day, with two
/// orders open at a small loss.
///
/// Chosen so every figure has a value AND a sign — a preview where everything is positive hides
/// what the by-sign colour mode does.
/// The roster the preview arranges its sample column by: the shipped one.
fn preview_roster() -> ArbViewCfg {
    ArbViewCfg::default()
}

fn sample_inputs() -> LabelInputs {
    let stats = BasisStats {
        open_orders: 2,
        pos_size: 0.35,
        spent: 100.0,
        pnl_quote: -4.11,
        exposure: 495.96,
        has_exposure: true,
        has_position: true,
    };
    LabelInputs {
        ticker: "BTC-USDT".to_string(),
        core_name: "Core-1".to_string(),
        venue: "Binance".to_string(),
        quote: "USDT".to_string(),
        strategy: "Alpha".to_string(),
        detect_strategy: "BTC Sniper".to_string(),
        detect_msg: "Delta 5m 3.4% · vol x7".to_string(),
        last_price: Some(51234.5),
        scale_badge: Some(12),
        compare_pct: Some(1.2),
        delta_1h: Some(3.8),
        delta_24h: Some(-2.1),
        // Both sample venues count as connected, so the preview shows the column the way a
        // configured terminal sees it rather than dimmed throughout.
        arb_reachable: vec![(4, String::new()), (9, String::new())],
        // The sample column: two venues, one above this market and one below it, so the preview
        // shows both directions the spread can take.
        arb: vec![
            moon_core::market::ArbQuote {
                venue: moon_core::market::ArbVenue::from_code(4),
                dex_name: String::new(),
                price: 51_290.0,
                my_price: 51_234.5,
                spread_pct: 0.11,
                deposit_blocked: false,
                withdraw_blocked: false,
            },
            moon_core::market::ArbQuote {
                venue: moon_core::market::ArbVenue::from_code(9),
                dex_name: String::new(),
                price: 51_180.0,
                my_price: 51_234.5,
                spread_pct: -0.11,
                deposit_blocked: true,
                withdraw_blocked: false,
            },
        ],
        // Every new figure answers here too: a preview that printed nothing for a field the user
        // just picked reads as a broken label rather than as an empty market.
        figures: Some(moon_core::market::MarketFiguresReadout {
            bid: Some(51_230.0),
            ask: Some(51_239.0),
            mark: Some(51_236.0),
            price_step: Some(0.5),
            vol_24h: Some(184_000_000.0),
            max_leverage: Some(50),
            max_order: moon_core::market::MaxOrder {
                value: 2_000_000.0,
                source: moon_core::market::MaxOrderSource::Stated,
            },
            tags: vec![
                moon_core::market::CoinTag::Seed,
                moon_core::market::CoinTag::Alpha,
            ],
            pos_size: Some(0.35),
            pos_price: Some(50_980.0),
            liq_price: Some(41_120.0),
            leverage_x: Some(10),
            isolated: Some(true),
            session_pnl: Some(-12.40),
            coin_balance: Some(0.42),
        }),
        windows: Some(moon_core::market::MarketWindowsReadout {
            windows: [moon_core::market::WindowFigures {
                delta_pct: Some(0.58),
                volume_quote: Some(1_240_000.0),
                buy_share_pct: Some(56.0),
            }; moon_core::config::LABEL_WINDOW_COUNT],
        }),
        context: Some(moon_core::market::MarketContextReadout {
            exchange_1h_pct: 0.4,
            exchange_24h_pct: -1.1,
            btc_1h_pct: 0.9,
            btc_24h_pct: -0.6,
            btc_72h_pct: 4.75,
            funding_pct: Some(0.01),
            funding_at_ms: Some(5 * 3_600_000 + 33 * 60_000),
        }),
        // The countdown is measured against this, so the pair prints a fixed `5ч 33м`.
        now_ms: 0,
        basis: [stats; 3],
    }
}

#[cfg(test)]
mod tests;
