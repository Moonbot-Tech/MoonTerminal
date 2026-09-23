//! Background loads of the "Entry/Exit" axis, in two stages.
//!
//! Stage A reads the scope's deals and the grid's "now" values off the database. Its
//! completion resolves, on the UI thread, where each deal's prints live — the core's exchange
//! key and the coin's market, which only the live market source knows — and publishes the rows
//! at once, without their tape (stage B). Stage C then asks the replay worker for the held tape of every row in
//! ONE batch of queries, reads the archived entry lines, runs the model on the parameters as
//! of the buy, and folds the answers into the published rows. In one batch because it is one
//! round trip for the table: the worker's coordinator answers held queries off no venue call,
//! so each costs a lock and a disk read, and a thousand of them asked together come back in
//! the time of one.
//!
//! This file only ever WRITES `TicksState`; the rendering only reads it.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use gpui::*;

use super::super::super::AnalyticsView;
use super::state::{DealRow, NowValue, OwnValues, RowAddress, TapeStatus, TicksData};
use super::tape;
use crate::analytics::bg::ReadLane;
use crate::analytics::refresh::{CatchUpOutcome, report_result_is_stale};
use moon_core::db::ReadFail;
use moon_core::db::order_traces::{TraceEntry, read_many};
use moon_core::db::tuner::ticks::{
    Deal, DealsRead, EntryParams, ModelSettings, OwnLines, deltas, entry_model_for, infer_tick,
    model_window, params, prepare_deal, required_spans, verify,
};
use moon_core::db::tuner::{strategy_current_values, strategy_values_at};
use moon_core::feed::report_traces::ArchivedLineKind;
use moon_core::feed::types::Tick;
use moon_core::market::kline_cache::KlineCache;
use moon_core::market::trade_replay::venue_caps::trade_route;
use moon_core::market::trade_replay::worker::inside_retention;
use moon_core::market::trade_replay::{
    Coverage, ReplayWindow, TickQuery, TickStatus, long_position_ms, margin_ms, query_held,
};

/// How long a held query waits for the worker's answer. The coordinator answers held queries
/// off no venue call, so the wait is normally milliseconds; the ceiling is for a disk that
/// stalls — past it the rows still unanswered fold as missing, and the log says how many.
const HELD_ANSWER_WAIT: Duration = Duration::from_secs(240);

