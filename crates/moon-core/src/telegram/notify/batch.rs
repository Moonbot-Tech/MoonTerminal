//! Bounded notification drafts: selected trade leaves, small chat state and newly queued rows.

use std::collections::BTreeMap;

use super::trades::{ClosedTrade, floor_from, read_from_utc};
use super::updates::{QueueUpdate, SeenPrune, Update};
use super::{CardKey, ChatNotify, NotifyFile, NotifyLedger};

/// A caller describes its candidate trades before editing; untouched history and queued cards
/// never enter this draft. The result is a typed journal patch, applied by the store in place.
pub struct NotificationBatch {
    before: NotifyFile,
    /// Only selected chats and trade leaves, plus the new notifications this batch creates.
    pub file: NotifyFile,
    prunes: Vec<SeenPrune>,
}

impl NotificationBatch {
    /// Select new decisions and unresolved waits beside the pure decision kernel. Already-seen
    /// history stays borrowed; waiting cards remain candidates even after visibility is revoked.
    pub fn trade_candidates(
        entry: &ChatNotify,
        visible: &[u64],
        trades: &[ClosedTrade],
        now_utc: i64,
    ) -> Vec<ClosedTrade> {
        let trades_from = entry
            .settings
            .trades
            .on
            .then(|| read_from_utc(&entry.ledger, now_utc))
            .flatten();
        let charts_from = entry
            .settings
            .charts
            .on
            .then(|| floor_from(entry.ledger.charts.enabled_utc, now_utc))
            .flatten();
        trades
            .iter()
            .filter(|trade| {
                if entry
                    .ledger
                    .cards
                    .get(&trade.core)
                    .is_some_and(|rows| rows.contains_key(&trade.rec_id))
                {
                    return true;
                }
                let trades = trades_from.is_some()
                    && (!entry
                        .ledger
                        .seen
                        .get(&trade.core)
                        .is_some_and(|rows| rows.contains_key(&trade.rec_id))
                        || entry
                            .ledger
                            .held
                            .get(&trade.core)
                            .is_some_and(|rows| rows.contains_key(&trade.rec_id)));
                let charts = charts_from.is_some()
                    && (!entry
                        .ledger
                        .charts
                        .seen
                        .get(&trade.core)
                        .is_some_and(|rows| rows.contains_key(&trade.rec_id))
                        || entry
                            .ledger
                            .charts
                            .held
                            .get(&trade.core)
                            .is_some_and(|rows| rows.contains_key(&trade.rec_id)));
                if !(trades || charts) || !visible.contains(&trade.core) {
                    return false;
                }
                let selected = match &entry.settings.trades.cores {
                    super::CoreScope::All => true,
                    super::CoreScope::Only(ids) => ids.contains(&trade.core),
                };
                (trades && selected && trades_from.is_some_and(|floor| trade.close_utc >= floor))
                    || (charts && charts_from.is_some_and(|floor| trade.close_utc >= floor))
            })
            .cloned()
            .collect()
    }

    /// Capture selected candidates without cloning the full document or its existing outbox.
    /// A trade decision supplies its expiry edge; down/event/report batches supply `None`.
    pub fn new(
        source: &NotifyFile,
        chats: &[i64],
        candidates: &[CardKey],
        prune_before: Option<i64>,
    ) -> Self {
        let mut file = NotifyFile {
            next_id: source.next_id,
            ..Default::default()
        };
        let mut prunes = Vec::new();
        for chat in chats {
            let Some(entry) = source.chats.get(chat) else {
                continue;
            };
            let mut state = small_state(entry);
            // Held closes and waiting cards are unresolved reader state; unlike seen history,
            // every one can change when a read stops offering it or its wait expires.
            if prune_before.is_some() {
                state.ledger.held = entry.ledger.held.clone();
                state.ledger.cards = entry.ledger.cards.clone();
                state.ledger.charts.held = entry.ledger.charts.held.clone();
            }
            for key in candidates {
                for (from, to) in [
                    (&entry.ledger.seen, &mut state.ledger.seen),
                    (&entry.ledger.charts.seen, &mut state.ledger.charts.seen),
                ] {
                    if let Some(value) = from.get(&key.core).and_then(|rows| rows.get(&key.rec_id))
                    {
                        to.entry(key.core).or_default().insert(key.rec_id, *value);
                    }
                }
            }
            if let Some(edge) = prune_before {
                for (charts, on, seen) in [
                    (false, entry.settings.trades.on, &entry.ledger.seen),
                    (true, entry.settings.charts.on, &entry.ledger.charts.seen),
                ] {
                    if seen
                        .values()
                        .any(|rows| rows.is_empty() || !on || rows.values().any(|at| *at < edge))
                    {
                        prunes.push(SeenPrune {
                            chat: *chat,
                            charts,
                            before: on.then_some(edge),
                        });
                    }
                }
            }
            file.chats.insert(*chat, state);
        }
        Self {
            before: file.clone(),
            file,
            prunes,
        }
    }

    /// Describe changed candidate leaves and append-only payloads, preserving older queue rows.
    pub fn finish(self) -> Update {
        let mut update = Update::between(&self.before, &self.file);
        for (chat, change) in &mut update.chats {
            let Some(change) = change else { continue };
            if let Some(old) = self.before.chats.get(chat) {
                expand_removed_cores(&mut change.seen, &old.ledger.seen);
                expand_removed_cores(&mut change.chart_seen, &old.ledger.charts.seen);
            }
            change.prune_empty_seen = true;
        }
        update.queue = (!self.file.outbox.is_empty())
            .then(|| QueueUpdate::Append(self.file.outbox.into_iter().collect()));
        update.prunes = self.prunes;
        update
    }
}

/// Removing a core from a candidate projection removes only its captured leaves in real history.
fn expand_removed_cores(
    changes: &mut super::updates::Trades<i64>,
    old: &BTreeMap<u64, BTreeMap<i64, i64>>,
) {
    for (core, change) in changes {
        if change.is_none() {
            *change = Some(
                old.get(core)
                    .into_iter()
                    .flat_map(|rows| rows.keys().map(|id| (*id, None)))
                    .collect(),
            );
        }
    }
}

/// Copy only state that does not grow with remembered trades or queued deliveries.
pub(super) fn small_state(entry: &ChatNotify) -> ChatNotify {
    let mut ledger = NotifyLedger {
        trades_enabled_utc: entry.ledger.trades_enabled_utc,
        down_announced: entry.ledger.down_announced.clone(),
        reports: entry.ledger.reports,
        ..Default::default()
    };
    ledger.charts.enabled_utc = entry.ledger.charts.enabled_utc;
    ChatNotify {
        report_view: entry.report_view,
        settings: entry.settings.clone(),
        ledger,
        revision: entry.revision,
    }
}

#[cfg(test)]
mod tests;
