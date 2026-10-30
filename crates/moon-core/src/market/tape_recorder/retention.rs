//! Conservative retention planning and non-blocking cache maintenance state.

use super::{Key, catchup};
use crate::market::trade_replay::{
    Coverage,
    trade_cache::{CompactFail, Inventory, KeepMap, TrimOutcome, TrimProgress},
};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, mpsc::Receiver};

/// The retention phase the recorder thread last entered, as an index into [`PHASE_NAMES`]; read
/// by the resource line on the metrics thread ([`phase_name`]).
static PHASE: AtomicU8 = AtomicU8::new(0);

/// What [`phase_name`] prints for each published phase; 0 is idle and prints nothing.
const PHASE_NAMES: [&str; 6] = [
    "idle",
    "listing wait",
    "inventory",
    "keep map",
    "trim",
    "recount",
];

/// The keep map is built synchronously on the recorder thread inside the inventory phase, and on
/// a large protection set it is a phase of its own as far as memory is concerned.
pub(super) const KEEP_MAP: u8 = 3;

/// Say which phase the recorder is in, for the resource line.
pub(super) fn publish(code: u8) {
    PHASE.store(code, Ordering::Relaxed);
}

/// The retention phase in progress, `None` while idle or when no recorder runs.
pub(crate) fn phase_name() -> Option<&'static str> {
    match PHASE.load(Ordering::Relaxed) {
        0 => None,
        code => PHASE_NAMES.get(usize::from(code)).copied(),
    }
}

/// Retention age and report attribution have independent horizons.
pub(super) struct KeepPolicy {
    pub edge: Option<i64>,
    /// Earliest closed row the cores' listings enumerate.
    pub listed_from: i64,
    pub record_emulator: bool,
    /// Keep tape only for trades the tuner can replay.
    pub tuner_only: bool,
    pub shape: catchup::Shape,
}

/// Keep fresh unattributable spans, real-trade edges, and all live plain windows.
///
/// `trades` is borrowed so the recorder can pass its protection set as it stands: copying
/// millions of trades for this one pass was the hourly memory spike of #1081.
pub(super) fn keep_map<'a>(
    inventory: &Inventory,
    trades: impl IntoIterator<Item = &'a catchup::CatchupTrade>,
    live: &HashMap<Key, Coverage>,
    incomplete: &HashSet<Key>,
    policy: KeepPolicy,
) -> KeepMap {
    let mut grouped = HashMap::<(&str, &str), Vec<&catchup::CatchupTrade>>::new();
    for trade in trades {
        grouped
            .entry((trade.exchange.as_str(), trade.market.as_str()))
            .or_default()
            .push(trade);
    }
    /// Borrow an owned map key without giving the two strings independent lifetimes.
    fn named(key: &Key) -> (&str, &str) {
        (key.0.as_str(), key.1.as_str())
    }
    let keys: HashSet<_> = inventory
        .keys
        .iter()
        .map(named)
        .chain(live.keys().map(named))
        .chain(grouped.keys().copied())
        .collect();
    let live_by_name: HashMap<_, _> = live.iter().map(|(key, held)| (named(key), held)).collect();
    let incomplete_by_name: HashSet<_> = incomplete.iter().map(named).collect();
    keys.into_iter()
        .map(|key| {
            let is_incomplete = incomplete_by_name.contains(&key);
            let edge = if is_incomplete {
                i64::MIN
            } else {
                policy.edge.unwrap_or(i64::MIN)
            };
            let mut keep = Coverage::one((edge, i64::MAX));
            let mut real = live_by_name
                .get(&key)
                .map(|held| (*held).clone())
                .unwrap_or_default();
            let mut emulated = Coverage::none();
            let mut untunable = Coverage::none();
            let mut recorded = Coverage::none();
            for trade in grouped.get(&key).into_iter().flatten() {
                let focus = catchup::needed(trade, &policy.shape);
                let drop_untunable = policy.tuner_only && !trade.tunable;
                if trade.emulator || drop_untunable {
                    // Unlisted real closes can have tails after the close-age cutoff. Only
                    // beyond that cutoff plus the full tail is emulator or untunable subtraction
                    // attributable.
                    let listed = focus.clip(&Coverage::one((
                        policy.listed_from.saturating_add(
                            policy.shape.margin_ms.max(policy.shape.tail_ms).max(0),
                        ),
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
            if !policy.record_emulator && !is_incomplete {
                keep = keep.minus(&emulated.minus(&real));
            }
            // Tape only trades the tuner cannot replay need goes; a tunable trade's (a recorded
            // emulator one included) or a live window's tape survives even where it overlaps.
            if policy.tuner_only && !is_incomplete {
                keep = keep.minus(&untunable.minus(&real).minus(&recorded));
            }
            ((key.0.to_owned(), key.1.to_owned()), keep)
        })
        .collect()
}

/// The post-delete recount: one coverage read per market, every known trade judged from it.
#[derive(Default)]
pub(super) struct Recount {
    /// Reads still unanswered, one per market.
    pub reads: HashMap<Key, Receiver<rusqlite::Result<Vec<(i64, i64)>>>>,
    /// Answered markets; `None` marks a failed read, whose trades stay unsettled.
    pub held: HashMap<Key, Option<Coverage>>,
    /// Markets touched again while their read was in flight; read once more after it answers.
    pub rerun: HashSet<Key>,
    /// Markets already granted their one re-read this recount.
    pub reread: HashSet<Key>,
    /// Span bounds the answered reads carried, for the pass's log line.
    pub spans: usize,
}

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
        recount: Recount,
        failure: Option<CompactFail>,
        freed: u64,
    },
}

impl Phase {
    /// This phase's index into [`PHASE_NAMES`].
    pub(super) fn code(&self) -> u8 {
        match self {
            Phase::Idle => 0,
            Phase::Listing { .. } => 1,
            Phase::Inventory { .. } => 2,
            Phase::Trim { .. } => 4,
            Phase::Recount { .. } => 5,
        }
    }
}

/// Wall-clock scheduling and compaction retries remain independent of catch-up availability.
pub(super) struct Retention {
    pub phase: Phase,
    pub hour: Option<i64>,
    pub retry_at: Option<i64>,
    pub summary_minute: Option<i64>,
    /// Markets a protect landed on during maintenance; the recount re-reads only these.
    pub recount_stale: HashSet<Key>,
    /// An eviction during maintenance can shrink any market, so every market is read again.
    pub recount_all: bool,
    /// Resume a conservatively withheld emulator or untunable purge once protecting reports
    /// arrive.
    pub attributed_deferred: bool,
    /// When the running pass left its listing wait (Unix ms), for the phase lines' timings.
    pub pass_started: Option<i64>,
}

impl Default for Retention {
    /// A fresh station considers its finite horizon on its first tick.
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            hour: None,
            retry_at: None,
            summary_minute: None,
            recount_stale: HashSet::new(),
            recount_all: false,
            attributed_deferred: false,
            pass_started: None,
        }
    }
}

#[cfg(test)]
mod tests;
