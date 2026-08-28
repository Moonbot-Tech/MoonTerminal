//! Core endpoint, health, API-key expiry, and startup-status feed models.

use std::net::IpAddr;

/// Exchange API-key expiration reported by one core.
///
/// A CURRENT core sends a timezone-safe remaining duration, which MoonProto turns into an absolute
/// date against the CLIENT's clock — so the core's own time zone cannot shift it. A LEGACY core
/// (the 8-byte payload) sends only a server-local timestamp, which nothing normalizes; that answer
/// carries no [`Self::at_unix`] and its day count can be off by the terminal↔core zone difference.
///
/// Only a SUCCESSFUL check produces this value. A core that cannot check an exchange at all answers
/// `success = false` (observed live on Bitget/Gate/OKX), which becomes a failure event and never
/// reaches this type — that is the protocol's own line between "unlimited" and "could not check",
/// and no consumer should try to redraw it from the fields below.
///
/// Note what a failure does NOT do: it does not erase what came before. The store keeps the last
/// successful answer, so a core whose checks start failing keeps showing its previous state until
/// the connection is replaced ([`crate::session::CoreData`] clears it on a new connection attempt).
/// A core that has never answered has no value at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiKeyExpiry {
    /// The core answered that this key has NO expiration — an unlimited key.
    ///
    /// Derived at the wire boundary rather than inferred later: an empty date field alone does not
    /// mean unlimited, because the same answer can still carry a real day count (the parser fills
    /// the date with zero whenever the core's timestamp is unusable, yet returns the count beside
    /// it). Only an empty date AND no positive count is an unlimited key.
    pub unlimited: bool,
    /// Whether the response carried a usable expiration DATE.
    pub known: bool,
    /// Whole days left AT THE MOMENT OF THE CHECK, as counted by the core. Present whenever the
    /// answer carried a usable count — with or without a date beside it — and `None` for an
    /// unlimited key or an unusable answer. Read it through [`Self::days_left_at`], which ages it:
    /// this raw field goes stale between polls and while the core is down.
    pub days_left: Option<i32>,
    /// Absolute expiration as whole Unix seconds, on the terminal's clock. `None` for a legacy
    /// answer that carries no normalized duration, and while `known` is false.
    pub at_unix: Option<i64>,
    /// Unix ms the terminal received this answer, so a stored day count can be aged.
    pub checked_ms: i64,
}

impl ApiKeyExpiry {
    /// Days left as of `now_ms`, negative once the key has expired.
    ///
    /// A stored answer must never be read as-is: it is a snapshot, and the terminal keeps showing
    /// it while a core is down — for weeks, if the core stays down. Aged here so a key that runs
    /// out during an outage still crosses the warning threshold instead of standing frozen at the
    /// count it had when the core was last reachable.
    ///
    /// The absolute date is preferred because it does not accumulate error; the legacy path ages
    /// the core's own count by whole elapsed days instead.
    ///
    /// Args:
    ///     now_ms: Current Unix milliseconds.
    ///
    /// Returns:
    ///     Whole days remaining, or `None` for a key with no expiration or no usable count.
    pub fn days_left_at(&self, now_ms: i64) -> Option<i32> {
        // No guard on `known`: an answer can carry a count with no usable date, and dropping it
        // here would hide a real countdown. The zero that means "no expiry" never reaches this
        // field — the converter records that as [`Self::unlimited`] instead.
        if let Some(at_unix) = self.at_unix {
            let seconds_left = at_unix.saturating_sub(now_ms.div_euclid(1_000));
            // Floor division, so a key with 23 hours left reads 0 days and only a key already past
            // its date goes negative.
            return Some(clamp_to_i32(seconds_left.div_euclid(86_400)));
        }
        let days = self.days_left?;
        let aged = now_ms.saturating_sub(self.checked_ms).max(0) / 86_400_000;
        Some(days.saturating_sub(clamp_to_i32(aged)))
    }

    /// Whether two answers say the same thing.
    ///
    /// The absolute date is compared at DAY granularity on purpose: MoonProto rebuilds it as
    /// `client_now + remaining`, so an unchanged key answers with a date a few seconds apart every
    /// poll, and an exact comparison would report a change every six hours.
    pub fn answer_eq(&self, other: &Self) -> bool {
        self.unlimited == other.unlimited
            && self.known == other.known
            && self.days_left == other.days_left
            && self.at_unix.map(|at| at.div_euclid(86_400))
                == other.at_unix.map(|at| at.div_euclid(86_400))
    }
}

