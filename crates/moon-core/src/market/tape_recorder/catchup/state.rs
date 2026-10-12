//! Recorder-owned planning, latches and settlements; the fetch thread never owns a tape cache.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Sender};

use super::executor::{self, Failure, Fault, PassResult};
use super::{CatchupJob, CatchupPlan, CatchupTrade, Latch, STATUS, TradeId};
use crate::market::trade_replay::trade_cache::TradeCache;
use crate::market::trade_replay::{Coverage, gate};
use crate::station_api::CatchupFailReason;

const HOUR_MS: i64 = 3_600_000;

/// Warn lines one pass writes per failure reason; the summary line counts the rest.
pub(super) const WARN_PER_REASON: usize = 5;

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
    /// Last applied generation; settings changes invalidate settled source decisions.
    switch_gen: u64,
    /// Markets whose fetched tape the disk reserve refused to file during the running pass.
    refused: HashSet<(String, String)>,
}

/// One serial fetch pass; continuation pages in the same UTC hour share the archive budget.
struct Pass {
    jobs: Vec<CatchupJob>,
    archive_bytes: u64,
}

impl RecorderCatchup {
    /// Merge session pages without duplicating a trade already waiting for recovery.
    pub(crate) fn submit(&mut self, trades: Vec<CatchupTrade>) {
        if !self.active {
            self.sync_switches();
        }
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
    /// `archives` comes from the caller's switch snapshot, so one tick never mixes generations.
    pub(super) fn prepare(
        &mut self,
        held: impl Fn(&CatchupTrade) -> Coverage,
        now_ms: i64,
        margin: i64,
        long: i64,
        archives: bool,
    ) -> CatchupPlan {
        self.prune(now_ms);
        let ready: Vec<_> = self.pending.values().cloned().collect();
        let batch = if archives {
            super::plan(&ready, held, &self.latched, now_ms, margin, long)
        } else {
            super::plan_with(&ready, held, &self.latched, now_ms, margin, long, false)
        };
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

    /// The disk reserve refused to file fetched tape: its job fails as `no_room`, not unnamed.
    pub(crate) fn filing_refused(&mut self, exchange: String, market: String) {
        self.refused.insert((exchange, market));
    }

    /// Evicted tape invalidates settled IDs so the next session listing checks coverage again.
    pub(crate) fn evicted(&mut self) {
        self.complete.clear();
    }

    /// Reconsider permanent refusals and completed IDs when source choices change.
    fn sync_switches(&mut self) -> (bool, bool) {
        let (generation, auto, archives) = super::switches();
        if self.switch_gen != generation {
            self.latched.retain(|_, latch| *latch != Latch::Permanent);
            self.complete.clear();
            self.switch_gen = generation;
        }
        (auto, archives)
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
        if self.active {
            return;
        }
        let (auto, archives) = self.sync_switches();
        if !auto || self.pending.is_empty() {
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
            status.failed_by.clear();
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

    /// Settle after all filing commands; a reserve refusal or failed write must remain missing.
    pub(crate) fn finish(&mut self, cache: &TradeCache, result: PassResult, now_ms: i64) {
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
        let pending = self.pending.len().min(u32::MAX as usize) as u32;
        let report = pass_report(filled, pending, &failures);
        log::info!("{}", report.summary);
        for warn in &report.warns {
            log::warn!("{warn}");
        }
        self.sync_switches();
        let mut status = STATUS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        status.filled = status.filled.saturating_add(filled);
        status.failed = failed;
        status.failed_by = report.counts;
        status.running = false;
        status.archive_bytes = self.archive_bytes;
        status.pending = pending;
    }

    /// Keep queued and retrying trades visible while a pass is running or waiting for its hour.
    fn publish_pending(&self) {
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

#[cfg(test)]
mod tests;
