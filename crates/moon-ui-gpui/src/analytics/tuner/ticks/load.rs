//! Background loads of the "Entry/Exit" axis, in two stages.
//!
//! Stage A reads the scope's deals, the grid's "now" values and the search ranges' automatic
//! spans (over the live strategies, `params::range`) off the database. Its
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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use gpui::*;

use super::super::super::AnalyticsView;
use super::state::{DealRow, NowValue, OwnValues, RowAddress, TapeStatus, TicksData};
use super::tape;
use super::unmodelled::{UnmodelledMap, unmodelled_map};
use crate::analytics::bg::ReadLane;
use crate::analytics::refresh::{CatchUpOutcome, report_result_is_stale};
use moon_core::db::ReadFail;
use moon_core::db::order_traces::{TraceEntry, read_many};
use moon_core::db::tuner::ticks::params::range::{FieldSpan, Population, field_span};
use moon_core::db::tuner::ticks::unmodelled::watched_keys;
use moon_core::db::tuner::ticks::{
    Deal, DealsRead, EntryParams, ModelSettings, OwnLines, ParamKind, TICK_PARAMS, deltas,
    entry_model_for, infer_tick, model_window, params, prepare_deal, required_spans, verify,
};
use moon_core::db::tuner::{StrategyValuesReader, strategy_current_values, strategy_values_at};
use moon_core::feed::report_traces::ArchivedLineKind;
use moon_core::feed::strategy_deps::FieldDeps;
use moon_core::feed::types::Tick;
use moon_core::market::kline_cache::KlineCache;
use moon_core::market::trade_replay::venue_caps::{
    ARCHIVE_DEPTH_MS, Fetchable, fetchable, trade_route,
};
use moon_core::market::trade_replay::{
    Coverage, ReplayWindow, TickQuery, TickStatus, long_position_ms, margin_ms, query_held,
};

/// How long a held query waits for the worker's answer. The coordinator answers held queries
/// off no venue call, so the wait is normally milliseconds; the ceiling is for a disk that
/// stalls — past it the rows still unanswered fold as missing, and the log says how many.
const HELD_ANSWER_WAIT: Duration = Duration::from_secs(240);

/// What stage A brings back: the deals, the grid's "now" values, the strategies' own values, the
/// exit fields outside the model they switch on, the selected strategies' values, the rules all
/// of it was read under — the grid chooses its rows by them — and the search ranges' spans.
type StageA = (
    Result<DealsRead, ReadFail>,
    HashMap<String, NowValue>,
    OwnValues,
    Arc<UnmodelledMap>,
    Vec<SelectedValues>,
    FieldDeps,
    Arc<HashMap<&'static str, FieldSpan>>,
);

/// What stage B publishes beside the rows: the strategies' values, the grid's layout, the
/// search ranges' spans and integer fields, and the period the rows were read under.
struct ScopeView {
    /// `(from, to)` of the load's query, UTC seconds (`TicksState::loaded_period`).
    period: (i64, i64),
    now: HashMap<String, NowValue>,
    own: OwnValues,
    unmodelled: Arc<UnmodelledMap>,
    grid: Arc<[super::sections::GridSection]>,
    spans: Arc<HashMap<&'static str, FieldSpan>>,
    integers: Arc<HashSet<&'static str>>,
}

impl AnalyticsView {
    /// Recompute the axis for the current scope.
    pub(in crate::analytics) fn reload_ticks(&mut self, cx: &mut Context<Self>) {
        self.reload_ticks_inner(false, true, false, cx);
    }

    /// Replace card rows for a new selection while preserving the independently scoped batch.
    pub(in crate::analytics::tuner) fn reload_ticks_for_selection(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.reload_ticks_inner(false, true, true, cx);
    }

    /// Recompute report-stale data through the writer-driven catch-up path.
    pub(in crate::analytics) fn reload_ticks_after_report(
        &mut self,
        show_overlay: bool,
        cx: &mut Context<Self>,
    ) {
        self.reload_ticks_inner(true, show_overlay, false, cx);
    }

