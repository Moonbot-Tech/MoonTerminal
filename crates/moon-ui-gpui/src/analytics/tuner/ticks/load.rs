//! Background loads of the "Entry/Exit" axis, in two stages.
//!
//! Stage A reads the scope's deals, the whole-scope "Fact" KPI (the same SQL every axis'
//! "Fact" comes from) and the grid's "now" values off the database. Its completion resolves,
//! on the UI thread, where each deal's prints live — the core's exchange key and the coin's
//! market, which only the live market source knows — and publishes the rows at once, without
//! their tape (stage B). Stage C then asks the replay worker for the held tape of every row in
//! ONE batch of queries, reads the archived entry lines, runs the model on the parameters as
//! of the buy, and folds the answers into the published rows. In one batch, because the worker
//! is one thread that serves held queries only between its walks: asked one at a time, a
//! thousand rows would each wait for a walk of the fetch batch that may be running.
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
    Deal, DealsRead, EntryParams, entry_model_for, infer_tick, params, required_spans, verify,
};
use moon_core::db::tuner::{VarStats, Variant, strategy_current_values, strategy_values_at};
use moon_core::feed::report_traces::ArchivedLineKind;
use moon_core::feed::types::Tick;
use moon_core::market::trade_replay::{
    Coverage, TickQuery, margin_ms, query_held, replay_window_ms,
};

/// How long a held query waits for the worker's answer. The worker serves a held query
/// between its jobs, and a walk of the fetch batch can hold it for up to its trade deadline
/// plus a candle stage, with chart windows' own stages queued ahead; past this the rows still
/// unanswered fold as missing, and the log says how many.
const HELD_ANSWER_WAIT: Duration = Duration::from_secs(240);

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
            ReadLane::TicksVariants,
            ReadLane::TicksSearch,
        ]);
        self.ticks.seq = self.ticks.seq.wrapping_add(1);
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
                // One line per load, so "no deals" can be read against the scope that was
                // actually asked — period, strategies — instead of guessed from the panel.
                match &deals {
                    Ok(read) => log::info!(
                        target: moon_core::diagnostics::TICKS_AXIS_TARGET,
                        "[x] ticks load: {} deal(s) with ms stamps, {} without, period {}..{}, {} strategy target(s)",
                        read.deals.len(),
                        read.without_ms,
                        q.from,
                        q.to,
                        q.strategies.len()
                    ),
                    Err(error) => log::info!(
                        target: moon_core::diagnostics::TICKS_AXIS_TARGET,
                        "[x] ticks load failed: {error:?}, period {}..{}, {} strategy target(s)",
                        q.from,
                        q.to,
                        q.strategies.len()
                    ),
                }
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

    /// Stage B: the rows, published at once without their tape; stage C follows.
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
        self.spawn_latest_db(
            &[ReadLane::TicksReplay],
            false,
            cx,
            move || {
                let rows: Vec<DealRow> = read
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
                // The fetch job runs on across reloads and windows: a window that finds a batch
                // running listens to it from here on.
                if super::fetch::job::progress().active {
                    this.attach_fetch_listener(cx);
                }
                this.start_tape_stage(req, cx);
                if after_report {
                    this.settle_report_refresh_retry(false, cx);
                }
                cx.notify();
            },
        );
    }

    /// Stage C: the held tape of every published row, asked from the worker in one batch, the
    /// archived entry lines, and the model on the parameters as of the buy — folded into the
    /// rows when all of it is in.
    fn start_tape_stage(&mut self, req: u64, cx: &mut Context<Self>) {
        let Some(data) = self.ticks.data.data() else {
            return;
        };
        let targets: Vec<(Deal, Arc<RowAddress>)> = data
            .rows
            .iter()
            .filter_map(|r| Some((r.deal.clone(), r.address.clone()?)))
            .collect();
        if targets.is_empty() {
            return;
        }
        let defaults = self.filter_defaults(cx);
        self.ticks.tape_reading = true;
        // The fetch job may answer rows while this stage reads them; the ones it answered after
        // this instant are read again at the end, or the stage would fold the tape it read
        // BEFORE the answer over the answer.
        let reading_since = std::time::Instant::now();
        self.spawn_latest_db(
            &[ReadLane::TicksReplay],
            false,
            cx,
            move || {
                let mut tapes = held_tapes(&targets);
                let mut rows: Vec<DealRow> = targets
                    .into_iter()
                    .map(|(deal, address)| DealRow {
                        deal,
                        tape: TapeStatus::Missing,
                        verdict: None,
                        address: Some(address),
                        ticks: None,
                        entry_start: None,
                    })
                    .collect();
                let mut traces = archived_lines(&rows);
                for row in &mut rows {
                    let lines = traces.remove(&row.deal.report_uid).unwrap_or_default();
                    let tape = tapes.remove(&row.deal.report_uid);
                    replay_row_with(row, &defaults, lines, tape);
                }
                // Rows the job answered while the batch was read: read again, each behind
                // whatever walk is running. A row the job answers during THIS loop is kept
                // covered by the fold (`update_rows`), not re-read once more.
                let late = super::fetch::job::finished_after(reading_since);
                if !late.is_empty() {
                    let traces = archived_lines(&rows);
                    for row in rows
                        .iter_mut()
                        .filter(|r| late.contains(&r.deal.report_uid))
                    {
                        if moon_core::db::current_is_cancelled() {
                            break;
                        }
                        let lines = traces
                            .get(&row.deal.report_uid)
                            .cloned()
                            .unwrap_or_default();
                        replay_row(row, &defaults, lines);
                    }
                }
                rows
            },
            move |this, rows, cx| {
                if this.ticks.seq != req {
                    return;
                }
                this.ticks.tape_reading = false;
                this.ticks.update_rows(rows);
                // The row the fetch job is out for says so again after the fold.
                this.mark_fetch_in_flight();
                // The replayable set may have changed under the variant columns: rescore them.
                this.arm_ticks_variants(cx);
                cx.notify();
            },
        );
    }
}

