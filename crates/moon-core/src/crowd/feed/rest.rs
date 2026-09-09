//! The two public boards, polled.
//!
//! Neither has a stream: the service recomputes them on its own clock and hands them over whole,
//! and twelve guessed STOMP subscriptions on one connection for seventy-five seconds delivered
//! nothing at all (`docs-internal/stat-moonbot-trade-feed.md`). So this is a GET on a timer, on
//! its own thread, and what it reads replaces what was there.
//!
//! Its own agent rather than `market::trade_replay::rest::agent`: that one is built for exchange
//! REST and its timeouts are chosen there, while these two calls are a slow public page read on a
//! minute-long timer.
//!
//! * `/getCoin24h` — ten coins, the best five and the worst five of the rolling day.
//! * `/getRait24h` — fifty traders, ranked by profit. The site itself reloads it every 57 s.
//! * `/getRes` — the day in two numbers: every trade and every dollar, everybody included.
//!
//! Measured against the live service rather than taken from a specification: 311 bytes in 344 ms
//! and 4219 bytes in 181 ms, both `200`, with the field names the parsers below expect.
//!
//! No identity of any kind goes out: these are public pages, read exactly as a browser reads them.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Weak};
use std::time::Duration;

use crate::crowd::board::{CoinDay, DaySummary, Trader};

use super::{FeedConfig, FeedEvent, Wants, waited};

/// How often the boards are re-read.
///
/// The service recomputes the day about once a minute and the site reloads on 57 s; asking faster
/// only spends requests on an answer that has not changed.
const POLL: Duration = Duration::from_secs(57);
/// How long to wait after a failed round before trying again.
const RETRY: Duration = Duration::from_secs(20);
/// How long one request may take before it is given up on.
const TIMEOUT: Duration = Duration::from_secs(15);

/// Read both boards until the reader that asked for them is gone.
///
/// Args:
///     config: Where to read from.
///     tx: Where the boards go.
///     alive: The reader's own handle; when it cannot be upgraded, nobody is listening and this
///         thread ends. A failed send says the same thing, but only a round that HAD something to
///         send can learn it — and a service that is down gives this thread nothing to send.
///     wants: Which of the two boards is shown. An endpoint nobody draws is not fetched: the
///         trader board carries the day's total, so that pair travels together.
///     epoch: Which generation of this half the thread belongs to, carried on every event.
pub fn run(
    config: &Arc<FeedConfig>,
    tx: &Sender<FeedEvent>,
    alive: &Weak<()>,
    wants: Wants,
    epoch: u64,
) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .into();
    loop {
        if alive.upgrade().is_none() {
            return;
        }
        let coins = wants
            .coins
            .then(|| fetch(&agent, &format!("{}/getCoin24h", config.base)).map(|body| coins(&body)))
            .flatten();
        let traders = wants
            .traders
            .then(|| {
                fetch(&agent, &format!("{}/getRait24h", config.base)).map(|body| traders(&body))
            })
            .flatten();
        let summary = wants
            .traders
            .then(|| fetch(&agent, &format!("{}/getRes", config.base)).and_then(|b| summary(&b)))
            .flatten();
        // A round counts as good if EITHER board answered: one endpoint being down is not a
        // reason to stop reading the other, and a screen showing one table is better than a
        // screen showing none.
        // An EMPTY board is not a board. The service always has both, so nothing parsed out of
        // an answer means the answer was not one — an error page, a captive portal, a redirect
        // followed to a landing page — and replacing a good table with the `crowd.day.silent`
        // notice on the strength of that is worse than showing the last one for another twenty
        // seconds.
        let mut got = false;
        if let Some(coins) = coins.filter(|rows| !rows.is_empty()) {
            got = true;
            if tx.send(FeedEvent::Coins(epoch, coins)).is_err() {
                return;
            }
        }
        if let Some(traders) = traders.filter(|rows| !rows.is_empty()) {
            got = true;
            if tx.send(FeedEvent::Traders(epoch, traders)).is_err() {
                return;
            }
        }
        if let Some(summary) = summary {
            got = true;
            if tx.send(FeedEvent::Summary(epoch, summary)).is_err() {
                return;
            }
        }
        if !waited(alive, if got { POLL } else { RETRY }) {
            return;
        }
    }
}

/// One GET, or `None` if it did not answer.
fn fetch(agent: &ureq::Agent, url: &str) -> Option<String> {
    agent.get(url).call().ok()?.body_mut().read_to_string().ok()
}

