//! Updates-mode table: the retained update-history log merged with attempts still in flight.
//!
//! A read-only log, in-flight attempts first, then closed history rows newest first. Every
//! column is non-sortable: order comes from the merge itself, never from the table. Modeled
//! directly on `warnings.rs`, the sibling read-only log surface.

use std::collections::HashMap;
use std::net::IpAddr;
use std::rc::Rc;

use moon_core::feed::{UpdateTarget, named_build_label};
use moon_core::session::CoreId;
use moon_core::session::core_update::{CoreUpdateOutcome, CoreUpdatePhase, CoreUpdateRecord};
use moon_ui::{
    MoonDataCell, MoonDataRow, MoonDataTable, MoonDataTableColumn, MoonProgress, MoonSize,
    MoonSteps,
};

use super::model::CoreStatusRow;
use super::presentation::{BuildParts, build_key, build_parts, update_tooltip};
use super::*;
use crate::controls::core_update::{self, OfferCounts};

/// Declared width of the outcome column. Five horizontal Xs steps need 5 * 120 and wrap below that.
const OUTCOME_COLUMN_W: f32 = 220.0;
/// Standard-density table row (`tokens.table_row_h`). The outcome cell has to fit inside it.
const STANDARD_ROW_H: f32 = 25.0;
/// MoonSteps horizontal floor: one step's `min_w` before the row wraps (`STEP_MIN_WIDTH`).
const STEP_MIN_W: f32 = 120.0;
/// Xs marker height: control height 20 minus 4. One unwrapped row is this tall.
const STEP_ROW_H: f32 = 16.0;
/// Vertical gap MoonSteps inserts between wrapped rows.
const STEP_GAP_Y: f32 = 6.0;
const STEP_COUNT: f32 = 5.0;

/// Updates column ids in source order. A dragged order is stored under these ids.
pub(super) const ORDER_KEYS: &[&str] = &[
    "time", "core", "server", "from_to", "target", "outcome", "duration",
];

