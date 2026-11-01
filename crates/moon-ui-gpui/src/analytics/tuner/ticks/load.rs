//! Background loads of the "Entry/Exit" axis: scope, row publication, then tape replay.
//!
//! Stage A reads the scope's deals, the grid's "now" values and the search ranges' automatic
//! spans (over the live strategies, `params::range`) off the database. The same worker resolves
//! where each deal's prints live and checks whether the current publication can be retained.
//! Its completion builds the grid from compact scope inputs; stage B prepares rows in the
//! background and publishes them without their missing tape. Stage C asks for held tape in
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
use super::signature::{ReloadKey, SamplePolicy};
use super::state::{DealRow, NowValue, OwnValues, RowAddress, SettledMiss, TapeStatus, TicksData};
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
    infer_tick, model_window, params, prepare_deal, quotes, required_spans, verify,
};
use moon_core::db::tuner::{StrategyValuesReader, strategy_current_values, strategy_values_at};
use moon_core::feed::report_traces::ArchivedLineKind;
use moon_core::feed::strategy_deps::FieldDeps;
use moon_core::feed::types::Tick;
use moon_core::market::kline_cache::KlineCache;
use moon_core::market::trade_replay::venue_caps::{Fetchable, fetchable, trade_route};
use moon_core::market::trade_replay::{
    Coverage, ReplayWindow, TickQuery, TickStatus, long_position_ms, margin_ms, query_held,
};

/// How long a held query waits for the worker's answer. The coordinator answers held queries
/// off no venue call, so the wait is normally milliseconds; the ceiling is for a disk that
/// stalls — past it the rows still unanswered fold as missing, and the log says how many.
const HELD_ANSWER_WAIT: Duration = Duration::from_secs(240);

