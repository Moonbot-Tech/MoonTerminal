//! Back-pressure for order batches: a live core keeps at most one of them in its feed channel.
//!
//! The channel itself is unbounded and stays a plain FIFO. What bounds it is the producer: a feed
//! thread asks [`OrderGate::admit`] before it builds a table or a line copy, and builds nothing
//! while the consumer still has the previous one to take. The debt stays with the feed thread,
//! which publishes one fresh batch the moment the consumer catches up.
//!
//! The gate binds only a producer that asks. The live loop does at every publication. The
//! synthetic core sends its one table and one line copy back to back at start without asking,
//! and the count then holds both until they are taken.
//!
//! Before this, a station whose main loop stalled kept receiving a full open-order table from
//! every core four times a second. Seventy cores piled up 126,423 of them, 9 GiB of heap charged
//! to the feed threads, and the process was killed for memory (#1081).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use super::{FeedMsg, FeedTx};

#[cfg(test)]
mod tests;

/// Whether a message is an order batch, the only kind the gate counts.
pub(super) fn is_order_batch(msg: &FeedMsg) -> bool {
    matches!(msg, FeedMsg::Orders(_) | FeedMsg::OrderLines(_))
}

/// How many order batches of one core wait in its channel, and whether its feed thread waits for
/// that number to reach zero.
///
/// Both sides use `SeqCst`. The producer writes `waiting` and then reads `queued`; the consumer
/// writes `queued` and then reads `waiting`. With a weaker ordering each could miss the other's
/// write, and the feed thread would sleep on a gate that is already open.
#[derive(Debug, Default)]
pub(super) struct OrderGate {
    queued: AtomicUsize,
    waiting: AtomicBool,
}

impl OrderGate {
    /// Counts a batch that is about to enter the channel.
    pub(super) fn sent(&self) {
        self.queued.fetch_add(1, Ordering::SeqCst);
    }

    /// Takes a batch off the count.
    ///
    /// Saturating: a count that wrapped below zero would read as "always queued" and stop this
    /// core's order publications for the rest of the process.
    ///
    /// Returns:
    ///     `true` when this emptied the channel of order batches and the feed thread asked to be
    ///     woken for that. The request is consumed.
    pub(super) fn taken(&self) -> bool {
        let before = self
            .queued
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |queued| {
                queued.checked_sub(1)
            });
        before.is_ok_and(|queued| queued == 1) && self.waiting.swap(false, Ordering::SeqCst)
    }

    /// Whether the feed thread may queue another order batch now.
    ///
    /// A refusal arms the wake: the consumer's next [`Self::taken`] that empties the channel
    /// reports it, so the feed thread does not have to poll.
    ///
    /// Returns:
    ///     `true` when no order batch of this core is waiting to be taken.
    pub(super) fn admit(&self) -> bool {
        if self.queued.load(Ordering::SeqCst) == 0 {
            return true;
        }
        self.waiting.store(true, Ordering::SeqCst);
        if self.queued.load(Ordering::SeqCst) == 0 {
            // Taken between the two reads. A wake already sent for it is one spare loop turn.
            self.waiting.store(false, Ordering::SeqCst);
            return true;
        }
        false
    }
}

/// Receiving half of one core's feed channel.
///
/// Every message leaves through [`Self::try_recv`], which keeps the queue count and the order
/// gate in step with the channel. The raw receiver is private for that reason: an order batch
/// taken past the gate would leave it closed for good.
pub struct FeedRx {
    rx: Receiver<FeedMsg>,
    orders: Arc<OrderGate>,
    /// The feed thread's own wake, the one its command sender rings.
    producer: Sender<()>,
}

impl FeedRx {
    /// Pairs a receiver with the sender that feeds it.
    ///
    /// Args:
    ///     rx: Receiving end of the channel `tx` sends on.
    ///     tx: The core's sender; its order gate becomes this receiver's.
    ///     producer: Wake of the thread that publishes through `tx`.
    pub(super) fn new(rx: Receiver<FeedMsg>, tx: &FeedTx, producer: Sender<()>) -> Self {
        Self {
            rx,
            orders: tx.order_gate(),
            producer,
        }
    }

    /// A receiver nothing sends to, left in a handle whose real one moved to its retirement.
    pub(super) fn closed() -> Self {
        Self {
            rx: std::sync::mpsc::channel().1,
            orders: Arc::default(),
            producer: std::sync::mpsc::channel().0,
        }
    }

    /// Takes the next queued message without waiting.
    ///
    /// Taking the core's last queued order batch wakes its feed thread when that thread holds a
    /// publication back for it.
    ///
    /// Returns:
    ///     The message, or why there is none.
    pub fn try_recv(&self) -> Result<FeedMsg, TryRecvError> {
        let msg = self.rx.try_recv()?;
        self.account(&msg);
        Ok(msg)
    }

    /// Waits for the next message, for tests that watch a feed from outside the session drain.
    #[cfg(test)]
    pub(crate) fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Result<FeedMsg, std::sync::mpsc::RecvTimeoutError> {
        let msg = self.rx.recv_timeout(timeout)?;
        self.account(&msg);
        Ok(msg)
    }

    /// Books one message as taken.
    fn account(&self, msg: &FeedMsg) {
        super::note_drained();
        if is_order_batch(msg) && self.orders.taken() {
            // A feed thread that already exited has nobody left to wake.
            let _ = self.producer.send(());
        }
    }
}
