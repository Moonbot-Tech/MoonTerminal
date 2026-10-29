//! Incremental notification transactions: changed ledger leaves and ordered queue payloads.

use super::{CardWait, ChatNotify, NotifyFile, NotifyLedger, Outbox, Pending};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Absolute assignments make a checkpointed transaction harmless when replayed again.
type Changes<K, V> = Vec<(K, Option<V>)>;
/// A core can disappear, or have individual remembered trades assigned or removed.
type Trades<V> = Changes<u64, Changes<i64, V>>;

/// One chat's small settings/state plus changes to its potentially large trade maps.
#[derive(Debug, Deserialize, Serialize)]
struct ChatUpdate {
    state: ChatNotify,
    seen: Trades<i64>,
    held: Trades<i64>,
    cards: Trades<CardWait>,
    chart_seen: Trades<i64>,
    chart_held: Trades<i64>,
}

/// Changed queue payloads and their exact resulting order. Legacy duplicate ids use a full array.
#[derive(Debug, Deserialize, Serialize)]
enum QueueUpdate {
    Ordered {
        order: Vec<u64>,
        changed: Vec<Pending>,
    },
    Legacy(Box<Outbox>),
}

/// A durable owner transaction sharing the delivery journal's contiguous sequence.
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Update {
    chats: Changes<i64, ChatUpdate>,
    queue: Option<QueueUpdate>,
    next_id: u64,
}

/// Compare sorted maps without serializing or cloning their unchanged values.
fn changes<K: Ord + Clone, V: PartialEq + Clone>(
    old: &BTreeMap<K, V>,
    new: &BTreeMap<K, V>,
) -> Changes<K, V> {
    old.keys()
        .filter(|key| !new.contains_key(*key))
        .map(|key| (key.clone(), None))
        .chain(
            new.iter()
                .filter(|(key, value)| old.get(*key) != Some(*value))
                .map(|(key, value)| (key.clone(), Some(value.clone()))),
        )
        .collect()
}

/// Diff only changed cores, then only changed trade leaves within each core.
fn trades<V: PartialEq + Clone>(
    old: &BTreeMap<u64, BTreeMap<i64, V>>,
    new: &BTreeMap<u64, BTreeMap<i64, V>>,
) -> Trades<V> {
    old.keys()
        .filter(|key| !new.contains_key(*key))
        .map(|key| (*key, None))
        .chain(
            new.iter()
                .filter(|(key, value)| old.get(*key) != Some(*value))
                .map(|(key, value)| {
                    (
                        *key,
                        Some(changes(old.get(key).unwrap_or(&BTreeMap::new()), value)),
                    )
                }),
        )
        .collect()
}

/// Restore a leaf map without disturbing unrelated entries.
fn apply_changes<K: Ord, V>(target: &mut BTreeMap<K, V>, changes: Changes<K, V>) {
    for (key, value) in changes {
        if let Some(value) = value {
            target.insert(key, value);
        } else {
            target.remove(&key);
        }
    }
}

/// Restore nested trade maps, including deliberately retained empty cores.
fn apply_trades<V>(target: &mut BTreeMap<u64, BTreeMap<i64, V>>, changes: Trades<V>) {
    for (key, value) in changes {
        if let Some(value) = value {
            apply_changes(target.entry(key).or_default(), value);
        } else {
            target.remove(&key);
        }
    }
}

impl ChatUpdate {
    /// Capture settings and scalar state without copying any remembered trade population.
    fn between(old: &ChatNotify, new: &ChatNotify) -> Self {
        let mut ledger = NotifyLedger {
            trades_enabled_utc: new.ledger.trades_enabled_utc,
            down_announced: new.ledger.down_announced.clone(),
            reports: new.ledger.reports,
            ..Default::default()
        };
        ledger.charts.enabled_utc = new.ledger.charts.enabled_utc;
        Self {
            state: ChatNotify {
                report_view: new.report_view,
                settings: new.settings.clone(),
                ledger,
                revision: new.revision,
            },
            seen: trades(&old.ledger.seen, &new.ledger.seen),
            held: trades(&old.ledger.held, &new.ledger.held),
            cards: trades(&old.ledger.cards, &new.ledger.cards),
            chart_seen: trades(&old.ledger.charts.seen, &new.ledger.charts.seen),
            chart_held: trades(&old.ledger.charts.held, &new.ledger.charts.held),
        }
    }