/// What stage A brings back: the deals, the grid's "now" values, the strategies' own values, the
/// exit fields outside the model they switch on, the selected strategies' values, the rules all
/// of it was read under — the grid chooses its rows by them — the search ranges' spans, the
/// reload key's scope signature and where each deal's prints live.
type StageA = (
    Result<DealsRead, ReadFail>,
    HashMap<String, NowValue>,
    OwnValues,
    Arc<UnmodelledMap>,
    Vec<SelectedValues>,
    FieldDeps,
    Arc<HashMap<&'static str, FieldSpan>>,
    Option<ReloadKey>,
    HashMap<(u64, String), Option<Arc<RowAddress>>>,
    Vec<Deal>,
    Option<super::signature::SkipCheck>,
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

    /// Reload card data, stopping a search the user left and keeping one a report refresh joins.
    fn reload_ticks_inner(
        &mut self,
        after_report: bool,
        show_overlay: bool,
        cx: &mut Context<Self>,
    ) {
        let show_overlay = self.axis_read_overlay(show_overlay);
        if !after_report {
            self.report_busy_retries.reset();
        }
        // Whether the rows in force were fully judged when this reload was asked: only then can
        // a reload that finds nothing changed keep them (`signature.rs`). A tape stage the
        // cancel below stops leaves rows it never judged.
        let settled_before = !self.ticks.tape_reading
            && matches!(self.ticks.data, crate::load_state::LoadState::Ready(_));
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
        let mut resolver = self.address_resolver(cx);
        let skip_snapshot = (after_report && settled_before).then(|| {
            (
                self.ticks.data.data().cloned(),
                self.ticks.reload_key.clone(),
                Arc::clone(&self.ticks.settled),
                self.ticks.rows_rev,
                self.ticks.reload_until_ms,
                self.ticks.judged_under,
            )
        });
        self.spawn_latest_db(
            &[ReadLane::Ticks],
            show_overlay,
            cx,
            move || {
                let deals = moon_core::db::tuner::ticks::read_deals(&q);
                if let Ok(read) = &deals {
                    super::fetch::station::note_axis(&read.axis);
                }
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
                let scope = deals
                    .as_ref()
                    .ok()
                    .map(|read| {
                        scope_signature(read, &now, &own, &spans, &selected, &defaults, &deps)
                    });
                let addresses = deals
                    .as_ref()
                    .map(|read| super::fetch::resolve_in(&mut resolver, &read.deals))
                    .unwrap_or_default();
                let grid_deals = deals.as_ref().map(|read| super::sections::grid_deals(&read.deals)).unwrap_or_default();
                let key = scope.map(|scope| ReloadKey {
                    scope, period, model: super::model_cfg::current(),
                    sample: SamplePolicy::current(),
                    refusal_revision: super::fetch::station::refusal_revision(),
                    tape_rev: moon_core::market::trade_replay::trade_cache::filed_since(u64::MAX).0,
                    addresses: super::signature::addresses_signature(&addresses),
                });
                let skip = skip_snapshot.and_then(|(data, stored, settled, rows_rev, until, judged)| {
                    let data = data?;
                    let key = key.as_ref()?;
                    let admission = super::tail::Admission::current();
                    if judged != Some(key.model) || !super::signature::can_skip(
                        stored.as_ref(), key, &data.rows, admission.now_ms, until,
                        |row| missing_still_holds(row, &settled, &admission),
                    ) { return None; }
                    let deadline = super::super::ready::verdict_deadline(
                        &data.rows, admission.now_ms, key.sample.min_tail_s,
                    ).unwrap_or(i64::MAX).min(until);
                    Some(super::signature::SkipCheck {
                        key: key.clone(), rows_rev, checked_at_ms: admission.now_ms, until_ms: deadline,
                    })
                });
                (
                    deals,
                    now,
                    own,
                    Arc::new(unmodelled),
                    selected,
                    deps,
                    spans,
                    key,
                    addresses,
                    grid_deals,
                    skip,
                )
            },
            move |this,
                  (deals, now, own, unmodelled, selected, deps, spans, key, addresses, grid_deals, skip): StageA,
                  cx| {
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
                    &grid_deals,
                    own.iter()
                        .map(|(&(sid, core), values)| ((sid, Some(core)), values.as_ref()))
                        .chain(selected.iter().map(|(key, values)| (*key, values.as_ref()))),
                    &deps,
                )
                .into();
                let key = key.map(|key| ReloadKey {
                    model: super::model_cfg::current(),
                    sample: SamplePolicy::current(),
                    refusal_revision: super::fetch::station::refusal_revision(),
                    tape_rev: moon_core::market::trade_replay::trade_cache::filed_since(u64::MAX).0,
                    ..key
                });
                // A report that moved without changing a closed deal of the scope — an open
                // position's update, every few seconds live — reads the same rows: the ones in
                // force stay, and stages B and C are not run again.
                if after_report
                    && settled_before
                    && let Some(key) = &key
                    && this.ticks.judged_under == Some(key.model)
                    && this.ticks.reload_key.as_ref() == Some(key)
                    && skip.as_ref().is_some_and(|skip| skip.holds(
                        key, this.ticks.rows_rev, moon_core::util::time::now_unix_ms_i64(),
                    ))
                {
                    this.ticks.resume();
                    this.ticks.dirty = report_result_is_stale(
                        report_req,
                        this.current_report_generation(),
                        false,
                    );
                    // The reload's cancel may have stopped a variant scoring in flight.
                    this.arm_ticks_variants(cx);
                    this.settle_report_refresh_retry(false, cx);
                    cx.notify();
                    return;
                }
                this.ticks.capture_formula_identity(projected_formula_identity(
                    own.values().map(|values| values.as_ref())
                        .chain(selected.iter().map(|(_, values)| values.as_ref())),
                ));
                this.start_replay_stage(
                    req,
                    report_req,
                    after_report,
                    key,
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

    /// The fetch's own resolver, taken on the UI thread to resolve deals off it
    /// ([`super::fetch::resolve_in`]).
    pub(in crate::analytics::tuner) fn address_resolver(
        &self,
        cx: &Context<Self>,
    ) -> super::fetch::FetchResolver {
        super::fetch::FetchResolver::of(self.backend.read(cx))
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
        key: Option<ReloadKey>,
        read: DealsRead,
        scope: ScopeView,
        addresses: HashMap<(u64, String), Option<Arc<RowAddress>>>,
        cx: &mut Context<Self>,
    ) {
        let model = super::model_cfg::current();
        let period = scope.period;
        // Retain the existing immutable publication in constant time; row copies belong to
        // the background stage below, alongside the construction of its replacement.
        let previous = (self.ticks.judged_under == Some(model))
            .then(|| self.ticks.data.data().cloned())
            .flatten();
        let settled = Arc::clone(&self.ticks.settled);
        let sample = SamplePolicy::current();
        // A refusal is of what the store held when it was said: a row whose market gained prints
        // since — the station's recording, the core's archive — is asked about again.
        let (tape_rev, refiled) =
            moon_core::market::trade_replay::trade_cache::filed_since(self.ticks.tape_rev);
        // The rows the last stage left missing off a settled read, whose verdict was made under
        // what is in force now and whose market gained no prints since: asking again would
        // read the same.
        self.spawn_latest_db(
            &[ReadLane::TicksReplay],
            false,
            cx,
            move || {
                let judged = previous
                    .as_ref()
                    .map(|data| {
                        data.rows
                            .iter()
                            .filter(|row| {
                                matches!(row.tape, TapeStatus::Covered | TapeStatus::Refused(_))
                            })
                            .map(|row| (row.deal.report_uid, row.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                let judged = super::refiled::drop_refiled(
                    judged,
                    &refiled,
                    moon_core::util::time::now_unix_ms_i64(),
                );
                let misses: HashMap<i64, (DealRow, SettledMiss)> = {
                    // Each revision a miss was judged under, with the markets whose refusals changed
                    // since — a refusal elsewhere leaves the miss standing.
                    let mut refusals: HashMap<u64, HashSet<(String, String)>> = HashMap::new();
                    previous
                        .as_ref()
                        .map(|d| {
                            d.rows
                                .iter()
                                .filter(|r| r.tape == TapeStatus::Missing)
                                .filter_map(|r| {
                                    let miss = settled.get(&r.deal.report_uid)?;
                                    let refused = refusals
                                        .entry(miss.refusal_revision)
                                        .or_insert_with(|| {
                                            super::fetch::station::refusals_since(
                                                miss.refusal_revision,
                                            )
                                            .1
                                        });
                                    let fresh = miss.model == model
                                        && miss.sample == sample
                                        && !refused.contains(&(
                                            miss.market.0.clone(),
                                            miss.market.1.clone(),
                                        ))
                                        && miss.held == r.held
                                        && !refiled.contains(&(
                                            miss.market.0.clone(),
                                            miss.market.1.clone(),
                                        ));
                                    fresh.then(|| (r.deal.report_uid, (r.clone(), miss.clone())))
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                };
                // Release the publication as soon as the carried rows own their copies.
                drop(previous);
                drop(settled);
                let admission = super::tail::Admission::current();
                let mut carried: HashSet<i64> = HashSet::new();
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
                            .filter(|before| after_report || !before.lost_tape(&admission))
                            .or_else(|| {
                                let (before, miss) = misses.get(&deal.report_uid)?;
                                let address = address.as_deref()?;
                                miss_holds(before, miss, &deal, address, &admission)
                                    .then_some(before)
                            })
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
                            carried.insert(row.deal.report_uid);
                            row.take_replay(before);
                        }
                        row
                    })
                    .collect();
                let mut kinds: Vec<String> = rows.iter().map(|r| r.deal.kind.clone()).collect();
                kinds.sort();
                kinds.dedup();
                let row_index = Arc::new(
                    rows.iter()
                        .enumerate()
                        .map(|(index, row)| (row.deal.report_uid, index))
                        .collect(),
                );
                let mut data = TicksData {
                    all: read.all,
                    rows: rows.into(),
                    row_index,
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
                super::fetch::station::retain_rows(data.rows.iter().map(|row| {
                    (
                        row.deal.core_uid,
                        row.deal.report_uid,
                        row.address.as_deref().map(|address| {
                            (address.exchange_key.as_str(), address.market.as_str())
                        }),
                    )
                }));
                this.ticks.publish(Ok(data), false);
                this.ticks.loaded_period = Some(period);
                // The rows just published are judged once their tape stage folds.
                this.ticks.reload_key = None;
                Arc::make_mut(&mut this.ticks.settled).retain(|uid, _| carried.contains(uid));
                // The published rows' refusals now stand checked against this revision: the ones
                // whose market gained prints before it were not carried.
                this.ticks.tape_rev = tape_rev;
                // The fetch job runs on across reloads and windows: a window that finds a batch
                // running listens to it from here on.
                if super::fetch::job::progress().active {
                    this.attach_fetch_listener(cx);
                }
                let carried = (!carried.is_empty()).then(|| (model, Arc::new(carried)));
                this.start_tape_stage(req, carried, key, cx);
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
        self.ticks.stop_search("model settings");
        // Every verdict of the table is of the old settings now: none may be carried by a
        // reload until this stage has judged them all again.
        self.ticks.judged_under = None;
        self.ticks.reload_key = None;
        let req = self.ticks.seq;
        self.start_tape_stage(req, None, None, cx);
    }

    /// Stage C: the held tape of every published row, asked from the worker in one batch, the
    /// archived entry lines, and the model on the parameters as of the buy — folded into the
    /// rows when all of it is in.
    ///
    /// Args:
    ///     req: The load generation this stage belongs to.
    ///     carried: The settings the rows stage B carried were judged under, and those rows'
    ///         `reportuid`s, when it carried any: those rows are not read again. `None` reads
    ///         and replays every row.
    ///     key: What the load's rows were read from; in force once this stage has judged them.
    fn start_tape_stage(
        &mut self,
        req: u64,
        carried: Option<(ModelSettings, Arc<HashSet<i64>>)>,
        key: Option<ReloadKey>,
        cx: &mut Context<Self>,
    ) {
        // One set of model settings for the whole stage, read once: every row of a table is
        // judged by the same rules.
        let model = super::model_cfg::current();
        // Rows carried under settings changed since stage B read them are not carried: the
        // stage reads and replays them too, so the table folds under `model` alone.
        let mut carried = carried.filter(|(m, _)| *m == model).map(|(_, uids)| uids);
        let mut key = key;
        // This window's ticks are put in force before any row is judged (`entry_tape.rs`). A
        // change makes every carried row's verdict one of the other requirement — as does one a
        // tick put in force after this load chose its carried rows.
        if super::entry_tape::replace(self.ticks.entry_tape_wanted()) {
            self.ticks.entry_tape_moved = true;
        }
        // Then coverage may shrink, not only grow: a row held whole for its exit alone is
        // missing once its entry is owed again (`update_rows`). Cleared only by a stage that
        // folds: one superseded before its fold leaves the next to do it.
        let rejudged = self.ticks.entry_tape_moved;
        if rejudged {
            carried = None;
            key = key.map(|key| ReloadKey {
                sample: SamplePolicy::current(),
                ..key
            });
        }
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
                carried
                    .as_ref()
                    .is_none_or(|uids| !uids.contains(&r.deal.report_uid))
            })
            .filter_map(|r| Some((r.deal.clone(), r.address.clone()?)))
            .collect();
        if targets.is_empty() {
            let line = tape_line(data);
            log_changed(&mut self.ticks.logged.1, line);
            self.ticks.entry_tape_moved = false;
            self.ticks.tape_reading = false;
            self.ticks.judged_under = Some(model);
            self.ticks
                .settle_key(key, moon_core::util::time::now_unix_ms_i64());
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
        // What a settled miss is judged under, read before the stage: a change during it makes
        // the record disagree with the next reload, which then asks again.
        let sample = SamplePolicy::current();
        let refusal_revision = super::fetch::station::refusal_revision();
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
                    // One read of the strategies for the whole stage, shared by every row.
                    let reads = PassReads::new();
                    let (mut rows, mut misses) = replay_table(
                        targets,
                        &defaults,
                        model,
                        klines.as_ref(),
                        long_position_ms,
                        &reads,
                    );
                    let line = replay_line(&rows);
                    // Rows the job answered while the batch was read: read again, each behind
                    // whatever walk is running. A row the job answers during THIS loop is kept
                    // covered by the fold (`update_rows`), not re-read once more.
                    let late = super::fetch::job::finished_after(reading_since);
                    if !late.is_empty() {
                        // Read again behind the job's answer: no settled read speaks for them.
                        misses.retain(|uid, _| !late.contains(uid));
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
                                Some(&reads),
                            );
                        }
                    }
                    (rows, misses, line)
                })
            },
            move |this, (rows, misses, line), cx| {
                if this.ticks.seq != req || this.ticks.tape_seq != tape_req {
                    return;
                }
                this.ticks.tape_reading = false;
                this.ticks.judged_under = Some(model);
                // This stage's rows stand under the requirement in force.
                this.ticks.entry_tape_moved = false;
                log_changed(&mut this.ticks.logged.0, line);
                // Every row this stage judged has its settled miss replaced by this stage's word.
                let settled = Arc::make_mut(&mut this.ticks.settled);
                for row in &rows {
                    settled.remove(&row.deal.report_uid);
                }
                for (uid, (held, short_allowed, market)) in misses {
                    settled.insert(
                        uid,
                        SettledMiss {
                            held,
                            short_allowed,
                            model,
                            sample,
                            refusal_revision,
                            market,
                        },
                    );
                }
                this.ticks.update_rows(rows, rejudged);
                this.ticks
                    .settle_key(key, moon_core::util::time::now_unix_ms_i64());
                if let Some(data) = this.ticks.data.data() {
                    let line = tape_line(data);
                    log_changed(&mut this.ticks.logged.1, line);
                }
                // The row the fetch job is out for says so again after the fold.
                this.mark_fetch_in_flight();
                // The rows still missing are what the user is looking at: a running batch
                // takes them next.
                this.ticks_prioritize_visible();
                // Fill this table's missing tape even beyond the startup pass's thirty-day scope.
                this.ticks_fetch_missing(cx);

                // The replayable set may have changed under the variant columns: rescore them.
                this.arm_ticks_variants(cx);
                cx.notify();
            },
        );
    }
}

/// Stage C's replay of the table: each core's step lag calibrated off the rows' archived
/// lines first — they read no tape — then the held tape asked and replayed in chunks bounded by
/// the prints they hold ([`replay_bounded`]), so the table never holds every row's raw tape at
/// once. The rows come out as one unbounded batch would give them.
///
/// Args:
///     targets: The deals to replay, each with where its prints live.
///     defaults: The strategy-field defaults of the live schema.
///     model: The model settings, one set for the whole batch.
///     klines: The kline cache the live deltas read, when the market source has one.
///     long_position_ms: One window split for the whole batch ([`ask_held`]).
///     reads: The stage's shared reads of the strategies.
///
/// Returns:
///     The replayed rows, in the order of `targets`, and the settled misses among them
///     ([`Misses`]).
fn replay_table(
    targets: Vec<(Deal, Arc<RowAddress>)>,
    defaults: &HashMap<String, f64>,
    model: ModelSettings,
    klines: Option<&KlineCache>,
    long_position_ms: i64,
    reads: &PassReads,
) -> (Vec<DealRow>, Misses) {
    let rows = blank_rows(targets);
    let traces = archived_lines(&rows);
    // Before any row is replayed: each core's step lag, off these rows' archives.
    super::lags::calibrate_from(&rows, &traces, defaults, model);
    replay_chunked(
        rows,
        traces,
        TapeBudget::app(),
        |chunk| held_tapes(chunk, long_position_ms),
        defaults,
        model,
        klines,
        moon_core::util::now_unix_ms_i64(),
        Some(reads),
    )
}

/// [`replay_bounded`] with the model on every chunk, noting the settled misses.
#[allow(clippy::too_many_arguments)]
fn replay_chunked(
    rows: Vec<DealRow>,
    traces: HashMap<i64, ArchivedLines>,
    budget: TapeBudget,
    held: impl FnMut(&[DealRow]) -> (HashMap<i64, HeldTape>, HashSet<i64>),
    defaults: &HashMap<String, f64>,
    model: ModelSettings,
    klines: Option<&KlineCache>,
    now_ms: i64,
    reads: Option<&PassReads>,
) -> (Vec<DealRow>, Misses) {
    let mut misses = Misses::new();
    let rows = replay_bounded(rows, traces, budget, held, |fetched| {
        let settled = fetched.settled.clone();
        replay_fetched_with(
            fetched,
            defaults,
            model,
            klines,
            now_ms,
            reads,
            &mut |row, short_allowed| {
                if row.tape == TapeStatus::Missing
                    && settled.contains(&row.deal.report_uid)
                    && let Some(address) = row.address.as_ref()
                {
                    misses.insert(
                        row.deal.report_uid,
                        (
                            row.held,
                            short_allowed.unwrap_or(false),
                            (
                                address.exchange_key.clone(),
                                address.market.clone(),
                                address.venue,
                            ),
                        ),
                    );
                }
            },
        )
    });
    (rows, misses)
}

/// The rows a stage left missing off a settled held read, by `reportuid`: the held coverage
/// read, whether the short tail was admitted, and the `(exchange key, market, venue)` asked.
type Misses = HashMap<
    i64,
    (
        Option<(i64, i64)>,
        bool,
        (String, String, Option<moon_core::venue::Venue>),
    ),
>;

/// How far ahead stage C asks for held tape before it replays: at most `step` rows per ask, and
/// a replay as soon as the prints read reach `max_bytes`.
#[derive(Clone, Copy, Debug)]
pub(in crate::analytics::tuner) struct TapeBudget {
    pub(in crate::analytics::tuner) step: usize,
    pub(in crate::analytics::tuner) max_bytes: usize,
}

impl TapeBudget {
    /// The app's: the readiness pass's in-flight tape budget, asked a few hundred rows at a time
    /// so the worker's reader pool stays busy.
    fn app() -> Self {
        Self {
            step: 256,
            max_bytes: super::super::ready::READY_INFLIGHT_BYTES,
        }
    }
}

/// Ask and replay `rows` in chunks: each chunk's held tape is asked through `held`, and what
/// was read is replayed through `replay` once its raw prints reach the budget — so at most the
/// budget plus one chunk's prints are held at a time. Rows come out in their order.
///
/// Args:
///     rows: The rows to replay, each with its address, still without tape.
///     traces: Every row's archived lines, by `reportuid`.
///     budget: The chunk cut.
///     held: Asks the held tape of a chunk; returns the answers and the settled `reportuid`s.
///     replay: Replays a fetched chunk.
pub(in crate::analytics::tuner) fn replay_bounded(
    rows: Vec<DealRow>,
    mut traces: HashMap<i64, ArchivedLines>,
    budget: TapeBudget,
    mut held: impl FnMut(&[DealRow]) -> (HashMap<i64, HeldTape>, HashSet<i64>),
    mut replay: impl FnMut(Fetched) -> Vec<DealRow>,
) -> Vec<DealRow> {
    let mut out = Vec::with_capacity(rows.len());
    let mut pending = Fetched::default();
    let mut bytes = 0usize;
    let mut rest = rows.into_iter();
    loop {
        let chunk: Vec<DealRow> = rest.by_ref().take(budget.step.max(1)).collect();
        let done = chunk.is_empty();
        if !done {
            let (tapes, settled) = held(&chunk);
            bytes += tapes
                .values()
                .map(|(ticks, _, _)| ticks.len() * std::mem::size_of::<Tick>())
                .sum::<usize>();
            for row in &chunk {
                if let Some(lines) = traces.remove(&row.deal.report_uid) {
                    pending.traces.insert(row.deal.report_uid, lines);
                }
            }
            pending.rows.extend(chunk);
            pending.tapes.extend(tapes);
            pending.settled.extend(settled);
        }
        if (done || bytes >= budget.max_bytes) && !pending.rows.is_empty() {
            out.extend(replay(std::mem::take(&mut pending)));
            bytes = 0;
        }
        if done {
            return out;
        }
    }
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
/// each one's held tape by report uid and each one's archived lines, and the rows whose held
/// read read every stretch it needed (`TickAnswer::settled`).
#[derive(Default)]
pub(in crate::analytics::tuner) struct Fetched {
    pub(in crate::analytics::tuner) rows: Vec<DealRow>,
    pub(in crate::analytics::tuner) tapes: HashMap<i64, HeldTape>,
    pub(in crate::analytics::tuner) traces: HashMap<i64, ArchivedLines>,
    pub(in crate::analytics::tuner) settled: HashSet<i64>,
}

/// A batch's reads before the model: the held tape of every target asked in one batch, then
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
    let rows = blank_rows(targets);
    let (tapes, settled) = held_tapes(&rows, long_position_ms);
    let traces = archived_lines(&rows);
    Fetched {
        rows,
        tapes,
        traces,
        settled,
    }
}

/// The targets as rows still without their tape, each `Missing` until replayed.
fn blank_rows(targets: Vec<(Deal, Arc<RowAddress>)>) -> Vec<DealRow> {
    targets
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
        .collect()
}

/// Recalibrate each core's step lag and round trip off a fetched batch's archives — the
/// process-global values every later replay reads. Only the card's own load writes them; the
/// readiness pass replays on whatever that load found.
#[cfg(test)]
pub(in crate::analytics::tuner) fn calibrate_lags(
    fetched: &Fetched,
    defaults: &HashMap<String, f64>,
    model: ModelSettings,
) {
    super::lags::calibrate_from(&fetched.rows, &fetched.traces, defaults, model);
}

/// A batch's CPU half: the model on every fetched row.
///
/// Args:
///     fetched: What [`fetch_batch`] read.
///     defaults: The strategy-field defaults of the live schema.
///     model: The model settings, one set for the whole batch.
///     klines: The kline cache the live deltas read, when the market source has one.
///     now_ms: One clock for the batch's "cannot be fetched" verdicts.
///     reads: A pass's shared reads; `None` reads every row afresh.
///
/// Returns:
///     Rows in fetch order.
pub(in crate::analytics::tuner) fn replay_fetched(
    fetched: Fetched,
    defaults: &HashMap<String, f64>,
    model: ModelSettings,
    klines: Option<&KlineCache>,
    now_ms: i64,
    reads: Option<&PassReads>,
) -> Vec<DealRow> {
    replay_fetched_with(
        fetched,
        defaults,
        model,
        klines,
        now_ms,
        reads,
        &mut |_, _| {},
    )
}

/// [`replay_fetched`], handing every replayed row to `on_row` with whether its short tail was
/// admitted (`None` without a tape to judge).
#[allow(clippy::too_many_arguments)]
fn replay_fetched_with(
    fetched: Fetched,
    defaults: &HashMap<String, f64>,
    model: ModelSettings,
    klines: Option<&KlineCache>,
    now_ms: i64,
    reads: Option<&PassReads>,
    on_row: &mut dyn FnMut(&DealRow, Option<bool>),
) -> Vec<DealRow> {
    let Fetched {
        mut rows,
        mut tapes,
        mut traces,
        settled: _,
    } = fetched;
    for row in &mut rows {
        let lines = traces.remove(&row.deal.report_uid).unwrap_or_default();
        let tape = tapes.remove(&row.deal.report_uid);
        let answered = tape.is_some();
        let short_allowed = replay_row_with(row, defaults, model, lines, tape, klines, reads);
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
        on_row(row, short_allowed);
    }
    rows
}

/// The held tape of every addressed row, asked from the worker in one batch and collected in
/// order, and the rows whose answer read every stretch it needed. A query the worker did not
/// answer in time, or one cancelled by a scope change, is absent.
fn held_tapes(rows: &[DealRow], long_position_ms: i64) -> (HashMap<i64, HeldTape>, HashSet<i64>) {
    let deadline = std::time::Instant::now() + HELD_ANSWER_WAIT;
    let asked: Vec<(
        i64,
        mpsc::Receiver<moon_core::market::trade_replay::TickAnswer>,
        ReplayWindow,
    )> = rows
        .iter()
        .filter_map(|row| {
            let (rx, window) = ask_held(row.address.as_deref()?, &row.deal, long_position_ms)?;
            Some((row.deal.report_uid, rx, window))
        })
        .collect();
    let asked_n = asked.len();
    let mut out = HashMap::with_capacity(asked_n);
    let mut settled = HashSet::with_capacity(asked_n);
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
        if answer.settled {
            settled.insert(uid);
        }
        out.insert(uid, (answer.ticks, answer.covered, window));
    }
    if unanswered > 0 {
        log::info!(
            target: moon_core::diagnostics::TICKS_AXIS_TARGET,
            "[x] ticks load: {unanswered} of {asked_n} held queries unanswered within {} s, folded as missing",
            HELD_ANSWER_WAIT.as_secs()
        );
    }
    (out, settled)
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
pub(super) fn now_values(
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
        let slots = params::formula_slot_values(&values);
        for key in keys {
            seen.entry(key.clone()).or_default().push(
                slots
                    .get(key)
                    .map(|value| format!("{value:?}"))
                    .or_else(|| values.get(key).cloned()),
            );
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

/// Bind projected slots to one shared comparison structure, refusing missing or mixed formulas.
fn projected_formula_identity<'a>(
    values: impl IntoIterator<Item = &'a HashMap<String, String>>,
) -> Option<params::FormulaIdentity> {
    let mut identities = values.into_iter().map(params::formula_identity);
    let first = identities.next()??;
    identities
        .all(|identity| identity.as_ref() == Some(&first))
        .then_some(first)
}

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
    keys.push("SellByCustomEMA".into());
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
                    params::formula_slot_values(values)
                        .get(key)
                        .copied()
                        .or_else(|| {
                            values
                                .get(key)
                                .and_then(|text| text.trim().replace(',', ".").parse::<f64>().ok())
                                .or(default)
                        })
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
                .unwrap_or(i64::MAX),
        })),
    }
}

