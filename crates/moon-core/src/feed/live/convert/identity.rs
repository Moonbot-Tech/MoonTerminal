//! Connection fault and API-key identity projections.

use super::settings::init_step_by_name;
use super::*;

/// Project what `BaseCheck` reported into the moonproto-free identity facts.
///
/// A pure copy of what the payload held, with NO inference added. In particular nothing here
/// derives "the core answered" from the startup step mask: the mask and the payload come from two
/// different publication paths, and MoonProto refreshes the payload's snapshot only at Ready
/// (`publish_snapshot_profiled` is gated on `startup.is_none()`, and a failed attempt publishes
/// nothing at all), so on a failing attempt the mask can say `BaseCheck` completed while the payload
/// is still the empty default. Combining the two manufactures "answered, and reported no version"
/// out of a core that has merely not published yet, which is a confident lie about the user's core.
/// [`CoreIdentityFacts`] carries the full argument.
///
/// Args:
///     info: What `MoonClient::server_info` held at the failure site, if anything.
///
/// Returns:
///     The identity facts a verdict may rest on.
fn identity_facts(info: Option<moonproto::ServerInfo>) -> CoreIdentityFacts {
    let info = info.unwrap_or_default();
    CoreIdentityFacts {
        has_identity: info.has_identity(),
        server_version: info.server_version,
        moonproto_version: info.moonproto_version,
    }
}

/// Project MoonProto's typed startup failure into the moonproto-free [`ConnFault`].
///
/// This is the ONLY place the typed `ConnectError` is read. It is deliberately a projection rather
/// than a classification: it renames the wire shape and nothing more, so the decision about WHICH
/// user-facing cause a shape means — and the wording for it — stays in the layer that can localize.
///
/// Args:
///     error: MoonProto's typed failure for this connection attempt.
///     info: `BaseCheck` result, when the core got that far.
///     startup: Startup snapshot polled AT the failure site.
///
/// Returns:
///     The fault record retained for this core until it next reaches `Ready`.
pub(in crate::feed::live) fn conn_fault_from_proto(
    error: moonproto::ConnectError,
    info: Option<moonproto::ServerInfo>,
    startup: moonproto::StartupStatus,
) -> ConnFault {
    let kind = match error {
        // Both mean "the terminal stopped this attempt", never "the core is bad", so they carry no
        // stage: there is nothing about the core to report.
        moonproto::ConnectError::Canceled => ConnFaultKind::Aborted,
        moonproto::ConnectError::Init(moonproto::InitError::SendChannelClosed) => {
            ConnFaultKind::Aborted
        }
        moonproto::ConnectError::ConnectTimedOut { timeout } => ConnFaultKind::ConnectTimedOut {
            timeout_ms: timeout.as_millis() as u64,
        },
        moonproto::ConnectError::Init(moonproto::InitError::NotAuthenticated) => {
            ConnFaultKind::NotAuthenticated
        }
        moonproto::ConnectError::Init(moonproto::InitError::CriticalStepTimedOut(step)) => {
            ConnFaultKind::InitStepTimedOut {
                step: init_step_by_name(step),
                raw_step: step.to_string(),
            }
        }
        moonproto::ConnectError::Init(moonproto::InitError::CriticalStepFailed {
            step,
            message,
        }) => ConnFaultKind::InitStepFailed {
            step: init_step_by_name(step),
            raw_step: step.to_string(),
            message,
        },
    };
    fault(kind, info, startup_status_from_proto(startup))
}

/// Assemble one fault record.
///
/// The three producers differ only in the kind they name; identity and the frozen snapshot are the
/// same record whatever ended the attempt, and are built here so a new field on [`ConnFault`], or
/// any change to how identity is derived, is one edit rather than three.
///
/// Args:
///     kind: What ended the attempt.
///     info: `BaseCheck` result, when the core got that far.
///     startup: Startup snapshot captured at the failure site.
///
/// Returns:
///     The fault record.
fn fault(
    kind: ConnFaultKind,
    info: Option<moonproto::ServerInfo>,
    startup: CoreStartupStatus,
) -> ConnFault {
    ConnFault {
        kind,
        identity: identity_facts(info),
        startup,
    }
}