/// The held tape of every target, asked from the worker in one batch and collected in order.
/// A query the worker did not answer in time, or one cancelled by a scope change, is absent.
fn held_tapes(targets: &[(Deal, Arc<RowAddress>)]) -> HashMap<i64, HeldTape> {
    let deadline = std::time::Instant::now() + HELD_ANSWER_WAIT;
    let asked: Vec<(
        i64,
        mpsc::Receiver<moon_core::market::trade_replay::TickAnswer>,
        Coverage,
    )> = targets
        .iter()
        .filter_map(|(deal, address)| {
            let (rx, spans) = ask_held(address, deal)?;
            Some((deal.report_uid, rx, spans))
        })
        .collect();
    let asked_n = asked.len();
    let mut out = HashMap::with_capacity(asked_n);
    let mut unanswered = 0usize;
    for (uid, rx, spans) in asked {
        if moon_core::db::current_is_cancelled() {
            break;
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Ok(answer) = rx.recv_timeout(remaining) else {
            unanswered += 1;
            continue;
        };
        out.insert(uid, (answer.ticks, answer.covered, spans));
    }
    if unanswered > 0 {
        log::info!(
            target: moon_core::diagnostics::TICKS_AXIS_TARGET,
            "[x] ticks load: {unanswered} of {asked_n} held queries unanswered within {} s, folded as missing",
            HELD_ANSWER_WAIT.as_secs()
        );
    }
    out
}

/// One held query sent, with the spans it asked for; the answer arrives on the receiver.
fn ask_held(
    address: &RowAddress,
    deal: &Deal,
) -> Option<(
    mpsc::Receiver<moon_core::market::trade_replay::TickAnswer>,
    Coverage,
)> {
    let window = replay_window_ms(deal.buy_ms, deal.close_ms, margin_ms())?;
    let spans = window.focus_spans();
    let (reply, rx) = mpsc::channel();
    query_held(TickQuery {
        exchange_key: address.exchange_key.clone(),
        market: address.market.clone(),
        spans: spans.clone(),
        reply,
    });
    Some((rx, spans))
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

/// The held prints of one deal's window, their coverage, and the spans that were asked for.
type HeldTape = (Vec<Tick>, Coverage, Coverage);

/// The held prints of one deal's window, through the worker. `None` when the worker did not
/// answer in time.
pub(super) fn held_tape(address: &RowAddress, deal: &Deal) -> Option<HeldTape> {
    let (rx, spans) = ask_held(address, deal)?;
    let answer = rx.recv_timeout(HELD_ANSWER_WAIT).ok()?;
    Some((answer.ticks, answer.covered, spans))
}

/// Run the model on one row, from what the worker holds — asked here, one query; a row
/// without an address is left as it is.
pub(super) fn replay_row(row: &mut DealRow, defaults: &HashMap<String, f64>, lines: ArchivedLines) {
    let tape = row
        .address
        .as_ref()
        .and_then(|address| held_tape(address, &row.deal));
    replay_row_with(row, defaults, lines, tape);
}

/// Run the model on one row from a tape already asked for. A covered row keeps its tape and
/// its archived entry start for the variants; a row without an address is left as it is.
pub(super) fn replay_row_with(
    row: &mut DealRow,
    defaults: &HashMap<String, f64>,
    lines: ArchivedLines,
    tape: Option<HeldTape>,
) {
    row.ticks = None;
    row.entry_start = lines.entry_start;
    let Some(address) = row.address.clone() else {
        return;
    };
    let Some((ticks, covered, spans)) = tape else {
        row.tape = TapeStatus::Missing;
        row.verdict = None;
        return;
    };
    // Coverage is the worker's own word on what was walked; a quiet run-up with no print in it
    // is covered all the same, which the tape's first stamp could not tell from a missing one.
    // What must be covered is the model's own rule (`required_spans`), not the whole margin.
    if ticks.is_empty() || !covered.covers(&required_spans(&row.deal, &spans)) {
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
