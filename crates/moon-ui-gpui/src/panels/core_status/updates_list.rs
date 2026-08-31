//! Updates-mode table: the retained update-history log merged with attempts still in flight.
//!
//! A read-only log, in-flight attempts first, then closed history rows newest first. Every
//! column is non-sortable: order comes from the merge itself, never from the table. Modeled
//! directly on `warnings.rs`, the sibling read-only log surface.

use std::collections::HashMap;
use std::net::IpAddr;
use std::rc::Rc;

use moon_core::feed::UpdateTarget;
use moon_core::session::core_update::{CoreUpdateOutcome, CoreUpdatePhase, CoreUpdateRecord};
use moon_ui::{MoonDataCell, MoonDataRow, MoonDataTable, MoonDataTableColumn};

use super::model::CoreStatusRow;
use super::presentation::update_tooltip;
use super::*;

/// The fixed set of Updates columns (no sorting: the merge already orders the rows).
fn columns() -> Vec<MoonDataTableColumn> {
    vec![
        MoonDataTableColumn::new("time", t!("core_update.col.time").to_string(), 150.0),
        MoonDataTableColumn::new("core", t!("core_update.col.core").to_string(), 130.0),
        MoonDataTableColumn::new("server", t!("core_update.col.server").to_string(), 110.0),
        MoonDataTableColumn::new("from_to", t!("core_update.col.from_to").to_string(), 100.0),
        MoonDataTableColumn::new("target", t!("core_update.col.target").to_string(), 90.0),
        MoonDataTableColumn::new("outcome", t!("core_update.col.outcome").to_string(), 220.0),
        MoonDataTableColumn::new("duration", t!("core_update.col.duration").to_string(), 80.0)
            .right(),
    ]
}

/// Render the Updates list table.
///
/// Args:
///     id: Stable table element identity.
///     live_rows: Cores currently tracked in the update queue (`Queued`/`Sent`/`Waiting`),
///         already scoped to the panel's effective core selection. Drawn first.
///     history: Closed history records, already scoped, capped and ordered newest first. Drawn
///         after every in-flight row.
///     server_names: Server display name per lane/endpoint IP -- never the raw address.
///     state: Persisted table interaction state.
///     zone: User-selected display time zone.
///     now_ms: Injected clock, used to compute a live in-flight duration.
///     cx: Panel context.
///
/// Returns:
///     A full-size data-table host, or the localized empty state.
pub(super) fn updates_table(
    id: &'static str,
    live_rows: Rc<Vec<CoreStatusRow>>,
    history: Rc<Vec<CoreUpdateRecord>>,
    server_names: Rc<HashMap<IpAddr, String>>,
    state: &Entity<MoonDataTableState>,
    zone: chrono_tz::Tz,
    now_ms: i64,
    cx: &Context<CoreStatusView>,
) -> impl IntoElement {
    let live_count = live_rows.len();
    let row_count = live_count + history.len();
    let empty = row_count == 0;
    let p = MoonPalette::active(cx);

    crate::panels::common::data_table_host(
        SharedString::from(format!("{id}-host")),
        empty,
        t!("core_update.list_empty").to_string(),
        p,
        cx,
        MoonDataTable::new(id, row_count, move |ix, _window, _app| {
            if ix < live_count {
                live_row(ix, &live_rows[ix], &server_names, zone, now_ms)
            } else {
                history_row(ix, &history[ix - live_count], &server_names, zone)
            }
        })
        .columns(columns())
        .state(state)
        .header_height(design::TABLE_HEAD_H)
        .row_height(design::TABLE_ROW_H),
    )
}

/// Build one row for a core still tracked in the update queue.
///
/// `Done`/`None` never reach this in practice: the caller (`mod.rs`) filters live rows to the
/// three in-flight phases before building the table -- a finished attempt is represented by its
/// history record instead. Both are still handled here for an exhaustive, panic-free match.
fn live_row(
    ix: usize,
    row: &CoreStatusRow,
    server_names: &HashMap<IpAddr, String>,
    zone: chrono_tz::Tz,
    now_ms: i64,
) -> MoonDataRow {
    let server = row
        .endpoint
        .and_then(|ep| server_names.get(&ep.address).cloned())
        .unwrap_or_else(|| "—".to_string());
    let core = row.name.clone();
    let Some(phase) = row.update.as_ref() else {
        return cells(ix, "—", core, server, "—", "—", "—", "—");
    };
    let outcome = update_tooltip(phase);
    match phase {
        CoreUpdatePhase::Sent {
            target,
            from,
            sent_at_ms,
            ..
        }
        | CoreUpdatePhase::Waiting {
            target,
            from,
            sent_at_ms,
            ..
        } => cells(
            ix,
            time_text(*sent_at_ms, zone),
            core,
            server,
            format!("{} \u{2192} \u{2026}", number_or_dash(*from)),
            target_text(target),
            outcome,
            crate::panels::common::warn_duration_text(*sent_at_ms, Some(now_ms)),
        ),
        // `Queued` carries only a lane address and a stall flag -- no timestamp, target or
        // baseline build -- so every column but the outcome word stays a dash until the queue
        // actually sends something. See the `CoreUpdatePhase` shape in
        // `crates/moon-core/src/session/core_update.rs`.
        CoreUpdatePhase::Queued { .. } | CoreUpdatePhase::Done(_) => {
            cells(ix, "—", core, server, "—", "—", outcome, "—")
        }
    }
}

