//! How far this machine's own clock is from true UTC, measured over SNTP.
//!
//! MoonProto's Ping clock (`MoonClient::server_time_delta_ms`) is the core's clock minus THIS
//! machine's OS clock, deliberately without the library's transport NTP correction — which it
//! does not expose. A PC whose clock has drifted ten seconds would therefore fold those ten
//! seconds into every core's offset, and the report rows converted with it would land ten seconds
//! off the exchange's own trades on the chart. Subtracting this machine's error turns the delta
//! into the core's offset from true UTC, which is what the report axis needs.
//!
//! A minimal SNTP client (RFC 4330): one 48-byte UDP request on port 123, the four timestamps of
//! the exchange, `((t2 − t1) + (t3 − t4)) / 2`. It runs on its own thread, started by the first
//! feed that asks — DNS and a blocking UDP wait have no place on a feed loop — and refreshes every
//! [`REFRESH`]. The hosts are tried in order until one answers: the public pool MoonProto's own
//! syncer asks, the pool's Russian zone, and Microsoft's time service, so one blocked or
//! unreachable host does not leave the correction empty. A reply whose round trip exceeds
//! [`MAX_RTT_MS`] is refused: the error of the formula is bounded by half the round trip.
//!
//! Where nothing answers — UDP 123 closed by a firewall or a provider — there is no correction at
//! all, and the offsets are measured against the OS clock exactly as MoonProto reports them,
//! which is what a synced PC needs anyway.
//!
//! A measurement holds only while the wall clock and the monotonic clock advance together. When
//! they part by more than [`STEP_MS`] — the OS clock was stepped (a Windows resync, a manual fix)
//! or the machine slept (the monotonic clock stops during suspend on macOS and Linux) — the
//! measurement is dropped and a new round starts at once. Until it ends the Ping clock waits
//! ([`PcClock::Pending`]), bounded by [`PENDING_MAX`], so a hung resolver never leaves the cores
//! without an offset.

use std::hash::BuildHasher;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::thread::Thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// SNTP hosts, tried in order until one answers.
const HOSTS: [&str; 3] = ["pool.ntp.org", "ru.pool.ntp.org", "time.windows.com"];
const NTP_PORT: u16 = 123;
/// How long one host may take to answer once resolved.
const TIMEOUT: Duration = Duration::from_secs(2);
/// How often a good measurement is refreshed: a PC clock drifts by milliseconds per hour, and the
/// estimator only acts on seconds.
const REFRESH: Duration = Duration::from_secs(600);
/// How soon a round in which no host answered is retried.
const RETRY: Duration = Duration::from_secs(60);
/// The longest the Ping clock waits for a round: past it, the round counts as unanswered. Name
/// resolution has no timeout of its own, so this is what bounds a hung resolver.
const PENDING_MAX: Duration = Duration::from_secs(10);
/// How far the wall and monotonic clocks may part before a measurement no longer holds.
const STEP_MS: i64 = 1_000;
/// The longest round trip whose answer is used: the formula's own error is at most half of it.
const MAX_RTT_MS: i64 = 1_000;
/// A larger answer is a broken reply, not a clock.
const MAX_ERROR_MS: i64 = 86_400_000;
/// Seconds from the NTP epoch (1900-01-01) to the Unix epoch.
const NTP_TO_UNIX_SECS: i64 = 2_208_988_800;

/// What this machine knows about its own clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PcClock {
    /// A round is under way and has been for less than [`PENDING_MAX`]: a sample taken now could
    /// be corrected later and its neighbours not, so the Ping clock waits.
    Pending,
    /// No usable measurement: measure against the OS clock as is.
    Unavailable,
    /// `true UTC − this machine's clock`, in milliseconds (positive: this PC is behind).
    Error(i64),
}

impl PcClock {
    /// Whether the Ping clock may take samples.
    pub(super) fn settled(self) -> bool {
        self != PcClock::Pending
    }

    /// The correction to subtract from a Ping delta, if any.
    pub(super) fn error_ms(self) -> Option<i64> {
        match self {
            PcClock::Error(error) => Some(error),
            PcClock::Pending | PcClock::Unavailable => None,
        }
    }
}

/// One good measurement and the two clocks at the moment it was taken.
#[derive(Clone, Copy, Debug)]
struct Anchor {
    error_ms: i64,
    wall_ms: i64,
    mono: Instant,
}

