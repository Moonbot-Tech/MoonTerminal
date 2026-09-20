//! Background loads of the "Entry/Exit" axis, in two stages.
//!
//! Stage A reads the scope's deals, the whole-scope "Fact" KPI (the same SQL every axis'
//! "Fact" comes from) and the grid's "now" values off the database. Its completion resolves,
//! on the UI thread, where each deal's prints live — the core's exchange key and the coin's
//! market, which only the live market source knows — and starts stage B, which asks the
//! replay worker for the held tape of every deal, reads the archived entry line, and runs the
//! model on the parameters as of the buy. The axis' `LoadState` stays "loading" across both.
//!
//! This file only ever WRITES `TicksState`; the rendering only reads it.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use gpui::*;

use super::super::super::AnalyticsView;
use super::state::{DealRow, NowValue, RowAddress, TapeStatus, TicksData};
use crate::analytics::bg::ReadLane;
use crate::analytics::refresh::{CatchUpOutcome, report_result_is_stale};
use moon_core::db::ReadFail;
use moon_core::db::order_traces::{TraceEntry, read_many};
use moon_core::db::tuner::ticks::{
    Deal, DealsRead, EntryParams, entry_model_for, infer_tick, params, verify,
};
use moon_core::db::tuner::{VarStats, Variant, strategy_current_values, strategy_values_at};
use moon_core::feed::report_traces::ArchivedLineKind;
use moon_core::feed::types::Tick;
use moon_core::market::trade_replay::{
    Coverage, TickQuery, margin_ms, query_held, replay_window_ms,
};

/// How long stage B waits for the worker's answer on one deal. The worker serves a held
/// query right after candle jobs, so an answer past this means the worker is gone.
const HELD_ANSWER_WAIT: Duration = Duration::from_secs(10);

/// The tape must reach this far back before the buy for the corridor to have a run-up.
const RUN_UP_MS: i64 = 30_000;

/// What stage A brings back.
type StageA = (
    Result<DealsRead, ReadFail>,
    Result<Vec<VarStats>, ReadFail>,
    HashMap<String, NowValue>,
);

impl AnalyticsView {
    /// Recompute the axis for the current scope.
    pub(in crate::analytics) fn reload_ticks(&mut self, cx: &mut Context<Self>) {
        self.reload_ticks_inner(false, true, cx);
    }

    /// Recompute report-stale data through the writer-driven catch-up path.
    pub(in crate::analytics) fn reload_ticks_after_report(
        &mut self,
        show_overlay: bool,
        cx: &mut Context<Self>,
    ) {
        self.reload_ticks_inner(true, show_overlay, cx);
    }

    fn reload_ticks_inner(
        &mut self,
        after_report: bool,
        show_overlay: bool,
        cx: &mut Context<Self>,
    ) {
        if !after_report {
            self.report_busy_retries.reset();
        }
        self.latest_reads.cancel(&[
            ReadLane::Ticks,
            ReadLane::TicksReplay,
            ReadLane::TicksFetch,
            ReadLane::TicksVariants,
            ReadLane::TicksSearch,
        ]);
        self.ticks.seq = self.ticks.seq.wrapping_add(1);
        self.ticks.fetch.clear();
        // A search over the previous deal set answers nothing about the new one; the lane
        // cancel above does not reach its handle, only this does.
        self.ticks.stop_search();
        let req = self.ticks.seq;
        let report_req = self.current_report_generation();
        let q = self.tuner_query();
        let targets: Vec<(i64, Option<u64>)> = self.visible_target_keys(self.read_core_ids());
        let keys = params::param_keys();
        self.ticks.data.begin();
        self.ticks.kpi.begin();
        self.spawn_latest_db(
            &[ReadLane::Ticks],
            show_overlay,
            cx,
            move || {
                let deals = moon_core::db::tuner::ticks::read_deals(&q);
                let fact = moon_core::db::tuner::variant_stats(&q, &[Variant::default()]);
                let now = now_values(&targets, &keys);
                (deals, fact, now)
            },
            move |this, (deals, fact, now): StageA, cx| {
                if this.ticks.seq != req {
                    return;
                }
                let (read, fact) = match (deals, fact) {
                    (Ok(read), Ok(fact)) => (read, fact),
                    (Err(error), _) | (_, Err(error)) => {
                        let outcome = CatchUpOutcome::of_read::<()>(&Err(error.clone()));
                        this.ticks.dirty = report_result_is_stale(
                            report_req,
                            this.current_report_generation(),
                            true,
                        );
                        let keep = this.keep_on_catch_up(after_report, outcome, report_req);
                        this.ticks.publish(Err(error), keep);
                        if after_report {
                            this.settle_report_refresh_retry(outcome.is_transient(), cx);
                        }
                        cx.notify();
                        return;
                    }
                };
                let addresses = this.resolve_addresses(&read.deals, cx);
                this.start_replay_stage(
                    req,
                    report_req,
                    after_report,
                    read,
                    fact,
                    now,
                    addresses,
                    cx,
                );
            },
        );
    }

