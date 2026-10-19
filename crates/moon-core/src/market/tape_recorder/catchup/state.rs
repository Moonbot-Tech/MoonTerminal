//! Recorder-owned planning, latches and settlements; the fetch thread never owns a tape cache.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Sender};

use super::executor::{self, Failure, Fault, PassResult};
use super::{CatchupJob, CatchupPlan, CatchupTrade, Latch, STATUS, TradeId};
use crate::market::trade_replay::trade_cache::TradeCache;
use crate::market::trade_replay::{Coverage, gate};
use crate::station_api::CatchupFailReason;

const HOUR_MS: i64 = 3_600_000;

/// Whether every missing span starts strictly beyond the recorded margin.
pub(super) fn tail_only(missing: &Coverage, tail_start_ms: i64) -> bool {
    missing
        .spans()
        .iter()
        .all(|&(from, _)| from > tail_start_ms)
}

/// Warn lines one pass writes per failure reason; the summary line counts the rest.
pub(super) const WARN_PER_REASON: usize = 5;

/// Jobs one pass carries; the rest stay pending for the next pass, which starts as soon as this
/// one settles. Under the shared venue gate (100 ms per request, a few pages per job) a pass of
/// this size settles in minutes, so the station's counters and log move while a backlog drains.
pub(super) const PASS_JOBS: usize = 200;

