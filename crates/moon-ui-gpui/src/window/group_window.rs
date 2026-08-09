//! Group-window support: enumerate active groups and open or focus their windows. Ported from egui
//! `App::show_group` and extracted from `main.rs`.

use gpui::*;

use moon_ui::{MoonBackgroundPolicy, Root};

use moon_core::config::{AppConfig, WindowLayout};
use moon_core::session::CoreId;

use crate::Backend;
use crate::shell::Shell;
use crate::window::windowing;

/// Returns unique active configuration groups in encounter order, or a single `default` fallback.
pub(crate) fn groups(cfg: &AppConfig) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in &cfg.servers {
        if s.active && cfg.group(&s.group).active && !out.contains(&s.group) {
            out.push(s.group.clone());
        }
    }
    if out.is_empty() {
        out.push("default".into());
    }
    out
}

/// Opens a group window or focuses it when already open. Startup calls this once per group, and the
/// settings Show Group action calls it on demand, matching egui `App::show_group`. Geometry comes
/// from the saved layout or uses a cascade derived from `offset`.
///
/// Args:
///     cx: Application context used to create or focus the native window.
///     backend: Shared runtime state that owns group-window lifecycle registries.
///     cfg: Committed configuration used to construct the Shell and window appearance.
///     group: Unique group name to open or focus.
///     epoch: Shared chart/session time origin.
///     layout: Persisted geometry and window state.
///     offset: Cascade offset applied when no geometry was saved.
///
/// Returns:
///     Nothing; a successful new window publishes its final workspace transition immediately.
pub(crate) fn spawn_group_window(
    cx: &mut App,
    backend: &Entity<Backend>,
    cfg: &AppConfig,
    group: String,
    epoch: f64,
    layout: &WindowLayout,
    offset: f32,
) {
    spawn_group_window_inner(cx, backend, cfg, group, epoch, layout, offset, true);
}

/// Open one settings-managed group window without publishing an intermediate workspace revision.
///
/// Settings pre-registers every intended group as Opening, calls this for each group, and publishes
/// once after the complete registry is restored. All arguments otherwise match
/// [`spawn_group_window`].
///
/// Args:
///     cx: Application context used to create the native window.
///     backend: Shared runtime state with the pre-registered Opening group.
///     cfg: Committed configuration used to construct the Shell and window appearance.
///     group: Unique group name to create during the batch.
///     epoch: Shared chart/session time origin.
///     layout: Persisted geometry and window state.
///     offset: Cascade offset applied when no geometry was saved.
///
/// Returns:
///     Nothing; the caller owns the single publication after every intended open completes.
pub(crate) fn spawn_group_window_deferred(
    cx: &mut App,
    backend: &Entity<Backend>,
    cfg: &AppConfig,
    group: String,
    epoch: f64,
    layout: &WindowLayout,
    offset: f32,
) {
    spawn_group_window_inner(cx, backend, cfg, group, epoch, layout, offset, false);
}

