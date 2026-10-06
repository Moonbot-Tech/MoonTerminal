//! Which markets gained stored prints, and in what order — for a reader that decided "no tape"
//! earlier and should ask again once some arrives.
//!
//! A tuner row on a venue with no public route (Bybit, Hyperliquid) is judged unservable the
//! moment its trade closes, when nothing is stored for it yet; the station's recording of the
//! live stream, or the core's archive, lands minutes later (2026-10-06: PONS on Bybit closed at
//! 13:34:53, the station's prints were filed at 13:45:46), and nothing told the row. A market is
//! marked where prints reach a store a held query reads: the cache worker after a write that
//! stored anything (the station's import lands only there), and the replay worker after it fills
//! its in-memory tiles with prints (the core's archive, a venue's walk), whatever the file did.
//! The log is a revision counter and, per `(exchange, market)`, the revision of its last mark. A
//! spare mark costs a reader one more ask of the store, never a wrong answer.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex, PoisonError};

/// The log: the running revision and each market's last one.
#[derive(Default)]
struct FiledLog {
    rev: u64,
    by_market: HashMap<(String, String), u64>,
}

/// One log for the process — the cache is one file, written by one worker.
static FILED: LazyLock<Mutex<FiledLog>> = LazyLock::new(Mutex::default);

/// The log under its lock. A panic elsewhere while holding it leaves a map and a counter that are
/// each still whole, so a poisoned lock is read through rather than propagated.
fn lock() -> std::sync::MutexGuard<'static, FiledLog> {
    FILED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Mark a market as having gained stored prints — called by the cache worker after a write that
/// stored anything (a span already held whole is no news), and by the replay worker for the tiles
/// it fills when there is no file to write.
pub(in crate::market::trade_replay) fn note(exchange: &str, market: &str) {
    let mut log = lock();
    log.rev += 1;
    let rev = log.rev;
    log.by_market
        .insert((exchange.to_string(), market.to_string()), rev);
}

/// The current revision and every market whose prints grew after `since`, read under one lock so
/// no write falls between the two. A caller keeps the revision it got and asks with it next time.
///
/// Args:
///     since: The revision the caller last saw; 0 for "ever, in this process".
///
/// Returns:
///     `(revision now, markets as (exchange, market))`.
pub fn filed_since(since: u64) -> (u64, HashSet<(String, String)>) {
    let log = lock();
    let fresh = log
        .by_market
        .iter()
        .filter(|&(_, &rev)| rev > since)
        .map(|(key, _)| key.clone())
        .collect();
    (log.rev, fresh)
}

#[cfg(test)]
mod tests;
