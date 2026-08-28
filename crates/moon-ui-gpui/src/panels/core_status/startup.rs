//! Startup-telemetry presentation for both Core Status modes.
//!
//! Two surfaces over one polled snapshot: the single-cell summary the column shows, and the
//! structured hover behind it. The hover is BUILT FROM the same assembled facts the summary reads,
//! never written a second time by hand, so the two cannot drift apart — the idiom
//! `panels/report/totals.rs::{footer_facts, footer_tooltip}` established.
//!
//! Everything here is pure and GPUI-free so the decisions can be tested: `moon-ui-gpui` is a binary
//! crate with no `[lib]`, and a panel decision that needs a real test has to be a free function
//! first.

use moon_core::feed::{ConnFault, ConnStatus, Diagnosis};
use moon_core::session::{CoreStartupState, CoreStartupStatus};
use rust_i18n::t;

#[cfg(test)]
mod tests;

/// What the startup column shows for one row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum StartupCell {
    /// The core is still coming up: steps completed out of the total, and the running clock.
    Progress {
        done: u8,
        total: u8,
        elapsed_ms: u64,
    },
    /// The core finished coming up. The figure is how long that TOOK and never advances again.
    Done { elapsed_ms: u64 },
    /// Nothing meaningful to show for this row.
    Absent,
}

/// Decide what the startup column shows, from the connection state AND the polled snapshot.
///
/// Both inputs are required on purpose. The snapshot alone cannot tell a core that is genuinely
/// connecting from one that never connected at all — a row with no core data at all reports
/// `ConnStatus::Disconnected` while a default snapshot reports `Connecting` — so reading it without
/// its connection context lets the status column and this column contradict each other on the same
/// row. Taking both makes that disagreement unrepresentable rather than merely unlikely.
///
/// Args:
///     status: The row's connection state, the same value the status column renders.
///     s: The polled startup snapshot retained for this core.
///
/// Returns:
///     The cell to render.
pub(super) fn startup_cell(status: &ConnStatus, s: &CoreStartupStatus) -> StartupCell {
    // A core that is down has no startup in flight, whatever the retained snapshot still says.
    if matches!(status, ConnStatus::Failed(_) | ConnStatus::Disconnected) {
        return StartupCell::Absent;
    }
    match s.state {
        CoreStartupState::Ready => StartupCell::Done {
            elapsed_ms: s.elapsed_ms,
        },
        CoreStartupState::Connecting
        | CoreStartupState::Initializing
        | CoreStartupState::Reconnecting => {
            let (done, total) = s.progress_pair();
            StartupCell::Progress {
                done,
                total,
                elapsed_ms: s.elapsed_ms,
            }
        }
        CoreStartupState::Failed | CoreStartupState::Disconnected | CoreStartupState::Unknown => {
            StartupCell::Absent
        }
    }
}

/// Render the startup cell as text.
///
/// The finished figure carries a localized PAST-TENSE preposition rather than relying on colour.
/// After the core settles the snapshot freezes, so a bare `8.4 s` sitting beside a live CPU
/// percentage would read as a running clock; the preposition is part of the string, so it survives
/// a screenshot, every theme, and a reader who cannot see the colour.
///
/// Args:
///     cell: The decision from [`startup_cell`].
///
/// Returns:
///     Localized cell text, or an ASCII unavailable marker.
pub(super) fn startup_cell_text(cell: StartupCell) -> String {
    match cell {
        StartupCell::Progress {
            done,
            total,
            elapsed_ms,
        } => format!(
            "{} · {}",
            t!("core_status.startup.progress", done = done, total = total),
            t!(
                "core_status.startup.secs",
                t = crate::conn_diag::secs(elapsed_ms)
            ),
        ),
        StartupCell::Done { elapsed_ms } => t!(
            "core_status.startup.took",
            t = t!(
                "core_status.startup.secs",
                t = crate::conn_diag::secs(elapsed_ms)
            )
        )
        .to_string(),
        StartupCell::Absent => "-".to_string(),
    }
}

/// One labelled line of the hover surface.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct StartupFact {
    pub(super) label: String,
    pub(super) value: String,
}

/// Localized name of one startup phase.
pub(super) fn state_label(state: CoreStartupState) -> String {
    match state {
        CoreStartupState::Connecting => t!("core_status.startup.state.connecting"),
        CoreStartupState::Initializing => t!("core_status.startup.state.initializing"),
        CoreStartupState::Ready => t!("core_status.startup.state.ready"),
        CoreStartupState::Reconnecting => t!("core_status.startup.state.reconnecting"),
        CoreStartupState::Failed => t!("core_status.startup.state.failed"),
        CoreStartupState::Disconnected => t!("core_status.startup.state.disconnected"),
        CoreStartupState::Unknown => t!("core_status.startup.state.unknown"),
    }
    .to_string()
}