/// Build the fault for `LifecycleEvent::BindFailed` — the terminal's OWN socket, not the core's.
///
/// It takes the same identity and startup arguments as [`conn_fault_from_proto`] so both faults are
/// the same record whatever produced them; both are normally empty here, because a client that
/// cannot bind never reached the core to learn anything about it.
///
/// Args:
///     consecutive_failures: Complete 200-port bind sweeps that failed in a row.
///     info: `BaseCheck` result, when an earlier connection on this client got that far.
///     startup: Startup snapshot polled at the moment the bind failure was reported.
///
/// Returns:
///     The fault record for a local bind failure.
pub(in crate::feed::live) fn bind_fault(
    consecutive_failures: u32,
    info: Option<moonproto::ServerInfo>,
    startup: moonproto::StartupStatus,
) -> ConnFault {
    fault(
        ConnFaultKind::LocalBindFailed {
            consecutive_failures,
        },
        info,
        startup_status_from_proto(startup),
    )
}

/// Build the fault for a first startup this terminal gave up on — see
/// [`crate::feed::live::startup_watchdog`].
///
/// Carries the reason across the crate boundary as facts, so the panel words it through the
/// existing localized "stalled at this step" verdict rather than through the untranslatable
/// raw-text fallback every other `live::run` error lands on. It gets its OWN kind rather than
/// borrowing [`ConnFaultKind::InitStepTimedOut`]: see that variant's sibling for why a step nobody
/// answered must not be read as evidence about the core.
///
/// Takes the ALREADY-PROJECTED snapshot, unlike its two siblings: the caller is the startup poll,
/// which converted it one line earlier, and re-reading `startup_status()` here would describe a
/// slightly later moment than the one that decided to give up.
///
/// Args:
///     info: `BaseCheck` result, when the core got that far before stalling.
///     startup: Startup snapshot that the stall was detected on.
///
/// Returns:
///     The fault record for a stalled startup.
pub(in crate::feed::live) fn stall_fault(
    info: Option<moonproto::ServerInfo>,
    startup: CoreStartupStatus,
) -> ConnFault {
    fault(ConnFaultKind::StartupStalled, info, startup)
}

/// Build the fault for a key that could not be decoded — no client was ever built.
///
/// There is no client at this point, so there are no identity facts and no startup snapshot to
/// carry — the defaults are the honest values, and going through the shared [`fault`] assembler
/// keeps the three constructors in step.
///
/// Args:
///     empty: `true` when the field was blank after trimming; `false` when something was pasted
///         that is not a MoonBot key export.
///
/// Returns:
///     The fault record for an unreadable key.
pub(in crate::feed::live) fn key_fault(empty: bool) -> ConnFault {
    fault(
        ConnFaultKind::KeyUnparsable { empty },
        None,
        CoreStartupStatus::default(),
    )
}

/// Build the fault for a hand-typed address that could not be used — no client was ever built,
/// so, as for [`key_fault`], the defaults are the honest identity and startup values.
///
/// Args:
///     unresolved: `true` when a host name did not resolve; `false` when the field is not an
///         address at all.
///
/// Returns:
///     The fault record for an unusable endpoint override.
pub(in crate::feed::live) fn endpoint_fault(unresolved: bool) -> ConnFault {
    fault(
        ConnFaultKind::EndpointUnusable { unresolved },
        None,
        CoreStartupStatus::default(),
    )
}

