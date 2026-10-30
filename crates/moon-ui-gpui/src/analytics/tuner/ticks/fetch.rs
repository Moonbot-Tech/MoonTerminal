//! "Fetch the tape" — the view's side of the batch that [`job`] runs.
//!
//! The view resolves what a request needs while it still has the live source (the core's
//! exchange identity, the contract terms), hands the rows to the process-wide job, and listens:
//! a row that lands is folded into the table by id, whatever scope the table shows by then. The
//! job outlives the window; a reopened window attaches a fresh listener and reads the same
//! progress. The resolution itself ([`FetchResolver`]) needs no view — the startup autoload
//! ([`autoload`]) runs it off the coordination tick with no Analytics window open.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

use gpui::*;

use super::super::super::AnalyticsView;
use super::state::{RowAddress, RowEdit, TapeStatus};
use crate::Backend;
use moon_core::db::tuner::ticks::{Deal, model_window};
use moon_core::market::MarketDataSource;
use moon_core::market::trade_replay::{long_position_ms, margin_ms};

pub(crate) mod autoload;
pub(in crate::analytics::tuner) mod job;
pub(in crate::analytics::tuner) mod station;

/// Whether the station is still filling candidates before venue work can begin.
pub(super) fn station_active() -> bool {
    station::active()
}

/// Count recoverable rows and station candidates not already refused in this generation.
pub(super) fn station_load_candidate(
    row: &super::state::DealRow,
    admission: &super::tail::Admission,
) -> bool {
    if !row.station_candidate_with(admission) {
        return false;
    }
    let Some(address) = &row.address else {
        return false;
    };
    let from = row.held.map_or(row.deal.buy_ms, |(_, trail)| {
        row.deal
            .close_ms
            .saturating_add(trail.max(0))
            .saturating_add(1)
    });
    let to = row
        .deal
        .close_ms
        .saturating_add(moon_core::db::tuner::ticks::tail::record_tail_ms(
            super::tail::current_s(),
            margin_ms(),
        ));
    job::reachable_after_station(
        address.venue,
        &address.market,
        from,
        to,
        row.deal.close_ms,
        admission.now_ms,
    ) || admission.candidate(row.deal.core_uid, row.deal.report_uid)
}

/// How long one blocking receive holds a background-pool thread before checking back.
const LISTEN_SLICE: Duration = Duration::from_secs(2);

/// What a deal needs from the live session to become a request: the market source for the
/// exchange identity, the catalog and the contract terms, and each core's quote setting for
/// spelling the coin into a market. A snapshot of handles, not of state — the source is a
/// shared handle, so a core that connects after this was built resolves on the next ask.
pub(in crate::analytics::tuner) struct FetchResolver {
    source: MarketDataSource,
    /// `core_uid` to the core's quote (`ServerConfig::market`).
    quotes: HashMap<u64, String>,
    /// Per distinct `(core, coin)`, what it resolved to — a table of one coin's trades asks
    /// the catalog once.
    addresses: HashMap<(u64, String), Option<Arc<RowAddress>>>,
    /// Per core, BTC's market on its exchange — the BTC deltas are read off it.
    btc_markets: HashMap<u64, Option<String>>,
}

/// Where each deal's prints live, per distinct `(core, coin)` — `resolver` asked once per
/// distinct pair, off the UI thread. A core that is not connected, or a coin its catalog does
/// not spell, resolves to nothing and the row says so.
pub(in crate::analytics::tuner) fn resolve_in(
    resolver: &mut FetchResolver,
    deals: &[Deal],
) -> HashMap<(u64, String), Option<Arc<RowAddress>>> {
    deals
        .iter()
        .map(|deal| ((deal.core_uid, deal.coin.clone()), resolver.address(deal)))
        .collect()
}

impl FetchResolver {
    pub(in crate::analytics::tuner) fn of(backend: &Backend) -> Self {
        Self {
            source: backend.session.market_source(),
            quotes: backend
                .config
                .servers
                .iter()
                .map(|s| (s.id, s.market.clone()))
                .collect(),
            addresses: HashMap::new(),
            btc_markets: HashMap::new(),
        }
    }

