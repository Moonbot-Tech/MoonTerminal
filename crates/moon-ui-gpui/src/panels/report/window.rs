//! Standalone scoped Report window opened from Analytics.

use super::*;

/// Height of the standalone Report title bar.
pub(super) const REPORT_HEADER_H: f32 = 34.0;
const REPORT_WINDOW_W: f32 = 1640.0;
const REPORT_WINDOW_H: f32 = 1100.0;
const REPORT_WINDOW_TOP: f32 = 180.0;
const REPORT_WINDOW_MARGIN: f32 = 24.0;
const REPORT_WINDOW_MIN_W: f32 = 900.0;
const REPORT_WINDOW_MIN_H: f32 = 520.0;

/// Fit the requested large Report window inside one display's usable desktop.
///
/// Args:
///     visible: Selected display bounds excluding taskbar or dock areas.
///
/// Returns:
///     Global initial window bounds, or the preferred primary-screen fallback.
fn initial_report_bounds(visible: Option<Bounds<Pixels>>) -> Bounds<Pixels> {
    let Some(visible) = visible else {
        return Bounds {
            origin: point(px(120.0), px(REPORT_WINDOW_TOP)),
            size: size(px(REPORT_WINDOW_W), px(REPORT_WINDOW_H)),
        };
    };

    let width =
        REPORT_WINDOW_W.min((f32::from(visible.size.width) - 2.0 * REPORT_WINDOW_MARGIN).max(1.0));
    let height =
        REPORT_WINDOW_H.min((f32::from(visible.size.height) - 2.0 * REPORT_WINDOW_MARGIN).max(1.0));
    let spare_y = (f32::from(visible.size.height) - height).max(0.0);
    let edge_y = REPORT_WINDOW_MARGIN.min(spare_y / 2.0);
    let min_y = f32::from(visible.origin.y) + edge_y;
    let max_y = f32::from(visible.origin.y) + f32::from(visible.size.height) - height - edge_y;
    let y = (f32::from(visible.origin.y) + REPORT_WINDOW_TOP).clamp(min_y, max_y);

    Bounds {
        origin: point(
            px(f32::from(visible.origin.x) + (f32::from(visible.size.width) - width) / 2.0),
            px(y),
        ),
        size: size(px(width), px(height)),
    }
}

/// Restore saved Report geometry inside the selected display's current usable desktop.
///
/// Exact bounds survive while safe. Resolution, DPI, or work-area changes clamp an oversized or
/// edge-crossing rectangle without discarding the user's retained placement. Unusable dimensions
/// fall back to the normal large initial geometry.
///
/// Args:
///     saved: Last persisted standalone Report rectangle.
///     visible: Selected fallback display bounds.
///     saved_origin_is_visible: Whether an attached display contains the saved origin.
///
/// Returns:
///     Reachable saved bounds clamped to the current display, or safe initial bounds.
fn restored_report_bounds(
    saved: Option<moon_core::config::GeomRect>,
    visible: Option<Bounds<Pixels>>,
    saved_origin_is_visible: bool,
) -> Bounds<Pixels> {
    let fallback = initial_report_bounds(visible);
    let Some(saved) = saved.filter(|_| saved_origin_is_visible) else {
        return fallback;
    };
    let minimum_width = REPORT_WINDOW_MIN_W.min(f32::from(fallback.size.width));
    let minimum_height = REPORT_WINDOW_MIN_H.min(f32::from(fallback.size.height));
    let saved_width = saved.w as f32;
    let saved_height = saved.h as f32;
    if saved_width < minimum_width || saved_height < minimum_height {
        return fallback;
    }
    let Some(visible) = visible else {
        return Bounds {
            origin: point(px(saved.x as f32), px(saved.y as f32)),
            size: size(px(saved_width), px(saved_height)),
        };
    };

    let visible_x = f32::from(visible.origin.x);
    let visible_y = f32::from(visible.origin.y);
    let width = saved_width.min(f32::from(visible.size.width));
    let height = saved_height.min(f32::from(visible.size.height));
    let max_x = visible_x + f32::from(visible.size.width) - width;
    let max_y = visible_y + f32::from(visible.size.height) - height;
    Bounds {
        origin: point(
            px((saved.x as f32).clamp(visible_x, max_x)),
            px((saved.y as f32).clamp(visible_y, max_y)),
        ),
        size: size(px(width), px(height)),
    }
}

