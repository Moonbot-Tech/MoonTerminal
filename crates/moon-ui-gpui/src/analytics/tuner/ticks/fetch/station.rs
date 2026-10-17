//! The station as the first source of the autoload (STATION.md §4.9): the station records the live
//! stream around every trade while the terminal is off, so what the terminal lacks of a closed
//! trade's tape is asked of it before a core's archive or a venue — those only get what it did
//! not hold.

use std::collections::HashMap;
use std::time::Duration;

use moon_core::market::trade_replay::{Coverage, ReplayWindow, trade_cache};
use moon_core::station_api::TapeWant;
use moon_remote::ssh::Target;

/// How long the pass waits for the station's prints to reach the file before it judges which
/// rows they covered.
const FILE_SYNC: Duration = Duration::from_secs(10);

/// What the file lacks of `rows`' windows, per market: the whole window the tuner reads, not only
/// what marks a row covered — a stretch the station holds costs no core or venue request later —
/// minus the spans already filed.
///
/// Args:
///     rows: The rows.
///     place: A row's `(exchange key, market, window)`.
///     held_spans: `(exchange, market, from_ms, to_ms)` → the filed spans' bounds; `None` when the
///         read did not happen, and then the whole window is asked.
fn lacking<T>(
    rows: &[T],
    place: impl Fn(&T) -> (String, String, ReplayWindow),
    held_spans: impl Fn(&str, &str, i64, i64) -> Option<Vec<(i64, i64)>>,
) -> Vec<TapeWant> {
    let mut wanted: HashMap<(String, String), Coverage> = HashMap::new();
    for row in rows {
        let (exchange, market, window) = place(row);
        let need = wanted
            .entry((exchange, market))
            .or_insert_with(Coverage::none);
        for &span in window.focus_spans().spans() {
            need.add(span);
        }
    }
    let mut wants: Vec<TapeWant> = wanted
        .into_iter()
        .filter_map(|((exchange, market), need)| {
            let (from_ms, to_ms) = need.hull()?;
            let held = held_spans(&exchange, &market, from_ms, to_ms)
                .map(Coverage::from_spans)
                .unwrap_or_else(Coverage::none);
            let lacking = need.minus(&held);
            (!lacking.is_empty()).then(|| TapeWant {
                exchange,
                market,
                spans: lacking.spans().to_vec(),
            })
        })
        .collect();
    wants.sort_by(|a, b| (&a.exchange, &a.market).cmp(&(&b.exchange, &b.market)));
    wants
}

/// What one fill brought.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Filled {
    /// Markets asked.
    pub markets: usize,
    /// Covered stretches filed, and the prints in them.
    pub pieces: usize,
    pub prints: usize,
    /// The station could not be reached at all: the next passes leave it out for a while rather
    /// than wait for it again on every retry.
    pub unreachable: bool,
}

/// Ask the station for every stretch of `rows`' windows the file does not hold, and file what it
/// answers as the station's. Blocking — SSH round trips — so it runs on the pass's own thread.
///
/// Args:
///     rows: The rows the file does not fully hold.
///     place: A row's `(exchange key, market, window)`.
///     target: The station's server.
///     stop: Asked before every request: a Stop, or the switches going off, ends the fill there.
pub(super) fn fill<T>(
    rows: &[T],
    place: impl Fn(&T) -> (String, String, ReplayWindow),
    target: &Target,
    stop: impl Fn() -> bool,
) -> Filled {
    let mut filled = Filled::default();
    // Tape persistence switched off: nowhere to file what the station holds.
    let Some(cache) = trade_cache::handle() else {
        return filled;
    };
    let wants = lacking(rows, place, |exchange, market, from_ms, to_ms| {
        cache.held_spans(exchange, market, from_ms, to_ms)
    });
    filled.markets = wants.len();
    if wants.is_empty() {
        return filled;
    }
    let pull = match moon_remote::station::pull::Pull::open(target) {
        Ok(pull) => pull,
        Err(e) => {
            log::warn!(
                target: moon_core::diagnostics::TICKS_AXIS_TARGET,
                "[x] ticks autoload: the station is not reachable, left out for ten minutes: {e:#}"
            );
            filled.unreachable = true;
            return filled;
        }
    };
    let pulled = pull.tape(wants, stop, |want, from_ms, to_ms, ticks| {
        filled.pieces += 1;
        filled.prints += ticks.len();
        cache.insert_from_station(&want.exchange, &want.market, from_ms, to_ms, ticks);
    });
    // Whatever came before an error or a stop is filed too: the pass judges the rows by the file
    // right after this.
    if filled.pieces > 0 && !cache.sync(FILE_SYNC) {
        log::warn!(
            target: moon_core::diagnostics::TICKS_AXIS_TARGET,
            "[x] ticks autoload: the station's tape did not reach the file in time; the rest is judged without it"
        );
    }
    match pulled {
        Ok(count) => log::info!(
            target: moon_core::diagnostics::TICKS_AXIS_TARGET,
            "[x] ticks autoload: the station answered {} stretch(es), {} print(s), for {} market(s) in {} request(s){}",
            count.pieces,
            count.prints,
            filled.markets,
            count.requests,
            if count.stopped { ", stopped" } else { "" }
        ),
        Err(e) => log::warn!(
            target: moon_core::diagnostics::TICKS_AXIS_TARGET,
            "[x] ticks autoload: the station's tape pull broke off after {} stretch(es): {e:#}",
            filled.pieces
        ),
    }
    filled
}

#[cfg(test)]
mod tests;
