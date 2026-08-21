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

use moon_core::config::{ChartLabelField, ChartLabelPart, ChartLabelsCfg, PnlBasis, ROW_NAME_PART};
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
    /// Sign of the value behind the text, for a caption that colors by it. `None` means the value
    /// has no meaningful sign and the caption keeps the theme color.
    pub sign: Option<DeltaSign>,
}

/// A pane's caption state: what it read, and what it formatted from it.
#[derive(Clone, Debug, Default)]
pub(in crate::chartdx) struct LabelState {
    /// Inputs the current [`Self::texts`] were formatted from.
    inputs: LabelInputs,
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
        inputs: LabelInputs,
    ) -> bool {
        let locale = rust_i18n::locale().to_string();
        let same_cfg = self.cfg.as_ref().is_some_and(|held| Rc::ptr_eq(held, cfg));
        if same_cfg && self.inputs == inputs && self.locale == locale {
            return false;
        }
        self.locale = locale;
        self.cfg = Some(cfg.clone());
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
            if row.prints_name() {
                scratch.push(LabelText {
                    row: row_ix,
                    part: ROW_NAME_PART,
                    text: row.name.clone(),
                    sign: None,
                });
            }
            for (part_ix, part) in row.parts.iter().enumerate() {
                if !part.is_drawn() {
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
                    sign,
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

/// Format one caption, or report that it has nothing to print.
fn resolve(part: &ChartLabelPart, inputs: &LabelInputs) -> Option<(String, Option<DeltaSign>)> {
    let caption = part.resolved_style().caption;
    let stats = stats_for(inputs, part.pnl_basis);
    match part.field {
        ChartLabelField::None => None,
        ChartLabelField::Coin => non_empty(&inputs.ticker).map(|t| (t, None)),
        ChartLabelField::Core => non_empty(&inputs.core_name).map(|t| (t, None)),
        ChartLabelField::Venue => non_empty(&inputs.venue).map(|t| (t, None)),
        ChartLabelField::Quote => non_empty(&inputs.quote).map(|t| (t, None)),
        ChartLabelField::OrderStrategy => non_empty(&inputs.strategy).map(|t| (t, None)),
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
            (super::fmt_pct(pct), Some(sign))
        }),
        ChartLabelField::LastPrice => inputs
            .last_price
            .filter(|p| p.is_finite() && *p > 0.0)
            .map(|p| (fmt::adaptive(f64::from(p)), None)),
        ChartLabelField::Delta1h => inputs
            .delta_1h
            .and_then(|v| signed_pct_label(part, caption, v)),
        ChartLabelField::Delta24h => inputs
            .delta_24h
            .and_then(|v| signed_pct_label(part, caption, v)),
        ChartLabelField::OpenPnlPct => stats
            .pnl_pct()
            .and_then(|v| signed_pct_label(part, caption, v)),
        ChartLabelField::OpenPnlMoney => stats.has_position.then(|| {
            let (text, sign) = fmt::signed_amount(stats.pnl_quote, MONEY_DECIMALS);
            (with_caption(part, caption, &text), Some(sign))
        }),
        // Zero prints nothing rather than "0": the caption reports a position, and an empty corner
        // already says there is none.
        ChartLabelField::ExchangeDelta1h => inputs
            .context
            .and_then(|c| signed_pct_label(part, caption, c.exchange_1h_pct)),
        ChartLabelField::ExchangeDelta24h => inputs
            .context
            .and_then(|c| signed_pct_label(part, caption, c.exchange_24h_pct)),
        ChartLabelField::BtcDelta1h => inputs
            .context
            .and_then(|c| signed_pct_label(part, caption, c.btc_1h_pct)),
        ChartLabelField::BtcDelta24h => inputs
            .context
            .and_then(|c| signed_pct_label(part, caption, c.btc_24h_pct)),
        ChartLabelField::BtcDelta72h => inputs
            .context
            .and_then(|c| signed_pct_label(part, caption, c.btc_72h_pct)),
        // A market that charges no funding prints nothing: a zero there would read as "free",
        // which is a different claim from "this venue has no funding at all".
        ChartLabelField::Funding => inputs
            .context
            .and_then(|c| c.funding_pct)
            .and_then(|v| signed_pct_label(part, caption, v)),
        ChartLabelField::FundingIn => inputs
            .context
            .and_then(|c| c.funding_at_ms)
            .and_then(|at| fmt_countdown(at - inputs.now_ms))
            .map(|text| (with_caption(part, caption, text.as_str()), None)),
        ChartLabelField::OpenOrders => (stats.open_orders > 0).then(|| {
            (
                with_caption(part, caption, &stats.open_orders.to_string()),
                None,
            )
        }),
        ChartLabelField::Exposure => stats.has_exposure.then(|| {
            (
                with_caption(part, caption, &fmt::compact_si(stats.exposure)),
                None,
            )
        }),
        ChartLabelField::PosSize => (stats.pos_size != 0.0).then(|| {
            let sign = if stats.pos_size >= 0.0 {
                DeltaSign::Positive
            } else {
                DeltaSign::Negative
            };
            (
                with_caption(part, caption, &fmt::compact_si(stats.pos_size)),
                Some(sign),
            )
        }),
    }
}

/// Format a signed percentage with its optional caption, dropping a non-finite value.
fn signed_pct_label(
    part: &ChartLabelPart,
    caption: bool,
    v: f64,
) -> Option<(String, Option<DeltaSign>)> {
    let (text, sign) = fmt::signed_pct(v, 2)?;
    Some((with_caption(part, caption, &text), Some(sign)))
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

fn non_empty(s: &str) -> Option<String> {
    (!s.trim().is_empty()).then(|| s.to_string())
}

/// Prefix a value with its field's short caption, when the part asks for one.
///
/// The prefix comes from the dictionary rather than from a literal here: everything else this
/// feature prints is localized, and a caption drawn over the candles is the most visible text of
/// the lot.
fn with_caption(part: &ChartLabelPart, on: bool, value: &str) -> String {
    match part.field.caption_key().filter(|_| on) {
        Some(key) => format!("{}: {value}", t!(key)),
        None => value.to_string(),
    }
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
    let mut out = Vec::new();
    if row.prints_name() {
        out.push(PreviewCaption {
            text: row.name.clone(),
            sign: None,
            style: moon_core::config::ChartLabelRow::name_style(),
        });
    }
    for part in &row.parts[..row.used_parts()] {
        if !part.visible {
            continue;
        }
        if let Some((text, sign)) = resolve(part, &inputs) {
            out.push(PreviewCaption {
                text,
                sign,
                style: part.resolved_style(),
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
        last_price: Some(51234.5),
        scale_badge: Some(12),
        compare_pct: Some(1.2),
        delta_1h: Some(3.8),
        delta_24h: Some(-2.1),
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
