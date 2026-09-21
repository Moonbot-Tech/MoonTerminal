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
use super::state::{RowAddress, TapeStatus};
use crate::Backend;
use moon_core::db::tuner::ticks::Deal;
use moon_core::market::MarketDataSource;
use moon_core::market::trade_replay::{model_margin_ms, replay_window_ms};

pub(crate) mod autoload;
pub(in crate::analytics::tuner) mod job;

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
        let address = self
            .source
            .replay_address(deal.core_uid)
            .ok()
            .and_then(|address| {
                let market = self
                    .source
                    .resolve_market(deal.core_uid, quote, &deal.coin)?;
                let tick = self.source.price_step(deal.core_uid, &market);
                Some(Arc::new(RowAddress {
                    core_uid: deal.core_uid,
                    venue: address.venue,
                    exchange_key: address.exchange_key,
                    market,
                    tick,
                }))
            });
        self.addresses.insert(key, address.clone());
        address
    }

    /// The request for one addressed deal — the same addressing a trade window resolves: the
    /// live source for the exchange identity, the core's contract terms for how the prints are
    /// valued. `None` when the deal's stamps describe no window, or the core went away between
    /// the address and now.
    pub(in crate::analytics::tuner) fn queued_row(
        &self,
        deal: Deal,
        address: Arc<RowAddress>,
    ) -> Option<job::QueuedRow> {
        let window = replay_window_ms(deal.buy_ms, deal.close_ms, model_margin_ms())?;
        let replay_address = self.source.replay_address(address.core_uid).ok()?;
        let terms = self
            .source
            .market_contract_terms(address.core_uid, &address.market);
        let tick_value = moon_core::market::trade_replay::venue_caps::tick_value(
            replay_address.venue,
            terms.as_ref().map(|(quote, size)| (quote.as_str(), *size)),
        );
        Some(job::QueuedRow::new(
            deal,
            address,
            replay_address,
            tick_value,
            window,
        ))
    }
}

/// Numeric strategy-field defaults off the first core schema — what the model fills a field
/// the strategy leaves at default with, and what the tuner's filter hides unconfigured chips
/// by. Lowercase field names to values.
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
                    let Some(default) = field.default.as_ref() else {
                        continue;
                    };
                    if let Ok(value) = default
                        .trim()
                        .trim_end_matches('%')
                        .replace(',', ".")
                        .parse::<f64>()
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

impl AnalyticsView {
    /// Queue every fetchable row of the table with the job, oldest first: a fresh batch when
    /// none runs, or added to the running one — the startup autoload's, or a previous window's
    /// — which already knows some of them and takes the rest.
    pub(in crate::analytics::tuner) fn ticks_fetch_missing(&mut self, cx: &mut Context<Self>) {
        // Until the tape stage has folded, every row reads "missing": queuing them would ask the
        // venue for tape the tiles already hold.
        if self.ticks.tape_reading {
            return;
        }
        let Some(data) = self.ticks.data.data() else {
            return;
        };
        let backend = self.backend.read(cx);
        let resolver = FetchResolver::of(&backend);
        let mut rows: Vec<job::QueuedRow> = data
            .fetchable()
            .filter_map(|row| resolver.queued_row(row.deal.clone(), row.address.clone()?))
            .collect();
        // Oldest first, so the ones nearest the venues' retention edge go before it moves; the
        // job pops from the end.
        rows.reverse();
        let wanted: HashSet<i64> = rows.iter().map(|r| r.deal.report_uid).collect();
        let defaults = strategy_field_defaults(&backend);
        if job::enqueue(rows, defaults) > 0 {
            self.attach_fetch_listener(cx);
        }
        // The user asked for THESE rows: before whatever else their venues hold.
        job::prioritize(&wanted);
        cx.notify();
    }

    /// Mark the table's still-missing rows for a running batch — the ones the user is looking
    /// at go before the rest of their venues' queues. A no-op while no batch runs.
    pub(in crate::analytics::tuner) fn ticks_prioritize_visible(&self) {
        if !job::progress().active {
            return;
        }
        let Some(data) = self.ticks.data.data() else {
            return;
        };
        let wanted: HashSet<i64> = data
            .rows
            .iter()
            .filter(|r| matches!(r.tape, TapeStatus::Missing | TapeStatus::Fetching))
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

    /// Abandon the batch; every request in flight is cancelled by the job, and the startup
    /// autoload, if it still has rows to add, adds none.
    pub(in crate::analytics::tuner) fn ticks_fetch_stop(&mut self, cx: &mut Context<Self>) {
        // Read before the stop: the job clears its in-flight rows on their own threads the
        // moment the cancelled walks return, and no row event follows a cancellation.
        let in_flight = job::progress().in_flight;
        job::stop();
        autoload::cancel();
        for uid in in_flight.into_iter().flat_map(|(uids, _)| uids) {
            self.ticks.update_row(uid, |row| {
                if row.tape == TapeStatus::Fetching {
                    row.tape = TapeStatus::Missing;
                }
            });
        }
        cx.notify();
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
                        for event in events {
                            this.apply_fetch_event(event, cx);
                        }
                        cx.notify();
                    })
                });
                // The view is gone; the job goes on without a listener.
                if applied.is_err() {
                    break;
                }
                // The batch is over and the channel was drained: nothing more will come until
                // the next start, which attaches afresh.
                if !job::progress().active {
                    break;
                }
            }
            listening.store(false, Ordering::Relaxed);
        }));
    }

    /// Fold one job event into the table.
    fn apply_fetch_event(&mut self, event: job::JobEvent, cx: &mut Context<Self>) {
        match event {
            // A start still queued when the batch was stopped marks nothing: no answer would
            // follow to unmark it.
            job::JobEvent::Started(uid) if job::progress().active => {
                self.ticks.update_row(uid, |row| {
                    if row.tape == TapeStatus::Missing {
                        row.tape = TapeStatus::Fetching;
                    }
                });
            }
            job::JobEvent::Started(_) => {}
            job::JobEvent::Row(answer) => {
                let uid = answer.deal.report_uid;
                self.ticks.update_row(uid, |slot| {
                    slot.tape = answer.tape;
                    slot.verdict = answer.verdict;
                    slot.deal.tick = answer.deal.tick;
                    slot.ticks = answer.ticks;
                    slot.entry_line = answer.entry_line;
                });
                // A row joined the replayable set: the variant columns are due a rescore.
                self.arm_ticks_variants(cx);
            }
            job::JobEvent::Progress => {}
        }
        cx.notify();
    }

    /// Mark the rows the job is out for, after the table was rebuilt.
    pub(in crate::analytics::tuner) fn mark_fetch_in_flight(&mut self) {
        for uid in job::progress()
            .in_flight
            .into_iter()
            .flat_map(|(uids, _)| uids)
        {
            self.ticks.update_row(uid, |row| {
                if row.tape == TapeStatus::Missing {
                    row.tape = TapeStatus::Fetching;
                }
            });
        }
    }
}