/// What stage A brings back: the deals and the grid's "now" values.
type StageA = (
    Result<DealsRead, ReadFail>,
    HashMap<String, NowValue>,
    OwnValues,
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
        ]);
        self.ticks.seq = self.ticks.seq.wrapping_add(1);
        // The tape stage the cancel above dropped is not reading any more; the new load's own
        // stage C raises the flag again when it starts.
        self.ticks.tape_reading = false;
        // A search over a scope the user left answers nothing about the new one; the lane
        // cancel does not reach its handle, only this does. A report that moved is the SAME
        // scope with a trade more — a trade closing on any core, every few seconds on a busy
        // fleet — and the search runs on a copy of the deals: it finishes into В1 and the
        // columns are rescored over the reloaded rows. Stopped there, a run of a minute never
        // finished at all, and said nothing.
        if !after_report {
            self.latest_reads.cancel(&[ReadLane::TicksSearch]);
            self.ticks.stop_search();
        }
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
                // `q.strategies` is empty for "every strategy" (0 targets).
                match &deals {
                    Ok(read) => log::info!(
                        target: moon_core::diagnostics::TICKS_AXIS_TARGET,
                        "[x] ticks load: {} deal(s) with ms stamps, {} without, {} service, {} not tunable, period {}..{}, {} strategy target(s)",
                        read.deals.len(),
                        read.without_ms,
                        read.service,
                        read.untunable,
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
                let own = deals
                    .as_ref()
                    .map(|read| own_values(&read.deals, &keys))
                    .unwrap_or_default();
                let now = now_values(&targets, &keys, &own);
                (deals, now, own)
            },
            move |this, (deals, now, own): StageA, cx| {
                if this.ticks.seq != req {
                    return;
                }
                let read = match deals {
                    Ok(read) => read,
                    Err(error) => {
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
                    (now, own),
                    addresses,
                    cx,
                );
            },
        );
    }

    /// Where each deal's prints live, per distinct `(core, coin)` — the fetch's own resolver,
    /// asked once per distinct pair. A core that is not connected, or a coin its catalog does
    /// not spell, resolves to nothing and the row says so.
    fn resolve_addresses(
        &self,
        deals: &[Deal],
        cx: &Context<Self>,
    ) -> HashMap<(u64, String), Option<Arc<RowAddress>>> {
        let mut resolver = super::fetch::FetchResolver::of(&self.backend.read(cx));
        deals
            .iter()
            .map(|deal| ((deal.core_uid, deal.coin.clone()), resolver.address(deal)))
            .collect()
    }

    /// Stage B: the rows, published at once — each with what the last load already judged of
    /// it, when that still holds ([`carryable`]), else without its tape; stage C follows for
    /// the rest.
    ///
    /// A reload comes every time the report moves — a trade closing on any core, every few
    /// seconds on a busy fleet — and used to publish every row blank and read and replay the
    /// whole table again: the table and the KPI blinked empty for the length of that, and the
    /// work grew with the table, not with what changed. A row the model already judged under
    /// the settings in force keeps its verdict; stage C reads and replays only the others.
    fn start_replay_stage(
        &mut self,
        req: u64,
        report_req: u64,
        after_report: bool,
        read: DealsRead,
        (now, own): (HashMap<String, NowValue>, OwnValues),
        addresses: HashMap<(u64, String), Option<Arc<RowAddress>>>,
        cx: &mut Context<Self>,
    ) {
        let model = super::model_cfg::current();
        let judged: HashMap<i64, DealRow> = if self.ticks.judged_under == Some(model) {
            self.ticks
                .data
                .data()
                .map(|d| {
                    d.rows
                        .iter()
                        .filter(|r| matches!(r.tape, TapeStatus::Covered | TapeStatus::Refused(_)))
                        .map(|r| (r.deal.report_uid, r.clone()))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            HashMap::new()
        };
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
                        let before = judged
                            .get(&deal.report_uid)
                            .filter(|before| address.is_some() && carryable(&before.deal, &deal))
                            .filter(|before| after_report || !before.lost_tape())
                            .cloned();
                        let mut row = DealRow {
                            deal,
                            tape: if address.is_some() {
                                TapeStatus::Missing
                            } else {
                                TapeStatus::NoAddress
                            },
                            verdict: None,
                            address,
                            ticks: None,
                            entry_line: None,
                            held: None,
                        };
                        if let Some(before) = before {
                            row.take_replay(before);
                        }
                        row
                    })
                    .collect();
                let carried = rows
                    .iter()
                    .any(|r| matches!(r.tape, TapeStatus::Covered | TapeStatus::Refused(_)));
                let mut kinds: Vec<String> = rows.iter().map(|r| r.deal.kind.clone()).collect();
                kinds.sort();
                kinds.dedup();
                let mut data = TicksData {
                    rows,
                    without_ms: read.without_ms,
                    service: read.service,
                    untunable: read.untunable,
                    kpi: Vec::new(),
                    entry_share: (0, 0),
                    exit_share: (0, 0),
                    kinds,
                    now,
                    own,
                };
                data.retain_within_cap();
                data.refresh_summary();
                (data, carried)
            },
            move |this, (data, carried), cx| {
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
                this.start_tape_stage(req, carried.then_some(model), cx);
                if after_report {
                    this.settle_report_refresh_retry(false, cx);
                }
                cx.notify();
            },
        );
    }

    /// The model's settings changed: judge every row again under them — stage C alone, since
    /// the deals and their tape are what they were. A load still before its stage C reads the
    /// new settings when it gets there and is left alone: cancelling its lane would drop the
    /// rows it is about to publish.
    pub(in crate::analytics::tuner) fn ticks_replay_again(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.ticks.data, crate::load_state::LoadState::Ready(_)) {
            return;
        }
        self.latest_reads.cancel(&[
            ReadLane::TicksReplay,
            ReadLane::TicksVariants,
            ReadLane::TicksSearch,
        ]);
        self.ticks.stop_search();
        // Every verdict of the table is of the old settings now: none may be carried by a
        // reload until this stage has judged them all again.
        self.ticks.judged_under = None;
        let req = self.ticks.seq;
        self.start_tape_stage(req, None, cx);
    }

    /// Stage C: the held tape of every published row, asked from the worker in one batch, the
    /// archived entry lines, and the model on the parameters as of the buy — folded into the
    /// rows when all of it is in.
    ///
    /// Args:
    ///     req: The load generation this stage belongs to.
    ///     carried: The settings the rows stage B carried were judged under, when it carried
    ///         any: those rows are not read again. `None` reads and replays every row.
    fn start_tape_stage(
        &mut self,
        req: u64,
        carried: Option<ModelSettings>,
        cx: &mut Context<Self>,
    ) {
        // One set of model settings for the whole stage, read once: every row of a table is
        // judged by the same rules.
        let model = super::model_cfg::current();
        // Rows carried under settings changed since stage B read them are not carried: the
        // stage reads and replays them too, so the table folds under `model` alone.
        let carried = carried.filter(|m| *m == model);
        // This stage's own generation: a stage started after it — a changed setting re-judging
        // the table under the same load — makes its answer stale, even though a cancelled
        // stage still hands its partial rows to `store` (`spawn_latest_db`).
        self.ticks.tape_seq = self.ticks.tape_seq.wrapping_add(1);
        let tape_req = self.ticks.tape_seq;
        let Some(data) = self.ticks.data.data() else {
            return;
        };
        let targets: Vec<(Deal, Arc<RowAddress>)> = data
            .rows
            .iter()
            .filter(|r| {
                carried.is_none() || !matches!(r.tape, TapeStatus::Covered | TapeStatus::Refused(_))
            })
            .filter_map(|r| Some((r.deal.clone(), r.address.clone()?)))
            .collect();
        if targets.is_empty() {
            log_tape_budget(data);
            self.ticks.tape_reading = false;
            self.ticks.judged_under = Some(model);
            // Every row was carried: no fold follows to rescore the variant columns, and the
            // load's `invalidate` has already dropped their scores and the plan column — a
            // narrower selection over deals already judged would keep the edits and show
            // nothing for them.
            self.arm_ticks_variants(cx);
            return;
        }
        let defaults = self.filter_defaults(cx);
        // The kline cache the live deltas read their history bars off (`deltas::track_for`).
        let klines = self.backend.read(cx).session.market_source().kline_cache();
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
                // One threshold for the whole table, read once: every row of one load splits
                // its window the same way.
                let long_position_ms = moon_core::market::trade_replay::long_position_ms();
                let mut tapes = held_tapes(&targets, long_position_ms);
                let mut rows: Vec<DealRow> = targets
                    .into_iter()
                    .map(|(deal, address)| DealRow {
                        deal,
                        tape: TapeStatus::Missing,
                        verdict: None,
                        address: Some(address),
                        ticks: None,
                        entry_line: None,
                        held: None,
                    })
                    .collect();
                let mut traces = archived_lines(&rows);
                // Before any row is replayed: each core's step lag, off these rows' archives.
                super::lags::calibrate_from(&rows, &traces, &defaults, model);
                let now_ms = moon_core::util::now_unix_ms_i64();
                for row in &mut rows {
                    let lines = traces.remove(&row.deal.report_uid).unwrap_or_default();
                    let tape = tapes.remove(&row.deal.report_uid);
                    let answered = tape.is_some();
                    replay_row_with(row, &defaults, model, lines, tape, klines.as_ref());
                    // Said at load, not after a walk: a row the venue cannot serve is not
                    // "missing" — it would only ever come back refused. The fetch job's own
                    // path (`replay_row` after a walk) is NOT given this: its retries and its
                    // continuation read `Missing`, and its refusal is the walk's own word.
                    // Nor is a row whose held query went unanswered: the tile store was not
                    // read for it, so "cannot be fetched" would be said of a store never asked.
                    if answered && row.tape == TapeStatus::Missing {
                        if let Some(address) = row.address.as_ref() {
                            row.tape = unservable_status(address, &row.deal, now_ms)
                                .unwrap_or(TapeStatus::Missing);
                        }
                    }
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
                        replay_row(
                            row,
                            &defaults,
                            model,
                            lines,
                            long_position_ms,
                            klines.as_ref(),
                        );
                    }
                }
                rows
            },
            move |this, rows, cx| {
                if this.ticks.seq != req || this.ticks.tape_seq != tape_req {
                    return;
                }
                this.ticks.tape_reading = false;
                this.ticks.judged_under = Some(model);
                this.ticks.update_rows(rows);
                if let Some(data) = this.ticks.data.data() {
                    log_tape_budget(data);
                }
                // The row the fetch job is out for says so again after the fold.
                this.mark_fetch_in_flight();
                // The rows still missing are what the user is looking at: a running batch
                // takes them next.
                this.ticks_prioritize_visible();
                // With the autoload on, the rows of THIS table the venue can still serve go to
                // the fetch without a press: the switch is the consent to spend the budget,
                // and the startup pass covers only its own horizon (30 days, every core) —
                // a wider period on the table would otherwise sit behind a button.
                if moon_core::market::trade_replay::tape_autoload() {
                    this.ticks_fetch_missing(cx);
                }
                // The replayable set may have changed under the variant columns: rescore them.
                this.arm_ticks_variants(cx);
                cx.notify();
            },
        );
    }
}

