//! Station mode: a core link that keeps only what the headless station stores
//! (`docs-internal/STATION.md` §3.2) — the report replica, the order traces and the tape around
//! trades — and asks the core for nothing else.
//!
//! One process-wide switch, set by the station before any core is spawned and never cleared: a
//! terminal never sets it, so every branch below leaves the terminal as it was. What it turns off:
//!
//! - the client's periodic market refresh and its full-size history rings (`Compact`);
//! - every domain event but reports, archive answers and the core's log (the log is sampled for
//!   the clock offset alone, see [`keeps`]);
//! - the requests a terminal sends on Ready and on a reconnect — license, settings, hedge mode,
//!   balances, chart alerts, Telegram — and the recurring API-key poll;
//! - the Assets publications, the 5-minute kline recorder and the valuation outbox.
//!
//! What the core pushes regardless — orders, balances, `arb` — still arrives and is parsed by
//! moonproto; only the core or the library can stop that (§9, question 28).
//!
//! Without the periodic refresh the main client's market prices stay zero for the whole session
//! (moonproto fills bid/ask/mark on `UpdateMarketsList` ticks, not in Init) and a market listed
//! after Init stays unknown to it. Nothing on the station reads either — reports are stored as
//! sent, the tape comes through the recorder's own donor clients — and anything that starts to
//! must not take them from this client.

use std::sync::atomic::{AtomicBool, Ordering};

use moonproto::Event;

mod link;
pub use link::StationLink;

static STATION: AtomicBool = AtomicBool::new(false);

/// Switch this process into station mode. Call once, before the first core is spawned.
pub fn enable() {
    STATION.store(true, Ordering::Relaxed);
}

/// Whether this process is the station.
pub fn enabled() -> bool {
    STATION.load(Ordering::Relaxed)
}

/// Whether the station keeps a domain event.
///
/// `ServerLog` stays for one reader only: the clock-offset estimator samples it in a pass that
/// ignores `feed.log`, and nothing else reads it with the station's flags. Without it the offset
/// has no source at all — `Replica` and `Skew` are never produced (§9, question 27).
pub fn keeps(event: &Event) -> bool {
    matches!(
        event,
        Event::Report(_) | Event::MarketHistory(_) | Event::ServerLog(_)
    )
}

#[cfg(test)]
mod tests;
