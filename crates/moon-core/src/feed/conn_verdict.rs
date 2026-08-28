//! What a core's connection state MEANS, as a pure decision over the retained facts.
//!
//! One function, [`diagnose`], and the class it returns. It lives in `moon-core` rather than in a
//! panel because five separate surfaces ask the same question — the Core Status table, the Core
//! Status by-IP tree, the Connections tab, the status bar, and the Auto workspace rail — and a
//! domain decision they all share is not one panel's private detail. Everything here is a decision;
//! not one word of it is user-facing, because this crate structurally cannot localize
//! (`rust_i18n::i18n!` is declared in the UI binary). The wording lives beside `t!`, keyed off what
//! this returns.
//!
//! # The honesty rule, which is the whole design
//!
//! A cause this cannot DISTINGUISH is reported as [`FailureClass::Undetermined`] carrying the raw
//! stage name, never folded into a neighbouring class that happens to look plausible. Guessing is
//! worse than naming the stage: a user who is told "wrong key" while the real problem is a closed
//! port spends the evening regenerating keys. Every branch below therefore rests on a fact the
//! terminal actually observed, and where two causes share one observation the class SAYS so and the
//! wording names both.
//!
//! In particular there is no "the core is too old" CLASS. Nothing the terminal sees proves a core's
//! age at the moment an attempt dies, and this workspace declares no minimum core version to
//! compare against. What it can sometimes observe is a core that IDENTIFIED itself and still
//! reported no protocol version — MoonProto's append-only tail truncates exactly that way on an
//! older build — so that arrives as the [`Diagnosis::legacy_core`] FLAG beside whatever actually
//! failed, together with the figures the core did report. A fact, never an inference.
//!
//! That flag is deliberately hard to earn. `MoonClient::server_info` is backed by a snapshot
//! MoonProto publishes only at Ready, so on a failing attempt it is usually the all-empty default —
//! which is byte-identical to what a genuinely ancient core sends. Reading age out of that emptiness
//! would label a current core failing `AuthCheck` (the most-reported real cause: Kernel(VPS) off, or
//! an exchange account outside the referral) as "predates this terminal", and send its owner to
//! update MoonBot for an evening over a checkbox. So the flag needs a populated field, and the truly
//! ancient core simply goes unnamed. Silence about a fact beats confidence about the wrong one.

use super::{ConnFault, ConnFaultKind, ConnStatus, CoreInitStep, CoreStartupStatus};

#[cfg(test)]
mod tests;

/// The distinguishable reasons a core is not usable, each with its own next step for the user.
///
/// Ordered by the evidence they rest on, not by severity. Two classes deliberately name more than
/// one possible cause ([`Self::CoreUnidentified`], [`Self::Undetermined`]) because that is what the
/// terminal can honestly support.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureClass {
    /// THIS machine could not open its own UDP socket, so nothing was ever sent.
    ///
    /// Nothing here is about the core: a local VPN, a local firewall, or exhausted ephemeral ports.
    LocalPort {
        /// Complete bind sweeps that failed in a row.
        attempts: u32,
    },
    /// The transport handshake never completed — the port / hoster-firewall class.
    ///
    /// Physical packet counts and accepted payload bytes make the wording honest: zero inbound
    /// packets means nothing reached the socket, while inbound packets with zero accepted bytes
    /// prove the problem is above the local UDP receive boundary.
    NoResponse {
        /// Physical UDP datagrams successfully sent across the current and latest previous socket.
        packets_sent: u64,
        /// Physical UDP datagrams received before validation across those two sockets.
        packets_received: u64,
        /// Unique payload bytes accepted before the deadline expired.
        bytes: u64,
        /// The deadline that expired, ms.
        elapsed_ms: u64,
    },
    /// The core answered, but the account/authorization check did not pass — the key class.
    ///
    /// Covers a rejected transport, a timed-out `AuthCheck`, and an `AuthCheck` the core answered
    /// with an error. The three share one next step: the key must be the one exported from THIS
    /// bot, and the core must actually be in Kernel(VPS) mode — which stays unavailable while the
    /// exchange account is not under the referral.
    Access {
        /// `true` when the transport itself was never authorized, rather than the account check
        /// failing after it was.
        refused: bool,
        /// The core's own error text, when it sent one.
        message: Option<String>,
    },
    /// The core never finished identifying itself: `BaseCheck` timed out or came back an error.
    ///
    /// Deliberately NOT called "too old". `BaseCheck` is the compatibility handshake, so a failure
    /// there is consistent with a core that is not in Kernel mode, one that is busy or packet-
    /// starved, AND one that predates this protocol — and nothing in the observation separates
    /// them. The wording names the possibilities; [`Diagnosis::legacy_core`] is the only thing that
    /// may claim the age one.
    CoreUnidentified {
        /// The core's own error text, when it sent one rather than timing out.
        message: Option<String>,
    },
    /// The core is authorized and working through init, or stopped partway through it.
    ///
    /// The ordinary "connected, but no snapshot yet" state and a stall at a named later step are
    /// one class on purpose: they show the same progress figures and the same elapsed clock, and
    /// [`Self::Syncing::stalled`] is what separates "still going" from "gave up here" in the
    /// wording.
    Syncing {
        /// The step in flight, or the one it stopped at. `None` between recognised steps.
        step: Option<CoreInitStep>,
        /// Steps completed so far.
        done: u8,
        /// Steps to complete, clamped up to `done` so the pair can never read `9/8`.
        total: u8,
        /// Wall-clock time since startup began, ms.
        elapsed_ms: u64,
        /// `true` when init ENDED at this step rather than still running through it.
        stalled: bool,
    },
    /// The terminal stopped the attempt itself. Nothing for the user to do.
    Aborted,
    /// The cause could not be determined. Carries the raw stage verbatim and claims nothing.
    Undetermined {
        /// MoonProto's own stage name, or the technical failure token, or empty.
        raw_stage: String,
    },
}

