//! Conservative trade edges from event-time order rows, independent of UI/table throttling.
//!
//! Entry is `buy_order` for both directions. First-seen fills and snapshot prefixes seed silently;
//! an open requires an observed unfilled entry followed by positive executed quantity. Closure
//! requires SellDone and a filled, closed, uncancelled exit, never disappearance alone or an error.
//! MoonProto does not tag repair images or timestamp partial executions: a delayed repair can
//! still be the first observation of a transition. This is not an execution-time event stream.
//! Its queue also lacks atomic packet boundaries: a snapshot row drained before its trailing
//! marker is indistinguishable from a live update and can sound before the marker arrives.

use std::collections::{HashMap, HashSet};

use moonproto::state::OrderEvent;
use moonproto::{Event, OrderWorkerStatus};

/// Which actual trade transition was observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TradeEdge {
    /// Entry acquired a position, including its first partial fill.
    Open,
    /// Exit completed successfully, with no remaining exit quantity.
    Close,
}

/// Event identity stays attached until the UI selects the exchange's sound.
#[derive(Clone, Debug)]
pub struct TradeSound {
    /// Core-local task UID, not an exchange order ID.
    pub uid: u64,
    /// Canonical market name, preserved for diagnostics.
    pub market: String,
    /// Direction does not change which canonical leg represents entry.
    pub is_short: bool,
    /// Platform ordinal captured from the order rather than guessed from its market.
    pub platform: u8,
    /// Actual lifecycle edge.
    pub edge: TradeEdge,
}

/// Canonical facts needed by the reducer, also allowing tests without protocol diagnostics.
#[derive(Clone, Debug)]
struct Facts {
    uid: u64,
    market: String,
    short: bool,
    platform: u8,
    created: i64,
    emulator: bool,
    quantity: f64,
    remaining: f64,
    executed: f64,
    status: OrderWorkerStatus,
    exit_closed: bool,
    exit_canceled: bool,
    exit_quantity: f64,
    exit_remaining: f64,
    exit_executed: f64,
}

impl Facts {
    /// Capture wire facts before a newer snapshot can replace this row.
    fn from_order(order: &moonproto::state::Order) -> Self {
        Self {
            uid: order.uid,
            market: order.market_name.clone(),
            short: order.is_short,
            platform: order.platform.stable_id(),
            created: order.buy_order.create_time().unix_millis(),
            emulator: order.emulator_mode,
            quantity: order.buy_order.quantity,
            remaining: order.buy_order.quantity_remaining,
            executed: order.buy_order.actual_q,
            status: order.status,
            exit_closed: order.sell_order.is_closed(),
            exit_canceled: order.sell_order.canceled(),
            exit_quantity: order.sell_order.quantity,
            exit_remaining: order.sell_order.quantity_remaining,
            exit_executed: order.sell_order.actual_q,
        }
    }

    /// A changed instance, route or direction must seed rather than compare unrelated legs.
    fn same_order(&self, other: &Self) -> bool {
        self.market == other.market
            && self.short == other.short
            && self.platform == other.platform
            && self.created == other.created
            && self.emulator == other.emulator
    }

    /// Reject incomplete/nonfinite legs instead of inferring a fill from a worker status.
    fn filled(&self) -> bool {
        self.quantity.is_finite()
            && self.remaining.is_finite()
            && self.executed.is_finite()
            && self.quantity > 0.0
            && self.remaining >= 0.0
            && self.remaining < self.quantity
            && self.executed > 0.0
    }

    /// Successful final execution is stricter than the protocol's terminal-status predicate.
    fn closed(&self) -> bool {
        self.status == OrderWorkerStatus::SellDone
            && self.exit_closed
            && !self.exit_canceled
            && self.exit_quantity.is_finite()
            && self.exit_quantity > 0.0
            && self.exit_remaining == 0.0
            && self.exit_executed.is_finite()
            && self.exit_executed > 0.0
    }
}

/// Captured packet-order observations keep completed catalog boundaries between their rows.
#[derive(Clone)]
enum Observation {
    Snapshot,
    Row(Facts, bool),
}

/// Retained edge latches survive duplicates and quantity changes until removal or reconnect.
struct Seen {
    facts: Facts,
    held: bool,
    closed: bool,
}

/// One feed owns one reducer, so equal UIDs on different cores never collide.
#[derive(Default)]
pub(super) struct TradeSoundState {
    orders: HashMap<u64, Seen>,
    seeded: bool,
}

impl TradeSoundState {
    /// A connection generation starts with a silent baseline, including retained old rows.
    pub(super) fn reset(&mut self) {
        self.orders.clear();
        self.seeded = false;
    }

