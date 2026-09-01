//! Wording for a core's connection verdict: the localized reason and the next step.
//!
//! The decision itself is `moon_core::feed::diagnose` — a pure classification over the retained
//! lifecycle facts, in the crate that owns them. This module is the other half, and it lives in the
//! UI binary for one structural reason: `rust_i18n::i18n!` is declared in `main.rs`, so `t!` cannot
//! be called from `moon-core` at all.
//!
//! It sits at the top level rather than inside a panel because FIVE surfaces render this verdict —
//! the Core Status flat table, the Core Status by-IP tree, the Connections tab, the shell status
//! bar, and the Auto workspace rail — and none of them should import another panel's internals.
//!
//! Everything here is pure and GPUI-free, the same discipline
//! `panels/core_status/startup.rs` follows and for the same reason: `moon-ui-gpui` is a binary
//! crate with no `[lib]`, so a decision that needs a real test has to be a free function first.
//! [`fault_tooltip`] is DERIVED from [`fault_facts`] rather than written a second time, so a fact
//! added there appears in the hover without a second edit and the two can never disagree.

use moon_core::config::{ServerConfig, TransportVersion, seeded_transport};
use moon_core::feed::{Diagnosis, FailureClass, SiblingOutcome, suggest_alternate_mode};
use moon_core::session::{CoreId, CoreInitStep};
use rust_i18n::t;

#[cfg(test)]
mod tests;

/// One labelled line of the verdict hover.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct FaultFact {
    /// Localized row label.
    pub(crate) label: String,
    /// Localized row value.
    pub(crate) value: String,
}

/// Format seconds with one decimal, e.g. `8.4`.
///
/// Shared with the Core Status startup hover, which prints elapsed times on the same rows: two
/// copies of the same rounding would eventually disagree about the same core's clock.
pub(crate) fn secs(ms: u64) -> String {
    format!("{:.1}", ms as f64 / 1000.0)
}

/// Format a byte count as a compact decimal figure, e.g. `4.2 MB`.
///
/// Shared for the same reason as [`secs`]: the verdict and the startup hover quote byte figures
/// from the SAME channel, and two scalings of one number is a bug waiting to be reported.
pub(crate) fn bytes(value: u64) -> String {
    const KB: f64 = 1000.0;
    const MB: f64 = 1000.0 * KB;
    let v = value as f64;
    if v >= MB {
        format!("{:.1} {}", v / MB, t!("core_status.startup.mb"))
    } else if v >= KB {
        format!("{:.0} {}", v / KB, t!("core_status.startup.kb"))
    } else {
        format!("{} {}", value, t!("core_status.startup.b"))
    }
}

/// Localized name of one startup step.
///
/// The verdict and the Core Status startup hover both name these eight steps: a second translated
/// set would drift, and one step would end up called two things on two surfaces the user reads side
/// by side.
///
/// Args:
///     step: Initialization step whose localized label is needed.
///
/// Returns:
///     The active-locale label shared by startup and connection-verdict surfaces.
pub(crate) fn step_label(step: CoreInitStep) -> String {
    match step {
        CoreInitStep::BaseCheck => t!("core_status.startup.step.base_check"),
        CoreInitStep::AuthCheck => t!("core_status.startup.step.auth_check"),
        CoreInitStep::GetMarketsList => t!("core_status.startup.step.markets_list"),
        CoreInitStep::UpdateMarketsList => t!("core_status.startup.step.markets_update"),
        CoreInitStep::StrategySchema => t!("core_status.startup.step.strategy_schema"),
        CoreInitStep::PostInitFlush => t!("core_status.startup.step.post_init_flush"),
        CoreInitStep::StartupSnapshot => t!("core_status.startup.step.snapshot"),
        CoreInitStep::StartupEvents => t!("core_status.startup.step.events"),
    }
    .to_string()
}

/// Localized name of one init step, or a dash when the core is between steps this build can name.
///
/// Args:
///     step: The step, when one is known.
///
/// Returns:
///     The localized step name, or an ASCII unavailable marker.
fn step_label_opt(step: Option<CoreInitStep>) -> String {
    match step {
        Some(step) => step_label(step),
        None => "-".to_string(),
    }
}

/// The two-word cell form of a verdict, for a narrow column.
///
/// Short enough to fit beside a core name at the narrowest dock width; the hover carries the rest.
///
/// Args:
///     class: The classified failure.
///
/// Returns:
///     A short localized label.
pub(crate) fn fault_short(class: &FailureClass) -> String {
    match class {
        FailureClass::LocalPort { .. } => t!("core_status.fault.short.local_port"),
        FailureClass::NoResponse {
            packets_received: 0,
            bytes: 0,
            ..
        } => t!("core_status.fault.short.no_response"),
        FailureClass::NoResponse { .. } => t!("core_status.fault.short.unparsed"),
        FailureClass::Access { .. } => t!("core_status.fault.short.access"),
        FailureClass::CoreUnidentified { .. } => t!("core_status.fault.short.unidentified"),
        FailureClass::Syncing { stalled: false, .. } => t!("core_status.fault.short.syncing"),
        FailureClass::Syncing { stalled: true, .. } => t!("core_status.fault.short.stalled"),
        FailureClass::Aborted => t!("core_status.fault.short.aborted"),
        FailureClass::Undetermined { .. } => t!("core_status.fault.short.unknown"),
    }
    .to_string()
}

