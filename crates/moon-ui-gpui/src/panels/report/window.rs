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

/// Build the Report title bar with width reset and native window controls.
///
/// Args:
///     p: Active MoonUI palette.
///     table_state: Retained table state reset by the header action.
///     cx: Application context used for responsive sizing.
///
/// Returns:
///     The standalone Report header element.
pub(super) fn report_header(
    p: MoonPalette,
    table_state: &Entity<MoonDataTableState>,
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
                .title_cluster(
                    crate::persistence::panel_meta::panel_title("Report").to_string(),
                    cx,
                )
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
    let display_id =
        crate::window::windowing::saved_or_owner_display_id(None, owner, owner_display, cx);
    let display = display_id
        .and_then(|display_id| cx.find_display(display_id))
        .or_else(|| cx.primary_display());
    let bounds = initial_report_bounds(display.as_ref().map(|display| display.visible_bounds()));
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
        panel.update(cx, |panel, panel_cx| panel.mark_standalone(panel_cx));
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
