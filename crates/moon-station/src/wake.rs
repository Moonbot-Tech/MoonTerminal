//! Coalesced feed wakes for the station owner, using the terminal's feed-wake protocol.

use moon_core::feed::{FeedWakeRx, FeedWakeTx, feed_wake_pair_for_thread};

/// Wake the current owner directly, keeping the terminal's coalesced admission gate.
pub fn feeds() -> (FeedWakeTx, FeedWakeRx) {
    feed_wake_pair_for_thread(std::thread::current())
}

/// Clear admission before the consumer drains, so arrivals during its drain retain a new wake.
pub fn before_drain(incoming: &FeedWakeRx) {
    while incoming.try_recv().is_ok() {}
    incoming.clear();
}

#[cfg(test)]
mod tests;