/// The held tape of every target, asked from the worker in one batch and collected in order.
/// A query the worker did not answer in time, or one cancelled by a scope change, is absent.
fn held_tapes(
    targets: &[(Deal, Arc<RowAddress>)],
    long_position_ms: i64,
) -> HashMap<i64, HeldTape> {
    let deadline = std::time::Instant::now() + HELD_ANSWER_WAIT;
    let asked: Vec<(
        i64,
        mpsc::Receiver<moon_core::market::trade_replay::TickAnswer>,
        ReplayWindow,
    )> = targets
        .iter()
        .filter_map(|(deal, address)| {
            let (rx, window) = ask_held(address, deal, long_position_ms)?;
            Some((deal.report_uid, rx, window))
        })
        .collect();
    let asked_n = asked.len();
    let mut out = HashMap::with_capacity(asked_n);
    let mut unanswered = 0usize;
    for (uid, rx, window) in asked {
        if moon_core::db::current_is_cancelled() {
            break;
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Ok(answer) = rx.recv_timeout(remaining) else {
            unanswered += 1;
            continue;
        };
        out.insert(uid, (answer.ticks, answer.covered, window));
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

/// One held query sent, with the window whose spans it asked for; the answer arrives on the
/// receiver. The window is the model's (`model_window`: from the entry order's creation where
/// the report stamps it). `long_position_ms` is the caller's — a queued row's own window's, or
/// one read for a whole table — so the split is the one every other stage of that row used.
fn ask_held(
    address: &RowAddress,
    deal: &Deal,
    long_position_ms: i64,
) -> Option<(
    mpsc::Receiver<moon_core::market::trade_replay::TickAnswer>,
    ReplayWindow,
)> {
    let window = model_window(deal, margin_ms(), long_position_ms)?;
    let (reply, rx) = mpsc::channel();
    query_held(TickQuery {
        exchange_key: address.exchange_key.clone(),
        market: address.market.clone(),
        spans: window.focus_spans(),
        reply,
    });
    Some((rx, window))
}

/// The grid's "now" column: every selected strategy's current value per field, folded to
/// one value or "varies". A target on a known core that the deals' strategies already read
/// (`own`, from [`own_values`]) is not read again.
fn now_values(
    targets: &[(i64, Option<u64>)],
    keys: &[String],
    own: &OwnValues,
) -> HashMap<String, NowValue> {
    let mut seen: HashMap<String, Vec<Option<String>>> = HashMap::new();
    for &(sid, core) in targets {
        let values = match core.and_then(|core| own.get(&(sid, core))) {
            Some(values) => Arc::clone(values),
            None => Arc::new(strategy_current_values(sid, core, keys)),
        };
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

/// Every strategy of `deals` as it stands now, read once per `(strategy_id, core)` — the base
/// each deal's variants run over ([`TicksData::own`]). A strategy that cannot be read gets an
/// empty map, and its deals read every field at its default, as the grid's "now" does.
fn own_values(deals: &[Deal], keys: &[String]) -> OwnValues {
    let mut out = OwnValues::new();
    for deal in deals {
        out.entry((deal.strategy_id, deal.core_uid))
            .or_insert_with(|| {
                Arc::new(strategy_current_values(
                    deal.strategy_id,
                    Some(deal.core_uid),
                    keys,
                ))
            });
    }
    out
}

/// What the order archive holds of one deal's own lines: the entry line's points, the exit
/// line's points, and whether the core answered for the deal with lines at all — without them
/// a missing entry line proves nothing (`record::entry_placement`).
#[derive(Clone, Debug, Default)]
pub(super) struct ArchivedLines {
    pub(super) entry_points: Option<Arc<[(i64, f64)]>>,
    pub(super) exit_points: Option<Vec<(i64, f64)>>,
    pub(super) answered: bool,
}

impl ArchivedLines {
    /// Read from one archived entry.
    pub(super) fn of(entry: &TraceEntry) -> Self {
        let TraceEntry::Lines(lines) = entry else {
            return Self::default();
        };
        let entry_points = lines
            .iter()
            .find(|l| l.own && l.kind == ArchivedLineKind::Entry)
            .map(|l| l.points.iter().map(|&(t, p)| (t as i64, p)).collect());
        let exit_points = lines
            .iter()
            .find(|l| l.own && l.kind == ArchivedLineKind::Exit)
            .map(|l| l.points.iter().map(|&(t, p)| (t as i64, p)).collect());
        Self {
            entry_points,
            exit_points,
            answered: true,
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

/// The held prints of one deal's window, their coverage, and the window that was asked for.
type HeldTape = (Vec<Tick>, Coverage, ReplayWindow);

/// The held prints of one deal's window, through the worker. `None` when the worker did not
/// answer in time.
pub(super) fn held_tape(
    address: &RowAddress,
    deal: &Deal,
    long_position_ms: i64,
) -> Option<HeldTape> {
    let (rx, window) = ask_held(address, deal, long_position_ms)?;
    let answer = rx.recv_timeout(HELD_ANSWER_WAIT).ok()?;
    Some((answer.ticks, answer.covered, window))
}

/// Why a fetch of a row the terminal holds no tape for could only come back refused, said at
/// load rather than after a walk: no public route for the venue (the worker would serve such a
/// stage from the tile store alone, which the held query just found empty), or a window older
/// than the route's retention. `None` where the venue could serve it. The same rule the fetch
/// job and the startup autoload apply (`inside_retention`), asked here so the row does not
/// read as fetchable — and is not queued — when it is not.
fn unservable_status(address: &RowAddress, deal: &Deal, now_ms: i64) -> Option<TapeStatus> {
    let Some(route) = trade_route(address.venue) else {
        return Some(TapeStatus::Refused(TickStatus::NoRoute));
    };
    let window = model_window(deal, margin_ms(), long_position_ms())?;
    let retention_ms = route.retention_ms()?;
    (!inside_retention(route, window, now_ms)).then_some(TapeStatus::Refused(
        TickStatus::OutOfRetention { retention_ms },
    ))
}

/// Run the model on one row, from what the worker holds — asked here, one query; a row
/// without an address is left as it is. `long_position_ms` is the row's own threshold — see
/// [`ask_held`].
pub(super) fn replay_row(
    row: &mut DealRow,
    defaults: &HashMap<String, f64>,
    model: ModelSettings,
    lines: ArchivedLines,
    long_position_ms: i64,
    klines: Option<&KlineCache>,
) {
    let tape = row
        .address
        .as_ref()
        .and_then(|address| held_tape(address, &row.deal, long_position_ms));
    replay_row_with(row, defaults, model, lines, tape, klines);
}

/// Run the model on one row from a tape already asked for. A covered row keeps its tape and
/// its archived entry start for the variants; a row without an address is left as it is.
///
/// The model inputs read off the order archive go through `prepare_deal`, and the live deltas
/// through `deltas::track_for` — the same calls the `real_data` bench makes, so what it measures
/// is what this table shows. Without the kline cache the deal keeps the report's snapshot.
pub(super) fn replay_row_with(
    row: &mut DealRow,
    defaults: &HashMap<String, f64>,
    model: ModelSettings,
    lines: ArchivedLines,
    tape: Option<HeldTape>,
    klines: Option<&KlineCache>,
) {
    row.ticks = None;
    row.entry_line = lines.entry_points.clone();
    row.held = None;
    row.deal.delta_track = None;
    let Some(address) = row.address.clone() else {
        return;
    };
    let Some((ticks, covered, window)) = tape else {
        row.tape = TapeStatus::Missing;
        row.verdict = None;
        return;
    };
    row.held = covered.hull().map(|(from, to)| {
        (
            row.deal.buy_ms.saturating_sub(from).max(0),
            to.saturating_sub(row.deal.close_ms).max(0),
        )
    });
    // Coverage is the worker's own word on what was walked; a quiet run-up with no print in it
    // is covered all the same, which the tape's first stamp could not tell from a missing one.
    // What must be covered is the model's own rule (`required_spans`), not the whole margin.
    if ticks.is_empty() || !covered.covers(&required_spans(&window)) {
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
        EntryParams::MoonShot(params::mshot_params(&sv, model))
    } else {
        EntryParams::Fact
    };
    let exit = params::exit_params(&sv, model);
    // The deltas along the window, before the record's inputs: the stop anchor reads the stop
    // through them.
    row.deal.delta_track = klines.and_then(|cache| {
        deltas::track_for(
            cache,
            &address.exchange_key,
            &address.market,
            address.btc_market.as_deref(),
            &row.deal,
            &ticks,
            &covered,
        )
    });
    // What the core's own record fixes: the ask its take was lifted to, the take as placed,
    // where the entry order was placed, what the fact proves about the stop, the entry the
    // trade ran with.
    prepare_deal(
        &mut row.deal,
        &entry,
        &exit,
        OwnLines {
            entry: lines.entry_points.as_deref(),
            exit: lines.exit_points.as_deref(),
            answered: lines.answered,
        },
    );
    // The core's own clock for its PriceDown steps, as the last load calibrated it.
    row.deal.step_lag_ms = super::lags::step_lag_of(row.deal.core_uid);
    row.verdict = Some(verify(
        &row.deal,
        &ticks,
        &entry,
        &exit,
        lines.entry_points.as_deref(),
        lines.exit_points.as_deref(),
    ));
    row.ticks = Some(tape::PackedTape::pack(ticks));
}

/// One line per load on what the table holds of its tape — the numbers the memory budget
/// is judged by, read from the log instead of guessed.
fn log_tape_budget(data: &TicksData) {
    let b = data.tape_budget();
    log::info!(
        target: moon_core::diagnostics::TICKS_AXIS_TARGET,
        "[x] ticks tape: {} row(s), {} fit, {} with tape in memory, {} let go by the cap · {} print(s), {:.1} MiB of {:.0} MiB",
        b.rows,
        b.fit,
        b.replayable,
        b.dropped,
        b.prints,
        b.bytes as f64 / 1_048_576.0,
        super::state::MAX_RETAINED_BYTES as f64 / 1_048_576.0
    );
}

/// Whether what the last load judged of a trade still describes the row a reload read for it:
/// the same trade, with the same stamps and prices the model reads. A report row rewritten under
/// the same uid — a close booked late, a price corrected — is judged afresh.
pub(super) fn carryable(before: &Deal, now: &Deal) -> bool {
    before.report_uid == now.report_uid
        && before.core_uid == now.core_uid
        && before.strategy_id == now.strategy_id
        && before.buy_ms == now.buy_ms
        && before.close_ms == now.close_ms
        && before.buy_price == now.buy_price
        && before.sell_price == now.sell_price
        && before.order_open_ms() == now.order_open_ms()
}
