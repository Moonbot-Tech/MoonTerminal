//! Coalesces account repair requests around order changes that can affect balances.
//!
//! MoonProto normally pushes balance and transfer-asset events itself. This module keeps the
//! terminal's explicit refreshes as coalesced repair paths: the first account-relevant change after
//! an idle period requests immediately, later changes respect a cooldown, and an authoritative full
//! balance or Spot-wallet update cancels pending work.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use moonproto::state::{BalanceEvent, OrderEvent, TransferAssetsEvent};
use moonproto::{Event, ExchangeCode, ExchangeKind, OrderWorkerStatus};

/// Minimum interval between explicit full balance repair requests.
pub(super) const BALANCE_REPAIR_INTERVAL: Duration = Duration::from_secs(3);
/// Minimum interval between explicit Spot-wallet repair requests.
pub(super) const SPOT_WALLET_REPAIR_INTERVAL: Duration = Duration::from_secs(10);

/// Account-relevant fields from one retained order.
///
/// Price moves, stop edits, trace points, and corridor changes deliberately do not participate:
/// they can repaint order UI but cannot change the account balance. Floating-point execution
/// values are compared by bits so unchanged NaNs do not repeatedly queue repair work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OrderAccountStamp {
    status: OrderWorkerStatus,
    buy_actual_q: u64,
    sell_actual_q: u64,
    spot_wallet_relevant: bool,
}

impl OrderAccountStamp {
    /// Builds a stamp from the account-relevant order fields.
    fn new(
        status: OrderWorkerStatus,
        buy_actual_q: f64,
        sell_actual_q: f64,
        platform: ExchangeCode,
    ) -> Self {
        Self {
            status,
            buy_actual_q: buy_actual_q.to_bits(),
            sell_actual_q: sell_actual_q.to_bits(),
            spot_wallet_relevant: platform_uses_spot_wallet(platform),
        }
    }
}

/// Returns whether an order platform owns transferable Spot-wallet assets.
fn platform_uses_spot_wallet(platform: ExchangeCode) -> bool {
    [
        ExchangeCode::WasBittrex,
        ExchangeCode::Binance,
        ExchangeCode::Huobi,
        ExchangeCode::ByBit,
        ExchangeCode::Gate,
        ExchangeCode::BitGet,
        ExchangeCode::Hyper,
        ExchangeCode::OKX,
    ]
    .contains(&platform)
}

/// Kind of retained-order lifecycle change observed by the repair state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OrderChange {
    Created,
    Updated,
    Removed,
}

/// One coalesced repair deadline with an immediate idle request and a fixed cooldown.
#[derive(Clone, Copy, Debug)]
struct RepairDeadline {
    interval: Duration,
    due_at: Option<Instant>,
    last_attempt: Option<Instant>,
}

impl RepairDeadline {
    /// Creates an idle repair deadline with the supplied request cooldown.
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            due_at: None,
            last_attempt: None,
        }
    }

    /// Queues an immediate repair after idle or the earliest repair allowed by the cooldown.
    fn queue(&mut self, now: Instant) {
        if self.due_at.is_some() {
            return;
        }
        self.due_at = Some(
            self.last_attempt
                .map(|last_attempt| (last_attempt + self.interval).max(now))
                .unwrap_or(now),
        );
    }

    /// Records an authoritative pushed response and clears pending repair.
    fn satisfy(&mut self) {
        self.due_at = None;
    }

    /// Records an explicit request attempt and starts the request cooldown.
    fn mark_attempt(&mut self, now: Instant) {
        self.due_at = None;
        self.last_attempt = Some(now);
    }

    /// Returns whether the pending repair may run at `now`.
    fn is_due(&self, now: Instant) -> bool {
        self.due_at.is_some_and(|deadline| now >= deadline)
    }

    /// Returns the remaining wait for pending repair, preserving the original deadline.
    fn wait(&self, now: Instant) -> Option<Duration> {
        self.due_at
            .map(|deadline| deadline.saturating_duration_since(now))
    }
}

/// Tracks order-account stamps and independent balance and Spot-wallet repair deadlines.
pub(super) struct AccountReconciliation {
    orders: HashMap<u64, OrderAccountStamp>,
    balance: RepairDeadline,
    spot_wallet: RepairDeadline,
}