/// Saturate an `i64` day count into `i32`, so a nonsense date cannot wrap into a plausible one.
fn clamp_to_i32(days: i64) -> i32 {
    days.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// Network endpoint of one MoonBot core, decoded from its exported key by the live feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CoreEndpoint {
    /// Host IP selected for the MoonProto connection.
    pub address: IpAddr,
    /// UDP port selected for this core on the host.
    pub port: u16,
}

/// Latest resource telemetry for one core, from protocol v4 `Event::KernelHealth`.
///
/// Every `Option` is `None` until that field first arrives. CPU refreshes on
/// every Ping; memory and the logical-CPU count are a lower-rate tail (`None`
/// until the first memory-bearing Ping, then the retained snapshot keeps the last
/// value). Fields carry a SCOPE distinction (process vs whole machine), NOT a time
/// one: `system_cpu_percent` is machine-wide CPU, never an average of the process
/// CPU. Moonproto-free — the `KernelHealth` projection lives in `feed::live::convert`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CoreSysStatus {
    /// MoonBot process CPU, % of the whole machine.
    pub process_cpu_percent: Option<u8>,
    /// Whole-machine CPU, %. The process-vs-machine SCOPE counterpart of
    /// `process_cpu_percent`, not a time average.
    pub system_cpu_percent: Option<u8>,
    /// MoonBot process memory, MB (decimal).
    pub used_memory_mb: Option<u16>,
    /// Free physical memory on the machine, MB (decimal).
    pub free_physical_memory_mb: Option<u16>,
    /// Logical CPU count of the machine.
    pub logical_cpu_count: Option<u8>,
    /// Smoothed client↔core UDP round-trip time, ms — the transport ping between this terminal and
    /// the core (NOT the core→exchange path). `None` until the core has measured one Ping response.
    /// This is moonproto's `core_round_trip_ms`; MoonBot's displayed one-way ping is half of it.
    ///
    /// NOT the same figure as [`CoreStartupStatus::round_trip_ms`], which shares this field name on
    /// a sibling struct also reachable from `CoreData`: this one is LIVE and refreshes on every
    /// Ping, that one is STARTUP-scoped and freezes once the core reaches
    /// [`CoreStartupState::Ready`].
    pub round_trip_ms: Option<u32>,
    /// Smoothed core→exchange order-API latency, ms — how long the core's real order requests take
    /// to the exchange (NOT a standalone REST ping, and NOT the transport ping above). `None` until
    /// the core has an order sample. This is moonproto's `order_api_latency_ms`.
    pub order_api_latency_ms: Option<u16>,
    /// Receipt time of the last `KernelHealth`, unix ms (`0` — none yet).
    pub updated_ms: i64,
}

impl CoreSysStatus {
    /// Whether the telemetry metrics are equal, ignoring the `updated_ms` receipt stamp.
    /// The store bumps `sys_rev` only on a metric change; `updated_ms` advancing on
    /// every Ping must not churn the panel's repaint signature (the panel keeps
    /// "Updated" live via its own 1 Hz tick).
    pub fn metrics_eq(&self, other: &Self) -> bool {
        self.process_cpu_percent == other.process_cpu_percent
            && self.system_cpu_percent == other.system_cpu_percent
            && self.used_memory_mb == other.used_memory_mb
            && self.free_physical_memory_mb == other.free_physical_memory_mb
            && self.logical_cpu_count == other.logical_cpu_count
            && self.round_trip_ms == other.round_trip_ms
            && self.order_api_latency_ms == other.order_api_latency_ms
    }
}

/// Phase of one core's startup, mirrored from moonproto's `StartupState`.
///
/// Moonproto's enum is `#[non_exhaustive]`, so the projection must handle a phase this build has
/// never heard of. Folding such a phase into a known one would render a guess as fact, so it lands
/// on [`Self::Unknown`] and the UI shows "no data" rather than inventing progress. Moonproto-free —
/// the projection lives in `feed::live::convert`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CoreStartupState {
    /// Transport handshake has not completed yet. Mirrors moonproto's own default.
    #[default]
    Connecting,
    /// Transport is authorized and the mandatory init spine is running.
    Initializing,
    /// Init completed and the first active-library snapshot was published.
    Ready,
    /// The transport dropped while startup was still in progress.
    Reconnecting,
    /// Startup ended with a connect error.
    Failed,
    /// The application explicitly stopped the client.
    Disconnected,
    /// A phase a newer MoonProto reports and this build does not recognise.
    Unknown,
}