/// One core's connection verdict: the class, plus the facts a wording layer adds around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnosis {
    /// What went wrong, as far as the terminal can honestly tell.
    pub class: FailureClass,
    /// Whether a replacement attempt is already running behind this reason.
    ///
    /// The application-level reconnect loop retries retained faults except after an explicit
    /// disconnect. A `Disconnected` status means the terminal deliberately stopped the client, so
    /// saying that it is retrying would be a false promise; every other retained fault has another
    /// attempt on its way and the wording says so.
    pub retrying: bool,
    /// The core identified itself, and what it sent carried NO protocol version.
    ///
    /// The one age fact the terminal genuinely observes, and it requires POSITIVE evidence that the
    /// `BaseCheck` payload was read — an all-empty identity is indistinguishable from a snapshot
    /// MoonProto has not published yet, and claiming age from it mislabels healthy cores. It rides
    /// BESIDE the class rather than replacing it, because a core can be old AND be failing for an
    /// unrelated reason.
    pub legacy_core: bool,
    /// MoonBot version the core reported, when it reported one.
    ///
    /// Shown only beside [`Self::legacy_core`]. The protocol version deliberately does NOT ride
    /// here: whenever `legacy_core` holds, it is `None` by construction, and whenever it does not,
    /// nothing renders it — a field that can only ever carry a value no surface shows.
    pub server_version: Option<u32>,
}

/// Build the syncing class from one startup snapshot.
///
/// Args:
///     s: Retained startup snapshot that supplies progress and elapsed time.
///     step: Step currently in flight or where startup stopped.
///     stalled: Whether startup ended at that step rather than remaining in progress.
///
/// Returns:
///     The synchronization failure class with internally consistent progress figures.
fn syncing(s: &CoreStartupStatus, step: Option<CoreInitStep>, stalled: bool) -> FailureClass {
    let (done, total) = s.progress_pair();
    FailureClass::Syncing {
        step,
        done,
        total,
        elapsed_ms: s.elapsed_ms,
        stalled,
    }
}