/// Run the model on one row, from what the worker holds — asked here, one query; a row
/// without an address is left as it is. `long_position_ms` is the row's own threshold — see
/// [`ask_held`]; `reads` the caller's shared reads, `None` reads the strategies afresh.
#[allow(clippy::too_many_arguments)]
pub(super) fn replay_row(
    row: &mut DealRow,
    defaults: &HashMap<String, f64>,
    model: ModelSettings,
    lines: ArchivedLines,
    long_position_ms: i64,
    klines: Option<&KlineCache>,
    reads: Option<&PassReads>,
) {
    let tape = row
        .address
        .as_ref()
        .and_then(|address| held_tape(address, &row.deal, long_position_ms));
    replay_row_with(row, defaults, model, lines, tape, klines, reads);
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
///
/// Returns whether the row's short tail was admitted, once there was a tape to judge.
pub(super) fn replay_row_with(
    row: &mut DealRow,
    defaults: &HashMap<String, f64>,
    model: ModelSettings,
    lines: ArchivedLines,
    tape: Option<HeldTape>,
    klines: Option<&KlineCache>,
    reads: Option<&PassReads>,
) -> Option<bool> {
    row.ticks = None;
    row.entry_line = lines.entry_points.clone();
    row.held = None;
    row.deal.delta_track = None;
    row.deal.quotes = None;
    row.deal.bars = None;
    let address = row.address.clone()?;
    let Some((ticks, covered, window)) = tape else {
        row.tape = TapeStatus::Missing;
        row.verdict = None;
        return None;
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
    let short_allowed = short_allowed(
        &address,
        row.deal.report_uid,
        window.close_ms,
        row.held,
        super::tail::current_s(),
        chrono::Utc::now().timestamp_millis(),
    );
    let (exit_held, entry_held) = held_sides(&covered, &window, row.deal.buy_ms, short_allowed);
    if ticks.is_empty() || !exit_held {
        row.tape = TapeStatus::Missing;
        row.verdict = None;
        return Some(short_allowed);
    }
    let entry_short = !entry_held;
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
    let entry = params::entry_params_for(&row.deal.kind, &sv, model);
    let exit = params::exit_params(&sv, model);
    // An entry model walks the order from its creation; a tape that starts later cannot judge it.
    // Owed only while the ticks admit an entry field of the deal's kind (`entry_tape.rs`): then
    // the row waits for
    // that stretch like any missing tape. Otherwise the fact's entry is taken and the exit is
    // judged on the tape it has.
    let entry_unjudgeable = entry_short && !matches!(entry, EntryParams::Fact);
    if entry_unjudgeable && super::entry_tape::required_for(&row.deal.kind) {
        row.tape = TapeStatus::Missing;
        row.verdict = None;
        return Some(short_allowed);
    }
    row.tape = TapeStatus::Covered;
    // The price grid off the prints themselves, as the `real_data` bench reads it: the live
    // catalog has no tick to give — its `price_step` is the chart's `ask / 5000`, off the grid, and
    // rounding every level to it moved the lines off the core's on every row (2026-09-26: the
    // axis ✓ 65 % where the bench on the same trades read 92 %).
    row.deal.tick = infer_tick(&ticks);
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
        // The recorded market list is the terminal's own store like the kline cache, read with it:
        // the mark delta along the window and the book the stops read.
        row.deal.quotes = timed!(KLINES, || {
            quotes::read_for(&address.exchange_key, &address.market, &row.deal)
        });
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
        if entry_unjudgeable {
            &EntryParams::Fact
        } else {
            &entry
        },
        &exit,
        lines.entry_points.as_deref(),
        lines.exit_points.as_deref(),
    ));
    row.ticks = Some(tape::PackedTape::pack(ticks));
    Some(short_allowed)
}