impl CoreStartupState {
    /// Whether the phase is one startup can never leave — the point where the upstream snapshot
    /// freezes and every counter behind it stops moving.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Ready | Self::Failed | Self::Disconnected)
    }
}

/// One step of the mandatory startup sequence, mirrored from moonproto's `InitStep`.
///
/// The variants are ordered as the core runs them; [`INIT_STEPS`] is that order as an array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CoreInitStep {
    /// Verify protocol/core compatibility and read core identity.
    BaseCheck = 0,
    /// Verify account access and read account authorization metadata.
    AuthCheck = 1,
    /// Load the canonical market list and market metadata.
    GetMarketsList = 2,
    /// Load current prices and the server's market-index mapping.
    UpdateMarketsList = 3,
    /// Load the typed strategy settings schema.
    StrategySchema = 4,
    /// Queue initial domain refreshes and requested subscriptions.
    PostInitFlush = 5,
    /// Publish the first complete active-library snapshot.
    StartupSnapshot = 6,
    /// Publish startup events queued with that snapshot.
    StartupEvents = 7,
}

/// Every startup step, in the order the core runs them. Private: it exists to derive
/// [`INIT_STEPS_TOTAL`] from ONE list rather than a hand-written number, and nothing outside this
/// module needs the order itself.
const INIT_STEPS: [CoreInitStep; 8] = [
    CoreInitStep::BaseCheck,
    CoreInitStep::AuthCheck,
    CoreInitStep::GetMarketsList,
    CoreInitStep::UpdateMarketsList,
    CoreInitStep::StrategySchema,
    CoreInitStep::PostInitFlush,
    CoreInitStep::StartupSnapshot,
    CoreInitStep::StartupEvents,
];

/// How many steps a full startup runs — the denominator of the progress figure.
///
/// This is OUR constant because MoonProto exposes no readable one: both `InitStep::COUNT` and
/// `InitStep::ALL` are private to that crate. So a MoonProto that grows a ninth step leaves this
/// stale with nothing to detect it, which is why the UI clamps the denominator up to the observed
/// completed count instead of trusting this number alone.
pub const INIT_STEPS_TOTAL: u8 = INIT_STEPS.len() as u8;