    /// Where a deal's prints live: the core's exchange key and the catalog-verified market. A
    /// core that is not connected, or a coin its catalog does not spell, resolves to nothing
    /// and the row says so ([`TapeStatus::NoAddress`]).
    pub(in crate::analytics::tuner) fn address(&mut self, deal: &Deal) -> Option<Arc<RowAddress>> {
        let key = (deal.core_uid, deal.coin.clone());
        if let Some(address) = self.addresses.get(&key) {
            return address.clone();
        }
        let quote = self
            .quotes
            .get(&deal.core_uid)
            .map(String::as_str)
            .unwrap_or_default();
        let source = &self.source;
        let btc_market = self
            .btc_markets
            .entry(deal.core_uid)
            .or_insert_with(|| source.resolve_market(deal.core_uid, quote, "BTC"))
            .clone();
        let address = self
            .source
            .replay_address(deal.core_uid)
            .ok()
            .and_then(|address| {
                let market = self
                    .source
                    .resolve_market(deal.core_uid, quote, &deal.coin)?;
                Some(Arc::new(RowAddress {
                    core_uid: deal.core_uid,
                    venue: address.venue,
                    exchange_key: address.exchange_key,
                    market,
                    btc_market,
                }))
            });
        self.addresses.insert(key, address.clone());
        address
    }

    /// The request for one addressed deal — the model's window (`model_window`: from the entry
    /// order's creation where the report stamps it) and the same addressing a trade window
    /// resolves: the live source for the exchange identity, the core's contract terms for how
    /// the prints are valued. `None` when the deal's stamps describe no window, or the core went
    /// away between the address and now.
    pub(in crate::analytics::tuner) fn queued_row(
        &self,
        deal: Deal,
        address: Arc<RowAddress>,
    ) -> Option<job::QueuedRow> {
        let window = model_window(&deal, margin_ms(), long_position_ms())?;
        let replay_address = self.source.replay_address(address.core_uid).ok()?;
        let tick_value = self
            .source
            .replay_tick_value(address.core_uid, &address.market);
        Some(job::QueuedRow::new(
            deal,
            address,
            replay_address,
            tick_value,
            window,
        ))
    }
}

/// Strategy-field defaults off the first core schema — what the model fills a field the
/// strategy leaves at default with, and what the tuner's filter hides unconfigured chips by.
/// Lowercase field names to values: numbers as themselves, switches as 1/0, and zero for a
/// field the schema sends no default for ([`schema_default`]).
pub(in crate::analytics::tuner) fn strategy_field_defaults(
    backend: &Backend,
) -> HashMap<String, f64> {
    let store = backend.session.store();
    let mut defaults = HashMap::new();
    for (_, core) in store.cores() {
        let Some(schema) = core.schema.as_ref() else {
            continue;
        };
        for kind in &schema.kinds {
            for section in &kind.sections {
                for field in &section.fields {
                    if let Some(value) = schema_default(&field.type_name, field.default.as_deref())
                    {
                        defaults
                            .entry(field.name.to_ascii_lowercase())
                            .or_insert(value);
                    }
                }
            }
        }
        break;
    }
    defaults
}

/// One schema field's default as the table keeps it: a number as itself, a switch as 1 or 0.
///
/// moonproto sends a default only when it is not zero (`FLAG_DEFAULT_NZ` in its
/// `strategy_schema.rs`), so a number or switch field without one defaults to ZERO — a value,
/// not "no data" (`feed::strategies::zero_for_type` reads it the same way). Read as no default,
/// a field every strategy leaves out had no value at all: the search turned `UseSecondStop` on
/// and left `PriceToSwitch2Stop` unwritten, and the model scored it at its own fallback. A
/// switch's default arrives spelled `Yes`/`No` (`fmt_field`), which no number parse reads.
///
/// Args:
///     type_name: The schema type (`SchemaField::type_name`).
///     default: The formatted default, `None` when the schema sends none.
///
/// Returns:
///     The default, or `None` for a text field or a type the terminal does not know.
fn schema_default(type_name: &str, default: Option<&str>) -> Option<f64> {
    let numeric = matches!(
        type_name,
        "Int32" | "Int64" | "Double" | "Single" | "Byte" | "Word" | "UInt32" | "UInt64"
    );
    match default {
        None => (numeric || type_name == "Bool").then_some(0.0),
        Some(text) if type_name == "Bool" => {
            moon_core::feed::strategy_deps::as_bool(text).map(|on| if on { 1.0 } else { 0.0 })
        }
        Some(text) if numeric => text
            .trim()
            .trim_end_matches('%')
            .replace(',', ".")
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite()),
        Some(_) => None,
    }
}

