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
//! Only the two refusals the store can lift — no route, past the venue's retention — whoever said
//! them (`load.rs::unservable_status` at load, a walk, the worker's held answer). A refusal the
//! venue gave its own walk for another reason (an unknown symbol, a transient error) is the fetch
//! job's to retry, and resetting it would send the row into another walk whenever its market is
//! written to.

use std::collections::{HashMap, HashSet};

use moon_core::market::trade_replay::TickStatus;

use super::state::{DealRow, TapeStatus};

#[cfg(test)]
mod tests;

/// Drop from the carried rows every one refused as unservable whose market gained stored prints.
///
/// Args:
///     judged: The rows a reload would carry, by `reportuid`.
///     refiled: The markets filed since those rows were judged, as `(exchange key, market)` —
///         `trade_cache::filed_since`.
pub(super) fn drop_refiled(
    judged: HashMap<i64, DealRow>,
    refiled: &HashSet<(String, String)>,
) -> HashMap<i64, DealRow> {
    if refiled.is_empty() {
        return judged;
    }
    judged
        .into_iter()
        .filter(|(_, row)| {
            let refused = matches!(
                row.tape,
                TapeStatus::Refused(TickStatus::NoRoute | TickStatus::OutOfRetention { .. })
            );
            let regained = row
                .address
                .as_ref()
                .is_some_and(|a| refiled.contains(&(a.exchange_key.clone(), a.market.clone())));
            !(refused && regained)
        })
        .collect()
}