/// Startup progress and channel measurements for one core, from moonproto's `StartupStatus`.
///
/// Polled from the feed thread rather than pushed, because MoonProto publishes it as a passive
/// snapshot behind a lock at its own bounded rate. It FREEZES once [`Self::state`] reaches a
/// terminal phase, so after a successful startup `elapsed_ms` means "how long this core took to
/// come up", not a running clock — the UI must word it in the past tense. Moonproto-free — the
/// projection lives in `feed::live::convert`, and the store gates the Core Status panel with
/// `startup_rev` through [`Self::progress_eq`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CoreStartupStatus {
    /// Overall startup phase.
    pub state: CoreStartupState,
    /// Step the core is running now. `None` while handshaking and once init is done.
    pub current_step: Option<CoreInitStep>,
    /// Bit per completed [`CoreInitStep`], indexed by its discriminant. A mask rather than a count
    /// so the detail surface can name WHICH steps are done, not just how many.
    pub completed_mask: u16,
    /// Wall-clock time since startup began, ms. Frozen at a terminal phase.
    pub elapsed_ms: u64,
    /// Unique Sliced payload bytes accepted while starting this core.
    pub received_sliced_bytes: u64,
    /// Recent useful Sliced payload receive rate, bytes/s.
    pub receive_rate_bytes_per_sec: u64,
    /// Incomplete Sliced datagrams currently being reassembled.
    pub active_sliced_transfers: u16,
    /// Unique Sliced blocks accepted since startup began.
    pub received_sliced_blocks: u64,
    /// Duplicate Sliced blocks observed since startup began — the channel-loss tell: a rising
    /// count means retransmission, not a slow core.
    pub duplicate_sliced_blocks: u64,
    /// Unique blocks present in the currently active Sliced datagrams.
    pub active_received_blocks: u32,
    /// Total blocks advertised by the currently active Sliced datagrams.
    pub active_expected_blocks: u32,
    /// Milliseconds since the last unique Sliced block, `None` before the first one arrives.
    pub idle_for_ms: Option<u64>,
    /// Whole-step retries performed for the current step after a timeout or failure.
    pub current_step_retries: u32,
    /// Whole-step retries performed across the whole init attempt.
    pub total_init_retries: u32,
    /// Reconnect episodes observed while this core was starting.
    pub reconnect_count: u32,
    /// Local UDP port currently owned by this terminal's client socket.
    ///
    /// This is not the core's listening port and not necessarily the public port seen through NAT.
    pub current_local_udp_port: Option<u16>,
    /// Physical UDP datagrams successfully sent on the current local socket.
    pub current_port_sent_packets: u64,
    /// Physical UDP datagrams received on the current local socket, before protocol validation.
    pub current_port_received_packets: u64,
    /// Local UDP port closed by the latest automatic reconnect.
    pub previous_local_udp_port: Option<u16>,
    /// Final physical send count captured before the latest local-port change.
    pub sent_packets_before_last_port_change: u64,
    /// Final physical receive count captured before the latest local-port change.
    pub received_packets_before_last_port_change: u64,
    /// Automatic local-socket replacements observed while this client was starting.
    pub local_port_change_count: u32,
    /// Last full client↔core UDP round-trip reported by Ping, ms.
    ///
    /// NOT the same figure as [`CoreSysStatus::round_trip_ms`], which shares this field name on a
    /// sibling struct also reachable from `CoreData`: that one is LIVE and refreshes on every Ping,
    /// this one is STARTUP-scoped and freezes with the rest of this snapshot.
    pub round_trip_ms: Option<u32>,
    /// Last path MTU reported by Ping, bytes.
    pub path_mtu_bytes: Option<u16>,
    /// Server estimate of delivered server-to-client traffic, 0..=100 %.
    pub downlink_delivery_percent: Option<u8>,
}

impl CoreStartupStatus {
    /// Number of startup steps completed so far.
    pub fn completed_count(&self) -> u32 {
        self.completed_mask.count_ones()
    }

    /// Completed steps and the total to show them against, as one pair.
    ///
    /// The total is clamped UP to what was actually observed. MoonProto exposes no readable step
    /// count — both `InitStep::COUNT` and `InitStep::ALL` are private to that crate — so
    /// [`INIT_STEPS_TOTAL`] is OUR constant, and a MoonProto that grows a ninth step would leave it
    /// stale with nothing to detect it. The clamp guarantees the pair can never read `9/8`, nor
    /// claim completion while work visibly continues.
    ///
    /// ONE owner on purpose: the Core Status startup cell, its hover, and the connection verdict all
    /// show this pair, and a clamp re-derived in three places is a clamp that drifts.
    ///
    /// Returns:
    ///     `(done, total)`, with `done <= total` always.
    pub fn progress_pair(&self) -> (u8, u8) {
        let done = self.completed_count().min(u8::MAX as u32) as u8;
        (done, INIT_STEPS_TOTAL.max(done))
    }