    /// Reload card data with independent report catch-up and batch-lifetime policies.
    fn reload_ticks_inner(
        &mut self,
        after_report: bool,
        show_overlay: bool,
        keep_best: bool,
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
            if !keep_best {
                self.ticks.cancel_best();
            }
            self.ticks.stop_search("manual reload");
        }
        let req = self.ticks.seq;
        let report_req = self.current_report_generation();
        let q = self.tuner_query();
        let period = (q.from, q.to);
        let targets: Vec<(i64, Option<u64>)> = self.visible_target_keys(self.read_core_ids());
        // The models' fields, and every field the grid's sections draw fixed beside them, so
        // those rows show the strategies' values too. The models read theirs by name; the extra
        // keys only ride along in the maps. The schema signature comes off the same store read:
        // a schema that moves on after it makes the grid ask for another load (`grid.rs`), whose
        // keys then cover it.
        // The exit fields the model does not have ride along too, for the warning they raise.
        let mut keys = params::param_keys();
        for key in watched_keys() {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        let defaults = self.filter_defaults(cx);
        self.ticks.keys_sig = Some({
            let backend = self.backend.read(cx);
            let store = backend.session.store();
            for key in super::sections::schema_keys(store) {
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
            super::sections::schema_signature(store)
        });
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
                let (now, selected) = now_values(&targets, &keys, &own);
                // The rules are read per load: an edit of the file reaches the warning and the
                // grid on the axis' next load, never a process-lifetime copy.
                let deps = FieldDeps::load();
                let unmodelled = unmodelled_map(
                    own.iter()
                        .map(|(&(sid, core), values)| ((sid, Some(core)), values.as_ref()))
                        .chain(selected.iter().map(|(key, values)| (*key, values.as_ref()))),
                    &defaults,
                    &deps,
                );
                let kinds: Vec<String> = deals
                    .as_ref()
                    .map(|read| {
                        let mut kinds: Vec<String> =
                            read.deals.iter().map(|d| d.kind.clone()).collect();
                        kinds.sort();
                        kinds.dedup();
                        kinds
                    })
                    .unwrap_or_default();
                let spans = Arc::new(range_spans(&kinds, &selected, &defaults, &deps));
                (deals, now, own, Arc::new(unmodelled), selected, deps, spans)
            },
            move |this, (deals, now, own, unmodelled, selected, deps, spans): StageA, cx| {
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
                let integers = Arc::new(super::sections::integer_keys(
                    this.backend.read(cx).session.store(),
                ));
                let grid = super::sections::grid_for(
                    this.backend.read(cx).session.store(),
                    &read.deals,
                    own.iter()
                        .map(|(&(sid, core), values)| ((sid, Some(core)), values.as_ref()))
                        .chain(selected.iter().map(|(key, values)| (*key, values.as_ref()))),
                    &deps,
                )
                .into();
                let addresses = this.resolve_addresses(&read.deals, cx);
                this.start_replay_stage(
                    req,
                    report_req,
                    after_report,
                    read,
                    ScopeView {
                        period,
                        now,
                        own,
                        unmodelled,
                        grid,
                        spans,
                        integers,
                    },
                    addresses,
                    cx,
                );
            },
        );
    }

    /// Where each deal's prints live, per distinct `(core, coin)` — the fetch's own resolver,
    /// asked once per distinct pair. A core that is not connected, or a coin its catalog does
    /// not spell, resolves to nothing and the row says so.
    pub(in crate::analytics::tuner) fn resolve_addresses(
        &self,
        deals: &[Deal],
        cx: &Context<Self>,
    ) -> HashMap<(u64, String), Option<Arc<RowAddress>>> {
        let mut resolver = super::fetch::FetchResolver::of(self.backend.read(cx));
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
    #[allow(clippy::too_many_arguments)]
    fn start_replay_stage(
        &mut self,
        req: u64,
        report_req: u64,
        after_report: bool,
        read: DealsRead,
        scope: ScopeView,
        addresses: HashMap<(u64, String), Option<Arc<RowAddress>>>,
        cx: &mut Context<Self>,
    ) {
        let model = super::model_cfg::current();
        let period = scope.period;
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
        // A refusal is of what the store held when it was said: a row whose market gained prints
        // since — the station's recording, the core's archive — is asked about again.
        let (tape_rev, refiled) =
            moon_core::market::trade_replay::trade_cache::filed_since(self.ticks.tape_rev);
        let judged = super::refiled::drop_refiled(
            judged,
            &refiled,
            moon_core::util::time::now_unix_ms_i64(),
        );
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
                    all: read.all,
                    rows,
                    without_ms: read.without_ms,
                    service: read.service,
                    untunable: read.untunable,
                    kpi: Vec::new(),
                    entry_share: (0, 0),
                    exit_share: (0, 0),
                    accuracy: Default::default(),
                    kinds,
                    now: scope.now,
                    own: scope.own,
                    unmodelled: scope.unmodelled,
                    grid: scope.grid,
                    spans: scope.spans,
                    integers: scope.integers,
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
                this.ticks.loaded_period = Some(period);
                // The published rows' refusals now stand checked against this revision: the ones
                // whose market gained prints before it were not carried.
                this.ticks.tape_rev = tape_rev;
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
        self.close_best_run(cx);
        if !matches!(self.ticks.data, crate::load_state::LoadState::Ready(_)) {
            return;
        }
        self.latest_reads.cancel(&[
            ReadLane::TicksReplay,
            ReadLane::TicksVariants,
            ReadLane::TicksSearch,
        ]);
        self.ticks.stop_search("model settings");
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
            // The table's replay is CPU and may run beside a search: in the tuner's pool, within
            // its CPU budget.
            move || {
                moon_core::db::tuner::threshold_search::install_job(|| {
                    // One threshold for the whole table, read once: every row of one load splits
                    // its window the same way.
                    let long_position_ms = moon_core::market::trade_replay::long_position_ms();
                    let mut rows = replay_batch(
                        targets,
                        &defaults,
                        model,
                        klines.as_ref(),
                        long_position_ms,
                        true,
                    );
                    log_replay(&rows);
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
                })
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
                // With the venue autoload on, the rows of THIS table the venue can still serve
                // go to the fetch without a press: the switch is the consent to spend the
                // budget, and the startup pass covers only its own horizon (30 days, every
                // core) — a wider period on the table would otherwise sit behind a button. The
                // core autoload has no such gap: its reach is the cores' archives, two days,
                // which the startup pass already walks for every core, and what closes after it
                // the close-time capture files.
                if moon_core::market::trade_replay::tape_autoload() {
                    this.ticks_fetch_missing(super::fetch::job::RowOrigin::Autoload, cx);
                }
                // The replayable set may have changed under the variant columns: rescore them.
                this.arm_ticks_variants(cx);
                cx.notify();
            },
        );
    }
}

