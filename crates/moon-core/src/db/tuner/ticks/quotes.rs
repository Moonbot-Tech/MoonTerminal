//! The core's ticker around one trade — BID, ASK and the mark price — as the market list
//! recorded it (`market::price_recorder`), where the tape alone could only stand a print in for
//! the book.
//!
//! What the list is, measured 2026-10-31 (`docs-internal/TUNER_MARK_BIDASK.md`, one day of the
//! recording, the stop reasons that print the core's own `BID = … ASK: …`):
//!
//! - it IS the ticker the core's book-watching stop reads: its BID/ASK at a stop's activation were
//!   the stop reason's to the bit on 12 of 16 trades, where the last print of each side was within
//!   0.5 bp on 15 % of 1 184 even at its best moment;
//! - a point is stamped when the list ARRIVED, about a second after the core held it
//!   ([`LIST_DELAY_MS`]);
//! - the core's Last is the middle of the two (`LastPrice = (BID + ASK) / 2` in every stop reason
//!   that prints both), and its mark delta is `(middle − mark) / middle · 100`, which the list
//!   reproduces to the bit on about half the trades at the report's stamp — the rest move by what
//!   the book moved inside one list period.
//!
//! The recording holds only what changed, so an arrival that changed nothing leaves no point; a
//! reader samples between points on its own clock. Outside the recorded stretches nothing is
//! known, and every reader falls back to what it did before the recording existed.

use std::sync::Arc;

use super::Deal;
use super::deltas::snapshot_ms;
use crate::market::price_recorder::{self, PriceSpan};

/// How long after the core held a list's prices the terminal stamped their arrival, ms. Aligning
/// the list with the prints (Binance and Bybit: +1.5 s, Hyperliquid +2 s), with the stop reasons'
/// BID/ASK (+0.5 … +1.7 s, median +1 s) and with the report's `dmark` (+0.5 … +1.5 s) — the list
/// period averaged in. One value for every venue until a calibration names one per venue.
pub const LIST_DELAY_MS: i64 = 1_000;

/// How far the mark delta the list gives at the report's stamp may sit from the report's own
/// before the list is taken for another market's — per cent points. Inside one list period the
/// book moves the middle by up to a few tenths on a spike (Hook: 7 of 39 stamps past 0.2 pp).
pub const MARK_STAMP_TOLERANCE_PP: f64 = 0.5;

/// How far past the close the list is read: the stops' ticker samples up to the tape's tail.
const AFTER_CLOSE_MS: i64 = 60_000;

/// The ticker at one moment of the core's clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuotePoint {
    /// When the core held these prices: the arrival less [`LIST_DELAY_MS`].
    pub t_ms: i64,
    pub bid: f64,
    pub ask: f64,
    /// The mark price; zero when the list carried none.
    pub mark: f64,
}

impl QuotePoint {
    /// The middle of the spread — the core's Last.
    pub fn mid(&self) -> f64 {
        (self.bid + self.ask) / 2.0
    }

    /// The core's mark delta at this point, per cent: `(middle − mark) / middle · 100`; `None`
    /// without a mark.
    pub fn dmark(&self) -> Option<f64> {
        let mid = self.mid();
        (self.mark > 0.0 && mid > 0.0).then(|| (mid - self.mark) / mid * 100.0)
    }
}

/// One unbroken recorded stretch on the core's clock.
#[derive(Clone, Debug, PartialEq)]
struct Stretch {
    from_ms: i64,
    to_ms: i64,
    /// Ascending; the first holds from `from_ms`.
    points: Vec<QuotePoint>,
}

/// The ticker of one trade along its window — see the module doc.
#[derive(Clone, Debug, PartialEq)]
pub struct QuoteTrack {
    stretches: Vec<Stretch>,
    /// The report's `dmark` and the moment of the point in force at its stamp, when the list
    /// agrees with the report there ([`MARK_STAMP_TOLERANCE_PP`]); `None` keeps the snapshot.
    mark_anchor: Option<(i64, f64)>,
}