/// The localized REASON for a verdict: what the terminal observed, in one sentence.
///
/// Several classes fork here rather than in `moon-core`, because the fork is about WORDING, not
/// about the decision: `NoResponse` says "not one byte" only when it truly observed none, and
/// `Access` / `CoreUnidentified` quote the core's own error text when the core sent one instead of
/// timing out. Claiming silence that was not observed is the single easiest way to send a user
/// after the wrong problem.
fn reason(class: &FailureClass) -> String {
    match class {
        FailureClass::LocalPort { attempts } => {
            t!("core_status.fault.reason.local_port", n = attempts).to_string()
        }
        FailureClass::NoResponse {
            packets_received: 0,
            bytes: 0,
            elapsed_ms,
            ..
        } => t!("core_status.fault.reason.silent", t = secs(*elapsed_ms)).to_string(),
        FailureClass::NoResponse {
            packets_received,
            bytes: 0,
            elapsed_ms,
            ..
        } => t!(
            "core_status.fault.reason.unparsed",
            n = packets_received,
            t = secs(*elapsed_ms)
        )
        .to_string(),
        FailureClass::NoResponse {
            bytes: seen,
            elapsed_ms,
            ..
        } => t!(
            "core_status.fault.reason.partial",
            bytes = bytes(*seen),
            t = secs(*elapsed_ms)
        )
        .to_string(),
        FailureClass::Access { refused: true, .. } => {
            t!("core_status.fault.reason.access_refused").to_string()
        }
        FailureClass::Access {
            message: Some(msg), ..
        } => t!("core_status.fault.reason.access_failed", msg = msg).to_string(),
        FailureClass::Access { .. } => t!("core_status.fault.reason.access_timeout").to_string(),
        FailureClass::CoreUnidentified { message: Some(msg) } => {
            t!("core_status.fault.reason.unidentified_failed", msg = msg).to_string()
        }
        FailureClass::CoreUnidentified { .. } => {
            t!("core_status.fault.reason.unidentified_timeout").to_string()
        }
        FailureClass::Syncing {
            step,
            done,
            total,
            elapsed_ms,
            stalled,
        } => t!(
            if *stalled {
                "core_status.fault.reason.stalled"
            } else {
                "core_status.fault.reason.syncing"
            },
            step = step_label_opt(*step),
            done = done,
            total = total,
            t = secs(*elapsed_ms)
        )
        .to_string(),
        FailureClass::Aborted => t!("core_status.fault.reason.aborted").to_string(),
        FailureClass::Undetermined { .. } => t!("core_status.fault.reason.unknown").to_string(),
    }
}

/// The localized NEXT STEP for a verdict — the half that makes this feature worth having.
///
/// Every class has one, including the honest fallback, because a reason without an action leaves
/// the user exactly where `Connection 0/1` left them. The wording speaks to what the terminal
/// CANNOT see: it cannot inspect a hoster firewall, a Windows network profile or a referral status,
/// so it names those as the places to look rather than claiming to have checked them.
///
/// Args:
///     class: The classified failure.
///     suggested_mode: A fleet-wide "try this mode instead" suggestion
///         ([`fleet_mode_suggestion`]), when the evidence supports one. Only ever appended to a
///         [`FailureClass::NoResponse`] step: every other class already reached a point in the
///         handshake past which the transport mode is proven to work.
///
/// Returns:
///     The localized next-step text, with the evidence-backed mode suggestion only for
///     `NoResponse` failures.
fn next_step(class: &FailureClass, suggested_mode: Option<TransportVersion>) -> String {
    let base = match class {
        FailureClass::LocalPort { .. } => t!("core_status.fault.next.local_port"),
        FailureClass::NoResponse {
            packets_received: 0,
            bytes: 0,
            ..
        } => t!("core_status.fault.next.no_response"),
        FailureClass::NoResponse { .. } => t!("core_status.fault.next.unparsed"),
        FailureClass::Access { .. } => t!("core_status.fault.next.access"),
        FailureClass::CoreUnidentified { .. } => t!("core_status.fault.next.unidentified"),
        FailureClass::Syncing { stalled: false, .. } => t!("core_status.fault.next.syncing"),
        FailureClass::Syncing { stalled: true, .. } => t!("core_status.fault.next.stalled"),
        FailureClass::Aborted => t!("core_status.fault.next.aborted"),
        FailureClass::Undetermined { .. } => t!("core_status.fault.next.unknown"),
    }
    .to_string();
    match (class, suggested_mode) {
        (FailureClass::NoResponse { .. }, Some(mode)) => format!(
            "{base}. {}",
            t!("core_status.fault.next.try_mode", mode = mode.label())
        ),
        _ => base,
    }
}

