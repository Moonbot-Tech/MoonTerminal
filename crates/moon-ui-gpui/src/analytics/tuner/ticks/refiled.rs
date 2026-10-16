//! Which carried rows a reload must ask the tape store about again.
//!
//! A reload carries every row the last tape stage judged — covered, or refused — instead of
//! reading it again (`load.rs`). A refusal is said of what the store held then: on a venue with no
//! public route the row is refused the moment its trade closes, and the station's recording or
//! the core's archive lands minutes later. Carried as is, such a row read "no tape" until the
//! process restarted while the trade pane drew its prints (PONS on Bybit, 2026-10-06). A refused
//! row whose market gained stored prints since it was judged is therefore not carried: the tape
//! stage asks the store for it again.
//!
//! The store can lift no route, past the venue's retention, and a publication wait — whoever said
//! them (`load.rs::unservable_status` at load, a walk, the worker's held answer). A refusal the
//! venue gave its own walk for another reason (an unknown symbol, a transient error) is the fetch
//! job's to retry, and resetting it would send the row into another walk whenever its market is
//! written to.
//!
//! A daily publication wait is also released by its UTC deadline without a market write, so
//! the next reload re-reads the tape and can queue autoload after the exchange's publish lag.

use std::collections::{HashMap, HashSet};

use moon_core::market::trade_replay::TickStatus;

use super::state::{DealRow, TapeStatus};

#[cfg(test)]
mod tests;

/// Re-read due publication waits or recoverable refusals whose market gained stored prints.
///
/// Args:
///     judged: The rows a reload would carry, by `reportuid`.
///     refiled: The markets filed since those rows were judged, as `(exchange key, market)` —
///         `trade_cache::filed_since`.
///     now_ms: UTC clock used to release an archive wait without requiring a tape write.
pub(super) fn drop_refiled(
    judged: HashMap<i64, DealRow>,
    refiled: &HashSet<(String, String)>,
    now_ms: i64,
) -> HashMap<i64, DealRow> {
    judged
        .into_iter()
        .filter(|(_, row)| {
            let due = matches!(row.tape,
                TapeStatus::Refused(TickStatus::ArchivePending { retry_at_ms }) if retry_at_ms <= now_ms);
            let refused = matches!(
                row.tape,
                TapeStatus::Refused(TickStatus::NoRoute | TickStatus::OutOfRetention { .. } | TickStatus::ArchivePending { .. })
            );
            let regained = row
                .address
                .as_ref()
                .is_some_and(|a| refiled.contains(&(a.exchange_key.clone(), a.market.clone())));
            !(due || (refused && regained))
        })
        .collect()
}