/// Convert one successful `CheckAPIExpirationTime` answer into terminal state.
///
/// The day count is the core's own (`reported_days_left`), because the terminal's clock plays no
/// part in it. A legacy core that predates that field falls back to `days_until`, which compares
/// moonproto's UNNORMALIZED server-local timestamp against the local clock and can therefore be a
/// day out; that answer also carries no absolute date, so its reader ages the count instead.
///
/// An empty date field alone does NOT mean "no expiration": the parser zeroes the date whenever the
/// core's timestamp is unusable, and still returns the count beside it. Only an empty date with no
/// counting-down number is an unlimited key, and that is recorded as its own flag rather than
/// re-derived downstream — see [`api_days_and_unlimited`].
///
/// A day count outside [`SANE_DAYS`] is dropped rather than displayed — see that constant for why
/// the negative side is the narrow one.
///
/// Args:
///     expiration: The parsed expiration from `AccountEvent::ApiExpirationUpdated`.
///     now: Terminal clock used for the legacy fallback and the receipt stamp.
///
/// Returns:
///     Terminal-side expiration state for the store.
pub(in crate::feed::live) fn api_key_expiry_from_proto(
    expiration: moonproto::ApiExpirationTime,
    now: std::time::SystemTime,
) -> ApiKeyExpiry {
    let known = expiration.is_known();
    let reported = expiration.reported_days_left();
    // A CURRENT answer is the one that carries `reported_days_left`; only for those is the absolute
    // date normalized to this terminal's clock. A legacy answer's date is the core's own local
    // timestamp, which nothing corrects, so it is deliberately NOT kept: taking it would make the
    // un-normalized value the preferred source and put a core in another time zone a day out.
    let current = reported.is_some();
    let dated_days = known.then(|| expiration.days_until(now)).flatten();
    let (days_left, unlimited) = api_days_and_unlimited(known, reported, dated_days);
    let checked_ms = crate::util::unix_ms_i64_of(now);
    // The date is the third wire field and the day count is the second, so a core could answer with
    // one sane and the other not. Keep the date only while BOTH agree on being plausible — the
    // reader prefers the date, and an unchecked one would slip past the guard above.
    let at_unix = (known && current)
        .then(|| expiration.unix_seconds())
        .flatten()
        .filter(|_| days_left.is_some())
        .filter(|at_unix| {
            let days = (at_unix - crate::util::unix_ms_i64_of(now).div_euclid(1_000)) / 86_400;
            SANE_DAYS.contains(&days)
        });
    ApiKeyExpiry {
        unlimited,
        known,
        days_left,
        at_unix,
        checked_ms,
    }
}

/// Decide the day count and the unlimited flag from the three facts one answer carries.
///
/// Split out as a pure function because the shape that matters most cannot be built through
/// moonproto's public API: an answer with NO usable date but a real count. The parser produces it
/// (it zeroes the date whenever the core's timestamp is unusable, yet still returns the count
/// beside it), and gating the count on the date would render such a key as unlimited.
///
/// Unlimited is the absence of BOTH: no usable date and no count to run down. A zero count with no
/// date is exactly how the wire spells "this key does not expire", so it is not kept as a number.
///
/// Args:
///     known: Whether the answer carries a usable expiration date.
///     reported: The core's own day count, when the answer carries that field.
///     dated_days: Days derived from the date, for a legacy answer with no count field.
///
/// Returns:
///     The day count to retain, and whether the key has no expiration at all.
pub(super) fn api_days_and_unlimited(
    known: bool,
    reported: Option<i32>,
    dated_days: Option<i64>,
) -> (Option<i32>, bool) {
    // Decided BEFORE the plausibility filter: a core that sent a count said something about this
    // key's lifetime, even if the number is unusable. Deciding after would turn a rejected count
    // into "no expiry" — an infinity glyph built out of a value we threw away.
    let unlimited = !known && reported.is_none_or(|days| days == 0);
    let days_left = reported
        .map(i64::from)
        .or(dated_days)
        .filter(|days| SANE_DAYS.contains(days))
        // A zero on an answer with no date is "no expiry", not "expires today".
        .filter(|days| *days != 0 || known)
        .map(|days| days as i32);
    (days_left, unlimited)
}

/// Plausible remaining lifetime of an exchange API key, in days.
///
/// Asymmetric on purpose. Forward, a century covers any real key. Backward, only a quarter: a core
/// that is CONNECTED AND TRADING cannot be running on a key that expired years ago, so a large
/// negative count is not a fact about the key. Observed live: two connected cores answer `-1000`
/// while every other core on the same terminal reports "no expiry" — a core-side placeholder whose
/// meaning is not documented in the protocol. Rendering it as "expired" would put a red warning on
/// two healthy cores, which is worse than admitting the answer is unusable.
const SANE_DAYS: std::ops::RangeInclusive<i64> = -90..=36_500;