/// Resolve a "try another transport mode" suggestion for one core from its siblings.
///
/// The only fleet-level fact this feature can honestly add: per-core, `moon_core::feed::diagnose`
/// cannot separate a blocked route from a mode mismatch, but comparing every core THIS terminal
/// manages can. See `moon_core::feed::suggest_alternate_mode` for the comparison itself and its
/// honesty rule; this wrapper only resolves each configured server's effective mode
/// (`config::seeded_transport`) and excludes the failing core from its own sibling list.
///
/// Args:
///     core_id: The failing core.
///     servers: Every configured server (the live, saved config — not a Settings draft).
///     is_ready: Whether a core (by id) is currently `Ready`.
///
/// Returns:
///     The one alternate mode to suggest, or `None` when the fleet gives no clean signal, or when
///     this core's own mode cannot be resolved at all (no key, no stored mode).
pub(crate) fn fleet_mode_suggestion(
    core_id: CoreId,
    servers: &[ServerConfig],
    is_ready: impl Fn(CoreId) -> bool,
) -> Option<TransportVersion> {
    let failing_mode = servers
        .iter()
        .find(|s| s.id == core_id)
        .and_then(|s| seeded_transport(s.transport, s.key.expose()))?;
    let siblings = servers.iter().filter(|s| s.id != core_id).filter_map(|s| {
        let mode = seeded_transport(s.transport, s.key.expose())?;
        Some(SiblingOutcome {
            mode,
            connected: is_ready(s.id),
        })
    });
    suggest_alternate_mode(failing_mode, siblings)
}

/// Assemble the verdict hover's facts, in reading order.
///
/// Reason and next step are unconditional; the stage row appears only when the terminal could not
/// determine a cause and therefore owes the raw name instead; the core row appears only when the
/// core actually identified itself as older than this build. A row with nothing to say is OMITTED
/// rather than rendered as a dash, so every visible line carries information.
///
/// Args:
///     d: The verdict from `moon_core::feed::diagnose`.
///     suggested_mode: A fleet-wide "try this mode instead" suggestion ([`fleet_mode_suggestion`]),
///         when the evidence supports one.
///
/// Returns:
///     The labelled lines to show. Never empty.
pub(crate) fn fault_facts(
    d: &Diagnosis,
    suggested_mode: Option<TransportVersion>,
) -> Vec<FaultFact> {
    let mut out = Vec::new();
    let mut push = |label: String, value: String| out.push(FaultFact { label, value });

    // "reconnecting" rides the reason rather than taking a row of its own: it qualifies what the
    // user is seeing right now, and a separate line would read as a second finding.
    let mut why = reason(&d.class);
    if d.retrying {
        why = format!("{why} ({})", t!("core_status.fault.retrying"));
    }
    push(t!("core_status.fault.f.reason").to_string(), why);
    push(
        t!("core_status.fault.f.next").to_string(),
        next_step(&d.class, suggested_mode),
    );
    if let FailureClass::Undetermined { raw_stage } = &d.class
        && !raw_stage.is_empty()
    {
        push(
            t!("core_status.fault.f.stage").to_string(),
            raw_stage.clone(),
        );
    }
    if d.legacy_core {
        push(
            t!("core_status.fault.f.core").to_string(),
            match d.server_version {
                // Same dotted build the Core-Status column prints. A fault hover that said "769"
                // beside a column saying "7.69" would read as two different facts about one core.
                Some(v) => t!(
                    "core_status.fault.core.legacy_named",
                    server = moon_core::util::fmt::core_build(v)
                )
                .to_string(),
                None => t!("core_status.fault.core.legacy_silent").to_string(),
            },
        );
    }
    out
}

/// Render the assembled facts as the hover body, one `label: value` per line.
///
/// Args:
///     facts: The lines from [`fault_facts`].
///
/// Returns:
///     The hover text.
pub(crate) fn fault_tooltip(facts: &[FaultFact]) -> String {
    facts
        .iter()
        .map(|f| format!("{}: {}", f.label, f.value))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One-line form for the status bar, where each core gets a single line.
///
/// Reason and next step joined, because the status bar is where a user who has NOT opened any panel
/// first learns something is wrong — dropping the action there would leave the most-read surface
/// carrying the least useful half.
///
/// Args:
///     d: The verdict from `moon_core::feed::diagnose`.
///     suggested_mode: A fleet-wide "try this mode instead" suggestion ([`fleet_mode_suggestion`]),
///         when the evidence supports one.
///
/// Returns:
///     The single-line verdict.
pub(crate) fn fault_line(d: &Diagnosis, suggested_mode: Option<TransportVersion>) -> String {
    let mut why = reason(&d.class);
    if d.retrying {
        why = format!("{why} ({})", t!("core_status.fault.retrying"));
    }
    format!("{why} \u{2014} {}", next_step(&d.class, suggested_mode))
}