/// What a deal's held coverage reaches, as `(exit, entry)`: the exit model starts at the fill,
/// so it owes the window opened there; the entry model walks the order from its creation, where
/// the window itself opens. The station and the close-time capture both record from the fill, so
/// an order that waited longer than the margin less the run-up holds the first and not the
/// second.
///
/// Args:
///     covered: The worker's coverage of the window.
///     window: The deal's window (`model_window`), opened at the order's creation when known.
///     buy_ms: The fill.
///     short_allowed: An unfillable short tail is admitted: only the stretch through the close
///         is owed (`tail::through_close`).
fn held_sides(
    covered: &Coverage,
    window: &ReplayWindow,
    buy_ms: i64,
    short_allowed: bool,
) -> (bool, bool) {
    let required = |window: &ReplayWindow| {
        if short_allowed {
            super::tail::through_close(window)
        } else {
            required_spans(window)
        }
    };
    let from_fill = ReplayWindow {
        open_ms: window.open_ms.max(buy_ms),
        ..*window
    };
    (
        covered.covers(&required(&from_fill)),
        covered.covers(&required(window)),
    )
}

/// Whether a row may be judged on the tape through its close alone: its held trail is below the
/// short-tail floor and no enabled source can still extend it (`tail::fillable_at`).
fn short_allowed(
    address: &RowAddress,
    uid: i64,
    close_ms: i64,
    held: Option<(i64, i64)>,
    min_tail_s: u32,
    now_ms: i64,
) -> bool {
    held.is_some_and(|(_, trail)| trail < moon_core::db::tuner::ticks::tail::floor_ms(min_tail_s))
        && !super::tail::fillable_at(
            address,
            uid,
            close_ms,
            held.map_or(0, |(_, trail)| trail),
            now_ms,
        )
}

