//! Opening, restoring and remembering the Profit Monitor's own desktop window.
//!
//! Split from `mod.rs`, which owns the view inside this window. What lives here is the singleton
//! lifecycle: focus an existing one, restore saved geometry, record that it is open so the next
//! launch reopens it, and keep the startup path from stealing the foreground.

use gpui::*;
use moon_ui::{MoonBackgroundPolicy, Root};
use rust_i18n::t;

use super::{MIN_WINDOW_WIDTH, ProfitMonitorView};
use crate::Backend;
use crate::design;

#[cfg(test)]
mod tests;

/// Single native-create authority with foreground intent that can only be upgraded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ProfitMonitorOpenRequest {
    /// Whether a user action requires the eventual live window to take foreground.
    activate: bool,
}

impl ProfitMonitorOpenRequest {
    /// Create the first request in an otherwise idle single-flight slot.
    ///
    /// Args:
    ///     activate: Whether this producer is an explicit user action.
    ///
    /// Returns:
    ///     One pending request carrying the producer's foreground intent.
    fn new(activate: bool) -> Self {
        Self { activate }
    }

    /// Merge a later caller without allowing startup restore to downgrade a user click.
    ///
    /// Args:
    ///     activate: Foreground intent of the later caller.
    ///
    /// Returns:
    ///     Nothing; explicit activation is retained monotonically in place.
    fn upgrade(&mut self, activate: bool) {
        self.activate |= activate;
    }
}

/// Open or focus the independent singleton Profit Monitor window.
///
/// This toolbar action is one route back to the taskbar-hidden monitor; Alt+Tab is the other.
/// `activate_window` restores an iconic window before foregrounding it, so it reopens a monitor
/// that the user minimized.
///
/// Args:
///     backend: Shared terminal state retaining the singleton handle.
///     owner: Launching Main window, used only to choose the initial display.
///     owner_display: Display captured by the toolbar click.
///     cx: Application context used to create or activate the window.
///
/// Returns:
///     Nothing; the singleton window is focused or created as a side effect.
pub(crate) fn open(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    open_window(backend, owner, owner_display, true, cx);
}

/// Reopen the monitor at launch because the previous session left it open.
///
/// Separate from [`open`] for one reason: it must NOT activate. `activate_new_window` exists for an
/// explicit user action and its own documentation forbids bulk startup restoration, where each
/// restored window steals the foreground from the one before it — here, from Main.
///
/// Args:
///     backend: Shared terminal state retaining the singleton handle.
///     owner: A window already on screen, used only to choose a display.
///     cx: Application context used to create the window.
pub(crate) fn restore(backend: Entity<Backend>, owner: Option<AnyWindowHandle>, cx: &mut App) {
    open_window(backend, owner, None, false, cx);
}

/// Open or focus the monitor, activating it only for an explicit user action.
///
/// Args:
///     backend: Shared terminal state retaining the singleton handle.
///     owner: Launching window, used only to choose the initial display.
///     owner_display: Display captured by the caller.
///     activate: Whether a newly created window should take the foreground.
///     cx: Application context used to create or activate the window.
fn open_window(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    activate: bool,
    cx: &mut App,
) {
    if let Some(handle) = backend.read(cx).profit_monitor_window {
        // Liveness is probed with an EMPTY update, and the window is raised only for a deliberate
        // action. Activating here regardless would put the restored monitor in front of Main on
        // every launch — the very thing `restore` exists to avoid.
        let alive = if activate {
            handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        } else {
            handle.update(cx, |_, _, _| ()).is_ok()
        };
        if alive {
            mark_open(&backend, cx);
            return;
        }
    }
    let scheduled = backend.update(cx, |backend, _| {
        if let Some(request) = backend.profit_monitor_open_pending.as_mut() {
            request.upgrade(activate);
            return false;
        }
        backend.profit_monitor_window = None;
        backend.profit_monitor_open_pending = Some(ProfitMonitorOpenRequest::new(activate));
        true
    });
    if !scheduled {
        return;
    }

    cx.spawn(async move |cx| {
        let executor = cx.update(|cx| cx.background_executor().clone());
        // A real timer yield lets the toolbar or startup transaction paint before `open_window`
        // performs the new window's mandatory synchronous first draw. GPUI drains chained `defer`
        // callbacks in the same application turn, so `defer` is not an equivalent boundary.
        executor.timer(std::time::Duration::from_millis(1)).await;
        cx.update(|cx| {
            if backend.read(cx).quitting {
                backend.update(cx, |backend, _| {
                    backend.profit_monitor_open_pending = None;
                });
                return;
            }
            let Some(request) = backend.read(cx).profit_monitor_open_pending else {
                return;
            };
            if let Some(handle) = backend.read(cx).profit_monitor_window {
                let alive = if request.activate {
                    handle
                        .update(cx, |_, window, _| window.activate_window())
                        .is_ok()
                } else {
                    handle.update(cx, |_, _, _| ()).is_ok()
                };
                if alive {
                    backend.update(cx, |backend, _| {
                        backend.profit_monitor_open_pending = None;
                    });
                    mark_open(&backend, cx);
                    return;
                }
            }

            // Saved geometry is restored as-is. The platform rescues a rectangle whose centre no
            // longer belongs to a display; a second origin-based guard incorrectly moved valid
            // negative-x windows and overwrote their saved position.
            let saved = backend.read(cx).layout.profit_monitor_window;
            let bounds = saved.map_or(
                Bounds {
                    origin: point(px(160.0), px(120.0)),
                    size: size(px(720.0), px(520.0)),
                },
                |geometry| Bounds {
                    origin: point(px(geometry.x as f32), px(geometry.y as f32)),
                    size: size(px(geometry.w as f32), px(geometry.h as f32)),
                },
            );
            let display_id = crate::window::windowing::saved_or_owner_display_id(
                saved.and_then(|geometry| geometry.display_uuid),
                saved.map(|geometry| point(px(geometry.x as f32), px(geometry.y as f32))),
                owner,
                owner_display,
                cx,
            );
            let options = crate::window::windowing::profit_monitor_window_options(
                t!("profit_monitor.window_title").to_string(),
                crate::window::windowing::restored_window_bounds(saved, bounds),
                display_id,
                Some(size(design::ui_px(cx, MIN_WINDOW_WIDTH), px(320.0))),
            );
            let view_backend = backend.clone();
            let opened = cx.open_window(options, move |window, cx| {
                crate::window::windowing::configure_shell_clear_color(window, cx);
                crate::window::windowing::set_group_window_icon(window, 0);
                let view = cx.new(|cx| ProfitMonitorView::new(view_backend, window, cx));
                cx.new(|cx| {
                    Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque)
                })
            });
            backend.update(cx, |backend, _| {
                backend.profit_monitor_open_pending = None;
                backend.profit_monitor_window = opened.as_ref().ok().copied();
            });
            if let Ok(handle) = opened {
                mark_open(&backend, cx);
                if request.activate {
                    crate::window::windowing::activate_new_window(handle.into(), cx);
                }
            }
        });
    })
    .detach();
}

/// Record that the monitor is open so the next launch reopens it.
///
/// Written only after a window actually exists: a failed `open_window` that still set the flag
/// would make every subsequent startup retry the same failure and log the same error.
///
/// Args:
///     backend: Shared terminal state holding the layout.
///     cx: Application context used to persist.
fn mark_open(backend: &Entity<Backend>, cx: &mut App) {
    backend.update(cx, |backend, _| {
        if backend.layout.profit_monitor_open {
            return;
        }
        backend.layout.profit_monitor_open = true;
        backend.layout_dirty = true;
    });
}