/// Stage C's replay of a batch of rows: their held tape asked in one batch, their archived
/// lines, each core's step lag calibrated off them, then the model on every row. Shared by the
/// table's own stage C and the strategy list's readiness pass (`tuner::ready`), so the
/// "Tuning" column counts exactly the rows the card would.
///
/// Args:
///     targets: The deals to replay, each with where its prints live.
///     defaults: The strategy-field defaults of the live schema.
///     model: The model settings, one set for the whole batch.
///     klines: The kline cache the live deltas read, when the market source has one.
///     long_position_ms: One window split for the whole batch ([`ask_held`]).
///     calibrate: Whether the batch recalibrates each core's step lag and round trip. They
///         are process-global and every replay reads them, so only the card's own load
///         writes them; the readiness pass replays on whatever that load found.
///
/// Returns:
///     The replayed rows, in the order of `targets`.
pub(in crate::analytics::tuner) fn replay_batch(
    targets: Vec<(Deal, Arc<RowAddress>)>,
    defaults: &HashMap<String, f64>,
    model: ModelSettings,
    klines: Option<&KlineCache>,
    long_position_ms: i64,
    calibrate: bool,
) -> Vec<DealRow> {
    let fetched = fetch_batch(targets, long_position_ms);
    // Before any row is replayed: each core's step lag, off these rows' archives.
    if calibrate {
        calibrate_lags(&fetched, defaults, model);
    }
    replay_fetched(
        fetched,
        defaults,
        model,
        klines,
        moon_core::util::now_unix_ms_i64(),
        None,
        None,
    )
}

