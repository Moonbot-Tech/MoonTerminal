//! Strategies window: the header and the open/goto entry points.

use super::*;

pub(super) const STRATEGIES_HEADER_H: f32 = 32.0;

pub(super) fn strategies_header(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .id("strategies-window-header")
        .relative()
        .flex_none()
        .w_full()
        .h(design::fit_h_px(cx, STRATEGIES_HEADER_H, 14.0, 9.0))
        .justify_between()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .bg(moon(p.shell_high))
        .border_b(px(1.0))
        .border_color(moon_alpha(p.border, 1.0))
        .child(
            MoonWindowFrame::tool("strategies-titlebar-title", 0.0)
                .title_cluster(t!("strat.window_title").to_string(), cx)
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .when(design::show_custom_window_controls(), |this| {
            this.child(
                MoonWindowFrame::tool("strategies-window-frame-visual", 0.0)
                    .header_height(STRATEGIES_HEADER_H)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}

/// Which strategy a `Backend::strategies_goto` request points at.
///
/// Two forms because the two kinds of caller know different things: a chart order line or an
/// Orders row already holds the id, while a just-created copy does not — its id is assigned by
/// the core and only arrives with the echo, so it can be named but not numbered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RevealTarget {
    /// A strategy the caller already knows the id of.
    Id(u64),
    /// A strategy identified by its name on that core.
    Name(String),
}

/// Open or focus the Strategies window and navigate to `strat_id` on `core`.
/// Render drains the request, disables the active-only filter, expands the target core and folders,
/// and selects the strategy. Entry points include chart order-line context menus and the Orders
/// table's Strat column.
pub fn open_goto(
    backend: Entity<Backend>,
    core: CoreId,
    strat_id: u64,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    backend.update(cx, |b, bcx| {
        b.strategies_goto = Some((core, RevealTarget::Id(strat_id)));
        // Wake an existing window's observer because `open` only focuses a deduplicated window.
        bcx.notify();
    });
    open(backend, owner, owner_display, cx);
}

/// Ask an ALREADY-OPEN Strategies window to reveal a strategy by name.
///
/// By name because the caller cannot know the id: a created strategy's id is assigned core-side
/// and only arrives with the snapshot echo.
///
/// Deliberately does NOT open the window. A dialog in another window that spawns a second window
/// is a surprise, and a request parked on `Backend` for a window that is never opened would fire
/// hours later and yank the user's selection out from under them.
///
/// Liveness is proved by UPDATING the handle rather than by `is_some()`: nothing clears
/// `strategies_window` when the user closes the window, so the handle outlives it.
///
/// Returns:
///     Whether a live window accepted the request.
pub fn reveal_name(backend: &Entity<Backend>, core: CoreId, name: String, cx: &mut App) -> bool {
    let Some(handle) = backend.read(cx).strategies_window else {
        return false;
    };
    if handle.update(cx, |_, _, _| ()).is_err() {
        return false;
    }
    backend.update(cx, |b, bcx| {
        b.strategies_goto = Some((core, RevealTarget::Name(name)));
        bcx.notify();
    });
    true
}

/// Open the Strategies tool window, deduplicated through `Backend`.
pub fn open(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    // Focus an existing window.
    if let Some(handle) = backend.read(cx).strategies_window {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    // The tool window behaves as part of the terminal rather than a separate taskbar application.
    // Restore geometry persisted by `StrategiesView`.
    let saved = backend.read(cx).layout.strategies_window;
    let bounds = saved.map_or(
        Bounds {
            origin: point(px(120.0), px(90.0)),
            size: size(px(1180.0), px(680.0)),
        },
        |g| Bounds {
            origin: point(px(g.x as f32), px(g.y as f32)),
            size: size(px(g.w as f32), px(g.h as f32)),
        },
    );
    // Choose a display from the saved position where supported, otherwise from the owner. Without a
    // display id, GPUI creates the window on the primary display and may discard off-screen bounds.
    let display_id = crate::window::windowing::saved_or_owner_display_id(
        saved.map(|g| point(px(g.x as f32), px(g.y as f32))),
        owner,
        owner_display,
        cx,
    );
    let mut opts = crate::window::windowing::tool_window_options(
        t!("strat.window_title").to_string(),
        WindowBounds::Windowed(bounds),
        Some(size(px(920.0), px(560.0))),
        owner,
    );
    opts.display_id = display_id;
    let b = backend.clone();
    if let Ok(handle) = cx.open_window(opts, move |window, cx| {
        crate::window::windowing::configure_shell_clear_color(window, cx);
        let view = cx.new(|cx| StrategiesView::new(b, window, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    }) {
        backend.update(cx, |bk, _| bk.strategies_window = Some(handle));
        crate::window::windowing::activate_new_window(handle.into(), cx);
    }
}