#[cfg(test)]
thread_local! {
    /// Recount span reads issued on this thread, so tests can count them.
    pub(super) static SPAN_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Start the recount's read of every span one market holds.
fn read_spans(
    cache: &TradeCache,
    (exchange, market): &(String, String),
) -> mpsc::Receiver<rusqlite::Result<Vec<(i64, i64)>>> {
    #[cfg(test)]
    SPAN_READS.with(|reads| reads.set(reads.get() + 1));
    cache.held_spans_start(exchange, market, i64::MIN, i64::MAX)
}

/// Pending rows and refusals are owned by the recorder, alongside its sole cache handle.
#[derive(Default)]
pub(crate) struct RecorderCatchup {
    pending: HashMap<TradeId, CatchupTrade>,
    latched: HashMap<TradeId, Latch>,
    complete: HashSet<TradeId>,
    /// Cap clean core answers for tails and unlisted gaps independently of hourly retries.
    core_gap_tries: HashMap<TradeId, u8>,
    /// Final tail IDs remain countable until a setting change or retention expires them.
    tail_final: HashSet<TradeId>,
    /// Retained coverage can outlive catch-up's shorter archive-reach settlement horizon.
    tape_complete: HashSet<TradeId>,
    /// Resolved identities also protect retained tape during later purge planning.
    pub(super) known: HashMap<TradeId, CatchupTrade>,
    /// Skipped trades ([`super::skipped`]) remain known but never enter the fetch queue.
    skipped: HashMap<TradeId, CatchupTrade>,
    worker: Option<Sender<Pass>>,
    active: bool,
    hour: Option<i64>,
    archive_bytes: u64,
    /// Last applied generation; settings changes invalidate settled source decisions.
    switch_gen: u64,
    /// Markets whose fetched tape the disk reserve refused to file during the running pass.
    refused: HashSet<(String, String)>,
    /// Poll-only maintenance runs before another historical pass may start.
    retention: super::super::retention::Retention,
    /// Completion timestamps support a rolling catch-up throughput estimate.
    completions: std::collections::VecDeque<(i64, u32)>,
    rate_started: Option<i64>,
    /// Settlements cannot synchronously read coverage behind cache maintenance.
    deferred_finish: Option<(PassResult, i64)>,
}

/// One serial fetch pass; continuation pages in the same UTC hour share the archive budget.
struct Pass {
    jobs: Vec<CatchupJob>,
    archive_bytes: u64,
}

impl RecorderCatchup {
    /// Whether `id` is among the trades the catch-up and the retention know of.
    #[cfg(test)]
    pub(crate) fn knows(&self, id: TradeId) -> bool {
        self.known.contains_key(&id)
    }

    /// Merge protecting listings and live closes independently of the fetch queue.
    pub(crate) fn protect(&mut self, trades: Vec<CatchupTrade>) {
        let busy = self.maintenance_busy();
        for trade in trades {
            if busy {
                self.retention
                    .recount_stale
                    .insert((trade.exchange.clone(), trade.market.clone()));
            }
            self.known.insert(trade.id, trade);
        }
    }
    /// Each distinct market of the known trades, once.
    fn known_markets(&self) -> HashSet<(String, String)> {
        self.known
            .values()
            .map(|t| (t.exchange.clone(), t.market.clone()))
            .collect()
    }

    /// Whether cache maintenance can currently delay synchronous readers.
    pub(crate) fn maintenance_busy(&self) -> bool {
        use super::super::retention::Phase;
        matches!(
            self.retention.phase,
            Phase::Inventory { .. } | Phase::Trim { .. } | Phase::Recount { .. }
        )
    }

    /// Poll retention without waiting on the cache worker; queued narrowing keeps catch-up paused.
    pub(crate) fn retention_tick(
        &mut self,
        cache: &TradeCache,
        now: i64,
        live: &HashMap<super::super::Key, Coverage>,
        shape: &super::Shape,
    ) -> bool {
        use super::super::retention::{self, Phase};
        use crate::market::trade_replay::trade_cache::{Compact, CompactFail, TrimProgress};
        use crate::station_api::{PurgeFail, TapePurge};
        use std::sync::{Arc, atomic::Ordering, mpsc::TryRecvError};
        if !super::enabled() {
            return false;
        }
        let wanted = super::take_purge_wanted();
        if wanted {
            STATUS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .purge = Some(TapePurge::Running { done: 0, total: 0 });
        }
        if super::record_emulator() && !super::tuner_only() {
            self.retention.attributed_deferred = false;
        }
        let deferred = self.retention.attributed_deferred
            && super::listings_complete(super::relist_gen())
            && self.retention.retry_at.is_none_or(|at| now >= at);
        let hourly = super::retention_edge_ms(now).is_some()
            && self.retention.hour != Some(now.div_euclid(HOUR_MS))
            && self.retention.retry_at.is_none_or(|at| now >= at);
        let retry = self.retention.retry_at.is_some_and(|at| now >= at);
        if wanted || hourly || retry || deferred {
            if matches!(self.retention.phase, Phase::Idle) {
                self.retention.phase = Phase::Listing {
                    since: now,
                    generation: super::relist_gen(),
                };
            } else if wanted {
                // Preserve a narrowing request arriving during maintenance for the next pass.
                super::PURGE_WANTED.store(true, Ordering::Relaxed);
            }
        }
        let phase = std::mem::replace(&mut self.retention.phase, Phase::Idle);
        let mut status = None;
        self.retention.phase = match phase {
            Phase::Idle => Phase::Idle,
            Phase::Listing { since, generation } => {
                let since = if generation == super::relist_gen() {
                    since
                } else {
                    now
                };
                let generation = if generation == super::relist_gen() {
                    generation
                } else {
                    super::relist_gen()
                };
                let complete = super::listings_complete(generation);
                if !self.active && (complete || now.saturating_sub(since) >= 600_000) {
                    if !complete {
                        // This pass keeps all tape of every market; say which cores held it.
                        let cores = super::unlisted(generation).map_or_else(
                            || "core list not received yet".to_string(),
                            |cores| format!("cores not listed: {cores:?}"),
                        );
                        super::super::line(&format!(
                            "retention: listings incomplete after 10 min, nothing is deleted \
                             this pass; {cores}"
                        ));
                    }
                    self.retention.hour = Some(now.div_euclid(HOUR_MS));
                    status = Some(TapePurge::Running { done: 0, total: 0 });
                    Phase::Inventory {
                        rx: cache.inventory_start(),
                        generation,
                        complete,
                    }
                } else {
                    Phase::Listing { since, generation }
                }
            }
            Phase::Inventory {
                rx,
                generation,
                complete,
            } => match rx.try_recv() {
                Ok(Ok(inventory)) if generation == super::relist_gen() => {
                    self.retention.attributed_deferred =
                        !complete && (!super::record_emulator() || super::tuner_only());
                    let incomplete = if complete {
                        HashSet::new()
                    } else {
                        inventory
                            .keys
                            .iter()
                            .cloned()
                            .chain(live.keys().cloned())
                            .chain(
                                self.known
                                    .values()
                                    .map(|t| (t.exchange.clone(), t.market.clone())),
                            )
                            .collect()
                    };
                    let known: Vec<_> = self.known.values().cloned().collect();
                    let keep = retention::keep_map(
                        &inventory,
                        &known,
                        live,
                        &incomplete,
                        retention::KeepPolicy {
                            edge: super::retention_edge_ms(now),
                            listed_from: super::edge_ms(now),
                            record_emulator: super::record_emulator(),
                            tuner_only: super::tuner_only(),
                            shape: *shape,
                        },
                    );
                    let progress = Arc::new(TrimProgress::default());
                    let rx = cache.trim_start(
                        Arc::new(keep),
                        Compact::Incremental,
                        Some(progress.clone()),
                    );
                    Phase::Trim { rx, progress }
                }
                Ok(Ok(_)) => Phase::Listing {
                    since: now,
                    generation: super::relist_gen(),
                },
                Err(TryRecvError::Empty) => Phase::Inventory {
                    rx,
                    generation,
                    complete,
                },
                result => {
                    status = Some(self.retention_delete_failed(result, now));
                    Phase::Idle
                }
            },
            Phase::Trim { rx, progress } => match rx.try_recv() {
                Ok(Ok(outcome)) => {
                    self.sync_switches();
                    self.complete.clear();
                    self.tape_complete.clear();
                    // Reads issued now already see every protect that landed during the trim.
                    self.retention.recount_stale.clear();
                    self.retention.recount_all = false;
                    let mut recount = retention::Recount::default();
                    for key in self.known_markets() {
                        let rx = read_spans(cache, &key);
                        recount.reads.insert(key, rx);
                    }
                    Phase::Recount {
                        recount,
                        failure: outcome.compacted.err(),
                        freed: outcome.bytes_before.saturating_sub(outcome.bytes_after),
                    }
                }
                Err(TryRecvError::Empty) => {
                    status = Some(TapePurge::Running {
                        done: progress.done.load(Ordering::Relaxed),
                        total: progress.total.load(Ordering::Relaxed),
                    });
                    Phase::Trim { rx, progress }
                }
                result => {
                    status = Some(self.retention_delete_failed(result, now));
                    Phase::Idle
                }
            },
            Phase::Recount {
                mut recount,
                mut failure,
                freed,
            } => {
                // Only the markets a protect or an eviction touched are read again; the rest keep
                // their answers, so the phase converges however often live trades close.
                let mut stale = std::mem::take(&mut self.retention.recount_stale);
                if std::mem::take(&mut self.retention.recount_all) {
                    stale.extend(self.known_markets());
                }
                for key in stale {
                    if !recount.reread.insert(key.clone()) {
                        // One re-read per market per recount bounds the phase under any close
                        // rate; a later close is judged from that answer and at worst stays
                        // pending, which the next catch-up pass checks against the cache again.
                        continue;
                    }
                    match recount.reads.entry(key) {
                        // One read per market at a time: a close faster than a read answers
                        // queues the re-read instead of discarding the one in flight.
                        Entry::Occupied(open) => {
                            recount.rerun.insert(open.key().clone());
                        }
                        Entry::Vacant(slot) => {
                            let rx = read_spans(cache, slot.key());
                            slot.insert(rx);
                        }
                    }
                }
                recount.reads.retain(|key, rx| match rx.try_recv() {
                    Ok(Ok(spans)) => {
                        recount
                            .held
                            .insert(key.clone(), Some(Coverage::from_spans(spans)));
                        false
                    }
                    Err(TryRecvError::Empty) => true,
                    _ => {
                        failure = Some(CompactFail::Other);
                        recount.held.insert(key.clone(), None);
                        false
                    }
                });
                let due: Vec<_> = recount
                    .rerun
                    .iter()
                    .filter(|key| !recount.reads.contains_key(*key))
                    .cloned()
                    .collect();
                for key in due {
                    recount.rerun.remove(&key);
                    let rx = read_spans(cache, &key);
                    recount.reads.insert(key, rx);
                }
                if !recount.reads.is_empty() {
                    // A re-read market keeps its earlier answer here, so the bar never moves back.
                    let done = recount.held.len() as u64;
                    let unread = recount
                        .reads
                        .keys()
                        .filter(|key| !recount.held.contains_key(*key))
                        .count() as u64;
                    status = Some(TapePurge::Running {
                        done,
                        total: done + unread,
                    });
                    Phase::Recount {
                        recount,
                        failure,
                        freed,
                    }
                } else {
                    let complete: HashSet<TradeId> = self
                        .known
                        .values()
                        .filter(|t| {
                            recount
                                .held
                                .get(&(t.exchange.clone(), t.market.clone()))
                                .and_then(Option::as_ref)
                                .is_some_and(|held| held.covers(&super::needed(t, shape)))
                        })
                        .map(|t| t.id)
                        .collect();
                    self.tape_complete = complete.clone();
                    self.complete = complete;
                    self.pending = self
                        .known
                        .iter()
                        .filter(|(id, t)| {
                            !self.complete.contains(id)
                                && self.latched.get(id) != Some(&Latch::Permanent)
                                && !super::skipped(t)
                                && t.close_ms >= super::edge_ms(now)
                        })
                        .map(|(id, t)| (*id, t.clone()))
                        .collect();
                    self.publish_pending();
                    self.retention.hour = Some(now.div_euclid(HOUR_MS));
                    self.retention.retry_at = failure
                        .filter(|failure| *failure != CompactFail::NotShrinkable)
                        .map(|_| now + 60_000);
                    status = Some(match failure {
                        None => TapePurge::Done {
                            at_ms: now,
                            freed_bytes: freed,
                        },
                        Some(f) => TapePurge::Failed {
                            at_ms: now,
                            reason: match f {
                                CompactFail::NoRoom => PurgeFail::NoRoom,
                                CompactFail::NotShrinkable => PurgeFail::NotShrinkable,
                                CompactFail::Deferred => PurgeFail::Deferred,
                                CompactFail::Other => PurgeFail::Other,
                            },
                        },
                    });
                    self.retention.summary_minute = None;
                    Phase::Idle
                }
            }
        };
        if self.retention.summary_minute != Some(now.div_euclid(60_000)) {
            self.publish_storage(now);
            self.retention.summary_minute = Some(now.div_euclid(60_000));
        }
        let purging = !matches!(self.retention.phase, Phase::Idle)
            || super::PURGE_WANTED.load(Ordering::Relaxed);
        if purging && !matches!(status, Some(TapePurge::Running { .. })) {
            // Listing, inventory, recount and queued passes must not expose a prior Done.
            status = Some(TapePurge::Running { done: 0, total: 0 });
        }
        if let Some(purge) = status {
            STATUS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .purge = Some(purge);
        }
        if !self.maintenance_busy() {
            if let Some((result, at)) = self.deferred_finish.take() {
                self.finish(cache, result, at);
            }
        }
        purging
    }

    /// Inventory and delete failures share room-pressure reporting and the same retry backoff.
    fn retention_delete_failed<T>(
        &mut self,
        result: Result<rusqlite::Result<T>, mpsc::TryRecvError>,
        now: i64,
    ) -> crate::station_api::TapePurge {
        use crate::station_api::{PurgeFail, TapePurge};
        let no_room = matches!(result, Ok(Err(ref error))
            if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DiskFull));
        self.retention.retry_at = Some(now + if no_room { HOUR_MS } else { 60_000 });
        TapePurge::Failed {
            at_ms: now,
            reason: if no_room {
                PurgeFail::DeleteNoRoom
            } else {
                PurgeFail::DeleteFailed
            },
        }
    }

    /// Report complete known trades by close age, and main plus WAL bytes once per minute.
    fn publish_storage(&self, now: i64) {
        self.publish_storage_at(now, &crate::config::paths::tape_recorder_db_path());
    }

    /// Read a specified cache file so reporting can be verified against isolated fixtures.
    fn publish_storage_at(&self, now: i64, path: &std::path::Path) {
        let mut ages = std::collections::BTreeMap::<u16, (u32, u32)>::new();
        let mut count = 0u32;
        for trade in self
            .known
            .values()
            .filter(|t| self.tape_complete.contains(&t.id))
        {
            count = count.saturating_add(1);
            let days = (now.saturating_sub(trade.close_ms).max(0) / 86_400_000).min(u16::MAX as i64)
                as u16;
            let row = ages.entry(days).or_default();
            if trade.emulator {
                row.1 += 1;
            } else {
                row.0 += 1;
            }
        }
        let mut wal = path.as_os_str().to_os_string();
        wal.push("-wal");
        let bytes = [path, std::path::Path::new(&wal)]
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .sum();
        let mut status = STATUS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        status.db_bytes = Some(bytes);
        status.tape_trades = Some(count);
        status.tape_ages = ages
            .into_iter()
            .map(|(days, (plain, emulator))| crate::station_api::TapeAge {
                days,
                plain,
                emulator,
            })
            .collect();
    }
    /// Merge session pages without duplicating a trade already waiting for recovery.
    pub(crate) fn submit(&mut self, trades: Vec<CatchupTrade>) {
        if !self.active {
            self.sync_switches();
        }
        for trade in trades {
            self.known.insert(trade.id, trade.clone());
            if super::skipped(&trade) {
                self.pending.remove(&trade.id);
                self.skipped.insert(trade.id, trade);
                continue;
            }
            self.skipped.remove(&trade.id);
            if !self.complete.contains(&trade.id)
                && self.latched.get(&trade.id) != Some(&Latch::Permanent)
            {
                self.pending.insert(trade.id, trade);
            }
        }
        self.publish_pending();
    }

    /// Plan on the recorder thread, reading held spans through its own cache queue only.
    /// `archives` comes from the caller's switch snapshot, so one tick never mixes generations.
    pub(super) fn prepare(
        &mut self,
        held: impl Fn(&CatchupTrade) -> Coverage,
        now_ms: i64,
        shape: &super::Shape,
        archives: bool,
    ) -> CatchupPlan {
        self.prune(now_ms);
        self.route_skipped();
        let ready: Vec<_> = self.pending.values().cloned().collect();
        let batch = if archives {
            super::plan(&ready, held, &self.latched, now_ms, shape)
        } else {
            super::plan_with(&ready, held, &self.latched, now_ms, shape, false)
        };
        for &id in &batch.permanent_ids {
            self.latched.insert(id, Latch::Permanent);
            self.pending.remove(&id);
        }
        self.tail_final.extend(&batch.tail_final_ids);
        for &id in &batch.complete_ids {
            self.complete.insert(id);
            self.tape_complete.insert(id);
            self.pending.remove(&id);
            self.core_gap_tries.remove(&id);
        }
        self.publish_pending();
        batch
    }

    /// The disk reserve refused to file fetched tape: its job fails as `no_room`, not unnamed.
    pub(crate) fn filing_refused(&mut self, exchange: String, market: String) {
        self.refused.insert((exchange, market));
    }

    /// Evicted tape invalidates settled IDs so the next session listing checks coverage again.
    pub(crate) fn evicted(&mut self) {
        self.complete.clear();
        self.tape_complete.clear();
        if self.maintenance_busy() {
            self.retention.recount_all = true;
        }
    }

    /// Reconsider permanent refusals and completed IDs when source choices change.
    fn sync_switches(&mut self) -> (bool, bool) {
        let (generation, auto, archives) = super::switches();
        if self.switch_gen != generation {
            self.latched.retain(|_, latch| *latch != Latch::Permanent);
            self.complete.clear();
            self.tape_complete.clear();
            self.core_gap_tries.clear();
            self.tail_final.clear();
            for trade in self.known.values() {
                self.pending.insert(trade.id, trade.clone());
            }
            self.switch_gen = generation;
        }
        self.route_skipped();
        self.publish_pending();
        (auto, archives)
    }

    /// Keep skipped rows out of pending and return the ones the current choices admit again.
    fn route_skipped(&mut self) {
        let admitted: Vec<_> = self
            .skipped
            .iter()
            .filter(|(_, trade)| !super::skipped(trade))
            .map(|(id, _)| *id)
            .collect();
        for id in admitted {
            if let Some(trade) = self.skipped.remove(&id) {
                self.pending.insert(id, trade);
            }
        }
        self.pending.retain(|id, trade| {
            if super::skipped(trade) {
                self.skipped.insert(*id, trade.clone());
                false
            } else {
                true
            }
        });
    }

    /// Expire catch-up settlements at archive reach while retaining protection until its own edge.
    fn prune(&mut self, now_ms: i64) {
        let cutoff = super::retention_edge_ms(now_ms).unwrap_or(i64::MIN);
        let catchup_cutoff = super::edge_ms(now_ms);
        for trade in self
            .known
            .values()
            .filter(|trade| trade.close_ms < catchup_cutoff)
        {
            self.complete.remove(&trade.id);
            self.latched.remove(&trade.id);
            self.core_gap_tries.remove(&trade.id);
            self.tail_final.remove(&trade.id);
        }
        let expired: Vec<_> = self
            .known
            .iter()
            .filter(|(_, trade)| trade.close_ms < cutoff)
            .map(|(&id, _)| id)
            .collect();
        for id in expired {
            self.complete.remove(&id);
            self.latched.remove(&id);
            self.core_gap_tries.remove(&id);
            self.tail_final.remove(&id);
            if !self.pending.contains_key(&id) {
                self.known.remove(&id);
                self.skipped.remove(&id);
                self.tape_complete.remove(&id);
            }
        }
    }

    /// Lazily start one fetch thread and submit work only after the prior pass settled.
    pub(crate) fn tick(&mut self, cache: &TradeCache, now_ms: i64, shape: &super::Shape) {
        self.prune(now_ms);
        if self.active {
            return;
        }
        let (auto, archives) = self.sync_switches();
        if !auto || self.pending.is_empty() {
            return;
        }
        let batch = self.prepare(
            |trade| {
                let focus = super::needed(trade, shape);
                let Some((from, to)) = focus.hull() else {
                    return Coverage::none();
                };
                Coverage::from_spans(
                    cache
                        .held_spans(&trade.exchange, &trade.market, from, to)
                        .unwrap_or_default(),
                )
            },
            now_ms,
            shape,
            archives,
        );
        {
            let mut status = STATUS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            status.filled = status.filled.saturating_add(batch.complete);
            status.no_route = status.no_route.saturating_add(batch.no_route);
            status.out_of_reach = status.out_of_reach.saturating_add(batch.out_of_reach);
        }
        self.publish_pending();
        let hour = now_ms.div_euclid(HOUR_MS);
        if self.hour != Some(hour) {
            self.hour = Some(hour);
            self.archive_bytes = 0;
        }
        // Plan order is soonest expiry first; untaken jobs stay pending and unlatched.
        let jobs = admissible(batch.jobs);
        if jobs.is_empty() {
            return;
        }
        if self.worker.is_none() {
            self.worker = start_worker();
        }
        let pass = Pass {
            jobs,
            archive_bytes: self.archive_bytes,
        };
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.send(pass).is_ok())
        {
            self.active = true;
            self.rate_started.get_or_insert(now_ms);
            let mut status = STATUS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // `failed` keeps describing the last settled pass until this one settles.
            status.running = true;
            status.last_pass_ms = Some(now_ms);
            status.archive_bytes = self.archive_bytes;
        } else {
            self.worker = None;
            for trade in self.pending.values() {
                self.latched
                    .insert(trade.id, Latch::RetryAfter(next_pass(now_ms)));
            }
            super::super::line(
                "catch-up thread unavailable; pending trades wait for the next pass",
            );
        }
    }

    /// Settle after filing; cap clean unfilled core-only gaps while keeping failed writes retryable.
    pub(crate) fn finish(&mut self, cache: &TradeCache, result: PassResult, now_ms: i64) {
        if self.maintenance_busy() {
            self.deferred_finish = Some((result, now_ms));
            return;
        }
        self.archive_bytes = result.archive_bytes;
        let mut filled = 0_u32;
        let mut failed = 0_u32;
        let mut failures = Vec::new();
        for finished in result.finished {
            let job = finished.job;
            if finished.failure == Some(Failure::Permanent) {
                self.latched.insert(job.trade.id, Latch::Permanent);
                self.pending.remove(&job.trade.id);
                if finished.attempted {
                    failed += 1;
                    failures.push((job, finished.fault));
                }
                continue;
            }
            let mut held = Coverage::none();
            let mut read_answered = true;
            for &(from, to) in job.missing.spans() {
                match cache.held_spans(&job.trade.exchange, &job.trade.market, from, to) {
                    Some(spans) => {
                        for span in spans {
                            held.add(span);
                        }
                    }
                    None => read_answered = false,
                }
            }
            let remaining = job.missing.minus(&held);
            let complete = read_answered && remaining.is_empty();
            let expired_core_gap = job.trade.venue.is_none()
                && read_answered
                && !remaining.is_empty()
                && remaining.spans().iter().all(|&(_, to)| {
                    to < now_ms.saturating_sub(crate::market::CORE_ARCHIVE_RETENTION_MS)
                });
            if expired_core_gap {
                // Keep retained recovery successful without calling expired coverage complete.
                self.pending.remove(&job.trade.id);
                self.latched.insert(job.trade.id, Latch::Permanent);
                self.core_gap_tries.remove(&job.trade.id);
                if tail_only(&remaining, job.tail_start_ms) {
                    self.tail_final.insert(job.trade.id);
                }
            } else if complete {
                self.pending.remove(&job.trade.id);
                self.latched.remove(&job.trade.id);
                self.core_gap_tries.remove(&job.trade.id);
                if self.complete.insert(job.trade.id) {
                    filled += 1;
                }
                self.tape_complete.insert(job.trade.id);
            } else {
                let is_tail = tail_only(&remaining, job.tail_start_ms);
                let clean_core_gap = finished.attempted
                    && finished.failure.is_none()
                    && finished.fault.is_none()
                    && read_answered
                    && held.covers(&finished.filed)
                    && !self
                        .refused
                        .contains(&(job.trade.exchange.clone(), job.trade.market.clone()))
                    && (is_tail || job.trade.venue.is_none());
                let final_gap = clean_core_gap && {
                    let tries = self.core_gap_tries.entry(job.trade.id).or_default();
                    *tries = tries.saturating_add(1);
                    *tries >= 2
                };
                if final_gap {
                    if is_tail {
                        self.tail_final.insert(job.trade.id);
                    }
                    self.pending.remove(&job.trade.id);
                    self.latched.insert(job.trade.id, Latch::Permanent);
                } else {
                    self.pending.insert(job.trade.id, job.trade.clone());
                    self.latched
                        .insert(job.trade.id, Latch::RetryAfter(next_pass(now_ms)));
                }
                if finished.attempted {
                    failed += 1;
                    let fault = finished.fault.or_else(|| {
                        self.refused
                            .contains(&(job.trade.exchange.clone(), job.trade.market.clone()))
                            .then(|| Fault {
                                reason: CatchupFailReason::NoRoom,
                                detail: "disk reserve refused filing the fetched tape".into(),
                            })
                    });
                    failures.push((job, fault));
                }
            }
        }
        self.refused.clear();
        self.active = false;
        self.rate_started.get_or_insert(now_ms);
        self.completions.push_back((now_ms, filled));
        while self
            .completions
            .front()
            .is_some_and(|(at, _)| *at < now_ms - HOUR_MS)
        {
            self.completions.pop_front();
        }
        let elapsed = now_ms
            .saturating_sub(self.rate_started.unwrap_or(now_ms))
            .clamp(1_000, HOUR_MS);
        let per_hour = self
            .completions
            .iter()
            .map(|(_, n)| u64::from(*n))
            .sum::<u64>()
            .saturating_mul(HOUR_MS as u64)
            / elapsed as u64;
        let pending = self.pending.len().min(u32::MAX as usize) as u32;
        let report = pass_report(filled, pending, &failures);
        log::info!("{}", report.summary);
        for warn in &report.warns {
            log::warn!("{warn}");
        }
        self.sync_switches();
        let pending = self.pending.len().min(u32::MAX as usize) as u32;
        let mut status = STATUS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        status.filled = status.filled.saturating_add(filled);
        status.failed = failed;
        status.failed_by = report.counts;
        status.running = false;
        status.archive_bytes = self.archive_bytes;
        status.pending = pending;
        status.catchup_rate_per_hour = Some(per_hour.min(u32::MAX as u64) as u32);
    }

    /// Keep queued and retrying trades visible while a pass is running or waiting for its hour.
    fn publish_pending(&self) {
        super::TAIL_FINAL.store(
            self.tail_final.len().min(u32::MAX as usize) as u32,
            std::sync::atomic::Ordering::Relaxed,
        );
        STATUS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending = self.pending.len().min(u32::MAX as usize) as u32;
    }
}