/// What one readiness pass reads once and shares across its rows and threads: the strategies'
/// values ([`StrategyValuesReader`]). The card's own load reads each row afresh.
pub(in crate::analytics::tuner) struct PassReads {
    pub(in crate::analytics::tuner) values: StrategyValuesReader,
}

impl PassReads {
    /// Fresh reads for one pass.
    pub(in crate::analytics::tuner) fn new() -> Self {
        Self {
            values: StrategyValuesReader::new(),
        }
    }
}

/// What a batch's replay reads before the model runs: the rows, still without their tape,
/// each one's held tape by report uid and each one's archived lines.
pub(in crate::analytics::tuner) struct Fetched {
    pub(in crate::analytics::tuner) rows: Vec<DealRow>,
    pub(in crate::analytics::tuner) tapes: HashMap<i64, HeldTape>,
    pub(in crate::analytics::tuner) traces: HashMap<i64, ArchivedLines>,
}

/// The I/O half of [`replay_batch`]: the held tape of every target asked in one batch, then
/// the archived lines of every row.
///
/// Args:
///     targets: The deals to replay, each with where its prints live.
///     long_position_ms: One window split for the whole batch ([`ask_held`]).
///
/// Returns:
///     The rows in the order of `targets`, every one `Missing` until replayed, and what was
///     read for them.
pub(in crate::analytics::tuner) fn fetch_batch(
    targets: Vec<(Deal, Arc<RowAddress>)>,
    long_position_ms: i64,
) -> Fetched {
    let tapes = held_tapes(&targets, long_position_ms);
    let rows: Vec<DealRow> = targets
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
    let traces = archived_lines(&rows);
    Fetched {
        rows,
        tapes,
        traces,
    }
}

/// Recalibrate each core's step lag and round trip off a fetched batch's archives — the
/// process-global values every later replay reads ([`replay_batch`]'s `calibrate`).
pub(in crate::analytics::tuner) fn calibrate_lags(
    fetched: &Fetched,
    defaults: &HashMap<String, f64>,
    model: ModelSettings,
) {
    super::lags::calibrate_from(&fetched.rows, &fetched.traces, defaults, model);
}

