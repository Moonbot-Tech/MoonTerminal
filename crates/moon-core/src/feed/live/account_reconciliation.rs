//! Coalesces account repair requests around order changes that can affect balances.
//!
//! MoonProto normally pushes balance and transfer-asset events itself. This module keeps the
//! terminal's explicit refreshes as coalesced repair paths: the first account-relevant change after
//! an idle period requests immediately, later changes respect a cooldown, and an authoritative full
//! balance or Spot-wallet update cancels pending work.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::deadline::{CoalescedDeadline, PollDeadline};

use moonproto::state::{BalanceEvent, OrderEvent, TransferAssetsEvent};
use moonproto::{Event, ExchangeCode, ExchangeKind, OrderWorkerStatus};

/// Minimum interval between explicit full balance repair requests.
pub(super) const BALANCE_REPAIR_INTERVAL: Duration = Duration::from_secs(3);

/// How long after a repair request the core's balance answer is traced.
///
/// It sits beside the cooldown because the two only make sense read together, and reading them
/// apart is what went wrong: this window is LONGER than that interval, so on any core with steady
/// order flow the next repair re-arms it before it expires and the "diagnostic window" never
/// closes. Left wider on purpose — scoping the answer to the request is what makes the trace
/// readable when it is on, and at [`BALANCE_TRACE_LEVEL`] the overlap costs one clock read per
/// balance event and no log records at all.
///
/// Live, from `limits.balance_repeat_window_sec` in `cfg/diagnostics.toml`: someone who has just
/// switched the trace on is exactly the person who may need to see every request rather than one
/// per window, and making them rebuild for that would defeat the point of the switch.
pub(super) fn balance_refresh_log_window() -> Duration {
    crate::diagnostics::balance_repeat_window()
}

/// Level for the balance-repair diagnostic: the request, and the core's answer to it.
///
/// A constant rather than a literal macro choice so the decision is data one test can hold, and so
/// the three emit sites cannot drift apart.
///
/// `Debug` because of what this path costs at `info`, measured 2026-08-15 on 22 cores: repairs run
/// at the cooldown around the clock on a trading core, and these lines were **271 201 of the day's
/// 323 265 log records — 83.9%**. The ring the Log panel reads holds 5000 records, so that rate left
/// under ten minutes of history in which every other message was drowned. The default filter is
/// `warn,moon_ui_gpui=info,moon_gpui=info,moon_core=info` (`moon-ui-gpui/src/startup.rs`), so
/// `Debug` is dropped before it reaches the ring, the file, or the panel.
///
/// Turning it back on is `log.balances = true` in `cfg/diagnostics.toml` — applied without a
/// restart — or `MOON_DIAG_BALANCES=1`. Both resolve to a `moon_core::feed::live=debug` directive
/// appended to the default filter; see `crate::diagnostics::filter`.
///
/// What this costs, stated plainly: the phantom-Assets failure these lines were added for does not
/// survive a restart, so evidence can no longer be collected AFTER a user reports one — the trace
/// has to be enabled beforehand. Accepted because the question it was left on to answer has since
/// been answered by measurement: every repair in the sample was answered by a full snapshot, 54 of
/// 54 on the busiest core. Should phantoms return, the honest instrument is an alarm on the
/// condition itself, and that needs a state model this constant does not pretend to be.
pub(super) const BALANCE_TRACE_LEVEL: log::Level = log::Level::Debug;
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

/// Tracks order-account stamps, the balance and Spot-wallet repair deadlines, and the recurring
/// API-key expiration poll.
pub(super) struct AccountReconciliation {
    orders: HashMap<u64, OrderAccountStamp>,
    balance: CoalescedDeadline,
    spot_wallet: CoalescedDeadline,
    api_expiry: PollDeadline,
}

impl AccountReconciliation {
    /// Creates idle reconciliation state.
    pub(super) fn new(now: Instant) -> Self {
        Self {
            orders: HashMap::new(),
            balance: CoalescedDeadline::new(BALANCE_REPAIR_INTERVAL),
            spot_wallet: CoalescedDeadline::new(SPOT_WALLET_REPAIR_INTERVAL),
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
        self.api_expiry.defer(now);
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