/// Decide what to tell the user about one core, from its retained facts alone.
///
/// Total and clock-free: every input is already a snapshot, so the same arguments always produce
/// the same verdict and the function is trivially testable. Absence of a fault is meaningful rather
/// than missing — a core that merely never finished coming up produces no failure event at all, and
/// row 6 is the only thing that reports it.
///
/// Args:
///     status: The core's latest connection lifecycle state.
///     fault: Why its last attempt ended, when one has ended.
///     startup: The core's latest retained startup snapshot.
///
/// Returns:
///     The verdict to render, or `None` when there is nothing to say.
pub fn diagnose(
    status: &ConnStatus,
    fault: Option<&ConnFault>,
    startup: &CoreStartupStatus,
) -> Option<Diagnosis> {
    // A working core has nothing to explain. This gate is deliberately first and deliberately
    // independent of the store's own clearing of the fault: two guards, because a stale red verdict
    // beside a healthy core is the one failure mode that would make the whole feature untrustworthy.
    if matches!(status, ConnStatus::Ready) {
        return None;
    }

    let Some(fault) = fault else {
        // No failure was ever reported. The only thing worth saying is that the core is still
        // coming up, and only while it actually is: a `Disconnected` core with no fault carries no
        // evidence about why, and inventing one here is exactly what the honesty rule forbids.
        if !startup.state.is_terminal()
            && matches!(status, ConnStatus::Connecting | ConnStatus::Stage(_))
        {
            return Some(Diagnosis {
                class: syncing(startup, startup.current_step, false),
                retrying: false,
                legacy_core: false,
                server_version: None,
            });
        }
        // A `Failed` status with no fault behind it: `live::run` bailed on something that was not
        // a lifecycle event, so the only honest thing to show is its technical token as a stage.
        if let ConnStatus::Failed(raw) = status {
            return Some(Diagnosis {
                class: FailureClass::Undetermined {
                    raw_stage: raw.clone(),
                },
                retrying: false,
                legacy_core: false,
                server_version: None,
            });
        }
        return None;
    };

    // Read the progress figures from the fault's OWN frozen snapshot, not from the live one: the
    // live snapshot is reset when the next attempt starts, and a reason must describe the attempt
    // it explains.
    let s = &fault.startup;
    let class = match &fault.kind {
        ConnFaultKind::LocalBindFailed {
            consecutive_failures,
        } => FailureClass::LocalPort {
            attempts: *consecutive_failures,
        },
        ConnFaultKind::Aborted => FailureClass::Aborted,
        ConnFaultKind::NotAuthenticated => FailureClass::Access {
            refused: true,
            message: None,
        },
        ConnFaultKind::ConnectTimedOut { timeout_ms } => FailureClass::NoResponse {
            packets_sent: s
                .current_port_sent_packets
                .saturating_add(s.sent_packets_before_last_port_change),
            packets_received: s
                .current_port_received_packets
                .saturating_add(s.received_packets_before_last_port_change),
            bytes: s.received_sliced_bytes,
            elapsed_ms: *timeout_ms,
        },
        ConnFaultKind::InitStepTimedOut { step, raw_step } => step_class(*step, raw_step, None, s),
        // A startup this terminal gave up on. It goes straight to the stalled-sync class instead of
        // through `step_class`: nothing was reported, so the step it stopped on is a location, not
        // evidence, and the two arms that read a step AS evidence would turn silence into a
        // confident wrong cause.
        ConnFaultKind::StartupStalled => syncing(s, s.current_step, true),
        ConnFaultKind::InitStepFailed {
            step,
            raw_step,
            message,
        } => step_class(*step, raw_step, Some(message.clone()), s),
    };

    Some(Diagnosis {
        class,
        // The application-level reconnect loop runs for every failure it is TOLD about, so a
        // retained fault normally means another attempt is on its way. `Disconnected` is the one
        // exception and it is not a subtlety: every `client.disconnect()` in this crate is followed
        // by `run` returning `Ok(())`, which the outer loop treats as `break`. Telling a user that
        // a deliberately stopped core is reconnecting is the same over-claim the class table exists
        // to prevent, applied to a flag instead of to a cause.
        retrying: !matches!(status, ConnStatus::Disconnected),
        // The ONE age claim in this feature, and it fires only on POSITIVE evidence that the
        // `BaseCheck` payload was really read: a stable identity id, or a reported MoonBot version
        // (the append-only tail puts `server_version` BEFORE `moonproto_version`, so a truncated
        // tail is exactly this shape). An all-empty identity is what an unpublished snapshot looks
        // like too, so it claims nothing at all — see `CoreIdentityFacts`. The cost of that
        // honesty is a genuinely ancient core, whose empty payload is indistinguishable from
        // silence and which therefore goes unnamed; the alternative was labelling healthy cores
        // "too old" on their most common real failure.
        legacy_core: (fault.identity.has_identity || fault.identity.server_version.is_some())
            && fault.identity.moonproto_version.is_none(),
        server_version: fault.identity.server_version,
    })
}

/// Classify a failure that names an init STEP.
///
/// Split out so both the timed-out and the errored kinds route identically: which step died is the
/// evidence, and whether it timed out or answered an error only changes whether there is a message
/// to show.
///
/// Args:
///     step: The step, when this build recognises MoonProto's name for it.
///     raw_step: MoonProto's own name, kept verbatim for the undetermined fallback.
///     message: The core's own error text, when it sent one.
///     s: The startup snapshot frozen with the fault.
///
/// Returns:
///     The class for that step.
fn step_class(
    step: Option<CoreInitStep>,
    raw_step: &str,
    message: Option<String>,
    s: &CoreStartupStatus,
) -> FailureClass {
    match step {
        // The compatibility handshake never resolved. Three causes fit and nothing separates them,
        // so the class names the stage and the wording names all three.
        Some(CoreInitStep::BaseCheck) => FailureClass::CoreUnidentified { message },
        // The account check. This is the failure the Telegram reports keep describing: the core is
        // reachable and its key is fine, but Kernel(VPS) is off or the account is not under the
        // referral.
        Some(CoreInitStep::AuthCheck) => FailureClass::Access {
            refused: false,
            message,
        },
        // Past authorization: the core is talking and the terminal simply never received a
        // complete picture. Reported as a stalled sync with the step named, because that is what it
        // is — not as a distinct failure the user could act on differently.
        Some(step) => syncing(s, Some(step), true),
        // A step name this build has never heard of. Say the stage and claim nothing about it.
        None => FailureClass::Undetermined {
            raw_stage: raw_step.to_string(),
        },
    }
}