impl QuoteTrack {
    /// The track off the recorder's stretches for `deal`.
    ///
    /// Args:
    ///     spans: What the recording holds around the trade, on the arrival clock.
    ///     deal: The trade — its report `dmark` and the moment the report stamped it.
    ///
    /// Returns:
    ///     The track, or `None` when no stretch holds a usable price.
    pub fn new(spans: &[PriceSpan], deal: &Deal) -> Option<Self> {
        let mut stretches: Vec<Stretch> = Vec::new();
        for span in spans {
            let mut open: Option<Stretch> = None;
            for s in &span.samples {
                let t_ms = s.t_ms - LIST_DELAY_MS;
                let usable = s.bid > 0.0 && s.ask > 0.0 && s.bid.is_finite() && s.ask.is_finite();
                if !usable {
                    // A list without a usable book says nothing of it: the stretch ends where it
                    // arrived, as a hole would end it, rather than carry the price before it.
                    if let Some(mut stretch) = open.take() {
                        stretch.to_ms = t_ms - 1;
                        stretches.push(stretch);
                    }
                    continue;
                }
                let point = QuotePoint {
                    t_ms,
                    bid: s.bid,
                    ask: s.ask,
                    mark: if s.mark.is_finite() { s.mark } else { 0.0 },
                };
                match open.as_mut() {
                    Some(stretch) => stretch.points.push(point),
                    None => {
                        open = Some(Stretch {
                            from_ms: t_ms,
                            to_ms: t_ms,
                            points: vec![point],
                        })
                    }
                }
            }
            if let Some(mut stretch) = open {
                stretch.to_ms = (span.to_ms - LIST_DELAY_MS).max(stretch.from_ms);
                stretches.push(stretch);
            }
        }
        if stretches.is_empty() {
            return None;
        }
        let mut track = Self {
            stretches,
            mark_anchor: None,
        };
        track.mark_anchor = track.anchor_mark(deal);
        Some(track)
    }

    /// The point in force at the report's stamp, when the report filled `dmark` and the list
    /// agrees with it there.
    fn anchor_mark(&self, deal: &Deal) -> Option<(i64, f64)> {
        let report = deal.deltas.dmark;
        if report == 0.0 || !report.is_finite() {
            return None;
        }
        let point = self.at(snapshot_ms(deal)?)?;
        let listed = point.dmark()?;
        ((listed - report).abs() <= MARK_STAMP_TOLERANCE_PP).then_some((point.t_ms, report))
    }

    /// The ticker in force at `t_ms` on the core's clock; `None` outside every recorded stretch.
    pub fn at(&self, t_ms: i64) -> Option<&QuotePoint> {
        let stretch = self
            .stretches
            .iter()
            .find(|s| s.from_ms <= t_ms && t_ms <= s.to_ms)?;
        let after = stretch.points.partition_point(|p| p.t_ms <= t_ms);
        stretch.points.get(after.checked_sub(1)?)
    }

    /// The first recorded point after `after` and no later than `until`, inside a stretch.
    pub fn next_point(&self, after: i64, until: i64) -> Option<i64> {
        self.stretches
            .iter()
            .filter(|s| s.to_ms > after && s.from_ms <= until)
            .find_map(|s| {
                let i = s.points.partition_point(|p| p.t_ms <= after);
                s.points
                    .get(i)
                    .map(|p| p.t_ms)
                    .filter(|&t| t <= until && t <= s.to_ms)
            })
    }

    /// Whether the recording answers for the mark delta at all.
    pub fn mark_live(&self) -> bool {
        self.mark_anchor.is_some()
    }

    /// The core's mark delta at `t_ms`: the report's own at the point its stamp fell in — the
    /// report holds the core's exact figure there — and the list's elsewhere; `None` where the
    /// recording does not answer, which keeps the snapshot.
    pub fn dmark_at(&self, t_ms: i64) -> Option<f64> {
        let (stamp_point, report) = self.mark_anchor?;
        let point = self.at(t_ms)?;
        if point.t_ms == stamp_point {
            return Some(report);
        }
        point.dmark()
    }
}

/// The newer of the ticker's price on one side and a print on that side: a taker that printed
/// after the ticker's point traded at the book as it then stood.
///
/// Args:
///     quote: The ticker's point in force and its price on the side.
///     print: The last print on the side, with its moment.
pub fn newest(quote: Option<(i64, f64)>, print: Option<(i64, f64)>) -> Option<f64> {
    match (quote, print) {
        (Some((qt, q)), Some((pt, p))) => Some(if pt > qt { p } else { q }),
        (q, p) => q.or(p).map(|(_, price)| price),
    }
}

/// The recorded ticker of a trade, off the terminal's price file: from the run-up before its entry
/// order through a minute past its close.
///
/// Args:
///     exchange_key: The recorder's key of the trade's exchange (`"{code}:{dex:08x}"`).
///     market: The market as the core spells it.
///     deal: The trade.
///
/// Returns:
///     The track, `None` where nothing was recorded or the file could not be read.
pub fn read_for(exchange_key: &str, market: &str, deal: &Deal) -> Option<Arc<QuoteTrack>> {
    let (from, _) = super::deltas::eval_span(deal);
    let to = deal.close_ms.saturating_add(AFTER_CLOSE_MS);
    let spans = price_recorder::read(
        exchange_key,
        market,
        from.saturating_add(LIST_DELAY_MS),
        to.saturating_add(LIST_DELAY_MS),
    )
    .map_err(|e| log::debug!("[x] tuner quotes: {market} on {exchange_key}: {e}"))
    .ok()?;
    QuoteTrack::new(&spans, deal).map(Arc::new)
}

#[cfg(test)]
mod tests;
