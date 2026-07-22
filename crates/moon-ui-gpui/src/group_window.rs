//! Group-window support: enumerate active groups and open or focus their windows. Ported from egui
//! `App::show_group` and extracted from `main.rs`.

use gpui::*;

use moon_ui::{MoonBackgroundPolicy, Root};

use moon_core::config::{AppConfig, WindowLayout};
use moon_core::session::CoreId;

use crate::Backend;
use crate::shell::Shell;
use crate::windowing;

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
pub(crate) fn spawn_group_window(
    cx: &mut App,
    backend: &Entity<Backend>,
    cfg: &AppConfig,
    group: String,
    epoch: f64,
    layout: &WindowLayout,
    offset: f32,
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
    let theme = cfg.chart_theme().clone();
    let b = backend.clone();
    let g = group.clone();
    if let Ok(handle) = cx.open_window(opts, move |window, cx| {
        windowing::configure_dwm_window(window);
        windowing::configure_shell_clear_color(window, cx);
        windowing::set_group_window_icon(window, icon_id);
        let view = cx.new(|cx| Shell::new(b, g, focus, epoch, theme, window, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::NoFill))
    }) {
        backend.update(cx, |bk, _| {
            bk.group_windows.insert(group, handle);
        });
    }
}