    /// Whether the two snapshots say the same thing about PROGRESS, ignoring churn a reader can
    /// neither see nor act on.
    ///
    /// The store bumps `startup_rev` only when this is false, so it decides how often a starting
    /// core repaints the panel. Three clauses, each load-bearing:
    ///
    /// 1. Two snapshots in the SAME terminal phase are always equal. The upstream snapshot freezes
    ///    there, so nothing behind it can change — this is what makes a config of two hundred
    ///    already-started cores cost zero bumps forever.
    /// 2. `elapsed_ms` compares at whole-second resolution. Excluding it entirely would freeze a
    ///    starting core's clock whenever nothing else on that core is talking; comparing it exactly
    ///    would bump at poll rate.
    /// 3. Everything else compares exactly.
    pub fn progress_eq(&self, other: &Self) -> bool {
        if self.state == other.state && self.state.is_terminal() {
            return true;
        }
        self.state == other.state
            && self.current_step == other.current_step
            && self.completed_mask == other.completed_mask
            && self.elapsed_ms / 1000 == other.elapsed_ms / 1000
            && self.received_sliced_bytes == other.received_sliced_bytes
            && self.receive_rate_bytes_per_sec == other.receive_rate_bytes_per_sec
            && self.active_sliced_transfers == other.active_sliced_transfers
            && self.received_sliced_blocks == other.received_sliced_blocks
            && self.duplicate_sliced_blocks == other.duplicate_sliced_blocks
            && self.active_received_blocks == other.active_received_blocks
            && self.active_expected_blocks == other.active_expected_blocks
            && self.idle_for_ms == other.idle_for_ms
            && self.current_step_retries == other.current_step_retries
            && self.total_init_retries == other.total_init_retries
            && self.reconnect_count == other.reconnect_count
            && self.current_local_udp_port == other.current_local_udp_port
            && self.current_port_sent_packets == other.current_port_sent_packets
            && self.current_port_received_packets == other.current_port_received_packets
            && self.previous_local_udp_port == other.previous_local_udp_port
            && self.sent_packets_before_last_port_change
                == other.sent_packets_before_last_port_change
            && self.received_packets_before_last_port_change
                == other.received_packets_before_last_port_change
            && self.local_port_change_count == other.local_port_change_count
            && self.round_trip_ms == other.round_trip_ms
            && self.path_mtu_bytes == other.path_mtu_bytes
            && self.downlink_delivery_percent == other.downlink_delivery_percent
    }
}

/// Why one core's connection attempt ended, as FACTS rather than a sentence.
///
/// MoonProto reports the failure as a typed `ConnectError`, and until this type existed the feed
/// stringified it on arrival — so the one place that knew WHICH stage failed threw that knowledge
/// away, and the surface the user looks at could only show `Connection 0/1`. This carries the fact
/// across the crate boundary instead, because `moon-core` structurally cannot localize
/// (`rust_i18n::i18n!` is declared in the UI crate) and a pre-built String can never be translated.
///
/// It is a RECORD of one finished attempt, not live state: everything in it is captured at the
/// failure site and never updated afterwards. [`crate::session::CoreData`] retains it ACROSS the
/// backoff retry on purpose — the reason the previous attempt died is the only explanation of the
/// retry the user is watching — and clears it only when the core reaches `ConnStatus::Ready`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnFault {
    /// What ended the attempt.
    pub kind: ConnFaultKind,
    /// What the core managed to say about itself before the attempt died.
    pub identity: CoreIdentityFacts,
    /// Startup snapshot CAPTURED AT THE FAILURE SITE.
    ///
    /// Not read back from the store later: the feed polls `startup_status()` on its own cadence and
    /// the retained snapshot can be up to a poll interval stale at the moment of failure, while
    /// `CoreData::begin_connection_attempt` resets it outright on the next attempt. A fault that
    /// pointed at the live snapshot would therefore describe a different attempt than the one it
    /// explains.
    pub startup: CoreStartupStatus,
}