/// Pull the coin board out of a `/getCoin24h` body.
///
/// A row the service did not spell the way we expect is skipped rather than defaulted: a coin with
/// no name or no number is not a row, and inventing a zero for it would put a lie on the screen.
/// A ticker that arrives twice is taken once, for the same reason the trader board takes an
/// account once: the ticker is what that table's movement is tracked by.
///
/// Ordered by MONEY, biggest gain first and biggest loss last. The service hands over two ends of
/// its day — its best few, then its worst few — and inside that second half it counts DOWN from
/// the smallest loss, so taken as it arrives the column reads `+1, -100, -1`. One order for the
/// whole column, with the ticker breaking a tie so equal rows do not swap places between polls.
pub fn coins(body: &str) -> Vec<CoinDay> {
    let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(body) else {
        return Vec::new();
    };
    let mut seen: Vec<String> = Vec::with_capacity(rows.len());
    let mut rows: Vec<CoinDay> = rows
        .iter()
        .filter_map(|row| {
            let coin = row.get("c")?.as_str()?.trim();
            let profit = row.get("p")?.as_f64()?;
            if coin.is_empty() || !profit.is_finite() || seen.iter().any(|had| had == coin) {
                return None;
            }
            seen.push(coin.to_string());
            Some(CoinDay {
                coin: coin.to_string(),
                profit,
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        b.profit
            .partial_cmp(&a.profit)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.coin.cmp(&b.coin))
    });
    rows
}

/// Pull the trader board out of a `/getRait24h` body.
///
/// `pb` is the money and `c` the trades — checked against the service's own day totals rather
/// than guessed, because the row also carries a `p` that is NOT dollars. `i` is the ACCOUNT here,
/// though the same letter is the PLACE on the coin board.
///
/// The account and the place are REQUIRED, and a row without either is dropped rather than given
/// a zero. Everything on the screen is keyed by that account — which row is which, what moved,
/// what lit up — so a defaulted one is not a missing figure, it is two people wearing the same
/// name. The same goes for a repeat: the first row with an account keeps it.
///
/// And the board is put in rank order here rather than trusted to arrive in it, because the rank
/// is printed in the first column: rows drawn in one order with numbers from another read as a
/// broken table, and `take(TRADERS_SHOWN)` would then be showing an arbitrary twenty.
pub fn traders(body: &str) -> Vec<Trader> {
    let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(body) else {
        return Vec::new();
    };
    let mut out: Vec<Trader> = Vec::with_capacity(rows.len());
    for row in &rows {
        let Some(profit) = row.get("pb").and_then(|money| money.as_f64()) else {
            continue;
        };
        let Some(id) = row.get("i").and_then(|id| id.as_u64()) else {
            continue;
        };
        // A rank that does not fit the type is not a rank. Clamping it would print
        // 4294967295 in the first column of a board whose own type says "from one".
        let Some(place) = row
            .get("r")
            .and_then(|place| place.as_u64())
            .and_then(|place| u32::try_from(place).ok())
        else {
            continue;
        };
        if !profit.is_finite() || out.iter().any(|seen| seen.id == id) {
            continue;
        }
        out.push(Trader {
            place,
            id,
            handle: row
                .get("t")
                .and_then(|handle| handle.as_str())
                .unwrap_or("@")
                .to_string(),
            profit,
            trades: count(row.get("c")),
        });
    }
    out.sort_by_key(|row| row.place);
    out
}

/// Pull the day's two totals out of a `/getRes` body.
///
/// `None` for anything that is not both numbers: this is one figure printed as a whole, and half
/// of it is not a smaller truth — it is a number beside a blank where its pair should be.
///
/// Args:
///     body: The response.
pub fn summary(body: &str) -> Option<DaySummary> {
    let row: serde_json::Value = serde_json::from_str(body).ok()?;
    let profit = row.get("s")?.as_f64().filter(|money| money.is_finite())?;
    Some(DaySummary {
        trades: count(row.get("c")),
        profit,
    })
}

/// How many trades a row claims, from a field the service may spell either way.
///
/// It arrives as an integer today, and a JSON number that came back as `68.0` would otherwise
/// silently become "0 trades" beside a real profit — a defaulted lie of exactly the kind this
/// parser refuses for the account and the rank. Anything that is not a countable number at all is
/// zero, which is what an absent field means.
///
/// Args:
///     field: The row's `c`, if it has one.
fn count(field: Option<&serde_json::Value>) -> u64 {
    let Some(field) = field else {
        return 0;
    };
    if let Some(count) = field.as_u64() {
        return count;
    }
    field
        .as_f64()
        .filter(|count| count.is_finite() && *count >= 0.0)
        .map_or(0, |count| count as u64)
}

#[cfg(test)]
mod tests;
