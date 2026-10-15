//! Conservative retention planning and non-blocking cache maintenance state.

use super::{Key, catchup};
use crate::market::trade_replay::{
    Coverage,
    trade_cache::{CompactFail, Inventory, KeepMap, TrimOutcome, TrimProgress},
};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, mpsc::Receiver};

/// Retention age and report attribution have independent horizons.
pub(super) struct KeepPolicy {
    pub edge: Option<i64>,
    /// Earliest closed row enumerated by the protecting listings.
    pub listed_from: i64,
    pub record_emulator: bool,
    /// Keep tape only for trades the tuner can replay.
    pub tuner_only: bool,
    pub margin_ms: i64,
    pub long_ms: i64,
}

/// Keep fresh unattributable spans, real-trade edges, and all live plain windows.
pub(super) fn keep_map(
    inventory: &Inventory,
    trades: &[catchup::CatchupTrade],
    live: &HashMap<Key, Coverage>,
    incomplete: &HashSet<Key>,
    policy: KeepPolicy,
) -> KeepMap {
    let mut grouped = HashMap::<Key, Vec<&catchup::CatchupTrade>>::new();
    for trade in trades {
        grouped
            .entry((trade.exchange.clone(), trade.market.clone()))
            .or_default()
            .push(trade);
    }
    let keys: HashSet<_> = inventory
        .keys
        .iter()
        .cloned()
        .chain(live.keys().cloned())
        .chain(grouped.keys().cloned())
        .collect();
    keys.iter()
        .map(|key| {
            let edge = if incomplete.contains(key) {
                i64::MIN
            } else {
                policy.edge.unwrap_or(i64::MIN)
            };
            let mut keep = Coverage::one((edge, i64::MAX));
            let mut real = live.get(key).cloned().unwrap_or_default();
            let mut emulated = Coverage::none();
            let mut untunable = Coverage::none();
            let mut recorded = Coverage::none();
            for trade in grouped.get(key).into_iter().flatten() {
                let focus = catchup::needed(trade, policy.margin_ms, policy.long_ms);
                let drop_untunable = policy.tuner_only && !trade.tunable;
                if trade.emulator || drop_untunable {
                    // Unlisted real closes can have tails after the close-age cutoff. Only
                    // beyond that cutoff plus margin is emulator or untunable subtraction
                    // attributable.
                    let listed = focus.clip(&Coverage::one((
                        policy.listed_from.saturating_add(policy.margin_ms.max(0)),
                        i64::MAX,
                    )));
                    if trade.emulator && trade.tunable && policy.record_emulator {
                        for &span in focus.spans() {
                            recorded.add(span);
                        }
                    }
                    for &span in listed.spans() {
                        if trade.emulator {
                            emulated.add(span);
                        }
                        if drop_untunable {
                            untunable.add(span);
                        }
                    }
                } else if policy.edge.is_none_or(|edge| trade.close_ms >= edge) {
                    for &span in focus.spans() {
                        real.add(span);
                    }
                }
            }
            for &span in real.spans() {
                keep.add(span);
            }
            if !policy.record_emulator && !incomplete.contains(key) {
                keep = keep.minus(&emulated.minus(&real));
            }
            // Tape only trades the tuner cannot replay need goes; a tunable trade's (a recorded
            // emulator one included) or a live window's tape survives even where it overlaps.
            if policy.tuner_only && !incomplete.contains(key) {
                keep = keep.minus(&untunable.minus(&real).minus(&recorded));
            }
            (key.clone(), keep)
        })
        .collect()
}

/// One outstanding coverage read used by the post-delete recount.
pub(super) type Recount = Vec<(
    catchup::CatchupTrade,
    Receiver<rusqlite::Result<Vec<(i64, i64)>>>,
)>;

/// Maintenance phases are polled; no recorder tick waits for the cache thread.
pub(super) enum Phase {
    Idle,
    Listing {
        since: i64,
        generation: u64,
    },
    Inventory {
        rx: Receiver<rusqlite::Result<Inventory>>,
        generation: u64,
        complete: bool,
    },
    Trim {
        rx: Receiver<rusqlite::Result<TrimOutcome>>,
        progress: Arc<TrimProgress>,
    },
    Recount {
        reads: Recount,
        complete: HashSet<super::TradeId>,
        failure: Option<CompactFail>,
        freed: u64,
    },
}

/// Wall-clock scheduling and compaction retries remain independent of catch-up availability.
pub(super) struct Retention {
    pub phase: Phase,
    pub hour: Option<i64>,
    pub retry_at: Option<i64>,
    pub summary_minute: Option<i64>,
    pub recount_dirty: bool,
    /// Resume a conservatively withheld emulator or untunable purge once protecting reports
    /// arrive.
    pub attributed_deferred: bool,
}

impl Default for Retention {
    /// A fresh station considers its finite horizon on its first tick.
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            hour: None,
            retry_at: None,
            summary_minute: None,
            recount_dirty: false,
            attributed_deferred: false,
        }
    }
}

#[cfg(test)]
mod tests;
