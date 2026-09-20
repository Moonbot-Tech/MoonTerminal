//! "Fetch the tape" — the view's side of the batch that [`job`] runs.
//!
//! The view resolves what a request needs while it still has the live source (the core's
//! exchange identity, the contract terms), hands the rows to the process-wide job, and listens:
//! a row that lands is folded into the table by id, whatever scope the table shows by then. The
//! job outlives the window; a reopened window attaches a fresh listener and reads the same
//! progress for its caption.

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

use gpui::*;

use super::super::super::AnalyticsView;
use super::state::TapeStatus;
use moon_core::market::trade_replay::{margin_ms, replay_window_ms};

pub(in crate::analytics::tuner) mod job;

/// How long one blocking receive holds a background-pool thread before checking back.
const LISTEN_SLICE: Duration = Duration::from_secs(2);

impl AnalyticsView {
    /// Queue every fetchable row of the table with the job, oldest first.
    pub(in crate::analytics::tuner) fn ticks_fetch_missing(&mut self, cx: &mut Context<Self>) {
        // Until the tape stage has folded, every row reads "missing": queuing them would ask the
        // venue for tape the tiles already hold.
        if job::progress().active || self.ticks.tape_reading {
            return;
        }
        let Some(data) = self.ticks.data.data() else {
            return;
        };
        // The same addressing a trade window resolves: the live source for the exchange
        // identity, the core's contract terms for how the prints are valued.
        let backend = self.backend.read(cx);
        let source = backend.session.market_source();
        let mut rows: Vec<job::QueuedRow> = data
            .fetchable()
            .filter_map(|row| {
                let address = row.address.clone()?;
                let window = replay_window_ms(row.deal.buy_ms, row.deal.close_ms, margin_ms())?;
                let replay_address = source.replay_address(address.core_uid).ok()?;
                let terms = source.market_contract_terms(address.core_uid, &address.market);
                let tick_value = moon_core::market::trade_replay::venue_caps::tick_value(
                    replay_address.venue,
                    terms.as_ref().map(|(quote, size)| (quote.as_str(), *size)),
                );
                Some(job::QueuedRow::new(
                    row.deal.clone(),
                    address,
                    replay_address,
                    tick_value,
                    window,
                ))
            })
            .collect();
        // Oldest first, so the ones nearest the venues' retention edge go before it moves; the
        // job pops from the end.
        rows.reverse();
        let defaults = self.filter_defaults(cx);
        if job::start(rows, defaults) {
            self.attach_fetch_listener(cx);
        }
        cx.notify();
    }

    /// Abandon the batch; the request in flight is cancelled by the job.
    pub(in crate::analytics::tuner) fn ticks_fetch_stop(&mut self, cx: &mut Context<Self>) {
        // Read before the stop: the job clears its in-flight row on its own thread the moment
        // the cancelled walk returns, and no row event follows a cancellation.
        let in_flight = job::progress().in_flight;
        job::stop();
        if let Some((uid, _)) = in_flight {
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
                let applied = cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        for event in events {
                            this.apply_fetch_event(event, cx);
                        }
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
                    slot.entry_start = answer.entry_start;
                });
                // A row joined the replayable set: the variant columns are due a rescore.
                self.arm_ticks_variants(cx);
            }
            job::JobEvent::Progress => {}
        }
        cx.notify();
    }

    /// Mark the row the job is out for, after the table was rebuilt.
    pub(in crate::analytics::tuner) fn mark_fetch_in_flight(&mut self) {
        if let Some((uid, _)) = job::progress().in_flight {
            self.ticks.update_row(uid, |row| {
                if row.tape == TapeStatus::Missing {
                    row.tape = TapeStatus::Fetching;
                }
            });
        }
    }
}