    /// Consume captured rows in packet order, preserving live suffixes after completed catalogs.
    pub(super) fn observe(
        &mut self,
        events: &[Event],
        ready: bool,
        snapshot_orders: Option<&moonproto::state::Orders>,
    ) -> Vec<TradeSound> {
        let observations: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                Event::Order(OrderEvent::Snapshot) => Some(Observation::Snapshot),
                Event::Order(OrderEvent::Created(order) | OrderEvent::Updated(order)) => {
                    Some(Observation::Row(Facts::from_order(order), false))
                }
                Event::Order(OrderEvent::Removed(order)) => {
                    Some(Observation::Row(Facts::from_order(order), true))
                }
                _ => None,
            })
            .collect();
        self.observe_sequence(
            &observations,
            ready,
            snapshot_orders.map(|orders| orders.iter().map(Facts::from_order)),
        )
    }

    /// The pinned protocol emits Snapshot AFTER its catalog/snapshot rows. Only the suffix
    /// after the final marker is unambiguously incremental. The latest read model may contain
    /// that suffix or even a later packet, so marker drains use captured facts exclusively.
    fn observe_sequence(
        &mut self,
        observations: &[Observation],
        ready: bool,
        snapshot_orders: Option<impl IntoIterator<Item = Facts>>,
    ) -> Vec<TradeSound> {
        let boundary = observations
            .iter()
            .rposition(|e| matches!(e, Observation::Snapshot));
        if let Some(boundary) = boundary {
            self.observe_batch(
                Self::rows(&observations[..=boundary]),
                ready,
                true,
                None::<[Facts; 0]>,
            );
            self.seeded |= ready;
            return self.observe_batch(
                Self::rows(&observations[boundary + 1..]),
                ready,
                false,
                None::<[Facts; 0]>,
            );
        }
        self.observe_batch(Self::rows(observations), ready, false, snapshot_orders)
    }

    /// Preserve captured row order and removal facts on either side of a catalog boundary.
    fn rows(observations: &[Observation]) -> impl Iterator<Item = (Facts, bool)> + '_ {
        observations
            .iter()
            .filter_map(|observation| match observation {
                Observation::Row(facts, removed) => Some((facts.clone(), *removed)),
                Observation::Snapshot => None,
            })
    }

    /// Process terminal event facts before reconciling a silent batch to the latest read model.
    fn observe_batch(
        &mut self,
        rows: impl IntoIterator<Item = (Facts, bool)>,
        ready: bool,
        snapshot: bool,
        snapshot_orders: Option<impl IntoIterator<Item = Facts>>,
    ) -> Vec<TradeSound> {
        let silent = !ready || !self.seeded || snapshot;
        let mut sounds = Vec::new();
        let mut saw_order = false;
        for (facts, removed) in rows {
            saw_order = true;
            let uid = facts.uid;
            if let Some(sound) = self.observe_facts(facts, silent) {
                if !removed || sound.edge == TradeEdge::Close {
                    sounds.push(sound);
                }
            }
            if removed {
                self.orders.remove(&uid);
            }
        }
        let mut reconciled = false;
        if silent {
            if let Some(snapshot_orders) = snapshot_orders {
                self.reconcile(snapshot_orders);
                reconciled = true;
            }
        }
        self.seeded |= ready && (saw_order || reconciled);
        sounds
    }

    /// Spend snapshot edges silently and forget absent identities without inventing closures.
    fn reconcile(&mut self, rows: impl IntoIterator<Item = Facts>) {
        let mut retained = HashSet::new();
        for facts in rows {
            retained.insert(facts.uid);
            self.observe_facts(facts, true);
        }
        self.orders.retain(|uid, _| retained.contains(uid));
    }

    /// Advance latches even for silent observations so quiet/replay cannot defer an old edge.
    fn observe_facts(&mut self, facts: Facts, silent: bool) -> Option<TradeSound> {
        let held = facts.filled();
        let closed = facts.closed();
        let previous = self
            .orders
            .remove(&facts.uid)
            .filter(|s| s.facts.same_order(&facts));
        let mut edge = None;
        let mut was_held = false;
        let mut was_closed = false;
        if let Some(previous) = previous {
            was_held = previous.held;
            was_closed = previous.closed;
            if !silent && !facts.emulator && !was_closed {
                if was_held && closed {
                    edge = Some(TradeEdge::Close);
                } else if !was_held
                    && held
                    && !facts.status.is_terminal()
                    && previous.facts.quantity == facts.quantity
                    && previous.facts.remaining == facts.quantity
                    && previous.facts.executed == 0.0
                {
                    edge = Some(TradeEdge::Open);
                }
            }
        }
        let sound = edge.map(|edge| TradeSound {
            uid: facts.uid,
            market: facts.market.clone(),
            is_short: facts.short,
            platform: facts.platform,
            edge,
        });
        self.orders.insert(
            facts.uid,
            Seen {
                facts,
                held: was_held || held,
                closed: was_closed || closed,
            },
        );
        sound
    }
}

#[cfg(test)]
mod tests;