    /// Where each deal's prints live, per distinct `(core, coin)`: the core's exchange key and
    /// the catalog-verified market. A core that is not connected, or a coin its catalog does
    /// not spell, resolves to nothing and the row says so.
    fn resolve_addresses(
        &self,
        deals: &[Deal],
        cx: &Context<Self>,
    ) -> HashMap<(u64, String), Option<Arc<RowAddress>>> {
        let backend = self.backend.read(cx);
        let source = backend.session.market_source();
        let mut out: HashMap<(u64, String), Option<Arc<RowAddress>>> = HashMap::new();
        for deal in deals {
            let key = (deal.core_uid, deal.coin.clone());
            if out.contains_key(&key) {
                continue;
            }
            let quote = backend
                .config
                .servers
                .iter()
                .find(|s| s.id == deal.core_uid)
                .map(|s| s.market.as_str())
                .unwrap_or_default();
            let address = source
                .replay_address(deal.core_uid)
                .ok()
                .and_then(|address| {
                    let market = source.resolve_market(deal.core_uid, quote, &deal.coin)?;
                    let tick = source.price_step(deal.core_uid, &market);
                    Some(Arc::new(RowAddress {
                        core_uid: deal.core_uid,
                        exchange_key: address.exchange_key,
                        market,
                        tick,
                    }))
                });
            out.insert(key, address);
        }
        out
    }

    /// Stage B: the held tape of every deal, the archived entry line, the model on the
    /// parameters as of the buy. Off the UI thread; the worker's answers are waited for one
    /// at a time.
    #[allow(clippy::too_many_arguments)]
    fn start_replay_stage(
        &mut self,
        req: u64,
        report_req: u64,
        after_report: bool,
        read: DealsRead,
        fact: Vec<VarStats>,
        now: HashMap<String, NowValue>,
        addresses: HashMap<(u64, String), Option<Arc<RowAddress>>>,
        cx: &mut Context<Self>,
    ) {
        let defaults = self.filter_defaults(cx);
        self.spawn_latest_db(
            &[ReadLane::TicksReplay],
            false,
            cx,
            move || {
                let mut rows: Vec<DealRow> = read
                    .deals
                    .into_iter()
                    .map(|deal| {
                        let address = addresses
                            .get(&(deal.core_uid, deal.coin.clone()))
                            .cloned()
                            .flatten();
                        DealRow {
                            deal,
                            tape: if address.is_some() {
                                TapeStatus::Missing
                            } else {
                                TapeStatus::NoAddress
                            },
                            verdict: None,
                            address,
                            ticks: None,
                            entry_start: None,
                        }
                    })
                    .collect();
                let mut traces = archived_lines(&rows);
                for row in &mut rows {
                    // Each row waits on the worker; a scope change cancels this lane, and the
                    // wait is not a statement the progress handler could interrupt.
                    if moon_core::db::current_is_cancelled() {
                        break;
                    }
                    let lines = traces.remove(&row.deal.report_uid).unwrap_or_default();
                    replay_row(row, &defaults, lines);
                }
                let mut kinds: Vec<String> = rows.iter().map(|r| r.deal.kind.clone()).collect();
                kinds.sort();
                kinds.dedup();
                let mut data = TicksData {
                    rows,
                    without_ms: read.without_ms,
                    kpi: fact,
                    entry_share: (0, 0),
                    exit_share: (0, 0),
                    kinds,
                    now,
                };
                data.retain_within_cap();
                data.refresh_summary();
                data
            },
            move |this, data, cx| {
                if this.ticks.seq != req {
                    return;
                }
                this.ticks.dirty =
                    report_result_is_stale(report_req, this.current_report_generation(), false);
                this.ticks.publish(Ok(data), false);
                // The replayable set may have changed under the variant columns: rescore them.
                this.arm_ticks_variants(cx);
                if after_report {
                    this.settle_report_refresh_retry(false, cx);
                }
                cx.notify();
            },
        );
    }
}

/// The grid's "now" column: every selected strategy's current value per field, folded to
/// one value or "varies".
fn now_values(targets: &[(i64, Option<u64>)], keys: &[String]) -> HashMap<String, NowValue> {
    let mut seen: HashMap<String, Vec<Option<String>>> = HashMap::new();
    for &(sid, core) in targets {
        let values = strategy_current_values(sid, core, keys);
        for key in keys {
            seen.entry(key.clone())
                .or_default()
                .push(values.get(key).cloned());
        }
    }
    seen.into_iter()
        .map(|(key, values)| {
            let first = values.first().cloned().flatten();
            let same = values.iter().all(|v| v.as_deref() == first.as_deref());
            let value = if same {
                NowValue::Same(first.unwrap_or_default())
            } else {
                NowValue::Differs
            };
            (key, value)
        })
        .collect()
}