/// What one settled pass tells the station log and its status.
pub(super) struct PassReport {
    /// Failed jobs per reason, in the reasons' order.
    pub(super) counts: Vec<(CatchupFailReason, u32)>,
    /// The one info line for the pass.
    pub(super) summary: String,
    /// Up to [`WARN_PER_REASON`] lines per reason naming the job and the error's own text.
    pub(super) warns: Vec<String>,
}

/// Count the pass's failed jobs by reason and word its bounded log; a job failed with no named
/// fault (fetched in full, yet not held after filing) counts as `other`.
pub(super) fn pass_report(
    filled: u32,
    pending: u32,
    failures: &[(CatchupJob, Option<Fault>)],
) -> PassReport {
    let mut counts = std::collections::BTreeMap::<CatchupFailReason, u32>::new();
    let mut warns = Vec::new();
    for (job, fault) in failures {
        let reason = fault
            .as_ref()
            .map_or(CatchupFailReason::Other, |f| f.reason);
        let seen = counts.entry(reason).or_default();
        *seen = seen.saturating_add(1);
        if (*seen as usize) <= WARN_PER_REASON {
            let detail = fault
                .as_ref()
                .map_or("fetched tape not held after filing", |f| f.detail.as_str());
            warns.push(ascii(&format!(
                "catch-up failed ({}): {} {} window {}: {detail}",
                reason.key(),
                job.trade.exchange,
                job.trade.market,
                job.missing
            )));
        }
    }
    let failed: u32 = counts.values().fold(0, |sum, n| sum.saturating_add(*n));
    let breakdown = counts
        .iter()
        .map(|(reason, n)| format!("{}: {n}", reason.key()))
        .collect::<Vec<_>>()
        .join(", ");
    let summary = if failed == 0 {
        format!("catch-up pass: filled {filled}, failed 0, pending {pending}")
    } else {
        format!("catch-up pass: filled {filled}, failed {failed} ({breakdown}), pending {pending}")
    };
    PassReport {
        counts: counts.into_iter().collect(),
        summary,
        warns,
    }
}