/// Implement immediate and settings-batched group-window creation through one lifecycle path.
///
/// Args:
///     cx: Application context used to create or focus the native window.
///     backend: Shared runtime state that owns group-window lifecycle registries.
///     cfg: Committed configuration used to construct the Shell and window appearance.
///     group: Unique group name to create or focus.
///     epoch: Shared chart/session time origin.
///     layout: Persisted geometry and window state.
///     offset: Cascade offset applied when no geometry was saved.
///     publish_workspace_change: Whether a successful open publishes immediately.
///
/// Returns:
///     Nothing; failed opens remove their Opening marker without registering a handle.
fn spawn_group_window_inner(
    cx: &mut App,
    backend: &Entity<Backend>,
    cfg: &AppConfig,
    group: String,
    epoch: f64,
    layout: &WindowLayout,
    offset: f32,
    publish_workspace_change: bool,
) {
    // Focus an existing live window. handle.update returns an error for a window already closed.
    if let Some(handle) = backend.read(cx).group_windows.get(&group).copied() {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    // Do not open a Main market automatically at startup. Main starts empty until the user selects
    // one; the former behavior used server.market with BTCUSDT as fallback.
    let focus: Option<(CoreId, String)> = None;
    let saved = layout.groups.get(&group);
    let win_bounds = match saved {
        Some(g) => Bounds {
            origin: point(px(g.x as f32), px(g.y as f32)),
            size: size(px(g.w as f32), px(g.h as f32)),
        },
        None => Bounds {
            origin: point(px(80.0 + offset), px(80.0 + offset)),
            size: size(px(1280.0), px(720.0)),
        },
    };
    // Prefer the saved display UUID, which is stable across launches. If it is absent or unmatched,
    // try saved-origin containment on every platform. This is reliable where origins use global
    // coordinates; on macOS, display-relative x/y makes the fallback ambiguous but it still provides
    // a best-effort match for legacy layouts. Without a display_id, GPUI restores using the primary
    // display's scale, shifting or shrinking windows on displays with different DPI. MoonUI's GPUI
    // uses the target scale only when display_id is set, matching the detached-window round trip.
    let origin = win_bounds.origin;
    let saved_uuid = saved.and_then(|g| g.display_uuid.as_deref());
    let display_id = saved_uuid
        .and_then(|u| {
            cx.displays()
                .into_iter()
                .find(|d| d.uuid().ok().is_some_and(|du| du.to_string() == u))
        })
        .or_else(|| {
            cx.displays()
                .into_iter()
                .find(|d| d.bounds().contains(&origin))
        })
        .map(|d| d.id());
    let window_bounds = if saved.map(|g| g.fullscreen).unwrap_or(false) {
        WindowBounds::Fullscreen(win_bounds)
    } else if saved.map(|g| g.maximized).unwrap_or(false) {
        WindowBounds::Maximized(win_bounds)
    } else {
        WindowBounds::Windowed(win_bounds)
    };
    // Load the configured group-window icon from embedded `assets/icons/<id>.png`.
    let icon_id = cfg.group(&group).icon;
    let mut opts = windowing::trading_window_options(
        "MoonTerminal",
        &group,
        icon_id,
        window_bounds,
        display_id,
        Some(size(px(520.0), px(340.0))),
    );
    opts.window_background = WindowBackgroundAppearance::Opaque;
    // Clear with the theme's chart background. Otherwise scene-uncovered pixels use the renderer's
    // white default and flash during startup or resize and beneath the chart, where an own-pass
    // UnderScene layer cannot be covered by a later background.
    let cbg = cfg.chart_theme().bg;
    opts.window_clear_color = Some(gpui::rgb(
        ((cbg[0] as u32) << 16) | ((cbg[1] as u32) << 8) | cbg[2] as u32,
    ));
    // `open_window` constructs Shell before returning its handle. Record that narrow lifecycle
    // interval so a restored Auto core remains available during the first render.
    backend.update(cx, |bk, _| {
        bk.opening_group_windows.insert(group.clone());
    });
    let theme = cfg.chart_theme().clone();
    let b = backend.clone();
    let g = group.clone();
    let opened = cx.open_window(opts, move |window, cx| {
        windowing::configure_dwm_window(window);
        windowing::configure_shell_clear_color(window, cx);
        windowing::set_group_window_icon(window, icon_id);
        let view = cx.new(|cx| Shell::new(b, g, focus, epoch, theme, window, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::NoFill))
    });
    if let Ok(handle) = opened {
        backend.update(cx, |bk, bcx| {
            bk.opening_group_windows.remove(&group);
            bk.group_windows.insert(group.clone(), handle);
            if publish_workspace_change {
                bk.publish_workspace_window_change(&[], bcx);
            }
        });
        // A group window owns the detached panel windows of its group: they are OS-owned children
        // and died with the previous one, while their `DetachedSpec`s survived and the restored
        // dock does NOT contain those panels. Reclaiming them here rather than at the call sites is
        // what keeps every route whole — startup, the Settings rebuild and reconcile, and the
        // Settings "show group" button, which is reached by no other restore path. The reopen is
        // deferred and skips panels that already have a window, so the routes that also restore
        // panels themselves cannot produce a duplicate.
        crate::window::detached::respawn_all(backend, cx);
    } else {
        backend.update(cx, |bk, _| {
            bk.opening_group_windows.remove(&group);
        });
    }
}