/// Whether a settled miss still describes the row a reload read for its deal: the same trade
/// asked of the same market, a verdict the clock would not move — "cannot be fetched" not yet
/// said, the short tail admitted or not as it was — so a replay would read and say the same.
pub(super) fn miss_holds(
    before: &DealRow,
    miss: &SettledMiss,
    deal: &Deal,
    address: &RowAddress,
    admission: &super::tail::Admission,
) -> bool {
    let Some(window) = model_window(deal, miss.sample.margin_ms, miss.sample.long_position_ms)
    else {
        return false;
    };
    carryable(&before.deal, deal)
        && before.tape == TapeStatus::Missing
        && (
            address.exchange_key.as_str(),
            address.market.as_str(),
            address.venue,
        ) == (
            miss.market.0.as_str(),
            miss.market.1.as_str(),
            miss.market.2,
        )
        && unservable_status(address, deal, admission.now_ms).is_none()
        && short_allowed(
            address,
            deal.report_uid,
            window.close_ms,
            miss.held,
            miss.sample.min_tail_s,
            admission.now_ms,
        ) == miss.short_allowed
}

/// Whether a missing row a settled load left would still be judged missing now: the clock has
/// not carried it past a retention edge or into an admitted short tail. A row without an
/// address waits on no clock; one without a settled miss cannot be vouched for.
fn missing_still_holds(
    row: &DealRow,
    settled: &HashMap<i64, SettledMiss>,
    admission: &super::tail::Admission,
) -> bool {
    let Some(address) = row.address.as_deref() else {
        return true;
    };
    settled
        .get(&row.deal.report_uid)
        .is_some_and(|miss| miss_holds(row, miss, &row.deal, address, admission))
}