/// Keep a venue's own error text to ASCII so the station log stays ASCII.
fn ascii(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii() && !c.is_ascii_control() {
                c
            } else {
                '?'
            }
        })
        .collect()
}

/// Take at most one pass of jobs in plan order, independent of downloaded byte telemetry.
pub(super) fn admissible(jobs: Vec<CatchupJob>) -> Vec<CatchupJob> {
    jobs.into_iter().take(PASS_JOBS).collect()
}

/// Anchor recoverable retries to the next UTC hour rather than to each failure's elapsed delay.
fn next_pass(now_ms: i64) -> i64 {
    now_ms
        .div_euclid(HOUR_MS)
        .saturating_add(1)
        .saturating_mul(HOUR_MS)
}

/// Start the serial fetcher without giving it a database handle or another writer path.
fn start_worker() -> Option<Sender<Pass>> {
    let (sender, receiver) = mpsc::channel::<Pass>();
    std::thread::Builder::new()
        .name("tape-catchup".into())
        .spawn(move || {
            while let Ok(pass) = receiver.recv() {
                let jobs = pass.jobs.len();
                let result = executor::run_pass_with(
                    pass.jobs,
                    crate::util::now_unix_ms_i64,
                    gate::shared(),
                    (executor::fetch_rest, executor::fetch_archive),
                    pass.archive_bytes,
                    |job, _, _, rest, archive| super::run_job(job, rest, archive),
                    |job, outcome| {
                        if !outcome.file.is_empty() {
                            super::pacing::wait();
                            super::super::notify(super::super::Cmd::Backfilled {
                                exchange: job.trade.exchange.clone(),
                                market: job.trade.market.clone(),
                                file: outcome.file.clone(),
                                ticks: outcome.ticks.clone(),
                            });
                        }
                    },
                );
                for finished in &result.finished {
                    super::super::line(&format!(
                        "catch-up job core={} rec={} {} {} missing={} outcome={:?} attempted={}",
                        finished.job.trade.id.core,
                        finished.job.trade.id.rec_id,
                        finished.job.trade.exchange,
                        finished.job.trade.market,
                        finished.job.missing,
                        finished.failure,
                        finished.attempted
                    ));
                }
                super::super::line(&format!(
                    "catch-up pass jobs={jobs} archive_bytes={}",
                    result.archive_bytes
                ));
                super::super::notify(super::super::Cmd::CatchupDone(result));
            }
        })
        .map_err(|error| super::super::line(&format!("catch-up thread not started: {error}")))
        .ok()?;
    Some(sender)
}

#[cfg(test)]
mod tests;