/// The CPU half of [`replay_batch`]: the model on every fetched row.
///
/// Args:
///     fetched: What [`fetch_batch`] read.
///     defaults: The strategy-field defaults of the live schema.
///     model: The model settings, one set for the whole batch.
///     klines: The kline cache the live deltas read, when the market source has one.
///     now_ms: One clock for the batch's "cannot be fetched" verdicts.
///     reads: A pass's shared reads; `None` reads every row afresh.
///     handle: A batch search's cancellation; checked before starting each row.
///
/// Returns:
///     Rows in fetch order, possibly only partly replayed when cancelled. The batch caller
///     discards them on cancellation rather than publishing partial results.
pub(in crate::analytics::tuner) fn replay_fetched(
    fetched: Fetched,
    defaults: &HashMap<String, f64>,
    model: ModelSettings,
    klines: Option<&KlineCache>,
    now_ms: i64,
    reads: Option<&PassReads>,
    handle: Option<&moon_core::db::tuner::threshold_search::SearchHandle>,
) -> Vec<DealRow> {
    let Fetched {
        mut rows,
        mut tapes,
        mut traces,
    } = fetched;
    for row in &mut rows {
        if handle.is_some_and(|handle| handle.is_cancelled()) {
            break;
        }
        let lines = traces.remove(&row.deal.report_uid).unwrap_or_default();
        let tape = tapes.remove(&row.deal.report_uid);
        let answered = tape.is_some();
        replay_row_with(row, defaults, model, lines, tape, klines, reads);
        // Said at load, not after a walk: a row the venue cannot serve is not
        // "missing" — it would only ever come back refused. The fetch job's own
        // path (`replay_row` after a walk) is NOT given this: its retries and its
        // continuation read `Missing`, and its refusal is the walk's own word.
        // Nor is a row whose held query went unanswered: the tile store was not
        // read for it, so "cannot be fetched" would be said of a store never asked.
        if answered
            && row.tape == TapeStatus::Missing
            && let Some(address) = row.address.as_ref()
        {
            row.tape = unservable_status(address, &row.deal, now_ms).unwrap_or(TapeStatus::Missing);
        }
    }
    rows
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
        spans: moon_core::db::tuner::ticks::tail::focus_with_tail(
            &window,
            moon_core::db::tuner::ticks::tail::record_tail_ms(
                super::tail::current_s(),
                margin_ms(),
            ),
        ),
        reply,
    });
    Some((rx, window))
}

/// The grid's "now" column: every selected strategy's current value per field, folded to
/// one value or "varies" — and those values per target. A target on a known core that the deals'
/// strategies already read (`own`, from [`own_values`]) is not read again.
fn now_values(
    targets: &[(i64, Option<u64>)],
    keys: &[String],
    own: &OwnValues,
) -> (HashMap<String, NowValue>, Vec<SelectedValues>) {
    let mut seen: HashMap<String, Vec<Option<String>>> = HashMap::new();
    let mut read = Vec::with_capacity(targets.len());
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
        read.push(((sid, core), values));
    }
    let now = seen
        .into_iter()
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
        .collect();
    (now, read)
}

/// A selected strategy's current values, by `(strategy_id, core)`.
type SelectedValues = ((i64, Option<u64>), Arc<HashMap<String, String>>);