/// The fixed set of Updates columns (no sorting: the merge already orders the rows).
fn columns() -> Vec<MoonDataTableColumn> {
    vec![
        MoonDataTableColumn::new("time", t!("core_update.col.time").to_string(), 150.0),
        MoonDataTableColumn::new("core", t!("core_update.col.core").to_string(), 130.0),
        MoonDataTableColumn::new("server", t!("core_update.col.server").to_string(), 110.0),
        MoonDataTableColumn::new("from_to", t!("core_update.col.from_to").to_string(), 170.0),
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
///     live_rows: Cores currently tracked in the update queue
///         (`Queued`/`Sent`/`Waiting`/`Verifying`), already scoped to the panel's effective
///         core selection. Drawn first.
///     history: Closed history records, already scoped, capped and ordered newest first. Drawn
///         after every in-flight row.
///     newest: Per history row, whether it is its core's newest record, the only one that may
///         offer a retry. Built once per snapshot by `frame_cache::history_newest`.
///     phases: Current update phase per scoped core. A history retry is offered only when this
///         phase is `Done(Failed)` for that core.
///     server_names: Server display name per lane/endpoint IP -- never the raw address.
///     state: Persisted table interaction state.
///     zone: User-selected display time zone.
///     now_ms: Injected clock, used to compute a live in-flight duration.
///     backend: Shared terminal state a failed history row's retry button commands.
///     cx: Panel context.
///
/// Returns:
///     A full-size data-table host, or the localized empty state.
#[allow(clippy::too_many_arguments)]
pub(super) fn updates_table(
    id: &'static str,
    live_rows: Rc<Vec<CoreStatusRow>>,
    history: Rc<Vec<CoreUpdateRecord>>,
    newest: Rc<Vec<bool>>,
    phases: Rc<HashMap<CoreId, CoreUpdatePhase>>,
    server_names: Rc<HashMap<IpAddr, String>>,
    state: &Entity<MoonDataTableState>,
    zone: chrono_tz::Tz,
    now_ms: i64,
    backend: &Entity<Backend>,
    cx: &Context<CoreStatusView>,
) -> impl IntoElement {
    let live_count = live_rows.len();
    let row_count = live_count + history.len();
    let empty = row_count == 0;
    let p = MoonPalette::active(cx);
    let backend = backend.clone();

    crate::panels::common::data_table_host(
        SharedString::from(format!("{id}-host")),
        empty,
        t!("core_update.list_empty").to_string(),
        p,
        cx,
        MoonDataTable::new(id, row_count, move |ix, _window, app| {
            if ix < live_count {
                live_row(ix, &live_rows[ix], &server_names, zone, now_ms)
            } else {
                let hist_ix = ix - live_count;
                history_row(
                    ix,
                    &history[hist_ix],
                    &server_names,
                    zone,
                    newest[hist_ix],
                    phases.get(&history[hist_ix].core),
                    &backend,
                    app,
                )
            }
        })
        .columns(columns())
        .state(state)
        .style(design::table_style(p)),
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
        return cells(ix, "—", core, server, "—", "—", "—", "—", None);
    };
    match phase {
        CoreUpdatePhase::Sent {
            target,
            from,
            from_suffix,
            sent_at_ms,
            ..
        }
        | CoreUpdatePhase::Waiting {
            target,
            from,
            from_suffix,
            sent_at_ms,
            ..
        }
        | CoreUpdatePhase::Verifying {
            target,
            from,
            from_suffix,
            sent_at_ms,
            ..
        } => steps_row(
            ix,
            time_text(*sent_at_ms, zone),
            core,
            server,
            live_from_to(*from, from_suffix.as_deref()),
            target_text(target),
            phase,
            crate::panels::common::warn_duration_text(*sent_at_ms, Some(now_ms)),
        ),
        // `Queued` carries only a lane address and a stall flag -- no timestamp, target or
        // baseline build -- so every column but the phase steps stays a dash until the queue
        // actually sends something. See the `CoreUpdatePhase` shape in
        // `crates/moon-core/src/session/core_update.rs`.
        CoreUpdatePhase::Queued { .. } | CoreUpdatePhase::Done(_) => {
            steps_row(ix, "—", core, server, "—", "—", phase, "—")
        }
    }
}

/// Zero-based MoonSteps index. `Done` sits past the last label so every step reads finished.
fn phase_index(phase: &CoreUpdatePhase) -> usize {
    match phase {
        CoreUpdatePhase::Queued { .. } => 0,
        CoreUpdatePhase::Sent { .. } => 1,
        CoreUpdatePhase::Waiting { .. } => 2,
        CoreUpdatePhase::Verifying { .. } => 3,
        CoreUpdatePhase::Done(_) => 5,
    }
}

/// Whether horizontal Xs steps fit in one table row at this column width.
///
/// Five steps at the MoonSteps floor of 120 wrap inside the 220 outcome column, and the
/// wrapped stack is taller than the 25px standard row. A column wide enough for one row of
/// steps (600 at zoom 1) fits.
///
/// Args:
///     column_w: Outcome column width, design px.
///     row_h: Row height the cell must stay inside, design px.
///
/// Returns:
///     Whether the horizontal steps stay inside the row.
pub(super) fn outcome_steps_fit(column_w: f32, row_h: f32) -> bool {
    if column_w < STEP_MIN_W || row_h <= 0.0 {
        return false;
    }
    let per_row = (column_w / STEP_MIN_W).floor().max(1.0);
    let rows = (STEP_COUNT / per_row).ceil();
    let height = rows * STEP_ROW_H + (rows - 1.0).max(0.0) * STEP_GAP_Y;
    height <= row_h
}

/// MoonProgress value for one phase: index / 4, so Verifying is 75 and Done clamps at 100.
///
/// Args:
///     phase: The attempt's current phase.
///
/// Returns:
///     A percentage in `0..=125`. [`MoonProgress`] clamps the Done index (5) to 100.
pub(super) fn outcome_progress_value(phase: &CoreUpdatePhase) -> f32 {
    (phase_index(phase) as f32 / 4.0) * 100.0
}

/// Horizontal compact steps. The production column does not use these: they wrap at 220.
fn phase_steps(ix: usize, phase: &CoreUpdatePhase) -> MoonSteps {
    MoonSteps::new(SharedString::from(format!("cs-update-steps-{ix}")))
        .steps([
            t!("core_update.phase.queued").to_string(),
            t!("core_update.phase.sent").to_string(),
            t!("core_update.phase.waiting").to_string(),
            t!("core_update.phase.verifying").to_string(),
            t!("core_update.phase.succeeded").to_string(),
        ])
        .current(phase_index(phase))
        .vertical(false)
        .size(MoonSize::Xs)
}

/// Outcome cell that fits the standard row: horizontal steps when the column is wide enough,
/// otherwise the phase sentence and a progress bar at [`outcome_progress_value`].
fn outcome_cell(ix: usize, phase: &CoreUpdatePhase) -> AnyElement {
    let tip = update_tooltip(phase, None);
    let host = div()
        .id(SharedString::from(format!("cs-update-outcome-{ix}")))
        .debug_selector({
            let selector = format!("cs-update-outcome-{ix}");
            move || selector
        })
        .size_full()
        .min_w_0()
        .overflow_hidden()
        .tooltip(crate::panels::common::text_tooltip(tip));
    if outcome_steps_fit(OUTCOME_COLUMN_W, STANDARD_ROW_H) {
        return host.child(phase_steps(ix, phase)).into_any_element();
    }
    let label = update_tooltip(phase, None);
    host.flex()
        .items_center()
        .gap_1()
        .child(
            div()
                .min_w_0()
                .flex_1()
                .overflow_hidden()
                .truncate()
                .child(label),
        )
        .child(
            div().w(px(72.0)).flex_none().child(
                MoonProgress::new(SharedString::from(format!("cs-update-progress-{ix}")))
                    .value(outcome_progress_value(phase)),
            ),
        )
        .into_any_element()
}

/// A live row whose outcome cell is the phase steps, with the existing phase sentence as hover.
fn steps_row(
    ix: usize,
    time: impl Into<SharedString>,
    core: impl Into<SharedString>,
    server: impl Into<SharedString>,
    from_to: impl Into<SharedString>,
    target: impl Into<SharedString>,
    phase: &CoreUpdatePhase,
    duration: impl Into<SharedString>,
) -> MoonDataRow {
    let from_to: SharedString = from_to.into();
    MoonDataRow::new([
        MoonDataCell::text(time),
        MoonDataCell::text(core),
        MoonDataCell::text(server),
        MoonDataCell::element(
            div()
                .id(SharedString::from(format!("cs-update-from-to-{ix}")))
                .child(from_to.clone())
                .tooltip(crate::panels::common::text_tooltip_value(from_to)),
        ),
        MoonDataCell::text(target),
        MoonDataCell::element(outcome_cell(ix, phase)),
        MoonDataCell::text(duration),
    ])
}

/// Build one row for a closed history record.
fn history_row(
    ix: usize,
    record: &CoreUpdateRecord,
    server_names: &HashMap<IpAddr, String>,
    zone: chrono_tz::Tz,
    is_newest: bool,
    phase: Option<&CoreUpdatePhase>,
    backend: &Entity<Backend>,
    app: &App,
) -> MoonDataRow {
    let server = server_names
        .get(&record.lane_addr)
        .cloned()
        .unwrap_or_else(|| "—".to_string());
    let to = match &record.outcome {
        CoreUpdateOutcome::Succeeded { to, .. } => Some(*to),
        CoreUpdateOutcome::Unchanged { version, .. } => Some(*version),
        // THE WHOLE POINT of the variant: the cell must not claim a build the terminal never
        // actually confirmed.
        CoreUpdateOutcome::Unverified(_) => None,
        CoreUpdateOutcome::Failed(_) => None,
    };
    let duration = if record.started_ms > 0 {
        crate::panels::common::warn_duration_text(record.started_ms, Some(record.ended_ms))
    } else {
        "—".to_string()
    };
    // Reuses the same phase-to-word formatter the badge and hover tooltip already use, by
    // wrapping the closed outcome back into the `Done` phase it always closed from -- one
    // formatter, never a second match restating the same exhaustive phase/outcome cases.
    let outcome = update_tooltip(
        &CoreUpdatePhase::Done(record.outcome.clone()),
        Some(&record.target),
    );
    cells(
        ix,
        time_text(record.started_ms, zone),
        record.core_name.clone(),
        server,
        from_to_text(
            record.from,
            record.from_suffix.as_deref(),
            to,
            reported_to_suffix(&record.outcome),
            matches!(record.outcome, CoreUpdateOutcome::Succeeded { .. }),
            &record.target,
        ),
        target_text(&record.target),
        outcome,
        duration,
        if offer_history_retry(record, is_newest, phase) {
            failed_retry(ix, record, backend, app)
        } else {
            None
        },
    )
}

/// Whether a closed history row may draw the same retry the Flat version cell draws.
///
/// The button is offered only for a failed record that is that core's newest history entry
/// while the core's current phase is still `Done(Failed)`. An older failure, or a core that
/// has since succeeded, must not send an update the operator is not looking at.
///
/// Args:
///     record: The closed history row.
///     is_newest_for_core: Whether `record` is the first history entry for its core. History
///         is newest-first, so that is the latest attempt.
///     current_phase: The core's phase on the status row, or `None` when the core is gone.
///
/// Returns:
///     Whether the history row may show the retry button.
pub(super) fn offer_history_retry(
    record: &CoreUpdateRecord,
    is_newest_for_core: bool,
    current_phase: Option<&CoreUpdatePhase>,
) -> bool {
    is_newest_for_core
        && matches!(record.outcome, CoreUpdateOutcome::Failed(_))
        && matches!(
            current_phase,
            Some(CoreUpdatePhase::Done(CoreUpdateOutcome::Failed(_)))
        )
}

/// The same retry the Flat version cell draws, for one failed history row.
fn failed_retry(
    ix: usize,
    record: &CoreUpdateRecord,
    backend: &Entity<Backend>,
    app: &App,
) -> Option<(SharedString, AnyElement)> {
    if !matches!(record.outcome, CoreUpdateOutcome::Failed(_)) {
        return None;
    }
    let group = SharedString::from(format!("cs-update-retry-hover-{ix}"));
    let button = core_update::update_button(
        SharedString::from(format!("cs-update-retry-{}", record.core)),
        group.clone(),
        Rc::from([record.core]),
        OfferCounts::default(),
        Some(record.core),
        backend,
        MoonPalette::active(app),
        app,
    )?;
    Some((group, button))
}

/// Assemble the final row from resolved column text, in the fixed column order.
///
/// `ix` is the table's row index, used to give the build and outcome tooltip hosts stable,
/// unique element id -- the row itself carries no other identity `MoonDataTable` addresses cells
/// by (see `table.rs`'s own row-index-addressed cells for the same idiom).
///
/// Args:
///     from_to: Full reported builds, retained in the hover when the column clips them.
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
    retry: Option<(SharedString, AnyElement)>,
) -> MoonDataRow {
    let outcome: SharedString = outcome.into();
    let from_to: SharedString = from_to.into();
    let mut outcome_host = div()
        .id(SharedString::from(format!("cs-update-outcome-{ix}")))
        .flex()
        .items_center()
        .min_w_0()
        .gap_1()
        .overflow_hidden()
        .child(div().min_w_0().flex_1().truncate().child(outcome.clone()))
        .tooltip(crate::panels::common::text_tooltip(outcome));
    if let Some((group, button)) = retry {
        outcome_host = outcome_host.group(group).child(button);
    }
    MoonDataRow::new([
        MoonDataCell::text(time),
        MoonDataCell::text(core),
        MoonDataCell::text(server),
        MoonDataCell::element(
            div()
                .id(SharedString::from(format!("cs-update-from-to-{ix}")))
                .child(from_to.clone())
                .tooltip(crate::panels::common::text_tooltip_value(from_to)),
        ),
        MoonDataCell::text(target),
        MoonDataCell::element(outcome_host),
        MoonDataCell::text(duration),
    ])
}

/// `from → to`, each side through [`moon_core::util::fmt::core_build_named`].
///
/// A test build can carry the same number as the release, so `7.71 → 7.71` alone says nothing.
/// When the outcome already carries a letter, that letter is the name (`7.71 → 7.71 R3`) and the
/// target label is not appended again. A succeeded named attempt whose outcome has no letter
/// still appends the short target label. An `Unchanged` row does not.
///
/// Args:
///     from: Baseline build, or `None` when the core had never reported one.
///     from_suffix: Letter paired with `from`.
///     to: Build the outcome confirmed, or `None` when it confirmed none.
///     to_suffix: Letter paired with `to`. `None` and a blank letter both count as no letter.
///     succeeded: Whether the outcome is `Succeeded`.
///     target: What the attempt asked the core to install.
///
/// Returns:
///     The from-to cell text.
fn from_to_text(
    from: Option<u32>,
    from_suffix: Option<&str>,
    to: Option<u32>,
    to_suffix: Option<&str>,
    succeeded: bool,
    target: &UpdateTarget,
) -> String {
    let base = format!(
        "{} \u{2192} {}",
        build_text(from, from_suffix),
        build_text(to, to_suffix)
    );
    let letter_shown = to_suffix.is_some_and(|suffix| !suffix.trim().is_empty());
    match target {
        UpdateTarget::Named(name) if succeeded && !letter_shown => {
            format!("{base} {}", named_build_label(name))
        }
        _ => base,
    }
}

/// In-flight from-to cell: the baseline build and its letter, then an ellipsis.
///
/// The core has not reported the post-update build yet, so the right side stays unfinished.
///
/// Args:
///     from: Baseline build captured at send.
///     from_suffix: Letter paired with `from`.
///
/// Returns:
///     The from-to cell text.
fn live_from_to(from: Option<u32>, from_suffix: Option<&str>) -> String {
    format!("{} \u{2192} \u{2026}", build_text(from, from_suffix))
}

/// Letter the outcome stored for the confirmed build, when that outcome has one.
///
/// Args:
///     outcome: Closed attempt outcome.
///
/// Returns:
///     `to_suffix` for `Succeeded` and `Unchanged`, otherwise `None`.
fn reported_to_suffix(outcome: &CoreUpdateOutcome) -> Option<&str> {
    match outcome {
        CoreUpdateOutcome::Succeeded { to_suffix, .. }
        | CoreUpdateOutcome::Unchanged { to_suffix, .. } => to_suffix.as_deref(),
        CoreUpdateOutcome::Unverified(_) | CoreUpdateOutcome::Failed(_) => None,
    }
}

/// A reported build with its letter, or a dash when the core never reported a number.
///
/// Args:
///     version: Build number, or `None` when the core never reported one.
///     suffix: Letter from the same handshake. A blank letter renders as the number alone.
///
/// Returns:
///     Dotted build text, or a dash.
fn build_text(version: Option<u32>, suffix: Option<&str>) -> String {
    // The SAME formatter the MoonBot column uses (`presentation::version_text` ->
    // `fmt::core_build_named`), so a build reads `7.71 R3` here exactly as it does in the
    // telemetry table. A raw `771` beside a `7.71` elsewhere reads as two different facts.
    version
        .map(|build| moon_core::util::fmt::core_build_named(build, suffix))
        .unwrap_or_else(|| "—".to_string())
}

/// Localized target word -- net new, since no UI surface has ever had to word `UpdateTarget`
/// before this list.
fn target_text(target: &UpdateTarget) -> String {
    match target {
        UpdateTarget::Release => t!("core_update.target.release").to_string(),
        UpdateTarget::Named(name) => {
            t!("core_update.target.named", name = named_build_label(name)).to_string()
        }
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

/// One slice of the version bar: a reported build, or the trailing "no build" bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VersionShare {
    /// Number and letter, the same split the build column paints. The unknown bucket is the
    /// dash [`build_parts`] returns for a missing number.
    pub(super) parts: BuildParts,
    /// Cores in this slice.
    pub(super) cores: usize,
    /// This slice's number equals the newest build in play: the highest number anyone in
    /// this scope reported, or any row's fleet-behind target, whichever is higher. A scope
    /// whose local max is still behind that target flags nobody. The unknown bucket is
    /// never newest. A letter does not rank: two letters on that number are both newest.
    pub(super) newest: bool,
}

/// Group visible cores by the build they reported.
///
/// The group key is the number plus the trimmed letter from [`build_key`], so a blank suffix
/// and a missing suffix share a slice and surrounding spaces do not split a letter. Higher
/// numbers come first. The same number orders the bare build before letters, then letters
/// lexicographically. Cores with no number form one bucket after every reported build.
///
/// Args:
///     rows: Visible core snapshots. A retained build on a non-Ready core still counts: this
///         groups what was reported, and the store is what clears a build.
///
/// Returns:
///     Newest builds first, unknown bucket last when any core reported nothing. Empty when
///     `rows` is empty.
pub(super) fn version_distribution(rows: &[CoreStatusRow]) -> Vec<VersionShare> {
    let mut groups: Vec<(u32, Option<String>, usize)> = Vec::new();
    let mut unknown = 0usize;
    for row in rows {
        let Some(version) = row.server_version else {
            unknown += 1;
            continue;
        };
        let (number, tag) = build_key(version, row.server_version_suffix.as_deref());
        if let Some(group) = groups
            .iter_mut()
            .find(|group| group.0 == number && group.1 == tag)
        {
            group.2 += 1;
        } else {
            groups.push((number, tag, 1));
        }
    }
    let local_max = groups.iter().map(|group| group.0).max();
    let behind_target = rows.iter().filter_map(|row| row.version_behind).max();
    let newest = match (local_max, behind_target) {
        (Some(local), Some(target)) => Some(local.max(target)),
        (local, target) => local.or(target),
    };
    groups.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut shares: Vec<VersionShare> = groups
        .into_iter()
        .map(|(version, tag, cores)| VersionShare {
            parts: build_parts(Some(version), tag.as_deref()),
            cores,
            newest: Some(version) == newest,
        })
        .collect();
    if unknown > 0 {
        shares.push(VersionShare {
            parts: build_parts(None, None),
            cores: unknown,
            newest: false,
        });
    }
    shares
}

#[cfg(test)]
mod tests;