/// Build one row for a closed history record.
fn history_row(
    ix: usize,
    record: &CoreUpdateRecord,
    server_names: &HashMap<IpAddr, String>,
    zone: chrono_tz::Tz,
) -> MoonDataRow {
    let server = server_names
        .get(&record.lane_addr)
        .cloned()
        .unwrap_or_else(|| "—".to_string());
    let to = match &record.outcome {
        CoreUpdateOutcome::Succeeded { to, .. } => Some(*to),
        CoreUpdateOutcome::Unchanged { version } => Some(*version),
        CoreUpdateOutcome::Failed(_) => None,
    };
    let duration = if record.started_ms > 0 {
        crate::panels::common::warn_duration_text(record.started_ms, Some(record.ended_ms))
    } else {
        "—".to_string()
    };
    // Reuses the same phase-to-word formatter the badge and hover tooltip already use, by
    // wrapping the closed outcome back into the `Done` phase it always closed from -- one
    // formatter, never a second match restating the same seven cases.
    let outcome = update_tooltip(&CoreUpdatePhase::Done(record.outcome.clone()));
    cells(
        ix,
        time_text(record.started_ms, zone),
        record.core_name.clone(),
        server,
        format!(
            "{} \u{2192} {}",
            number_or_dash(record.from),
            number_or_dash(to)
        ),
        target_text(&record.target),
        outcome,
        duration,
    )
}

/// Assemble the final row from resolved column text, in the fixed column order.
///
/// `ix` is the table's row index, used only to give the outcome cell's tooltip host a stable,
/// unique element id -- the row itself carries no other identity `MoonDataTable` addresses cells
/// by (see `table.rs`'s own row-index-addressed cells for the same idiom).
///
/// Args:
///     outcome: Full localized outcome text, shown as the cell's text AND its hover -- the
///         longest translations (`NotReady` in Spanish among them) are wider than the fixed
///         column and MoonUI clips rather than wraps, so the hover is the only place the full
///         reason a HOT campaign failed stays readable in a narrow dock.
#[allow(clippy::too_many_arguments)]
fn cells(
    ix: usize,
    time: impl Into<SharedString>,
    core: impl Into<SharedString>,
    server: impl Into<SharedString>,
    from_to: impl Into<SharedString>,
    target: impl Into<SharedString>,
    outcome: impl Into<SharedString>,
    duration: impl Into<SharedString>,
) -> MoonDataRow {
    let outcome: SharedString = outcome.into();
    MoonDataRow::new([
        MoonDataCell::text(time),
        MoonDataCell::text(core),
        MoonDataCell::text(server),
        MoonDataCell::text(from_to),
        MoonDataCell::text(target),
        MoonDataCell::element(
            div()
                .id(SharedString::from(format!("cs-update-outcome-{ix}")))
                .child(outcome.clone())
                .tooltip(crate::panels::common::text_tooltip(outcome)),
        ),
        MoonDataCell::text(duration),
    ])
}

/// A reported build number, or a dash for "never reported".
fn number_or_dash(v: Option<u32>) -> String {
    // The SAME formatter the MoonBot column uses (`presentation::version_text` ->
    // `fmt::core_build`), so a build reads `7.69` here exactly as it does in the telemetry table.
    // A raw `769` beside a `7.69` elsewhere reads as two different facts about one core.
    v.map(moon_core::util::fmt::core_build)
        .unwrap_or_else(|| "—".to_string())
}

/// Localized target word -- net new, since no UI surface has ever had to word `UpdateTarget`
/// before this list.
fn target_text(target: &UpdateTarget) -> String {
    match target {
        UpdateTarget::Release => t!("core_update.target.release").to_string(),
        UpdateTarget::Named(name) => t!("core_update.target.named", name = name).to_string(),
    }
}

/// Formatted start time, or a dash when no timestamp is available (a merely `Queued` core has
/// none -- see [`live_row`]).
fn time_text(ms: i64, zone: chrono_tz::Tz) -> String {
    if ms <= 0 {
        return "—".to_string();
    }
    let s = moon_core::util::display_time::format_minute(ms / 1000, zone);
    if s.is_empty() { "—".to_string() } else { s }
}