/// Each number knob's automatic search span: the live strategies of the scope's `kinds`, read
/// once per state of the strategies file (`live_strategies`), under the field rules `deps`,
/// widened to the schema default and the selected strategies' values — a strategy that leaves a
/// field out holds its default. A field nothing is known of is left out.
fn range_spans(
    kinds: &[String],
    selected: &[SelectedValues],
    defaults: &HashMap<String, f64>,
    deps: &FieldDeps,
) -> HashMap<&'static str, FieldSpan> {
    let numbers: Vec<&'static str> = TICK_PARAMS
        .iter()
        .filter(|f| f.kind == ParamKind::Num)
        .map(|f| f.key)
        .collect();
    // The knobs and every field their rules read, so "in effect" is asked of real values.
    let mut keys: Vec<String> = numbers.iter().map(|k| k.to_string()).collect();
    for key in &numbers {
        for condition in deps.conditions_of(key) {
            if !keys.iter().any(|k| k.eq_ignore_ascii_case(condition)) {
                keys.push(condition.to_string());
            }
        }
    }
    let started = std::time::Instant::now();
    let live = moon_core::db::tuner::live_strategies(&keys);
    let population = Population::of(&live, defaults, deps);
    let spans: HashMap<&'static str, FieldSpan> = numbers
        .iter()
        .filter_map(|&key| {
            let default = defaults.get(&key.to_ascii_lowercase()).copied();
            let own: Vec<f64> = selected
                .iter()
                .filter_map(|(_, values)| {
                    values
                        .get(key)
                        .and_then(|text| text.trim().replace(',', ".").parse::<f64>().ok())
                        .or(default)
                })
                .collect();
            let span = field_span(&population.values(kinds, key), default, &own)?;
            Some((key, span))
        })
        .collect();
    log::info!(
        target: moon_core::diagnostics::TICKS_AXIS_TARGET,
        "[x] ticks ranges: {} span(s) over {} live strategies of kinds {kinds:?} in {} ms",
        spans.len(),
        live.len(),
        started.elapsed().as_millis()
    );
    spans
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
pub(in crate::analytics::tuner) struct ArchivedLines {
    pub(in crate::analytics::tuner) entry_points: Option<Arc<[(i64, f64)]>>,
    pub(in crate::analytics::tuner) exit_points: Option<Vec<(i64, f64)>>,
    pub(in crate::analytics::tuner) answered: bool,
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
pub(in crate::analytics::tuner) fn archived_lines(rows: &[DealRow]) -> HashMap<i64, ArchivedLines> {
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
pub(in crate::analytics::tuner) type HeldTape = (Vec<Tick>, Coverage, ReplayWindow);

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

/// Refuse windows beyond both public sources and the core archive before offering a fetch.
fn unservable_status(address: &RowAddress, deal: &Deal, now_ms: i64) -> Option<TapeStatus> {
    let window = model_window(deal, margin_ms(), long_position_ms())?;
    let (from, to) = window.focus_spans().hull()?;
    if address.venue.is_none() {
        return (!super::fetch::job::reachable_after_station(
            address.venue,
            &address.market,
            from,
            to,
            deal.close_ms,
            now_ms,
        ))
        .then_some(TapeStatus::Refused(TickStatus::OutOfRetention {
            retention_ms: moon_core::market::CORE_ARCHIVE_RETENTION_MS,
        }));
    }
    match fetchable(address.venue, &address.market, to, now_ms) {
        Fetchable::Rest | Fetchable::Archive => None,
        Fetchable::NoRoute => Some(TapeStatus::Refused(TickStatus::NoRoute)),
        Fetchable::OutOfRetention => Some(TapeStatus::Refused(TickStatus::OutOfRetention {
            retention_ms: address
                .venue
                .and_then(trade_route)
                .and_then(|route| route.retention_ms())
                .unwrap_or(ARCHIVE_DEPTH_MS),
        })),
    }
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
    replay_row_with(row, defaults, model, lines, tape, klines, None);
}

/// Call `read`, adding its time to the [`replay_timing`] slot named by `slot` in a test build;
/// just call it otherwise.
macro_rules! timed {
    ($slot:ident, $read:expr) => {{
        #[cfg(test)]
        let started = std::time::Instant::now();
        let out = ($read)();
        #[cfg(test)]
        replay_timing::$slot.add(started.elapsed());
        out
    }};
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
    reads: Option<&PassReads>,
) {
    row.ticks = None;
    row.entry_line = lines.entry_points.clone();
    row.held = None;
    row.deal.delta_track = None;
    row.deal.bars = None;
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
    let short_allowed = row.held.is_some_and(|(_, trail)| {
        trail < moon_core::db::tuner::ticks::tail::floor_ms(super::tail::current_s())
    }) && !super::tail::fillable_at(
        &address,
        row.deal.report_uid,
        window.close_ms,
        row.held.map_or(0, |(_, trail)| trail),
        chrono::Utc::now().timestamp_millis(),
        super::tail::switches(),
    );
    let required = if short_allowed {
        super::tail::through_close(&window)
    } else {
        required_spans(&window)
    };
    if ticks.is_empty() || !covered.covers(&required) {
        row.tape = TapeStatus::Missing;
        row.verdict = None;
        return;
    }
    row.tape = TapeStatus::Covered;
    // The price grid off the prints themselves, as the `real_data` bench reads it: the live
    // catalog has no tick to give — its `price_step` is the chart's `ask / 5000`, off the grid, and
    // rounding every level to it moved the lines off the core's on every row (2026-09-26: the
    // axis ✓ 65 % where the bench on the same trades read 92 %).
    row.deal.tick = infer_tick(&ticks);
    let keys = params::param_keys();
    let values = timed!(STRATEGY_VALUES, || {
        let (sid, core, at) = (
            row.deal.strategy_id,
            Some(row.deal.core_uid),
            row.deal.buy_ms,
        );
        match reads {
            Some(reads) => reads.values.values_at(sid, core, at, &keys),
            None => strategy_values_at(sid, core, at, &keys),
        }
    })
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
    // through them. The bars they were read off go with the deal for SellLevel's look-back.
    if let Some(cache) = klines {
        let history = timed!(KLINES, || {
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
        row.deal.delta_track = history.track;
        row.deal.bars = history.bars;
    }
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
        &covered,
    );
    // The core's own clock for its PriceDown steps, as the last load calibrated it.
    row.deal.step_lag_ms = super::lags::step_lag_of(row.deal.core_uid);
    // And its own replace round trip for the entry's re-places.
    row.deal.round_trip_ms = super::lags::round_trip_of(row.deal.core_uid);
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

/// One line per load on what its replay came to — the rows, those it covered, the exit share and
/// each core's step lag — to hold against the `real_data` bench over the same trades. Read off
/// the replayed rows alone: no second read of the database.
fn log_replay(rows: &[DealRow]) {
    let mut lags: Vec<(&str, f64)> = rows
        .iter()
        .map(|r| (r.deal.core_name.as_str(), r.deal.step_lag_ms))
        .collect();
    lags.sort_by(|a, b| a.0.cmp(b.0));
    lags.dedup_by(|a, b| a.0 == b.0);
    let covered = rows
        .iter()
        .filter(|r| r.tape == TapeStatus::Covered)
        .count();
    let exit = verify::share(
        rows.iter()
            .filter_map(|r| r.verdict.as_ref())
            .map(|v| v.exit),
    );
    log::info!(
        target: moon_core::diagnostics::TICKS_AXIS_TARGET,
        "[x] ticks replay: {} row(s), {covered} covered, exit ✓ {}/{} · step lag {lags:?}",
        rows.len(),
        exit.0,
        exit.1,
    );
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

/// Time spent in the replay's per-row reads, summed over every thread — for the readiness
/// bench, which tells the reads inside the model's half from the model itself. Only a test
/// build counts ([`timed`]).
#[cfg(test)]
pub(in crate::analytics::tuner) mod replay_timing {
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Calls and nanoseconds of one kind of read.
    pub(in crate::analytics::tuner) struct Slot {
        pub(in crate::analytics::tuner) calls: AtomicU64,
        pub(in crate::analytics::tuner) nanos: AtomicU64,
    }

    impl Slot {
        const fn new() -> Self {
            Self {
                calls: AtomicU64::new(0),
                nanos: AtomicU64::new(0),
            }
        }

        pub(in crate::analytics::tuner) fn add(&self, took: std::time::Duration) {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.nanos.fetch_add(
                u64::try_from(took.as_nanos()).unwrap_or(u64::MAX),
                Ordering::Relaxed,
            );
        }

        /// `(calls, seconds)` so far.
        pub(in crate::analytics::tuner) fn read(&self) -> (u64, f64) {
            (
                self.calls.load(Ordering::Relaxed),
                self.nanos.load(Ordering::Relaxed) as f64 / 1e9,
            )
        }
    }

    /// `strategy_values_at`, once per replayed row.
    pub(in crate::analytics::tuner) static STRATEGY_VALUES: Slot = Slot::new();
    /// `deltas::track_for`, the kline reads of the live deltas.
    pub(in crate::analytics::tuner) static KLINES: Slot = Slot::new();
}

#[cfg(test)]
mod tests;