impl AccountReconciliation {
    /// Creates idle reconciliation state.
    pub(super) fn new() -> Self {
        Self {
            orders: HashMap::new(),
            balance: RepairDeadline::new(BALANCE_REPAIR_INTERVAL),
            spot_wallet: RepairDeadline::new(SPOT_WALLET_REPAIR_INTERVAL),
        }
    }

    /// Observes one drained event batch and updates repair deadlines.
    ///
    /// Created orders affect reserved funds; removing an active order releases them, while removing
    /// an already-terminal order is only retained-state cleanup. Updated orders queue repair only
    /// when their phase or executed quantities change. Events are applied in arrival order: a later
    /// full balance snapshot or successful Spot-wallet update satisfies earlier work, while an
    /// incremental balance, failed wallet request, or unrelated wallet kind does not hide an
    /// authoritative repair.
    pub(super) fn observe_events(&mut self, events: &[Event], now: Instant) {
        for event in events {
            match event {
                Event::Order(OrderEvent::Created(order)) => {
                    self.observe_order(
                        OrderChange::Created,
                        order.uid,
                        OrderAccountStamp::new(
                            order.status,
                            order.buy_order.actual_q,
                            order.sell_order.actual_q,
                            order.platform,
                        ),
                        now,
                    );
                }
                Event::Order(OrderEvent::Updated(order)) => {
                    self.observe_order(
                        OrderChange::Updated,
                        order.uid,
                        OrderAccountStamp::new(
                            order.status,
                            order.buy_order.actual_q,
                            order.sell_order.actual_q,
                            order.platform,
                        ),
                        now,
                    );
                }
                Event::Order(OrderEvent::Removed(order)) => {
                    self.observe_order(
                        OrderChange::Removed,
                        order.uid,
                        OrderAccountStamp::new(
                            order.status,
                            order.buy_order.actual_q,
                            order.sell_order.actual_q,
                            order.platform,
                        ),
                        now,
                    );
                }
                Event::Balance(BalanceEvent::SnapshotApplied { .. }) => {
                    self.balance.satisfy();
                }
                Event::TransferAssets(TransferAssetsEvent::Updated {
                    kind: ExchangeKind::Spot,
                    ..
                }) => {
                    self.spot_wallet.satisfy();
                }
                _ => {}
            }
        }
    }

    /// Returns whether a full balance repair request is due.
    pub(super) fn balance_due(&self, now: Instant) -> bool {
        self.balance.is_due(now)
    }

    /// Records a full balance request attempt and starts its cooldown.
    pub(super) fn mark_balance_attempt(&mut self, now: Instant) {
        self.balance.mark_attempt(now);
    }

    /// Returns whether a Spot-wallet repair request is due.
    pub(super) fn spot_wallet_due(&self, now: Instant) -> bool {
        self.spot_wallet.is_due(now)
    }

    /// Records a Spot-wallet request attempt and starts its cooldown.
    pub(super) fn mark_spot_wallet_attempt(&mut self, now: Instant) {
        self.spot_wallet.mark_attempt(now);
    }

    /// Returns the earliest pending account-repair deadline.
    pub(super) fn next_wait(&self, now: Instant) -> Option<Duration> {
        match (self.balance.wait(now), self.spot_wallet.wait(now)) {
            (Some(balance), Some(wallet)) => Some(balance.min(wallet)),
            (Some(wait), None) | (None, Some(wait)) => Some(wait),
            (None, None) => None,
        }
    }

    /// Applies one pure order transition and queues repair only for account-relevant changes.
    fn observe_order(
        &mut self,
        change: OrderChange,
        uid: u64,
        stamp: OrderAccountStamp,
        now: Instant,
    ) {
        let (account_changed, spot_wallet_relevant) = match change {
            OrderChange::Created => {
                self.orders.insert(uid, stamp);
                (true, stamp.spot_wallet_relevant)
            }
            OrderChange::Updated => (
                self.orders.insert(uid, stamp) != Some(stamp),
                stamp.spot_wallet_relevant,
            ),
            OrderChange::Removed => {
                let removed = self.orders.remove(&uid).unwrap_or(stamp);
                (!removed.status.is_terminal(), removed.spot_wallet_relevant)
            }
        };
        if account_changed {
            self.balance.queue(now);
            if spot_wallet_relevant {
                self.spot_wallet.queue(now);
            }
        }
    }
}

#[cfg(test)]
mod tests;
