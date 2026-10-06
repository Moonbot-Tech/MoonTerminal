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

use moon_core::config::{ARB_PART_BASE, ArbViewCfg, FILTER_HEADER_PART};
use moon_core::config::{
    ChartLabelField, ChartLabelPart, ChartLabelsCfg, LabelSpan, PnlBasis, ROW_NAME_PART,
    SpanAnchor, VolumeUnits,
};
use moon_core::market::{
    ArbQuote, CoinTag, LiqSpanReadout, MarketFiguresReadout, MarketWindowsReadout, VolumeAt,
    VolumeSpan, VolumeSpanReadout,
};
use moon_core::util::fmt::{self, DeltaSign};
use rust_i18n::t;

use super::column_scroll;

use moon_core::feed::order_math::{MONEY_DECIMALS, order_pnl, position_qty};

mod arb;
mod format;
mod inputs;
mod pnl;
mod preview;

use arb::{push_arb_rows, push_filter_rows};
pub(in crate::chartdx) use format::hours_and_minutes;
use format::{
    LOCK_CLOSED, LOCK_OPEN, STAR_OFF, STAR_ON, caption_prefix, colored_sign, cut, detect_line,
    figure, fmt_countdown, fmt_tf_countdown, liquidations, marked, no_cursor_dash, non_empty,
    plain, price_label, signed_pct_label, span_label, tags_text, volume, volume_amount, volume_bar,
    window,
};
pub(in crate::chartdx) use inputs::{ActionInputs, BasisStats, LabelInputs, basis_index};
pub(in crate::chartdx) use pnl::collect_open_stats;
pub(crate) use preview::preview_row;

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
    /// to the front of it. [`Self::glued`] is the one place the two are put back together.
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
    /// The proportion bar drawn beside this caption, if it asked for one.
    pub bar: Option<VolumeBar>,
    /// Whether a right-click on this caption opens the VOLUME menu.
    ///
    /// Set on every caption of a volume block, its heading included, so the whole block is one
    /// target: the reader aims at the figures, not at the one line that happens to name the period.
    /// The menu edits the module the caption belongs to, which [`Self::row`] identifies.
    pub volume_menu: bool,
    /// What a press on this caption does, including the strategy-filter header control.
    ///
    /// `None` on reporting captions. Carried on the text
    /// rather than looked up from the configuration by the drawing pass, exactly like
    /// [`Self::volume_menu`] beside it: the pass has the resolved captions and not the parts, and
    /// re-deriving a button's state there would need the pane's inputs a second time.
    pub action: Option<LabelAction>,
}

/// An action on a caption: a market-button overlay or the GPU-drawn filters header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::chartdx) enum LabelAction {
    /// Existing market controls retain their own overlay, geometry, and authorization.
    Market(ActionMark),
    /// Fold or unfold this caption's strategy-filter module.
    ToggleStrategyFilters,
}

impl LabelAction {
    /// Return only actions that reserve a market-button overlay instead of drawing GPU text.
    pub fn market(self) -> Option<ActionMark> {
        match self {
            Self::Market(mark) => Some(mark),
            Self::ToggleStrategyFilters => None,
        }
    }
}

/// A drawn caption that can be PRESSED, and what it would do.
///
/// The caption's own CONFIGURATION and nothing else. Its state — armed, banned, allowed — is read
/// where the control is built, from the same values the label was formatted from: a mark carrying
/// a copy would be a second generation of the same fact, and the button would say one thing while
/// its plate said the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::chartdx) struct ActionMark {
    /// What pressing it does. How long a ban runs is asked at the press, by the button's own popup,
    /// so nothing about a span is carried here.
    pub action: moon_core::config::ChartAction,
}

