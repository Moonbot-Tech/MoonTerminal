//! Station mode: a core link that keeps only what the headless station needs
//! (`docs-internal/STATION.md` §3.2) and asks the core for nothing else.
//!
//! One process-wide switch, set by the station before any core is spawned and never cleared: a
//! terminal never sets it, so every branch below leaves the terminal as it was. It carries one of
//! two [`Profile`]s.
//!
//! [`Profile::Reports`] — the light station: the report replica with its USDT valuation, the order
//! traces and the tape around trades. What it turns off:
//!
//! - the client's periodic market refresh and its full-size history rings (`Compact`);
//! - every domain event but reports, archive answers and the core's log (the log is sampled for
//!   the clock offset alone, see [`keeps_reports`]);
//! - the requests a terminal sends on Ready and on a reconnect — license, settings, hedge mode,
//!   balances, chart alerts, Telegram — and the recurring API-key poll;
//! - the Assets publications and the 5-minute kline recorder.
//!
//! Without the periodic refresh the main client's market prices stay zero for the whole session
//! (moonproto fills bid/ask/mark on `UpdateMarketsList` ticks, not in Init) and a market listed
//! after Init stays unknown to it. Nothing on the light station reads either — reports are stored
//! as sent, the tape comes through the recorder's own donor clients — and anything that starts to
//! must not take them from this client.
//!
//! [`Profile::Account`] — the station that hosts the Mini App: everything above, plus the account
//! the Mini App shows and commands, taken the terminal's way — orders, balances and their repairs,
//! strategies, the core's health and run state (the Ready-time requests), the Assets publications
//! with the periodic price refresh they are priced by.
//! Still off: the full-size rings, the core's own Telegram, the API-key poll, transfer assets,
//! trade sounds, the strategy version archive (`strategies.sqlite`) and the kline recorder —
//! nothing the Mini App shows.
//!
//! What the core pushes regardless — orders, balances, `arb` — still arrives at a light station
//! and is parsed by moonproto; only the core or the library can stop that (§9, question 28).

use std::sync::atomic::{AtomicU8, Ordering};

use moonproto::Event;

mod link;
pub use link::StationLink;

/// What a station runs besides its reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// Reports, order traces and the tape: the light station.
    Reports,
    /// Also the account the Mini App shows and commands.
    Account,
}

impl Profile {
    /// Whether this station runs the account path the Mini App reads.
    pub fn runs_account(self) -> bool {
        self == Profile::Account
    }

    /// Whether this station keeps a domain event.
    pub fn keeps(self, event: &Event) -> bool {
        match self {
            Profile::Reports => keeps_reports(event),
            Profile::Account => keeps_account(event),
        }
    }
}

/// 0 — not a station; otherwise the profile's code below.
static STATION: AtomicU8 = AtomicU8::new(0);
const REPORTS: u8 = 1;
const ACCOUNT: u8 = 2;

/// Switch this process into station mode. Call once, before the first core is spawned.
pub fn enable(profile: Profile) {
    let code = match profile {
        Profile::Reports => REPORTS,
        Profile::Account => ACCOUNT,
    };
    STATION.store(code, Ordering::Relaxed);
}

/// The station's profile; `None` in a terminal.
pub fn profile() -> Option<Profile> {
    match STATION.load(Ordering::Relaxed) {
        REPORTS => Some(Profile::Reports),
        ACCOUNT => Some(Profile::Account),
        _ => None,
    }
}

/// Whether this process is the station, of either profile.
pub fn enabled() -> bool {
    profile().is_some()
}

/// Whether the light station keeps a domain event.
///
/// `ServerLog` stays for one reader only: the clock-offset estimator samples it in a pass that
/// ignores `feed.log`, and nothing else reads it with the station's flags. Without it the offset
/// has no source at all — `Replica` and `Skew` are never produced (§9, question 27).
pub fn keeps_reports(event: &Event) -> bool {
    matches!(
        event,
        Event::Report(_) | Event::MarketHistory(_) | Event::ServerLog(_)
    )
}

/// Whether the station hosting the Mini App keeps a domain event: the light station's, plus the
/// account — orders (the open-orders tab, Panic Sell's state), balances and account metadata (the
/// Cores tab and its repairs), strategies (the strategies tab), and the core's health and
/// settings (the cores tab: CPU, memory, ping, trading and auto-detect).
pub fn keeps_account(event: &Event) -> bool {
    keeps_reports(event)
        || matches!(
            event,
            Event::Order(_)
                | Event::Balance(_)
                | Event::Account(_)
                | Event::Strat(_)
                | Event::KernelHealth(_)
                | Event::Settings(_)
        )
}

#[cfg(test)]
mod tests;