/// Everything [`read`] decides from, behind one lock so no reader sees half an update.
#[derive(Clone, Copy, Debug)]
struct State {
    /// The measurement in force.
    anchor: Option<Anchor>,
    /// When the round now under way began; `None` when none is.
    pending_since: Option<Instant>,
}

static STATE: Mutex<State> = Mutex::new(State {
    anchor: None,
    pending_since: None,
});
/// The measuring thread, woken early when a measurement stops holding.
static THREAD: OnceLock<Option<Thread>> = OnceLock::new();

/// This machine's clock state, starting the measurement on the first call.
///
/// Returns:
///     The measured error, Pending while a round is under way (at most [`PENDING_MAX`]), or
///     Unavailable when there is no measurement.
pub(super) fn read() -> PcClock {
    let thread = THREAD.get_or_init(spawn);
    let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(a) = state.anchor {
        if holds(
            wall_ms_now() - a.wall_ms,
            a.mono.elapsed().as_millis() as i64,
        ) {
            return PcClock::Error(a.error_ms);
        }
        // Stepped or slept: this measurement no longer describes the clock. Measure again now.
        state.anchor = None;
        state.pending_since = Some(Instant::now());
        if let Some(thread) = thread {
            thread.unpark();
        }
    }
    match state.pending_since {
        Some(since) if since.elapsed() < PENDING_MAX => PcClock::Pending,
        _ => PcClock::Unavailable,
    }
}

/// Whether a measurement still describes the clock: the wall clock and the monotonic clock have
/// advanced together since it was taken, to within [`STEP_MS`].
///
/// Args:
///     wall_elapsed_ms: Wall-clock milliseconds since the measurement.
///     mono_elapsed_ms: Monotonic milliseconds since the measurement.
///
/// Returns:
///     `false` after an OS clock step or a suspend.
fn holds(wall_elapsed_ms: i64, mono_elapsed_ms: i64) -> bool {
    (wall_elapsed_ms - mono_elapsed_ms).abs() <= STEP_MS
}

/// Start the measuring thread.
///
/// Returns:
///     Its handle for an early wake-up, or `None` when it could not be spawned — the offsets then
///     stay on the OS clock, as without the correction, which is said once.
fn spawn() -> Option<Thread> {
    STATE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .pending_since = Some(Instant::now());
    let started = std::thread::Builder::new()
        .name("pc-clock-sntp".into())
        .spawn(run);
    match started {
        Ok(handle) => Some(handle.thread().clone()),
        Err(e) => {
            STATE
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pending_since = None;
            log::warn!(
                "pc clock: SNTP thread not started, core offsets stay on this PC's clock: {e}"
            );
            None
        }
    }
}

/// The measuring thread: a round, then a sleep until the next one or an early wake-up.
fn run() {
    let mut answered: Option<bool> = None;
    loop {
        let round = HOSTS.iter().find_map(|host| Some((*host, query(host)?)));
        let wait = {
            let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
            state.pending_since = None;
            match round {
                Some((host, anchor)) => {
                    state.anchor = Some(anchor);
                    if answered != Some(true) {
                        log::info!("pc clock: {} ms from UTC ({host})", anchor.error_ms);
                    }
                    answered = Some(true);
                    REFRESH
                }
                None => {
                    // Said once per outage: with UDP 123 closed this would otherwise repeat every
                    // minute for the whole session.
                    if answered != Some(false) {
                        if state.anchor.is_some() {
                            log::info!(
                                "pc clock: no SNTP host answered; keeping the last measurement"
                            );
                        } else {
                            log::info!(
                                "pc clock: no SNTP host answered; core offsets stay on this PC's clock"
                            );
                        }
                    }
                    answered = Some(false);
                    RETRY
                }
            }
        };
        std::thread::park_timeout(wait);
        STATE
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pending_since
            .get_or_insert_with(Instant::now);
    }
}