impl AnalyticsView {
    /// Re-read tape after filing or refusal changes so newly admitted tails regain their tape.
    /// A refusal change alone keeps the judged rows: the reload asks again only about the
    /// missing rows on the markets whose refusals changed (`station::refusals_since`).
    pub(super) fn refresh_station_tape(&mut self, cx: &mut Context<Self>) {
        let generation = station::filed_generation();
        let refusal_revision = station::refusal_revision();
        let filed = generation != self.ticks.station_generation;
        if filed || refusal_revision != self.ticks.station_refusal_revision {
            self.ticks.station_generation = generation;
            self.ticks.station_refusal_revision = refusal_revision;
            if filed {
                self.ticks.judged_under = None;
            }
            self.reload_ticks(cx);
        }
    }

    /// Queue every fetchable row of the table with the job, oldest first: a fresh batch when
    /// none runs, or added to the running one — the startup autoload's, or a previous window's
    /// — which already knows some of them and takes the rest.
    /// Automatic asks honor publication deadlines and stop when autoload is disabled.
    pub(in crate::analytics::tuner) fn ticks_fetch_missing(&mut self, cx: &mut Context<Self>) {
        // Until the tape stage has folded, every row reads "missing": queuing them would ask the
        // venue for tape the tiles already hold.
        if self.ticks.tape_reading {
            return;
        }
        if station::active() {
            return;
        }
        let Some(data) = self.ticks.data.data() else {
            return;
        };
        let backend = self.backend.read(cx);
        let resolver = FetchResolver::of(backend);
        let now_ms = moon_core::util::time::now_unix_ms_i64();
        let admission = super::tail::Admission::at(super::tail::effective_s(), now_ms);
        let station_first = crate::backend::station::known_target().is_some();
        let mut rows: Vec<job::QueuedRow> = data
            .rows
            .iter()
            .filter(|row| {
                if station_first {
                    row.station_candidate_with(&admission)
                        && station_load_candidate(row, &admission)
                } else {
                    row.fetchable_with(&admission)
                }
            })
            .filter(|row| row.autoload_ready_at(now_ms))
            .filter_map(|row| resolver.queued_row(row.deal.clone(), row.address.clone()?))
            .collect();
        // Oldest first, so the ones nearest the venues' retention edge go before it moves; the
        // job pops from the end.
        rows.reverse();
        if rows.is_empty() {
            return;
        }
        let wanted: HashSet<i64> = rows.iter().map(|r| r.deal.report_uid).collect();
        let defaults = strategy_field_defaults(backend);
        if station_first {
            let rx = station::prefill_then_enqueue(rows, defaults);
            self.attach_fetch_listener(cx);
            cx.spawn(async move |this, cx| {
                let executor = cx.update(|cx| cx.background_executor().clone());
                let mut rx = rx;
                loop {
                    let (returned_rx, received) = executor
                        .spawn(async move {
                            let received = rx.recv_timeout(LISTEN_SLICE);
                            (rx, received)
                        })
                        .await;
                    rx = returned_rx;
                    match received {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if this.update(cx, |_, _| ()).is_err() {
                                return;
                            }
                        }
                    }
                }
                let _ = this.update(cx, |this, cx| {
                    this.refresh_station_tape(cx);
                    if job::progress().active {
                        this.attach_fetch_listener(cx);
                    }
                    job::prioritize(&wanted);
                    cx.notify();
                });
            })
            .detach();
        } else {
            if job::enqueue(rows, defaults) > 0 {
                self.attach_fetch_listener(cx);
            }
            job::prioritize(&wanted);
        }
        cx.notify();
    }

    /// Prioritize missing rows and recoverable short tails in a running batch: the visible
    /// rows go before the rest of their venues' queues. A no-op while no batch runs.
    pub(in crate::analytics::tuner) fn ticks_prioritize_visible(&self) {
        if !job::progress().active {
            return;
        }
        let Some(data) = self.ticks.data.data() else {
            return;
        };
        let wanted: HashSet<i64> = data
            .fetchable()
            .chain(data.rows.iter().filter(|r| r.tape == TapeStatus::Fetching))
            .map(|r| r.deal.report_uid)
            .collect();
        let moved = job::prioritize(&wanted);
        if moved > 0 {
            log::info!(
                target: moon_core::diagnostics::TICKS_AXIS_TARGET,
                "[x] ticks fetch: {moved} row(s) of the open table marked to go first"
            );
        }
    }

    /// Listen to the job from this view, once per batch: what lands is folded into the table by
    /// id. The task ends with the batch, or with the view; a reopened window, or the next batch,
    /// attaches its own.
    pub(in crate::analytics::tuner) fn attach_fetch_listener(&mut self, cx: &mut Context<Self>) {
        if self.ticks.fetch_listening.load(Ordering::Relaxed) {
            return;
        }
        let listening = self.ticks.fetch_listening.clone();
        listening.store(true, Ordering::Relaxed);
        let rx: mpsc::Receiver<job::JobEvent> = job::attach();
        self.ticks.fetch_task = Some(cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            // The receiver travels into the background task and back out each iteration
            // instead of living behind a lock across the await, so the blocking `recv` never
            // holds anything this foreground task still needs between messages. The wait is
            // bounded: a batch asleep on a venue's backoff sends nothing for minutes, and a
            // window closed meanwhile must give the pool its thread back, not hold it until
            // the next event.
            let mut rx = rx;
            loop {
                let (returned_rx, received) = executor
                    .spawn(async move {
                        let received = rx.recv_timeout(LISTEN_SLICE);
                        (rx, received)
                    })
                    .await;
                rx = returned_rx;
                let mut events = Vec::new();
                match received {
                    Ok(event) => events.push(event),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
                // Everything else already queued goes in the same hop: the last row's answer
                // lands right before the batch goes idle, and a check on "idle" must not come
                // before it has been read.
                while let Ok(event) = rx.try_recv() {
                    events.push(event);
                }
                // A notify on every hop, events or not: the caption names the markets in
                // flight, and a walk that takes a minute changes nothing else the view could
                // hear.
                let applied = cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        this.apply_fetch_events(events, cx);
                        cx.notify();
                    })
                });
                // The view is gone; the job goes on without a listener.
                if applied.is_err() {
                    break;
                }
                // The batch is over and the channel was drained: nothing more will come until
                // the next start, which attaches afresh.
                if !job::progress().active && !station::active() {
                    break;
                }
            }
            listening.store(false, Ordering::Relaxed);
        }));
    }

    /// Fold one hop of job events into the table, in order, with one recount for the lot.
    fn apply_fetch_events(&mut self, events: Vec<job::JobEvent>, cx: &mut Context<Self>) {
        // A start still queued when the batch was stopped marks nothing: no answer would follow
        // to unmark it.
        let active = job::progress().active;
        let mut answered = false;
        let mut station_filled = false;
        let mut edits = Vec::new();
        for event in events {
            match event {
                job::JobEvent::StationFilled => station_filled = true,
                job::JobEvent::Started(uid) if active => {
                    edits.push((uid, RowEdit::MarkFetching));
                }
                job::JobEvent::Released(uid) => {
                    edits.push((uid, RowEdit::UnmarkFetching));
                }
                job::JobEvent::Rows(answers) => {
                    answered = true;
                    edits.extend(
                        answers.into_iter().map(|answer| {
                            (answer.deal.report_uid, RowEdit::Replay(Box::new(answer)))
                        }),
                    );
                }
                job::JobEvent::Started(_) | job::JobEvent::Progress => {}
            }
        }
        self.ticks.edit_rows(edits);
        if station_filled {
            self.refresh_station_tape(cx);
        }
        // Rows joined the replayable set: the variant columns are due a rescore — once.
        if answered {
            self.arm_ticks_variants(cx);
        }
    }

    /// Mark the rows the job is out for, after the table was rebuilt.
    pub(in crate::analytics::tuner) fn mark_fetch_in_flight(&mut self) {
        self.ticks.edit_rows(
            job::progress()
                .in_flight
                .into_iter()
                .flat_map(|(uids, _)| uids)
                .map(|uid| (uid, RowEdit::MarkFetching)),
        );
    }
}

#[cfg(test)]
mod tests;
