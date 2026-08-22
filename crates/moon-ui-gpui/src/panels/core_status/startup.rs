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

use moon_core::feed::ConnStatus;
use moon_core::session::{CoreInitStep, CoreStartupState, CoreStartupStatus, INIT_STEPS_TOTAL};
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

/// Completed steps and the total to show them against.
///
/// The total is clamped UP to what was actually observed. MoonProto exposes no readable step count
/// — both `InitStep::COUNT` and `InitStep::ALL` are private to that crate — so [`INIT_STEPS_TOTAL`]
/// is our own constant and a MoonProto that grows a ninth step would leave it stale with nothing to
/// detect it. The clamp guarantees the cell can never render `9/8`, nor claim completion while work
/// visibly continues. ONE owner, because the cell and the hover both show this pair and a rule
/// re-derived in two places is a rule that drifts.
///
/// Args:
///     s: The polled startup snapshot retained for this core.
///
/// Returns:
///     `(done, total)`, with `done <= total` always.
fn progress_pair(s: &CoreStartupStatus) -> (u8, u8) {
    let done = s.completed_count().min(u8::MAX as u32) as u8;
    (done, INIT_STEPS_TOTAL.max(done))
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
            let (done, total) = progress_pair(s);
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

/// Format seconds with one decimal, e.g. `8.4`.
fn secs(ms: u64) -> String {
    format!("{:.1}", ms as f64 / 1000.0)
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
            t!("core_status.startup.secs", t = secs(elapsed_ms)),
        ),
        StartupCell::Done { elapsed_ms } => t!(
            "core_status.startup.took",
            t = t!("core_status.startup.secs", t = secs(elapsed_ms))
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

/// Localized name of one startup step.
pub(super) fn step_label(step: CoreInitStep) -> String {
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

/// Format a byte count as a compact decimal figure, e.g. `4.2 MB`.
fn bytes(value: u64) -> String {
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
        let (done, total) = progress_pair(s);
        push(
            t!("core_status.startup.f.step").to_string(),
            format!("{} ({done}/{total})", step_label(step)),
        );
    }
    push(
        t!("core_status.startup.f.elapsed").to_string(),
        t!("core_status.startup.secs", t = secs(s.elapsed_ms)).to_string(),
    );
    push(
        t!("core_status.startup.f.received").to_string(),
        format!(
            "{} ({})",
            bytes(s.received_sliced_bytes),
            t!(
                "core_status.startup.rate",
                v = bytes(s.receive_rate_bytes_per_sec)
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
            t!("core_status.startup.secs", t = secs(idle)).to_string(),
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