/// Assemble the hover's facts, in reading order.
///
/// A fact whose underlying value never arrived is OMITTED rather than rendered as a dash: a hover
/// listing `MTU: -` beside `RTT: -` teaches the reader nothing and buries the lines that do carry
/// a number. The ones that are always meaningful (phase, elapsed, received) are unconditional.
///
/// Args:
///     s: The polled startup snapshot retained for this core.
///
/// Returns:
///     The labelled lines to show, possibly empty.
pub(super) fn startup_facts(s: &CoreStartupStatus) -> Vec<StartupFact> {
    let mut out = Vec::new();
    let mut push = |label: String, value: String| out.push(StartupFact { label, value });

    push(
        t!("core_status.startup.f.phase").to_string(),
        state_label(s.state),
    );
    if let Some(step) = s.current_step {
        let (done, total) = s.progress_pair();
        push(
            t!("core_status.startup.f.step").to_string(),
            format!("{} ({done}/{total})", crate::conn_diag::step_label(step)),
        );
    }
    push(
        t!("core_status.startup.f.elapsed").to_string(),
        t!(
            "core_status.startup.secs",
            t = crate::conn_diag::secs(s.elapsed_ms)
        )
        .to_string(),
    );
    if let Some(port) = s.current_local_udp_port {
        push(
            t!("core_status.startup.f.local_port").to_string(),
            format!(
                "{} · {}",
                port,
                t!(
                    "core_status.startup.packets",
                    sent = s.current_port_sent_packets,
                    recv = s.current_port_received_packets
                )
            ),
        );
    }
    if let Some(port) = s.previous_local_udp_port {
        push(
            t!("core_status.startup.f.previous_port").to_string(),
            format!(
                "{} · {}",
                port,
                t!(
                    "core_status.startup.packets",
                    sent = s.sent_packets_before_last_port_change,
                    recv = s.received_packets_before_last_port_change
                )
            ),
        );
    }
    if s.local_port_change_count > 0 {
        push(
            t!("core_status.startup.f.port_changes").to_string(),
            s.local_port_change_count.to_string(),
        );
    }
    push(
        t!("core_status.startup.f.received").to_string(),
        format!(
            "{} ({})",
            crate::conn_diag::bytes(s.received_sliced_bytes),
            t!(
                "core_status.startup.rate",
                v = crate::conn_diag::bytes(s.receive_rate_bytes_per_sec)
            )
        ),
    );
    push(
        t!("core_status.startup.f.blocks").to_string(),
        format!(
            "{} · {} {}",
            s.received_sliced_blocks,
            t!("core_status.startup.f.dupes"),
            s.duplicate_sliced_blocks
        ),
    );
    if s.active_sliced_transfers > 0 {
        push(
            t!("core_status.startup.f.transfers").to_string(),
            format!(
                "{} ({}/{})",
                s.active_sliced_transfers, s.active_received_blocks, s.active_expected_blocks
            ),
        );
    }
    if let Some(idle) = s.idle_for_ms {
        push(
            t!("core_status.startup.f.idle").to_string(),
            t!("core_status.startup.secs", t = crate::conn_diag::secs(idle)).to_string(),
        );
    }
    if s.current_step_retries > 0 || s.total_init_retries > 0 {
        push(
            t!("core_status.startup.f.retries").to_string(),
            format!("{} · {}", s.current_step_retries, s.total_init_retries),
        );
    }
    if s.reconnect_count > 0 {
        push(
            t!("core_status.startup.f.reconnects").to_string(),
            s.reconnect_count.to_string(),
        );
    }
    if let Some(rtt) = s.round_trip_ms {
        push(
            t!("core_status.startup.f.rtt").to_string(),
            format!("{} {}", rtt, t!("core_status.ms")),
        );
    }
    if let Some(mtu) = s.path_mtu_bytes {
        push(
            t!("core_status.startup.f.mtu").to_string(),
            format!("{} {}", mtu, t!("core_status.startup.b")),
        );
    }
    if let Some(pct) = s.downlink_delivery_percent {
        push(
            t!("core_status.startup.f.delivery").to_string(),
            format!("{pct}%"),
        );
    }
    out
}

/// Render the assembled facts as the hover text.
///
/// Derived FROM [`startup_facts`] rather than composed independently, so a fact added there appears
/// here without a second edit, and the two can never disagree about what the row reports.
///
/// Args:
///     facts: The lines from [`startup_facts`].
///
/// Returns:
///     The hover body, one `label: value` per line.
pub(super) fn startup_tooltip(facts: &[StartupFact]) -> String {
    facts
        .iter()
        .map(|f| format!("{}: {}", f.label, f.value))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Return the complete localized startup diagnostic for reuse outside Core Status.
///
/// Args:
///     status: Latest polled startup snapshot for one core.
///
/// Returns:
///     The same labelled diagnostic lines rendered by the Core Status startup hover.
pub(crate) fn startup_diagnostic_text(status: &CoreStartupStatus) -> String {
    startup_tooltip(&startup_facts(status))
}

/// Combine the shared connection verdict with the complete startup telemetry for one problem core.
///
/// The actionable reason stays first; physical socket evidence follows under the same Startup
/// heading used by Core Status. Settings, the by-IP tree, and the Auto rail all call this rather
/// than maintaining three hand-written combinations. A retained fault supplies its own frozen
/// snapshot so a retry cannot relabel the previous attempt's verdict with a new socket's counters.
///
/// Args:
///     diagnosis: Classified reason and retry state to render first.
///     fault: Frozen failed-attempt evidence when this diagnosis came from a retained fault.
///     live_status: Latest startup snapshot, used only when no failed attempt exists.
///
/// Returns:
///     One localized tooltip whose verdict and telemetry describe the same attempt.
pub(crate) fn problem_diagnostic_text(
    diagnosis: &Diagnosis,
    fault: Option<&ConnFault>,
    live_status: &CoreStartupStatus,
) -> String {
    let evidence_status = fault.map(|fault| &fault.startup).unwrap_or(live_status);
    format!(
        "{}\n{}:\n{}",
        crate::conn_diag::fault_tooltip(&crate::conn_diag::fault_facts(diagnosis)),
        t!("core_status.col.startup"),
        startup_diagnostic_text(evidence_status)
    )
}