/// A buy/sell proportion bar, before it has any geometry.
///
/// Carried as a SHARE rather than as a finished rectangle because the caption pass has no geometry
/// yet: where a line lands is decided while it is drawn, and the bar is placed from the same
/// measurement as the text beside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::chartdx) struct VolumeBar {
    /// Share of the period's whole traded value this side accounts for, `0.0..=1.0`.
    ///
    /// Always read from the QUOTE amounts, whatever unit the figures are printed in: a proportion
    /// is the same proportion in either currency, and the quote halves are the pair that is exact
    /// over every span.
    pub fill: f32,
    /// Whether this is the SELLING side, which decides the colour it is drawn in.
    ///
    /// The side rather than the colour itself: the caption layer has no theme, and the order book's
    /// own bid/ask colours live where the rectangles are actually built.
    pub sell: bool,
}

impl LabelText {
    /// Prefix and value as ONE string, the way a caption is measured when it is not drawn split.
    ///
    /// The two are kept apart because they are COLOURED apart; every place that needs to know how
    /// wide the pair is glues them here rather than spelling the concatenation out again.
    pub fn glued(&self) -> String {
        format!("{}{}", self.prefix, self.text)
    }
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
    /// Advances whenever the formatted texts change.
    pub generation: u64,
    /// Reusable build buffer, so a re-format that changes nothing costs no allocation.
    scratch: Vec<LabelText>,
}

impl LabelState {
    /// How many lines the scrollable column of label row `row_ix` holds, unwindowed.
    ///
    /// `None` when that row draws no column under the configuration last formatted. Read from the
    /// same inputs and roster the column was built from, so the scroll range and the drawn window
    /// agree.
    pub(in crate::chartdx) fn column_len(&self, row_ix: usize) -> Option<usize> {
        let row = self.cfg.as_ref()?.rows.get(row_ix)?;
        let part = row
            .parts
            .iter()
            .find(|part| part.is_drawn() && part.field.is_column())?;
        match part.field {
            ChartLabelField::ArbColumn => {
                Some(self.arb_view.as_ref()?.arrange(&self.inputs.arb).len())
            }
            ChartLabelField::StrategyFilters => Some(
                self.inputs
                    .filter_lines
                    .iter()
                    .filter(|line| !line.is_empty())
                    .count(),
            ),
            _ => None,
        }
    }
}