/// The distinguishable ways a connection attempt can end — mostly mirrored from MoonProto's
/// `ConnectError` and `LifecycleEvent::BindFailed`, plus the ones this terminal decides for itself
/// when the library reports no failure at all ([`Self::StartupStalled`]).
///
/// Deliberately close to the wire shape rather than to the user-facing classes: turning these into
/// causes is a decision with its own tests, and it belongs in the layer that can also word them.
/// Anything this enum cannot name is carried raw rather than folded into a neighbouring variant —
/// naming the stage honestly beats guessing the cause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnFaultKind {
    /// THIS machine could not bind its own UDP socket — repeated 200-port sweeps failed.
    ///
    /// From `LifecycleEvent::BindFailed`, so it says nothing about the core: the usual causes are a
    /// VPN, a local firewall, or exhausted ephemeral ports on the terminal's own host.
    LocalBindFailed {
        /// Complete 200-port bind sweeps that failed in a row.
        consecutive_failures: u32,
    },
    /// The terminal tore the attempt down itself.
    ///
    /// Merges `ConnectError::Canceled` (an explicit disconnect) and `InitError::SendChannelClosed`
    /// (the client loop is gone). Both mean "we stopped it", never "the core is bad".
    Aborted,
    /// The transport handshake never completed within its deadline.
    ConnectTimedOut {
        /// The deadline that expired, ms.
        timeout_ms: u64,
    },
    /// The transport was never authorized, so init could not start.
    NotAuthenticated,
    /// A mandatory init step exceeded its deadline.
    InitStepTimedOut {
        /// The step, when this build recognises its name.
        step: Option<CoreInitStep>,
        /// MoonProto's own step name, always kept: it is what an unrecognised step reports instead
        /// of a guessed cause.
        raw_step: String,
    },
    /// THIS terminal gave up on a startup that stopped advancing — see
    /// `feed::live::startup_watchdog`.
    ///
    /// Separate from [`Self::InitStepTimedOut`] although both concern a step, because that one is
    /// MoonProto reporting a step it could not finish, and the verdict reads the earliest two steps
    /// as evidence about the CORE — a wrong build on `BaseCheck`, a refused account on `AuthCheck`.
    /// A stall is evidence about neither: nothing answered, and which step it happened to be
    /// sitting on says nothing about why. Folding it into that kind would put a confident wrong
    /// diagnosis on the most likely stall steps.
    ///
    /// Carries nothing: where it stopped is already in [`ConnFault::startup`], frozen at the same
    /// instant, and a second copy is only something to keep in sync.
    StartupStalled,
    /// A mandatory init step returned a server-side error.
    InitStepFailed {
        /// The step, when this build recognises its name.
        step: Option<CoreInitStep>,
        /// MoonProto's own step name, always kept.
        raw_step: String,
        /// The CORE's own error text, passed through untouched.
        message: String,
    },
}

/// What the core told this terminal about itself during `BaseCheck`.
///
/// The only evidence a version verdict may rest on, and it is deliberately a WEAK one.
///
/// # Why every field here is positive evidence or nothing
///
/// MoonProto publishes the snapshot backing `MoonClient::server_info` only once the WHOLE init
/// sequence reaches Ready: `publish_snapshot_profiled` is gated on `startup.is_none()`, and a
/// FAILED attempt publishes nothing at all. So on the very attempt this struct describes, that
/// accessor normally returns the all-empty default — which is byte-identical to the empty payload
/// an OLD core legitimately answers `BaseCheck` with. "Nothing has been published yet" and "a core
/// too old to say more" are therefore INDISTINGUISHABLE from the payload, and they demand opposite
/// verdicts.
///
/// The consequence is stated here rather than worked around: a POPULATED field is proof the payload
/// was really read, and an empty one proves nothing whatsoever. Nothing downstream may claim an age
/// except from a field that is populated — `feed::conn_verdict` enforces that. There is no
/// minimum-version constant anywhere in this workspace, and nothing here invents one.
///
/// Combining these fields with the startup step MASK to reconstruct "answered" is specifically
/// wrong and was specifically tried: the two come from different publication paths, so on a failure
/// after `BaseCheck` the mask says the step completed while the payload is still defaulted, and the
/// pair reads as "answered, and reported no version" for a perfectly current core.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CoreIdentityFacts {
    /// Whether the answer carried the core's stable identity id (`ServerInfo::has_identity`).
    ///
    /// The cheapest proof that a real `BaseCheck` payload was read rather than defaulted.
    pub has_identity: bool,
    /// MoonBot version number the core reported.
    pub server_version: Option<u32>,
    /// MoonProto protocol version the core reported.
    ///
    /// `None` means EITHER a core older than the field OR a payload that never arrived; only the
    /// other fields tell those apart.
    pub moonproto_version: Option<u32>,
}

impl CoreIdentityFacts {
    /// Fold these facts into a hasher without allocating.
    ///
    /// The Settings tab recomputes a repaint signature over every core on every backend notify, so
    /// the obvious `format!("{self:?}").hash(..)` would build and discard one string per core per
    /// notify — at two hundred cores, purely to produce a few bytes of hash. Everything here is a
    /// primitive, so it folds in directly.
    ///
    /// Args:
    ///     h: The hasher the caller is accumulating into.
    ///
    /// Returns:
    ///     Nothing; the hasher advances in place.
    pub fn hash_into<H: std::hash::Hasher>(&self, h: &mut H) {
        use std::hash::Hash;
        self.has_identity.hash(h);
        self.server_version.hash(h);
        self.moonproto_version.hash(h);
    }
}
