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
/// Interval between API-key expiration polls.
///
/// Unlike the two repair deadlines above, this one is not event-driven: the core pushes nothing
/// when a key ages, so the terminal has to ask. Six hours is far below the one-day granularity the
/// answer carries, while the request itself reaches the exchange and must stay rare.
pub(super) const API_EXPIRY_POLL_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Retry delay after an API-key check that did not answer.
///
/// Short enough that a core which was merely busy is re-asked within the session, long enough that
/// a core which cannot answer the method at all (an older MoonBot) is not re-asked every minute.
pub(super) const API_EXPIRY_RETRY_INTERVAL: Duration = Duration::from_secs(15 * 60);

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

/// A plain recurring deadline: unlike [`RepairDeadline`] it needs no trigger, because nothing in
/// the event stream announces that a value went stale.
#[derive(Clone, Copy, Debug)]
struct PollDeadline {
    interval: Duration,
    due_at: Instant,
    last_attempt: Option<Instant>,
}

impl PollDeadline {
    /// Creates a poll that is due immediately, so the first value arrives as soon as the core can
    /// answer rather than one interval later.
    fn new(interval: Duration, now: Instant) -> Self {
        Self {
            interval,
            due_at: now,
            last_attempt: None,
        }
    }

    /// Brings the poll due at once, unless one ran within `min_gap`.
    ///
    /// For an event that means "this value may have changed" — a core reaching Ready. The gap is
    /// what keeps a flapping core from asking on every reconnect.
    fn poll_now_unless_recent(&mut self, now: Instant, min_gap: Duration) {
        let recent = self
            .last_attempt
            .is_some_and(|last| now.saturating_duration_since(last) < min_gap);
        if !recent {
            self.due_at = now;
        }
    }

    /// Brings the next poll forward to `delay` from now, for a retry after an unanswered check.
    /// Never pushes it further out than it already is.
    fn retry_in(&mut self, now: Instant, delay: Duration) {
        let retry_at = now + delay;
        if retry_at < self.due_at {
            self.due_at = retry_at;
        }
    }

    /// Pushes a due poll out by `delay`, for a caller that cannot act on it yet.
    ///
    /// Unconditional, unlike [`Self::retry_in`]: a deadline that stays due while its caller keeps
    /// declining it leaves the loop with a zero-length wait, and the thread spins.
    fn defer(&mut self, now: Instant, delay: Duration) {
        self.due_at = now + delay;
    }

    /// Returns whether the next poll may run at `now`.
    fn is_due(&self, now: Instant) -> bool {
        now >= self.due_at
    }

    /// Records a poll and schedules the next one a full interval out.
    fn mark_attempt(&mut self, now: Instant) {
        self.due_at = now + self.interval;
        self.last_attempt = Some(now);
    }

    /// Returns the remaining wait before the next poll.
    fn wait(&self, now: Instant) -> Duration {
        self.due_at.saturating_duration_since(now)
    }
}

/// Tracks order-account stamps, the balance and Spot-wallet repair deadlines, and the recurring
/// API-key expiration poll.
pub(super) struct AccountReconciliation {
    orders: HashMap<u64, OrderAccountStamp>,
    balance: RepairDeadline,
    spot_wallet: RepairDeadline,
    api_expiry: PollDeadline,
}

impl AccountReconciliation {
    /// Creates idle reconciliation state.
    pub(super) fn new(now: Instant) -> Self {
        Self {
            orders: HashMap::new(),
            balance: RepairDeadline::new(BALANCE_REPAIR_INTERVAL),
            spot_wallet: RepairDeadline::new(SPOT_WALLET_REPAIR_INTERVAL),
            api_expiry: PollDeadline::new(API_EXPIRY_POLL_INTERVAL, now),
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

    /// Returns whether the recurring API-key expiration poll is due.
    pub(super) fn api_expiry_due(&self, now: Instant) -> bool {
        self.api_expiry.is_due(now)
    }

    /// Records an API-key expiration poll and schedules the next one.
    pub(super) fn mark_api_expiry_attempt(&mut self, now: Instant) {
        self.api_expiry.mark_attempt(now);
    }

    /// Brings the next API-key poll forward after a check that did not answer.
    pub(super) fn retry_api_expiry(&mut self, now: Instant) {
        self.api_expiry.retry_in(now, API_EXPIRY_RETRY_INTERVAL);
    }

    /// Pushes the API-key poll out when it comes due on a core that cannot be asked yet.
    ///
    /// A FULL interval, not the retry delay: nothing is learned by waking a down core's thread to
    /// ask again, because the Ready transition arrives as a lifecycle event that wakes the loop and
    /// calls [`Self::poll_api_expiry_on_ready`] itself.
    pub(super) fn defer_api_expiry(&mut self, now: Instant) {
        self.api_expiry.defer(now, API_EXPIRY_POLL_INTERVAL);
    }

    /// Brings the API-key poll due when a core reaches Ready, unless one was just attempted.
    ///
    /// The gap is what keeps a flapping core from issuing an exchange-bound check per reconnect,
    /// while a core that has just come up for the first time is asked immediately.
    pub(super) fn poll_api_expiry_on_ready(&mut self, now: Instant) {
        self.api_expiry
            .poll_now_unless_recent(now, API_EXPIRY_RETRY_INTERVAL);
    }

    /// Returns the wait until the next API-key poll.
    ///
    /// Separate from [`Self::next_wait`] on purpose: that one answers "is repair pending?", and a
    /// recurring poll is always pending, which would erase the distinction.
    pub(super) fn api_expiry_wait(&self, now: Instant) -> Duration {
        self.api_expiry.wait(now)
    }

    /// Returns the earliest PENDING repair deadline, or `None` when no repair is queued.
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