impl LabelState {
    /// Synchronize held inputs in place and re-format only when inputs or metadata changed.
    /// The callback returns whether any input changed; the result reports drawn text changes.
    pub(in crate::chartdx) fn update_with(
        &mut self,
        cfg: &Rc<ChartLabelsCfg>,
        arb_view: &Rc<ArbViewCfg>,
        sync: impl FnOnce(&mut LabelInputs) -> bool,
    ) -> bool {
        let inputs_changed = sync(&mut self.inputs);
        let locale = rust_i18n::locale().to_string();
        let same_cfg = self.cfg.as_ref().is_some_and(|held| Rc::ptr_eq(held, cfg));
        let same_view = self
            .arb_view
            .as_ref()
            .is_some_and(|held| Rc::ptr_eq(held, arb_view));
        if same_cfg && same_view && !inputs_changed && self.locale == locale {
            return false;
        }
        self.locale = locale;
        self.cfg = Some(cfg.clone());
        self.arb_view = Some(arb_view.clone());
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.clear();
        for (row_ix, row) in cfg.rows.iter().enumerate() {
            // THE gate for a whole module: its own switch, and the question of whether anything on
            // it would print. Asked here rather than in the layout pass because a row that resolves
            // to nothing must also cost nothing downstream — no rows collected, no runs addressed.
            if !row.is_drawn() {
                continue;
            }
            // Whether this module prints volume at all, which is what makes its whole block —
            // heading included — a right-click target for the period menu.
            let row_reads_volume = row.parts[..row.used_parts()]
                .iter()
                .any(|p| p.is_drawn() && p.field.in_volume_block());
            // The row's own name leads its captions, which is where a reader looks for what the
            // row IS before reading the figures on it.
            let title = row
                .show_name
                .then(|| crate::controls::row_title(row))
                .flatten();
            if let Some(title) = title {
                scratch.push(LabelText {
                    row: row_ix,
                    part: ROW_NAME_PART,
                    text: title,
                    prefix: String::new(),
                    reachable: false,
                    venue: None,
                    sign: None,
                    color: None,
                    bar: None,
                    // A module's own NAME is part of its block, so the menu opens from it too.
                    volume_menu: row_reads_volume,
                    // Names are not buttons: a press must land on the control they name.
                    action: None,
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
                        let first = super::first_of(&self.inputs.column_scroll, row_ix);
                        match part.field {
                            ChartLabelField::ArbColumn => push_arb_rows(
                                &mut scratch,
                                row_ix,
                                &self.inputs,
                                self.arb_view.as_deref(),
                                part.resolved_style(),
                                first,
                            ),
                            ChartLabelField::StrategyFilters => push_filter_rows(
                                &mut scratch,
                                row_ix,
                                row,
                                &self.inputs.filter_lines,
                                first,
                            ),
                            _ => {}
                        }
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
                    prefix: caption_prefix(
                        part,
                        part.resolved_style().caption,
                        self.inputs.chart_tf_ms,
                    ),
                    reachable: false,
                    venue: None,
                    sign,
                    color: None,
                    bar: volume_bar(part, &self.inputs),
                    volume_menu: row_reads_volume && part.field.in_volume_block(),
                    action: action_mark(part.field),
                });
            }
        }
        // The inputs moved, but the drawn result often does not: a last price that ticked inside
        // its rounding, an order revision that changed a line nothing prints. Comparing the
        // FORMATTED result is what keeps those from repainting the pane.
        let changed = scratch != self.texts;
        if changed {
            self.generation = self.generation.wrapping_add(1);
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

/// Whether this caption is a button, and what pressing it would do.
///
/// Args:
///     field: The caption's field.
///
/// Returns:
///     The mark, or `None` for a caption that only reports.
fn action_mark(field: ChartLabelField) -> Option<LabelAction> {
    Some(LabelAction::Market(ActionMark {
        action: field.action()?,
    }))
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
        ChartLabelField::DetectStrategy => non_empty(&inputs.detect_strategy).map(|t| (t, None)),
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
        // The three trade captions answer from the handed trade and from nowhere else: a chart with
        // none prints nothing, which is what keeps them silent on every live chart rather than
        // falling back to whatever the market is doing now.
        ChartLabelField::TradeStrategy => inputs
            .trade
            .as_ref()
            .and_then(|trade| non_empty(&trade.strategy))
            .map(|t| (t, None)),
        // Cut like the live detect line beside it, and for the same reason: this is the core's own
        // prose, the widest thing the chart prints, and the width budget downstream measures a
        // string that has already been shaped.
        ChartLabelField::TradeDetect => inputs
            .trade
            .as_ref()
            .and_then(|trade| non_empty(&trade.detect))
            .map(|t| (cut(&t), None)),
        ChartLabelField::TradeSellReason => inputs
            .trade
            .as_ref()
            .and_then(|trade| non_empty(&trade.sell_reason))
            .map(|t| (cut(&t), None)),
        ChartLabelField::ScaleBadge => inputs.scale_badge.map(|pct| {
            // A range that rounds below a whole percent reads as "<1%", never as zero:
            // zero would claim the chart has no vertical span at all.
            let text = if pct == 0 {
                "<1%".to_string()
            } else {
                format!("{pct}%")
            };
            (text, None)
        }),
        // The shared compact duration, the same one the chart shot's header prints for this figure.
        ChartLabelField::TimeScaleBadge => inputs
            .time_scale_s
            .map(|secs| (crate::display_text::fmt_duration_short(secs as f64), None)),
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
            (super::fmt_pct(pct), colored_sign(min, f64::from(pct), sign))
        }),
        ChartLabelField::LastPrice => inputs
            .last_price
            .filter(|p| p.is_finite() && *p > 0.0)
            .map(|p| (fmt::adaptive(f64::from(p)), None)),
        ChartLabelField::Delta1h => inputs.delta_1h.and_then(|v| signed_pct_label(part, v)),
        ChartLabelField::Delta24h => inputs.delta_24h.and_then(|v| signed_pct_label(part, v)),
        ChartLabelField::OpenPnlPct => stats.pnl_pct().and_then(|v| signed_pct_label(part, v)),
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
        // Measured against the shared clock floored to ITS OWN minute, both for the figure and for
        // the elapsed test that suppresses it - one clock, or the caption disappears at a different
        // instant than the one it counts to. Funding's target does not sit on the bucket grid, so
        // when a candle countdown beside this pulls the shared clock down to its second step, an
        // unfloored reading would shift the printed minute and move the moment the caption goes
        // away. The floor is a no-op while the shared clock is already on the minute, which is
        // every chart that draws no candle countdown - so this is exactly the behaviour that
        // shipped before one existed.
        ChartLabelField::FundingIn => inputs
            .context
            .and_then(|c| c.funding_at_ms)
            .and_then(|at| fmt_countdown(at - inputs.now_ms.div_euclid(60_000) * 60_000))
            .map(|text| (text, None)),
        // Always printed: the countdown asks the clock, not the market, so there is no state in
        // which it has nothing to say - which is why this arm has no `None` at all.
        // The three pressable captions. Their text is the button's LABEL, and the pane's own state
        // is what it says: a chart with nothing to act on prints none of them, an armed panic
        // offers to stop instead of to start, and a ban that is running prints what is left of it.
        ChartLabelField::ActCancelBuy => inputs
            .actions
            .live
            .then(|| (t!("chart_labels.act.cancel_buy").to_string(), None)),
        ChartLabelField::ActPanicSell => inputs.actions.live.then(|| {
            let key = match inputs.actions.panic_armed {
                true => "chart_labels.act.stop_panic",
                false => "chart_labels.act.panic_sell",
            };
            (t!(key).to_string(), None)
        }),
        // The star, and nothing but the star: FILLED while the coin is marked, hollow while it is
        // not — and hollow too while the core has not said, which is the state its button is
        // disabled in rather than one the picture can express.
        ChartLabelField::ActFavorite => inputs.actions.live.then(|| {
            let glyph = match inputs.actions.favorite {
                Some(true) => STAR_ON,
                Some(false) | None => STAR_OFF,
            };
            (glyph.to_string(), None)
        }),
        // The lock, and nothing but the lock: OPEN while the coin trades, CLOSED while it is
        // banned. A glyph rather than a word for the same reason the chart's own pin and lock
        // buttons are glyphs — the state IS the picture, and a label beside it would repeat it.
        ChartLabelField::ActTempBan => inputs.actions.live.then(|| {
            let glyph = match inputs.actions.ban_until_ms.is_some() {
                true => LOCK_CLOSED,
                false => LOCK_OPEN,
            };
            (glyph.to_string(), None)
        }),
        // What is left of that ban, as a caption of its own — placed wherever the reader wants it,
        // and silent while nothing is banned. Through the shared formatter, which is what keeps
        // this caption, the coin menu's row and the dropdown's ban tab printing one figure.
        ChartLabelField::TempBanLeft => inputs.actions.ban_until_ms.map(|until| {
            (
                crate::display_text::fmt_ban_left(until - inputs.now_ms),
                None,
            )
        }),
        ChartLabelField::TfCloseIn => Some((
            fmt_tf_countdown(part.tf.remaining_ms(inputs.chart_tf_ms, inputs.now_ms)),
            None,
        )),
        ChartLabelField::OpenOrders => {
            (stats.open_orders > 0).then(|| (stats.open_orders.to_string(), None))
        }
        ChartLabelField::Exposure => stats
            .has_exposure
            .then(|| (plain(&fmt::compact_si(stats.exposure)), None)),
        // Expanded before this point, into its own run range: one caption, a dozen lines. Reaching
        // here would mean the expansion was skipped, and a single line saying "arbitrage" is not
        // what the caption is for.
        ChartLabelField::ArbColumn | ChartLabelField::StrategyFilters => None,
        ChartLabelField::CoinTags => inputs
            .figures
            .as_ref()
            .filter(|f| !f.tags.is_empty())
            .map(|f| (tags_text(&f.tags), None)),
        ChartLabelField::Bid => figure(inputs, |f| f.bid).map(price_label),
        ChartLabelField::Ask => figure(inputs, |f| f.ask).map(price_label),
        // The spread needs BOTH sides and a sane pair: a crossed book — which a stale snapshot can
        // show for a moment — would otherwise print a negative spread as if it were an arbitrage.
        ChartLabelField::Spread => inputs.figures.as_ref().and_then(|f| {
            let (bid, ask) = (f.bid?, f.ask?);
            let pct = (ask > bid).then(|| (ask - bid) / ask * 100.0)?;
            let (text, _) = fmt::pct(pct, 2)?;
            Some((text, None))
        }),
        ChartLabelField::MarkPrice => figure(inputs, |f| f.mark).map(price_label),
        // The deviation is read against the price the CHART is drawing, so the caption cannot
        // disagree with the candle beside it.
        ChartLabelField::MarkDelta => {
            let mark = figure(inputs, |f| f.mark)?;
            let last = f64::from(inputs.last_price.filter(|p| p.is_finite() && *p > 0.0)?);
            signed_pct_label(part, (mark - last) / last * 100.0)
        }
        ChartLabelField::PriceStep => figure(inputs, |f| f.price_step).map(price_label),
        ChartLabelField::Volume24h => {
            figure(inputs, |f| f.vol_24h).map(|v| (plain(&fmt::compact_si(v)), None))
        }
        ChartLabelField::WindowDelta => window(inputs, part)
            .and_then(|w| w.delta_pct)
            .and_then(|v| fmt::pct(v, 2))
            .map(|(text, _)| (text, None)),
        // Every traded amount comes from ONE readout, so the total a chart prints is always the sum
        // of the two halves printed beside it. A measuring caption with no pointer prints a dash —
        // see `no_cursor_dash`, which is the same answer for every figure in the block.
        // The total takes the deepest source that can answer it — see `total_quote_stated`, which
        // is why this one caption can be whole over a day while the sides beside it are marked.
        ChartLabelField::WindowVolume => volume(inputs, part)
            .map(|v| {
                let (quote, whole) = v.total_quote_stated();
                match part.units {
                    VolumeUnits::Quote => (marked(fmt::compact_si(quote), whole), None),
                    // The coin total has no deep source — the candle ring carries value only — so
                    // it is stated on the sides' own terms.
                    VolumeUnits::Base => volume_amount(part, v, quote, v.total_base()),
                }
            })
            .or_else(|| no_cursor_dash(part)),
        ChartLabelField::WindowBuyVolume => volume(inputs, part)
            .map(|v| volume_amount(part, v, v.buy_quote, v.buy_base))
            .or_else(|| no_cursor_dash(part)),
        ChartLabelField::WindowSellVolume => volume(inputs, part)
            .map(|v| volume_amount(part, v, v.sell_quote, v.sell_base))
            .or_else(|| no_cursor_dash(part)),
        ChartLabelField::WindowTrades => volume(inputs, part)
            .map(|v| {
                (
                    marked(fmt::compact_si(f64::from(v.trades)), v.complete),
                    None,
                )
            })
            .or_else(|| no_cursor_dash(part)),
        ChartLabelField::WindowLiquidations => liquidations(inputs, part)
            .map(|liq| {
                let value = match part.units {
                    VolumeUnits::Quote => liq.quote,
                    VolumeUnits::Base => liq.base,
                };
                let text = fmt::compact_si(value);
                let text = match liq.complete {
                    true => text,
                    false => format!("~{text}"),
                };
                (text, None)
            })
            .or_else(|| no_cursor_dash(part)),
        // The block's own heading. It prints even before any history arrives — the period is a
        // SETTING, not a reading, and a heading that vanished would take the right-click target
        // with it.
        ChartLabelField::WindowSpanName => Some((span_label(part), None)),
        ChartLabelField::WindowBuyShare => volume(inputs, part)
            .and_then(|v| v.buy_share_pct())
            .and_then(|v| fmt::pct(v, 1))
            .map(|(text, _)| (text, None))
            .or_else(|| no_cursor_dash(part)),
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
        ChartLabelField::ExchPosSize => inputs.figures.as_ref().and_then(|f| f.pos_size).map(|v| {
            let sign = if v >= 0.0 {
                DeltaSign::Positive
            } else {
                DeltaSign::Negative
            };
            (plain(&fmt::compact_si(v)), Some(sign))
        }),
        ChartLabelField::LiqPrice => figure(inputs, |f| f.liq_price).map(price_label),
        ChartLabelField::Leverage => inputs
            .figures
            .as_ref()
            .and_then(|f| f.leverage_x)
            .map(|v| (format!("x{v}"), None)),
        ChartLabelField::MarginMode => {
            inputs.figures.as_ref().and_then(|f| f.isolated).map(|iso| {
                let key = if iso {
                    "chart_labels.margin.isolated"
                } else {
                    "chart_labels.margin.cross"
                };
                (t!(key).to_string(), None)
            })
        }
        // A zero prints NOTHING. The core leaves this counter at zero on part of its venues while
        // MoonBot itself shows a figure there, so a printed `+0` would state "traded to break even"
        // about a coin that was nothing of the sort. Judged on the ROUNDED value, which is also
        // what hides a counter arriving in a unit two decimals cannot show — a coin-margined core
        // reports fractions of a BTC.
        ChartLabelField::SessionPnl => {
            inputs
                .figures
                .as_ref()
                .and_then(|f| f.core_pnl)
                .and_then(|v| {
                    let (text, sign) = fmt::signed_amount(v, MONEY_DECIMALS);
                    (sign != DeltaSign::Zero).then_some((text, Some(sign)))
                })
        }
        // A zero DOES print here, unlike the counter above: the core states this one as a snapshot
        // of its own, so a zero is "nothing earned since the reset" and absence is "this core does
        // not publish it" — the readout already carries that difference as `None`.
        //
        // The sign is dropped once the amount rounds to zero. `signed_amount` picks its prefix from
        // the ROUNDED value, so a loss of a third of a cent would otherwise render `+0` — a minus
        // wearing a plus. Bare `0` states the same magnitude without claiming a direction.
        ChartLabelField::SessionProfit => {
            inputs.figures.as_ref().and_then(|f| f.session).map(|v| {
                match fmt::signed_amount(v, MONEY_DECIMALS) {
                    (_, DeltaSign::Zero) => ("0".to_string(), Some(DeltaSign::Zero)),
                    (text, sign) => (text, Some(sign)),
                }
            })
        }
        ChartLabelField::CoinBalance => {
            figure(inputs, |f| f.coin_balance).map(|v| (plain(&fmt::compact_si(v)), None))
        }
        ChartLabelField::PosSize => (stats.pos_size != 0.0).then(|| {
            let sign = if stats.pos_size >= 0.0 {
                DeltaSign::Positive
            } else {
                DeltaSign::Negative
            };
            (plain(&fmt::compact_si(stats.pos_size)), Some(sign))
        }),
    }
}

#[cfg(test)]
mod tests;