/// What the order archive holds of one deal's own lines: the entry line's first point and
/// the exit line's points.
#[derive(Clone, Debug, Default)]
pub(super) struct ArchivedLines {
    pub(super) entry_start: Option<(i64, f64)>,
    pub(super) exit_points: Option<Vec<(i64, f64)>>,
}

impl ArchivedLines {
    /// Read from one archived entry.
    pub(super) fn of(entry: &TraceEntry) -> Self {
        let TraceEntry::Lines(lines) = entry else {
            return Self::default();
        };
        let entry_start = lines
            .iter()
            .find(|l| l.own && l.kind == ArchivedLineKind::Entry)
            .and_then(|l| l.points.first().map(|&(t, p)| (t as i64, p)));
        let exit_points = lines
            .iter()
            .find(|l| l.own && l.kind == ArchivedLineKind::Exit)
            .map(|l| l.points.iter().map(|&(t, p)| (t as i64, p)).collect());
        Self {
            entry_start,
            exit_points,
        }
    }
}

/// The archived lines of every deal, read once per core.
fn archived_lines(rows: &[DealRow]) -> HashMap<i64, ArchivedLines> {
    let mut by_core: HashMap<u64, Vec<i64>> = HashMap::new();
    for row in rows {
        by_core
            .entry(row.deal.core_uid)
            .or_default()
            .push(row.deal.report_uid);
    }
    let mut out = HashMap::new();
    for (core, uids) in by_core {
        let Ok(entries) = read_many(core, &uids) else {
            continue;
        };
        for (uid, entry) in entries {
            out.insert(uid, ArchivedLines::of(&entry));
        }
    }
    out
}

/// The held prints of one deal's window, through the worker. `None` when the worker did not
/// answer in time.
pub(super) fn held_tape(
    address: &RowAddress,
    deal: &Deal,
) -> Option<(Vec<Tick>, Coverage, Coverage)> {
    let window = replay_window_ms(deal.buy_ms, deal.close_ms, margin_ms())?;
    let spans = window.focus_spans();
    let (reply, rx) = mpsc::channel();
    query_held(TickQuery {
        exchange_key: address.exchange_key.clone(),
        market: address.market.clone(),
        spans: spans.clone(),
        reply,
    });
    let answer = rx.recv_timeout(HELD_ANSWER_WAIT).ok()?;
    Some((answer.ticks, answer.covered, spans))
}

/// Run the model on one row, from what the worker holds; a row without an address is left
/// as it is. A covered row keeps its tape and its archived entry start for the variants.
pub(super) fn replay_row(row: &mut DealRow, defaults: &HashMap<String, f64>, lines: ArchivedLines) {
    row.ticks = None;
    row.entry_start = lines.entry_start;
    let Some(address) = row.address.clone() else {
        return;
    };
    let Some((ticks, covered, spans)) = held_tape(&address, &row.deal) else {
        row.tape = TapeStatus::Missing;
        row.verdict = None;
        return;
    };
    let first = ticks.first().map(|t| t.time_ms as i64);
    let last = ticks.last().map(|t| t.time_ms as i64);
    let complete = covered.covers(&spans)
        && first.is_some_and(|f| f <= row.deal.buy_ms - RUN_UP_MS)
        && last.is_some_and(|l| l >= row.deal.close_ms);
    if !complete {
        row.tape = TapeStatus::Missing;
        row.verdict = None;
        return;
    }
    row.tape = TapeStatus::Covered;
    row.deal.tick = address.tick.or_else(|| infer_tick(&ticks));
    let keys = params::param_keys();
    let values = strategy_values_at(
        row.deal.strategy_id,
        Some(row.deal.core_uid),
        row.deal.buy_ms,
        &keys,
    )
    .unwrap_or_default();
    let sv = params::StrategyValues {
        values: &values,
        defaults,
    };
    let entry = if entry_model_for(&row.deal.kind) {
        EntryParams::MoonShot(params::mshot_params(
            &sv,
            moon_core::db::tuner::ticks::mshot::DEFAULT_LATENCY_MS,
        ))
    } else {
        EntryParams::Fact
    };
    let exit = params::exit_params(&sv);
    row.verdict = Some(verify(
        &row.deal,
        &ticks,
        &entry,
        &exit,
        lines.entry_start,
        lines.exit_points.as_deref(),
    ));
    row.ticks = Some(Arc::from(ticks));
}