/// One SNTP exchange with `host`.
///
/// Returns:
///     The measurement, or `None` on any failure — resolution, socket, timeout, or a reply that
///     does not pass [`parse`].
fn query(host: &str) -> Option<Anchor> {
    let addr: SocketAddr = (host, NTP_PORT)
        .to_socket_addrs()
        .ok()?
        .find(SocketAddr::is_ipv4)?;
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.set_read_timeout(Some(TIMEOUT)).ok()?;
    socket.connect(addr).ok()?;
    let mut request = [0u8; 48];
    // LI 0, version 4, mode 3 (client).
    request[0] = 0x23;
    // The transmit timestamp, which the server echoes back as the originate timestamp: a reply
    // that does not carry it answers some other request. Its lowest fraction bits — under a
    // millisecond — are random, so an off-path sender cannot guess the echo from the clock.
    let mut sent = to_ntp(wall_ms_now());
    let noise = std::collections::hash_map::RandomState::new().hash_one(host) as u32 & 0x000F_FFFF;
    let frac = u32::from_be_bytes([sent[4], sent[5], sent[6], sent[7]]) ^ noise;
    sent[4..].copy_from_slice(&frac.to_be_bytes());
    request[40..48].copy_from_slice(&sent);
    let t1 = from_ntp(&sent);
    socket.send(&request).ok()?;
    let mut reply = [0u8; 48];
    let len = socket.recv(&mut reply).ok()?;
    let t4 = wall_ms_now();
    let mono = Instant::now();
    if len < 48 {
        return None;
    }
    Some(Anchor {
        error_ms: parse(&reply, &sent, t1, t4)?,
        wall_ms: t4,
        mono,
    })
}

/// Read the clock error out of an SNTP reply.
///
/// Args:
///     reply: The 48-byte server reply.
///     sent: The transmit timestamp the request carried.
///     t1_ms: This machine's clock when the request left, Unix milliseconds.
///     t4_ms: This machine's clock when the reply arrived, Unix milliseconds.
///
/// Returns:
///     `true − local` in milliseconds, or `None` for a reply that is not a usable server answer:
///     not mode 4, an unsynchronized server (leap indicator 3), stratum 0 (a kiss-o'-death) or
///     above 15, a different request's echo, a round trip past [`MAX_RTT_MS`], or an implausible
///     error.
fn parse(reply: &[u8; 48], sent: &[u8], t1_ms: i64, t4_ms: i64) -> Option<i64> {
    let leap = reply[0] >> 6;
    let mode = reply[0] & 0x07;
    let stratum = reply[1];
    if mode != 4 || leap == 3 || !(1..=15).contains(&stratum) || reply[24..32] != *sent {
        return None;
    }
    let t2 = from_ntp(&reply[32..40]);
    let t3 = from_ntp(&reply[40..48]);
    let rtt = (t4_ms - t1_ms) - (t3 - t2);
    if !(0..=MAX_RTT_MS).contains(&rtt) {
        return None;
    }
    let error = (((t2 - t1_ms) + (t3 - t4_ms)) as f64 / 2.0).round() as i64;
    (error.abs() < MAX_ERROR_MS).then_some(error)
}

/// Unix milliseconds as an NTP timestamp: seconds since 1900 and a 32-bit fraction.
fn to_ntp(unix_ms: i64) -> [u8; 8] {
    let secs = unix_ms.div_euclid(1_000) + NTP_TO_UNIX_SECS;
    let frac = ((unix_ms.rem_euclid(1_000) as u64) << 32) / 1_000;
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&(secs as u32).to_be_bytes());
    out[4..].copy_from_slice(&(frac as u32).to_be_bytes());
    out
}

/// An NTP timestamp as Unix milliseconds.
///
/// The 32-bit seconds wrap on 2036-02-07; a value in the lower half of the range is read as the
/// next era, which keeps every date from 1968 to 2104 right.
fn from_ntp(ts: &[u8]) -> i64 {
    let secs = i64::from(u32::from_be_bytes([ts[0], ts[1], ts[2], ts[3]]));
    let frac = i64::from(u32::from_be_bytes([ts[4], ts[5], ts[6], ts[7]]));
    let secs = if secs < 1 << 31 {
        secs + (1 << 32)
    } else {
        secs
    };
    // Rounded to the nearest millisecond, so a timestamp written by `to_ntp` reads back exactly.
    (secs - NTP_TO_UNIX_SECS) * 1_000 + ((frac * 1_000 + (1 << 31)) >> 32)
}

/// This machine's wall clock, Unix milliseconds.
fn wall_ms_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests;
