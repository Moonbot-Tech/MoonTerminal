//! The recorded prices as the two chart lines a closed trade's window draws.
//!
//! The file keeps what the market list carried — bid, ask and mark. A chart draws two lines from
//! it: Last, which the core itself draws as the midpoint of bid and ask, and Mark. This module
//! turns the file's stretches into those lines once, off the UI thread, so a chart read only lays
//! ready points over its own.

use super::store::{self, PriceSpan};
use crate::feed::PricePoint;

/// One unbroken recorded stretch as chart points.
#[derive(Clone, Debug, Default)]
pub struct StoredPriceSpan {
    /// First millisecond the stretch speaks for.
    pub from_ms: i64,
    /// Last millisecond the stretch speaks for.
    pub to_ms: i64,
    /// The core's Last line — the midpoint of bid and ask — at each change, in time order. Each
    /// value holds until the next point, or to [`Self::to_ms`] for the last one.
    pub last: Vec<PricePoint>,
    /// The mark price at each change, held the same way.
    pub mark: Vec<PricePoint>,
}

/// What the price file holds of one market over a trade's window.
#[derive(Clone, Debug, Default)]
pub struct StoredPriceLines {
    /// The recorded stretches in time order. Inside one, its points are the whole truth; outside
    /// every one, nothing is known and the live rings keep the say.
    pub spans: Vec<StoredPriceSpan>,
}

impl StoredPriceLines {
    /// Whether the file held nothing of the window.
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }
}

/// The Last and Mark lines the file holds for `market` on `exchange_key` over
/// `[from_ms, to_ms]`. Empty when there is no file yet.
///
/// Opens a read-only connection; call it off the UI thread. Logs one line per call — a window is
/// opened by hand, so this is the line a live check reads to see what the file gave a trade.
pub fn load_lines(
    exchange_key: &str,
    market: &str,
    from_ms: i64,
    to_ms: i64,
) -> rusqlite::Result<StoredPriceLines> {
    let lines = lines_of(&store::read(exchange_key, market, from_ms, to_ms)?);
    log::info!(
        "price recorder: {market} on {exchange_key} [{from_ms}, {to_ms}]: {} stretch(es), {} last / {} mark point(s)",
        lines.spans.len(),
        lines.spans.iter().map(|s| s.last.len()).sum::<usize>(),
        lines.spans.iter().map(|s| s.mark.len()).sum::<usize>(),
    );
    Ok(lines)
}

/// Convert the file's stretches into chart lines.
///
/// A side that is missing or not a price skips that sample's Last point rather than drawing a
/// midpoint against zero: a market whose book is empty on one side has no Last, and half a price
/// on the chart would be a lie. The previous value then holds, as it does between changes.
fn lines_of(spans: &[PriceSpan]) -> StoredPriceLines {
    let spans = spans
        .iter()
        .map(|span| {
            let mut last = Vec::with_capacity(span.samples.len());
            let mut mark = Vec::with_capacity(span.samples.len());
            for sample in &span.samples {
                let time_ms = sample.t_ms as f64;
                push_change(&mut last, time_ms, drawable(mid(sample.bid, sample.ask)));
                push_change(&mut mark, time_ms, drawable(sample.mark));
            }
            StoredPriceSpan {
                from_ms: span.from_ms,
                to_ms: span.to_ms,
                last,
                mark,
            }
        })
        .collect();
    StoredPriceLines { spans }
}

/// Append `price` at `time_ms` when it is a price and differs from the line's last one; a sample
/// that moved only the other line, or carried no price for this one, leaves this line holding.
fn push_change(line: &mut Vec<PricePoint>, time_ms: f64, price: Option<f32>) {
    let Some(price) = price else {
        return;
    };
    if line.last().is_none_or(|p| p.price != price) {
        line.push(PricePoint { time_ms, price });
    }
}

/// The midpoint of a book's two sides, or NaN when either side is not a price.
fn mid(bid: f64, ask: f64) -> f64 {
    if valid(bid) && valid(ask) {
        (bid + ask) / 2.0
    } else {
        f64::NAN
    }
}

/// Whether `price` is a real price.
fn valid(price: f64) -> bool {
    price.is_finite() && price > 0.0
}

/// `price` narrowed to the chart's `f32`, or `None` when it is not a price or does not survive the
/// narrowing.
fn drawable(price: f64) -> Option<f32> {
    let narrowed = price as f32;
    (valid(price) && narrowed.is_finite() && narrowed > 0.0).then_some(narrowed)
}

#[cfg(test)]
mod tests;
