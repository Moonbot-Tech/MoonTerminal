//! The live trade stream: SockJS `xhr` carrying STOMP.
//!
//! A direct WebSocket upgrade is refused upstream (HTTP 400 / close 1006) even from a browser on
//! the service's own origin, so the long-polling transport is the only one that works. It costs
//! one request per poll and blocks until there is something to deliver, which on a quiet night is
//! about one request every 25 seconds.
//!
//! The protocol was measured, not read off a spec: see `docs-internal/stat-moonbot-trade-feed.md`.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Weak};
use std::time::Duration;

use crate::crowd::{Rng, Trade};

use super::{FeedConfig, FeedEvent, waited};

/// STOMP frames end with a NUL byte.
const NUL: char = '\0';
/// Delay before rebuilding a session after any failure.
const RECONNECT: Duration = Duration::from_secs(10);
/// Longest a poll may block. The server holds a quiet poll for about 25 s.
const POLL_TIMEOUT: Duration = Duration::from_secs(45);

/// Why a session ended.
///
/// The two are worth telling apart, and used not to be: a broken wire is retried forever, while a
/// reader that has gone away means this thread has nobody left to talk to. Reconnecting for a
/// receiver that no longer exists leaves a thread and a socket per reader ever opened, polling a
/// server for the life of the process.
enum Ended {
    /// The connection broke; try again in a moment.
    Wire,
    /// Nobody is reading any more.
    Gone,
}

/// Connect, subscribe and forward trades until the wire breaks or the reader goes away.
///
/// Args:
///     config: Where to read from.
///     tx: Where the trades go.
///     alive: The reader's own handle. Checked between sessions AND between polls: a session that
///         only looked at it on the way round would sit in a poll for most of a minute after
///         being abandoned, which is the whole time a screen has to switch the table back on and
///         be given a SECOND socket beside this one.
///     epoch: Which generation of this half the thread belongs to, carried on every event so a
///         thread that is on its way out cannot feed a reader that has already moved on.
pub fn run(config: &Arc<FeedConfig>, tx: &Sender<FeedEvent>, alive: &Weak<()>, epoch: u64) {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(POLL_TIMEOUT))
        .build()
        .into();
    let mut rng = Rng::new(seed());
    loop {
        if alive.upgrade().is_none() {
            return;
        }
        match session(&agent, config, tx, &mut rng, alive, epoch) {
            Ended::Gone => return,
            // Say the wire is down before sleeping on it — and if even that cannot be said, the
            // reader is gone too.
            Ended::Wire => {
                if tx.send(FeedEvent::Connected(epoch, false)).is_err() {
                    return;
                }
            }
        }
        if !waited(alive, RECONNECT) {
            return;
        }
    }
}

/// One SockJS session: open, subscribe, then poll until it breaks.
fn session(
    agent: &ureq::Agent,
    config: &FeedConfig,
    tx: &Sender<FeedEvent>,
    rng: &mut Rng,
    alive: &Weak<()>,
    epoch: u64,
) -> Ended {
    // The `?` inside talks about the WIRE: every one of them is a request that failed. Only a
    // failed send means the reader is gone, and that is the one case returned explicitly.
    talk(agent, config, tx, rng, alive, epoch).unwrap_or(Ended::Wire)
}

