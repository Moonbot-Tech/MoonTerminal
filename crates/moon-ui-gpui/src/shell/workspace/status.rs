//! Workspace rail diagnostics, status presentation, and navigation actions.

use super::*;

/// Build one Auto core row's hover text, expanding a problem into its live diagnostics.
///
/// Args:
///     row: Derived roster row with typed connection and startup state.
///
/// Returns:
///     The standard core identity line, followed for problem rows by the exact connection detail
///     and every startup fact also exposed by the Core Status panel.
pub(super) fn workspace_core_tooltip(row: &WorkspaceRosterRow) -> String {
    let status = workspace_status_text(row.status);
    let mut lines = vec![
        t!(
            "workspace.core_tip",
            name = row.name.as_str(),
            group = row.group.as_str(),
            status = status
        )
        .to_string(),
    ];
    if row.status == WorkspaceCoreStatus::Problem
        && let Some(connection) = row.connection.as_ref()
    {
        let diag = moon_core::feed::diagnose(connection, row.fault.as_ref(), &row.startup);
        lines.push(format!(
            "{}: {}",
            t!("core_status.col.status"),
            crate::panels::connection_status_text(connection, diag.as_ref())
        ));
        // The reason and its next step come BEFORE the channel telemetry: a rail row is a glance
        // surface, and the actionable half must not sit under a dozen measurement lines.
        if let Some(diag) = diag.as_ref() {
            lines.push(crate::panels::problem_diagnostic_text(
                diag,
                row.fault.as_ref(),
                &row.startup,
                row.mode_suggestion,
            ));
        } else {
            lines.push(format!(
                "{}:\n{}",
                t!("core_status.col.startup"),
                crate::panels::startup_diagnostic_text(&row.startup)
            ));
        }
    }
    lines.join("\n")
}

/// Build the tinted danger pill that replaces the plain status label for a `Problem` core row in
/// Full density.
///
/// The density tier gives a 16/20/24-unit text-height badge inside the fixed 30-unit
/// row. Background and border keep the raw red hue; text uses the legible per-theme color.
///
////// Args:
///     status: Localized status text, already resolved to "Problem" in the active locale.
///     p: Active Moon palette.
///
/// Returns:
///     A rounded, tinted badge, well inside the fixed 30-unit cell at every supported density.
pub(super) fn rail_problem_pill(status: String, p: MoonPalette) -> impl IntoElement {
    MoonBadge::new(status)
        .variant(MoonBadgeVariant::Outline)
        .bg_color(p.red)
        .bg_alpha(design::RAIL_PILL_BG_ALPHA)
        .border_color(p.red)
        .border_alpha(design::RAIL_PILL_BORDER_ALPHA)
        .text_color(design::danger_color(p))
}

/// Build shared selection, hover, and disabled chrome for one interactive rail row.
///
/// Args:
///     id: Stable identity unique within the virtualized rail.
///     selected: Whether this row owns the current Auto scope.
///     selectable: Whether the row has a live owning group window and session.
///     horizontal_padding: Density-specific left and right inset in logical pixels.
///     gap: Density-specific gap between direct children in logical pixels.
///     p: Active Moon palette.
///     cx: Application context used for scaled padding.
///
/// Returns:
///     Row container ready for content and an optional click callback.
pub(super) fn rail_row_base(
    id: impl Into<ElementId>,
    selected: bool,
    selectable: bool,
    horizontal_padding: f32,
    gap: f32,
    p: MoonPalette,
    cx: &App,
) -> Stateful<Div> {
    div()
        .id(id)
        .size_full()
        .flex()
        .items_center()
        .min_w_0()
        .gap(design::ui_px(cx, gap))
        .px(design::ui_px(cx, horizontal_padding))
        .text_size(design::t_body(cx))
        .text_color(rgb(if selectable { p.text } else { p.text_muted }))
        .when(selected, |row| {
            row.bg(design::moon_alpha(
                p.accent,
                design::RAIL_ROW_SELECTED_ALPHA,
            ))
        })
        .when(selectable, |row| {
            row.cursor_pointer().hover(move |row| {
                row.bg(design::moon_alpha(p.accent, design::RAIL_ROW_HOVER_ALPHA))
            })
        })
}

/// Execute one pure rail navigation action without reparenting group-owned panels.
///
/// Args:
///     backend: Shared workspace authority and group-window registry.
///     action: Same-group selection or cross-group activation plan.
///     cx: Application context used to publish state and activate the destination window.
///
/// Returns:
///     Nothing; unavailable rows never produce an action.
pub(super) fn execute_workspace_navigation(
    backend: &Entity<Backend>,
    action: WorkspaceNavigationAction,
    cx: &mut App,
) {
    match action {
        WorkspaceNavigationAction::SelectCurrent { group, core } => {
            backend.update(cx, |backend, backend_cx| {
                backend.select_auto_workspace_core(&group, Some(core), backend_cx);
            });
        }
        WorkspaceNavigationAction::ActivateGroup { group, core } => {
            let handle = backend.update(cx, |backend, backend_cx| {
                if !backend
                    .workspace_core_availability(&group, core)
                    .is_available()
                    // The destination is about to SWITCH to Auto, so its viewing preset is that
                    // constant, not the group's current (pre-transition) one (frozen contract
                    // §10.4).
                    || !backend.core_displayed(Some(WorkspaceMode::AutoTrading), core)
                {
                    return None;
                }
                backend.activate_auto_workspace_core(&group, core, backend_cx);
                backend.group_windows.get(&group).copied()
            });
            if let Some(handle) = handle {
                let _ = handle.update(cx, |_, window, _| window.activate_window());
            }
        }
    }
}

/// Return localized status text for one roster state.
///
/// Args:
///     status: Localization-neutral workspace status.
///
/// Returns:
///     Visible status label in the active locale.
pub(super) fn workspace_status_text(status: WorkspaceCoreStatus) -> String {
    let key = match status {
        WorkspaceCoreStatus::Disabled => "workspace.status.disabled",
        WorkspaceCoreStatus::Unavailable => "workspace.status.unavailable",
        WorkspaceCoreStatus::Problem => "workspace.status.problem",
        WorkspaceCoreStatus::Ready => "workspace.status.ready",
    };
    t!(key).to_string()
}

/// Return the palette role used by one roster status dot.
///
/// Args:
///     status: Derived core status.
///     p: Active Moon palette.
///
/// Returns:
///     Theme-correct status color.
pub(super) fn workspace_status_color(status: WorkspaceCoreStatus, p: MoonPalette) -> u32 {
    match status {
        WorkspaceCoreStatus::Ready => design::positive_color(p),
        WorkspaceCoreStatus::Problem => design::danger_color(p),
        WorkspaceCoreStatus::Disabled => p.text_muted,
        WorkspaceCoreStatus::Unavailable => p.amber,
    }
}

/// Decide whether a rail row needs a visible status label beside its dot.
///
/// Ready is conveyed by the green dot; `Problem` and `Unavailable` both get an enlarged dot, and
/// `Problem` alone also gets, in Full density, a tinted danger pill. Failure states remain
/// explicit in Full density and stay available in every density through the row tooltip.
///
/// Args:
///     status: Derived core status.
///     density: Current responsive rail presentation.
///
/// Returns:
///     `true` only for a non-ready status in the full-width rail.
pub(super) fn workspace_status_label_visible(
    status: WorkspaceCoreStatus,
    density: WorkspaceRailDensity,
) -> bool {
    density == WorkspaceRailDensity::Full && status != WorkspaceCoreStatus::Ready
}
