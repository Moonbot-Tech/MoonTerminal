//! Recorder-owned planning, latches and settlements; the fetch thread never owns a tape cache.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Sender};

use super::executor::{self, Failure, PassResult};
use super::{CatchupJob, CatchupPlan, CatchupTrade, Latch, STATUS, TradeId};
use crate::market::trade_replay::trade_cache::TradeCache;
use crate::market::trade_replay::{Coverage, gate};

const HOUR_MS: i64 = 3_600_000;

/// Pending rows and refusals are owned by the recorder, alongside its sole cache handle.
#[derive(Default)]
pub(crate) struct RecorderCatchup {
    pending: HashMap<TradeId, CatchupTrade>,
    latched: HashMap<TradeId, Latch>,
    complete: HashSet<TradeId>,
    /// Close times bound settled IDs and refusal latches to the same depth as enumeration.
    closed: HashMap<TradeId, i64>,
    worker: Option<Sender<Pass>>,
    active: bool,
    hour: Option<i64>,
    archive_bytes: u64,
}

/// One serial fetch pass; continuation pages in the same UTC hour share the archive budget.
struct Pass {
    jobs: Vec<CatchupJob>,
    archive_bytes: u64,
}

impl RecorderCatchup {
    /// Merge session pages without duplicating a trade already waiting for recovery.
    pub(crate) fn submit(&mut self, trades: Vec<CatchupTrade>) {
        for trade in trades {
            if !self.complete.contains(&trade.id)
                && self.latched.get(&trade.id) != Some(&Latch::Permanent)
            {
                self.closed.insert(trade.id, trade.close_ms);
                self.pending.insert(trade.id, trade);
            }
        }
        self.publish_pending();
    }

    /// Plan on the recorder thread, reading held spans through its own cache queue only.
    pub(super) fn prepare(
        &mut self,
        held: impl Fn(&CatchupTrade) -> Coverage,
        now_ms: i64,
        margin: i64,
        long: i64,
    ) -> CatchupPlan {
        self.prune(now_ms);
        let ready: Vec<_> = self.pending.values().cloned().collect();
        let batch = super::plan(&ready, held, &self.latched, now_ms, margin, long);
        for &id in &batch.permanent_ids {
            self.latched.insert(id, Latch::Permanent);
            self.pending.remove(&id);
        }
        for &id in &batch.complete_ids {
            self.complete.insert(id);
            self.pending.remove(&id);
        }
        batch
    }

    /// Evicted tape invalidates settled IDs so the next session listing checks coverage again.
    pub(crate) fn evicted(&mut self) {
        self.complete.clear();
    }

    /// Release old settled identities even when no pending work remains to start another pass.
    fn prune(&mut self, now_ms: i64) {
        let cutoff =
            now_ms.saturating_sub(crate::market::trade_replay::venue_caps::ARCHIVE_DEPTH_MS);
        let expired: Vec<_> = self
            .closed
            .iter()
            .filter(|(_, close)| **close < cutoff)
            .map(|(&id, _)| id)
            .collect();
        for id in expired {
            self.complete.remove(&id);
            self.latched.remove(&id);
            if !self.pending.contains_key(&id) {
                self.closed.remove(&id);
            }
        }
    }

    /// Lazily start one fetch thread and submit work only after the prior pass settled.
    pub(crate) fn tick(&mut self, cache: &TradeCache, now_ms: i64, margin: i64, long: i64) {
        self.prune(now_ms);
        if self.active || self.pending.is_empty() {
            return;
        }
        let batch = self.prepare(
            |trade| {
                let focus = super::needed(trade, margin, long);
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
            margin,
            long,
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
        if batch.jobs.is_empty() {
            return;
        }
        let hour = now_ms.div_euclid(HOUR_MS);
        if self.hour != Some(hour) {
            self.hour = Some(hour);
            self.archive_bytes = 0;
        }
        if self.worker.is_none() {
            self.worker = start_worker();
        }
        let pass = Pass {
            jobs: batch.jobs,
            archive_bytes: self.archive_bytes,
        };
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.send(pass).is_ok())
        {
            self.active = true;
            let mut status = STATUS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            status.failed = 0;
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

    /// Settle after all filing commands; a reserve refusal or failed write must remain missing.
    pub(crate) fn finish(&mut self, cache: &TradeCache, result: PassResult, now_ms: i64) {
        self.archive_bytes = result.archive_bytes;
        let mut filled = 0_u32;
        let mut failed = 0_u32;
        for finished in result.finished {
            let job = finished.job;
            if finished.failure == Some(Failure::Permanent) {
                self.latched.insert(job.trade.id, Latch::Permanent);
                self.pending.remove(&job.trade.id);
                if finished.attempted {
                    failed += 1;
                }
                continue;
            }
            let held = job.missing.spans().iter().flat_map(|&(from, to)| {
                cache
                    .held_spans(&job.trade.exchange, &job.trade.market, from, to)
                    .unwrap_or_default()
            });
            let complete = Coverage::from_spans(held).covers(&job.missing);
            if complete {
                self.pending.remove(&job.trade.id);
                self.latched.remove(&job.trade.id);
                if self.complete.insert(job.trade.id) {
                    filled += 1;
                }
            } else {
                self.pending.insert(job.trade.id, job.trade.clone());
                self.latched
                    .insert(job.trade.id, Latch::RetryAfter(next_pass(now_ms)));
                if finished.attempted {
                    failed += 1;
                }
            }
        }
        self.active = false;
        let mut status = STATUS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        status.filled = status.filled.saturating_add(filled);
        status.failed = failed;
        status.archive_bytes = self.archive_bytes;
        status.pending = self.pending.len().min(u32::MAX as usize) as u32;
    }

    /// Keep queued and retrying trades visible while a pass is running or waiting for its hour.
    fn publish_pending(&self) {
        STATUS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending = self.pending.len().min(u32::MAX as usize) as u32;
    }
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