fn talk(
    agent: &ureq::Agent,
    config: &FeedConfig,
    tx: &Sender<FeedEvent>,
    rng: &mut Rng,
    alive: &Weak<()>,
    epoch: u64,
) -> Option<Ended> {
    let base = format!(
        "{}/exchange/{:03}/{}",
        config.base,
        (rng.next_u64() % 1000),
        session_id(rng)
    );
    // The first poll must see the SockJS "open" frame before anything is sent.
    if !poll(agent, &base)?.starts_with('o') {
        return Some(Ended::Wire);
    }
    send(
        agent,
        &base,
        &format!("CONNECT\naccept-version:1.1,1.0\nheart-beat:0,0\n\n{NUL}"),
    )?;
    poll(agent, &base)?;
    send(
        agent,
        &base,
        &format!("SUBSCRIBE\nid:sub-0\ndestination:/data/trades\n\n{NUL}"),
    )?;
    if tx.send(FeedEvent::Connected(epoch, true)).is_err() {
        return Some(Ended::Gone);
    }

    loop {
        if alive.upgrade().is_none() {
            return Some(Ended::Gone);
        }
        let frame = poll(agent, &base)?;
        match frame.chars().next() {
            // `a[...]` — an array of STOMP frames, each a JSON string.
            Some('a') => {
                for trade in trades_in(&frame) {
                    if tx.send(FeedEvent::Trade(epoch, trade)).is_err() {
                        return Some(Ended::Gone);
                    }
                }
            }
            // `h` is a heartbeat and `o` a re-open; neither carries data. Say we are still
            // connected anyway — it is true, and on a quiet market it is the ONLY thing that ever
            // touches the channel, which is how a thread whose reader has gone finds out.
            Some('h') | Some('o') => {
                if tx.send(FeedEvent::Connected(epoch, true)).is_err() {
                    return Some(Ended::Gone);
                }
            }
            // `c` closes the session, and anything else is not a frame we know.
            _ => return Some(Ended::Wire),
        }
    }
}

/// POST the long-poll and return the raw SockJS frame.
fn poll(agent: &ureq::Agent, base: &str) -> Option<String> {
    agent
        .post(format!("{base}/xhr"))
        .send("")
        .ok()?
        .body_mut()
        .read_to_string()
        .ok()
}

/// POST one STOMP frame.
fn send(agent: &ureq::Agent, base: &str, frame: &str) -> Option<()> {
    let body = serde_json::to_string(&[frame]).ok()?;
    agent
        .post(format!("{base}/xhr_send"))
        .content_type("text/plain;charset=UTF-8")
        .send(body)
        .ok()?;
    Some(())
}

/// Pull every trade out of a SockJS `a[...]` frame.
///
/// A malformed frame yields nothing rather than an error: this runs on the feed's own thread, and
/// one bad payload must not take the stream down with it.
fn trades_in(frame: &str) -> Vec<Trade> {
    let mut out = Vec::new();
    // The `a` is the SockJS frame kind and the JSON starts after it. Stripped rather than sliced
    // by index: this takes network text, and the one thing a parser of network text must not do
    // is assume the shape it was promised.
    let Some(body) = frame.strip_prefix('a') else {
        return out;
    };
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(body)
    else {
        return out;
    };
    for item in items {
        let Some(stomp) = item.as_str() else { continue };
        // A STOMP frame is headers, a blank line, then the body up to the NUL.
        let Some((_, body)) = stomp.split_once("\n\n") else {
            continue;
        };
        let body = body.trim_end_matches(NUL);
        let Ok(serde_json::Value::Array(rows)) = serde_json::from_str::<serde_json::Value>(body)
        else {
            continue;
        };
        for row in rows {
            let (Some(coin), Some(profit)) = (
                row.get("c").and_then(serde_json::Value::as_str),
                row.get("u").and_then(serde_json::Value::as_f64),
            ) else {
                continue;
            };
            if coin.is_empty() || !profit.is_finite() {
                continue;
            }
            // `at_ms` is filled in by `LiveFeed::drain` on the reader's clock; the wire's own
            // close time belongs to a different clock entirely.
            out.push(Trade::new(0, coin, profit));
        }
    }
    out
}

/// Eight alphanumerics, as the SockJS client does.
fn session_id(rng: &mut Rng) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    (0..8)
        .map(|_| ALPHABET[(rng.next_u64() % ALPHABET.len() as u64) as usize] as char)
        .collect()
}

fn seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos() as u64)
        .unwrap_or(0xC0FFEE)
}

#[cfg(test)]
mod tests;