/// Build the Report title bar with width reset and native window controls.
///
/// Args:
///     p: Active MoonUI palette.
///     table_state: Retained table state reset by the header action.
///     title: Localized Report title with an optional sole-core suffix.
///     cx: Application context used for responsive sizing.
///
/// Returns:
///     The standalone Report header element.
pub(super) fn report_header(
    p: MoonPalette,
    table_state: &Entity<MoonDataTableState>,
    title: String,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .id("report-window-header")
        .relative()
        .flex_none()
        .w_full()
        .h(design::fit_h_px(cx, REPORT_HEADER_H, 13.0, 10.5))
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, 6.0))
        .border_b_1()
        .border_color(rgb(p.border))
        .bg(rgb(p.shell_high))
        .child(
            MoonWindowFrame::tool("report-titlebar-title", 0.0)
                .title_cluster(title, cx)
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .child(crate::persistence::table_persist::reset_button(
            "report-reset-widths-window",
            table_state,
        ))
        .when(design::show_custom_window_controls(), |this| {
            this.child(
                MoonWindowFrame::tool("report-window-frame-visual", 0.0)
                    .header_height(REPORT_HEADER_H)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}

/// Build the standalone visual title, appending a resolved sole selected core.
///
/// Empty selection means all cores and therefore never implies a sole choice, even when only one
/// core is currently connected. Multi-selection and stale unresolved identities keep the base
/// title unchanged.
///
/// Args:
///     selected: Explicitly selected core identities; empty means All.
///     cores: Resolved core identities and display names.
///
/// Returns:
///     Localized base title with the resolved sole-core suffix when applicable.
pub(super) fn report_title(
    selected: &std::collections::HashSet<u64>,
    cores: &[(u64, String)],
) -> String {
    let base = crate::persistence::panel_meta::panel_title("Report").to_string();
    if selected.len() != 1 {
        return base;
    }
    let uid = selected.iter().next().copied();
    cores
        .iter()
        .find(|(core_uid, _)| Some(*core_uid) == uid)
        .map_or(base.clone(), |(_, name)| format!("{base} — {name}"))
}

/// Open or retarget the singleton standalone Report window to one Analytics strategy.
///
/// Args:
///     backend: Shared backend holding singleton handles.
///     scope: Exact replacement Report scope.
///     owner: Optional Analytics owner window.
///     owner_display: Owner display fallback for placement.
///     cx: Application context used to update or create the window.
///
/// Returns:
///     Nothing; window creation failures leave the existing application state usable.
pub fn open_scoped(
    backend: Entity<Backend>,
    scope: ReportScope,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    let existing = {
        let backend = backend.read(cx);
        (backend.report_window, backend.report_window_view.clone())
    };
    if let (Some(handle), Some(view)) = existing {
        let next = scope.clone();
        if handle
            .update(cx, |_, window, app| {
                let updated = view
                    .update(app, |panel, panel_cx| {
                        panel.apply_scope(next, window, panel_cx);
                    })
                    .is_ok();
                if updated {
                    window.activate_window();
                }
                updated
            })
            .is_ok_and(|updated| updated)
        {
            return;
        }
    }

    let group = report_group_for_core(&backend, scope.strategy.core_uid, cx);
    let saved = backend.read(cx).layout.report_window;
    let saved_origin = saved.map(|geometry| point(px(geometry.x as f32), px(geometry.y as f32)));
    // Report is the only tool window that judges its saved origin, and it does so to CLAMP the
    // rectangle into the visible area below, not to rescue an unreachable one — the platform
    // already handles that (`moon-gpui-windows`'s `retrieve_window_placement` substitutes
    // `display.default_bounds()` for bounds whose centre is on no monitor). A sibling window that
    // copies this check for the rescue alone is duplicating work the OS layer does better.
    let saved_origin_is_visible = saved_origin.is_some_and(|origin| {
        cx.displays()
            .into_iter()
            .any(|display| display.bounds().contains(&origin))
    });
    let display_id =
        crate::window::windowing::saved_or_owner_display_id(saved_origin, owner, owner_display, cx);
    let display = display_id
        .and_then(|display_id| cx.find_display(display_id))
        .or_else(|| cx.primary_display());
    let bounds = restored_report_bounds(
        saved,
        display.as_ref().map(|display| display.visible_bounds()),
        saved_origin_is_visible,
    );
    let min_size = size(
        px(REPORT_WINDOW_MIN_W.min(f32::from(bounds.size.width))),
        px(REPORT_WINDOW_MIN_H.min(f32::from(bounds.size.height))),
    );
    let mut options = crate::window::windowing::tool_window_options(
        crate::persistence::panel_meta::panel_title("Report").to_string(),
        WindowBounds::Windowed(bounds),
        Some(min_size),
        owner,
    );
    options.display_id = display.map(|display| display.id());
    let backend_for_window = backend.clone();
    if let Ok(handle) = cx.open_window(options, move |window, cx| {
        crate::window::windowing::configure_shell_clear_color(window, cx);
        let panel = cx.new(|cx| {
            ReportPanel::new_with_scope(backend_for_window.clone(), group, Some(scope), window, cx)
        });
        panel.update(cx, |panel, panel_cx| {
            panel.mark_standalone(window, panel_cx)
        });
        backend_for_window.update(cx, |backend, _| {
            backend.report_window_view = Some(panel.downgrade());
        });
        cx.new(|cx| {
            Root::new(panel, window, cx).background_policy(moon_ui::MoonBackgroundPolicy::Opaque)
        })
    }) {
        backend.update(cx, |backend, _| {
            backend.report_window = Some(handle);
        });
        crate::window::windowing::activate_new_window(handle.into(), cx);
    }
}

#[cfg(test)]
mod tests;