/// The scope's signature for a reload's key: its closed deals, the counts and baseline beside
/// them, the strategies' values and spans stage B publishes with the rows, and what the
/// unmodelled warning, the grid and the replay are made from beside them — the selected
/// strategies' values, the schema defaults and the field rules read this load.
fn scope_signature(
    read: &DealsRead,
    now: &HashMap<String, NowValue>,
    own: &OwnValues,
    spans: &HashMap<&'static str, FieldSpan>,
    selected: &[SelectedValues],
    defaults: &HashMap<String, f64>,
    deps: &FieldDeps,
) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write_u64(super::signature::signature_of(&read.deals));
    super::signature::hash_debug(
        &mut hasher,
        &(&read.all, read.without_ms, read.service, read.untunable),
    );
    super::signature::hash_sorted(&mut hasher, now.iter());
    super::signature::hash_sorted_maps(&mut hasher, own.iter());
    super::signature::hash_sorted(&mut hasher, spans.iter());
    super::signature::hash_sorted_maps(
        &mut hasher,
        selected.iter().map(|(key, values)| (key, values)),
    );
    super::signature::hash_sorted(&mut hasher, defaults.iter());
    hasher.write(deps.fingerprint().as_bytes());
    hasher.finish()
}

/// Log `line` unless it is the one `last` holds: a pass that comes to the same says nothing.
fn log_changed(last: &mut Option<String>, line: String) {
    if last.as_deref() != Some(line.as_str()) {
        log::info!(target: moon_core::diagnostics::TICKS_AXIS_TARGET, "{line}");
        *last = Some(line);
    }
}

/// The line on what a replay came to — the rows, those it covered, the exit share and each
/// core's step lag — to hold against the `real_data` bench over the same trades. Read off the
/// replayed rows alone: no second read of the database.
fn replay_line(rows: &[DealRow]) -> String {
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
    format!(
        "[x] ticks replay: {} row(s), {covered} covered, exit ✓ {}/{} · step lag {lags:?}",
        rows.len(),
        exit.0,
        exit.1,
    )
}

/// The line on what the table holds of its tape — the numbers the memory budget is judged by,
/// read from the log instead of guessed.
fn tape_line(data: &TicksData) -> String {
    let b = data.tape_budget();
    format!(
        "[x] ticks tape: {} row(s), {} fit, {} with tape in memory, {} let go by the cap · {} print(s), {:.1} MiB of {:.0} MiB",
        b.rows,
        b.fit,
        b.replayable,
        b.dropped,
        b.prints,
        b.bytes as f64 / 1_048_576.0,
        super::state::MAX_RETAINED_BYTES as f64 / 1_048_576.0
    )
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