    /// Replace small state while retaining and patching the old trade maps.
    fn apply(self, target: &mut ChatNotify) {
        let mut state = self.state;
        std::mem::swap(&mut state.ledger.seen, &mut target.ledger.seen);
        std::mem::swap(&mut state.ledger.held, &mut target.ledger.held);
        std::mem::swap(&mut state.ledger.cards, &mut target.ledger.cards);
        std::mem::swap(
            &mut state.ledger.charts.seen,
            &mut target.ledger.charts.seen,
        );
        std::mem::swap(
            &mut state.ledger.charts.held,
            &mut target.ledger.charts.held,
        );
        apply_trades(&mut state.ledger.seen, self.seen);
        apply_trades(&mut state.ledger.held, self.held);
        apply_trades(&mut state.ledger.cards, self.cards);
        apply_trades(&mut state.ledger.charts.seen, self.chart_seen);
        apply_trades(&mut state.ledger.charts.held, self.chart_held);
        *target = state;
    }
}

impl Update {
    /// Capture a clone-edit transaction, preserving arbitrary queue replacement and file order.
    pub(crate) fn between(old: &NotifyFile, new: &NotifyFile) -> Self {
        let chats = old
            .chats
            .keys()
            .filter(|key| !new.chats.contains_key(*key))
            .map(|key| (*key, None))
            .chain(
                new.chats
                    .iter()
                    .filter(|(key, value)| old.chats.get(*key) != Some(*value))
                    .map(|(key, value)| {
                        (
                            *key,
                            Some(ChatUpdate::between(
                                old.chats.get(key).unwrap_or(&ChatNotify::default()),
                                value,
                            )),
                        )
                    }),
            )
            .collect();
        let queue = (old.outbox != new.outbox).then(|| {
            let prior: HashMap<_, _> = old.outbox.iter().map(|row| (row.id, row)).collect();
            let next: HashMap<_, _> = new.outbox.iter().map(|row| (row.id, row)).collect();
            if prior.len() != old.outbox.len() || next.len() != new.outbox.len() {
                QueueUpdate::Legacy(Box::new(new.outbox.clone()))
            } else {
                QueueUpdate::Ordered {
                    order: new.outbox.iter().map(|row| row.id).collect(),
                    changed: new
                        .outbox
                        .iter()
                        .filter(|row| prior.get(&row.id).copied() != Some(*row))
                        .cloned()
                        .collect(),
                }
            }
        });
        Self {
            chats,
            queue,
            next_id: new.next_id,
        }
    }

    /// Apply a validated transaction before its sequence cursor is advanced by the journal.
    pub(crate) fn apply(self, file: &mut NotifyFile) -> anyhow::Result<()> {
        if let Some(queue) = self.queue {
            file.outbox = match queue {
                QueueUpdate::Legacy(rows) => *rows,
                QueueUpdate::Ordered { order, changed } => {
                    let mut rows: HashMap<_, _> = std::mem::take(&mut file.outbox)
                        .into_iter()
                        .map(|row| (row.id, row))
                        .collect();
                    rows.extend(changed.into_iter().map(|row| (row.id, row)));
                    let ordered = order
                        .into_iter()
                        .map(|id| {
                            rows.remove(&id).ok_or_else(|| {
                                anyhow::anyhow!("notification transaction missing row {id}")
                            })
                        })
                        .collect::<anyhow::Result<Vec<_>>>()?;
                    ordered.into()
                }
            };
        }
        for (chat, update) in self.chats {
            if let Some(update) = update {
                update.apply(file.chats.entry(chat).or_default());
            } else {
                file.chats.remove(&chat);
            }
        }
        file.next_id = self.next_id;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
