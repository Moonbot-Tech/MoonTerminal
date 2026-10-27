//! Stopping the old core of a move, over MoonProto, before the new one starts.
//!
//! Two cores on one exchange account trade against each other, so the new core is started only
//! once the old one is down — and a wrong "down" is the costly mistake here, so every doubt reads
//! as "still running". The library's shutdown request is one-shot and unanswered, and a core
//! ignores it while it holds an active take/sell order — on a trading core, the usual case. So
//! "stopped" is observed: the link the request went over is lost, and then the core answers not
//! even the transport handshake of a fresh connection, several times over about a minute.

use std::time::{Duration, Instant};

use moonproto::{
    ClientConfig, ConnectConfig, InitConfig, InitialStrategies, LifecycleEvent, MoonClient,
};

/// How often the waits below look at the client again.
const POLL: Duration = Duration::from_millis(250);
/// How long a core may take to go down after the request before it counts as refusing.
const GRACE: Duration = Duration::from_secs(45);
/// How long one probe waits for the core's transport handshake. A running core answers it in
/// well under a second (measured: 0.8 s); this is the room left for a slow link.
const PROBE: Duration = Duration::from_secs(30);
/// How many probes in a row must go unanswered before the core counts as down — one lost
/// handshake is a network blip, not a stopped core.
const SILENT_PROBES: u32 = 3;
/// The pause between two probes.
const PROBE_GAP: Duration = Duration::from_secs(10);

/// What a stop request came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopAnswer {
    /// The core went down after the request: it answers no connection.
    Stopped,
    /// The core keeps running. `open_deals` counts the deals it had unfinished — the orders that
    /// make a core refuse to stop — or is `None` when it could not be read.
    Refused { open_deals: Option<usize> },
    /// Nothing answers at the address at all. That is a stopped core only for a caller who has
    /// seen this core answer there before; otherwise it may as well be a wrong address with the
    /// core running elsewhere, and the caller must not take it as down.
    NotReached,
}

/// Ask the core at `cfg` to shut down and see whether it does. `timeout` bounds the connect the
/// request goes over; a core that cannot be connected to is checked with [`is_silent`]:
/// [`StopAnswer::NotReached`] when it is silent, an error when it answers.
pub fn request_stop(cfg: ClientConfig, timeout: Duration) -> anyhow::Result<StopAnswer> {
    let client = match super::strategies::connect(cfg.clone(), timeout) {
        Ok(client) => client,
        Err(e) => {
            return if is_silent(cfg) {
                Ok(StopAnswer::NotReached)
            } else {
                Err(e.context("the old core answers but could not be connected to"))
            };
        }
    };
    // The handshake's own lifecycle events are still queued: only what follows the request
    // tells whether the link was lost.
    client.drain_lifecycle_events_into(&mut Vec::new());
    client
        .settings()
        .request_core_shutdown()
        .map_err(|e| anyhow::anyhow!("ask the old core to stop: {e}"))?;
    let lost = wait_for_loss(&client, GRACE);
    // Read after the wait: the core resyncs its orders after Ready, and the retained snapshot
    // keeps them past a lost link.
    let open_deals = open_deals(&client);
    close(client);
    if lost && is_silent(cfg) {
        Ok(StopAnswer::Stopped)
    } else {
        Ok(StopAnswer::Refused { open_deals })
    }
}

/// Whether the core at `cfg` is down: [`SILENT_PROBES`] fresh connections in a row, [`PROBE_GAP`]
/// apart, each get no transport handshake within [`PROBE`]. A handshake completed by any probe
/// means the core still runs. The probes use the key the core accepted earlier in the move, so a
/// running core completes them; a core that rejected it would read as silent.
pub fn is_silent(cfg: ClientConfig) -> bool {
    for probe in 0..SILENT_PROBES {
        if probe > 0 {
            std::thread::sleep(PROBE_GAP);
        }
        if answers(cfg.clone()) {
            return false;
        }
    }
    true
}

/// Whether the core at `cfg` completes a transport handshake within [`PROBE`]. A client that
/// cannot even be created (no local socket) proves nothing and reads as an answer.
fn answers(cfg: ClientConfig) -> bool {
    // The same announcement as every other call of a move: no local strategies.
    let init = InitConfig {
        initial_strategies: Some(InitialStrategies::new(0, Vec::new())),
        ..Default::default()
    };
    let Ok(client) = MoonClient::connect(cfg, ConnectConfig::new(init)) else {
        return true;
    };
    let deadline = Instant::now() + PROBE;
    let mut events = Vec::new();
    let mut answered = false;
    while !answered && Instant::now() < deadline {
        client.drain_lifecycle_events_into(&mut events);
        answered = events.drain(..).any(|event| {
            matches!(
                event,
                LifecycleEvent::Connected { .. } | LifecycleEvent::Ready
            )
        });
        if !answered {
            std::thread::sleep(POLL);
        }
    }
    close(client);
    answered
}

/// Whether the client lost its established link to the core within `grace`.
fn wait_for_loss(client: &MoonClient, grace: Duration) -> bool {
    let deadline = Instant::now() + grace;
    let mut events = Vec::new();
    while Instant::now() < deadline {
        client.drain_lifecycle_events_into(&mut events);
        if events
            .drain(..)
            .any(|event| matches!(event, LifecycleEvent::Reconnecting))
        {
            return true;
        }
        std::thread::sleep(POLL);
    }
    false
}

/// Disconnect and wait for the client's runtime to end.
fn close(client: MoonClient) {
    let _ = client.disconnect();
    let _ = client.wait_finished();
}

/// The core's deals that are not finished yet (`job_is_done` unset), emulated ones aside.
fn open_deals(client: &MoonClient) -> Option<usize> {
    client.snapshot().map(|state| {
        state
            .orders()
            .iter()
            .filter(|order| !order.job_is_done && !order.emulator_mode)
            .count()
    })
}
