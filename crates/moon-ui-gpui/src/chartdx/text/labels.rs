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

use moon_core::config::{ChartLabelField, ChartLabelSlot, ChartLabelsCfg, PnlBasis};
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
    /// Index of the slot this came from, which is ALSO the index of its retained text run.
    ///
    /// Carried rather than implied by position: unresolved slots are skipped, so a position in the
    /// list is not a slot index, and addressing runs by position would hand a run a different
    /// string whenever a neighbour appeared or vanished — reshaping both of them.
    pub slot: usize,
    pub text: String,
    /// Sign of the value behind the text, for a slot that colors by it. `None` means the value has
    /// no meaningful sign and the slot keeps the caption color.
    pub sign: Option<DeltaSign>,
}

/// A pane's caption state: what it read, and what it formatted from it.
#[derive(Clone, Debug, Default)]
pub(in crate::chartdx) struct LabelState {
    /// Inputs the current [`Self::texts`] were formatted from.
    inputs: LabelInputs,
    /// Configuration the current [`Self::texts`] were formatted under.
    cfg: Option<ChartLabelsCfg>,
    /// Locale the current [`Self::texts`] were formatted in.
    ///
    /// Part of the cache key because a caption's short prefix comes from the dictionary and is
    /// BAKED into the stored string: without this a live language switch would leave the previous
    /// language on the chart until some unrelated input happened to move.
    locale: String,
    /// Formatted captions in slot order, skipping everything that resolved to nothing.
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
    pub(in crate::chartdx) fn update(&mut self, cfg: &ChartLabelsCfg, inputs: LabelInputs) -> bool {
        let locale = rust_i18n::locale().to_string();
        if self.cfg.as_ref() == Some(cfg) && self.inputs == inputs && self.locale == locale {
            return false;
        }
        self.locale = locale;
        self.cfg = Some(*cfg);
        self.inputs = inputs;
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.clear();
        for (ix, slot) in cfg.slots.iter().enumerate() {
            if !slot.is_drawn() {
                continue;
            }
            // A caption with nothing to say draws nothing and occupies no row. This is what lets
            // the comparison delta and the scale badge sit in the DEFAULT configuration without
            // leaving two blank rows on an ordinary chart.
            let Some((text, sign)) = resolve(slot, &self.inputs) else {
                continue;
            };
            scratch.push(LabelText {
                slot: ix,
                text,
                sign,
            });
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

/// Format one slot, or report that it has nothing to print.
fn resolve(slot: &ChartLabelSlot, inputs: &LabelInputs) -> Option<(String, Option<DeltaSign>)> {
    let caption = slot.resolved_style().caption;
    let stats = stats_for(inputs, slot.pnl_basis);
    match slot.field {
        ChartLabelField::None => None,
        ChartLabelField::Coin => non_empty(&inputs.ticker).map(|t| (t, None)),
        ChartLabelField::Core => non_empty(&inputs.core_name).map(|t| (t, None)),
        ChartLabelField::Venue => non_empty(&inputs.venue).map(|t| (t, None)),
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
            .and_then(|v| signed_pct_label(slot, caption, v)),
        ChartLabelField::Delta24h => inputs
            .delta_24h
            .and_then(|v| signed_pct_label(slot, caption, v)),
        ChartLabelField::OpenPnlPct => stats
            .pnl_pct()
            .and_then(|v| signed_pct_label(slot, caption, v)),
        ChartLabelField::OpenPnlMoney => stats.has_position.then(|| {
            let (text, sign) = fmt::signed_amount(stats.pnl_quote, MONEY_DECIMALS);
            (with_caption(slot, caption, &text), Some(sign))
        }),
        // Zero prints nothing rather than "0": the caption reports a position, and an empty corner
        // already says there is none.
        ChartLabelField::OpenOrders => (stats.open_orders > 0).then(|| {
            (
                with_caption(slot, caption, &stats.open_orders.to_string()),
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
                with_caption(slot, caption, &fmt::compact_si(stats.pos_size)),
                Some(sign),
            )
        }),
    }
}

/// Format a signed percentage with its optional caption, dropping a non-finite value.
fn signed_pct_label(
    slot: &ChartLabelSlot,
    caption: bool,
    v: f64,
) -> Option<(String, Option<DeltaSign>)> {
    let (text, sign) = fmt::signed_pct(v, 2)?;
    Some((with_caption(slot, caption, &text), Some(sign)))
}

fn non_empty(s: &str) -> Option<String> {
    (!s.trim().is_empty()).then(|| s.to_string())
}

/// Prefix a value with its field's short caption, when the slot asks for one.
///
/// The prefix comes from the dictionary rather than from a literal here: everything else this
/// feature prints is localized, and a caption drawn over the candles is the most visible text of
/// the lot.
fn with_caption(slot: &ChartLabelSlot, on: bool, value: &str) -> String {
    match slot.field.caption_key().filter(|_| on) {
        Some(key) => format!("{} {value}", t!(key)),
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
        // The rest needs a position and a usable pair of prices. A row that has neither degrades on
        // its own without dragging the count above down with it.
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

#[cfg(test)]
mod tests;
