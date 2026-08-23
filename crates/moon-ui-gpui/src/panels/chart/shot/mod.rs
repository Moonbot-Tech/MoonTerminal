//! Copying the active chart to the system clipboard — Moonbot's "make shot".
//!
//! The whole path, end to end:
//!
//! 1. **Which chart.** [`resolve`] takes the chart under the pointer when there is one, and
//!    otherwise the last chart the pointer was over IN THE SAME OS WINDOW
//!    (`Backend::last_chart`). Both are scoped to the window the keystroke actually reached: a
//!    hotkey is not a cursor gesture, and the chart hovered most recently anywhere may sit in a
//!    window that is now behind this one, where the pixels belong to whatever covers it.
//! 2. **Which region.** The chart engine publishes its `gpu_canvas` slot every frame
//!    (`chartdx::data_state::state::apply_slot_geometry`); [`rect::slot_capture_rect`] turns that
//!    into physical client pixels. The slot is the plot, the price and time axes, the order book
//!    and the corner caption — the chart and nothing of the panel's chrome around it.
//! 3. **Which surface.** On Windows, the composited desktop, through GDI. NOT the chart's own
//!    render target: see `win.rs` for why the own pass cannot supply a finished picture.
//! 4. **How it reaches the clipboard.** One ownership window publishing both `CF_DIB` and the
//!    registered `"PNG"` format (`clipboard_win.rs`). Nothing is written to disk at any step.
//!
//! Every outcome is reported to the user through a notification, because a hotkey that copies
//! something has no other way to say whether it worked.
//!
//! If capturing an OCCLUDED or off-screen chart ever becomes a requirement, the upgrade path is
//! Windows Graphics Capture — `windows-capture` is already in `Cargo.lock` under `zed-scap`, though
//! it is not built on the Windows target today and would bring an async frame pool with it.

pub(crate) mod rect;

#[cfg(windows)]
mod caption;
#[cfg(windows)]
mod clipboard_win;
#[cfg(windows)]
pub(crate) mod win;

#[cfg(not(windows))]
mod unsupported;

use gpui::{AnyWindowHandle, App, Entity};
use moon_ui::{MoonNotification, MoonWindowExt as _};
use rust_i18n::t;

use crate::Backend;
use crate::panels::ChartPanel;

/// What one press of the chart-shot hotkey achieved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShotOutcome {
    /// The picture is on the clipboard.
    Copied,
    /// No chart in this window to shoot, or it has never been painted.
    NoChart,
    /// This platform has no capture path. Only the non-Windows arm produces it, so on Windows it
    /// is matched but never constructed - which is the gate working, not dead code.
    #[cfg_attr(windows, allow(dead_code))]
    Unsupported,
    /// A chart was found and the capture failed; the reason is in the log.
    Failed,
}

impl ShotOutcome {
    /// The notification this outcome shows.
    ///
    /// Only [`Self::Copied`] is a success. The remaining outcomes use an error notification so the
    /// user knows not to paste; no persisted application state was lost.
    ///
    /// Returns:
    ///     The localized notification describing this outcome.
    fn notification(self) -> MoonNotification {
        match self {
            Self::Copied => MoonNotification::success(t!("hotkeys.chart_shot_done").to_string()),
            Self::NoChart => MoonNotification::error(t!("hotkeys.chart_shot_no_chart").to_string()),
            Self::Unsupported => {
                MoonNotification::error(t!("hotkeys.chart_shot_unsupported").to_string())
            }
            Self::Failed => MoonNotification::error(t!("hotkeys.chart_shot_failed").to_string()),
        }
    }
}

/// Copy the active chart of `window` to the clipboard and tell the user what happened.
///
/// Args:
///     backend: Application backend, holding the hovered and last-hovered charts.
///     window: The window the keystroke arrived at, which is also the window captured.
///     cx: Application context used to resolve entities and show the notification.
///
/// Returns:
///     Always `true`: the key is ours whether or not a picture resulted, so it never falls through
///     to whatever else the keystroke might mean in a focused text field.
pub(crate) fn copy_active_chart(
    backend: &Entity<Backend>,
    window: &mut gpui::Window,
    cx: &mut App,
) -> bool {
    let handle = window.window_handle();
    // Read the native handle HERE, off the `&mut Window` already in hand. Resolving it downstream
    // through `handle.update(cx, ..)` cannot work: this function is itself called from inside that
    // window's update, and gpui refuses the re-entrant borrow — which surfaced as a capture that
    // always reported "no HWND". The working precedent is `panels/chart/render.rs`, which reads it
    // straight off the `&Window` the frame already holds.
    let native = crate::window::windowing::window_hwnd(window);
    // `None` means the Windows arm took the shot over: it has to substitute the caption, wait for
    // that to reach the screen and only then photograph it, so it reports its own outcome when the
    // picture exists rather than here, several frames too early.
    let Some(outcome) = run(backend, handle, native, window, cx) else {
        return true;
    };
    if let ShotOutcome::Failed | ShotOutcome::NoChart = outcome {
        log::info!("chart shot: {outcome:?}");
    }
    // Pushed AFTER the capture has already been taken, so the notification cannot appear in the
    // picture it is announcing.
    window.push_notification(outcome.notification(), cx);
    true
}

/// Resolve the chart and dispatch the capture to the platform arm.
///
/// Args:
///     backend: Application backend holding the per-window hover trail.
///     window: OS window whose chart slot is captured.
///     native: That window's native handle, resolved by the caller while it still holds the
///         `&mut Window` — see `copy_active_chart` for why it cannot be resolved here.
///     gpui_window: The live window, which the Windows arm needs to schedule the frame callbacks
///         it waits on before capturing.
///     cx: Application context used to read the chosen chart.
///
/// Returns:
///     The outcome to announce now, or `None` when the Windows arm took over and will announce its
///     own once it has a picture.
fn run(
    backend: &Entity<Backend>,
    window: AnyWindowHandle,
    native: Option<isize>,
    gpui_window: &mut gpui::Window,
    cx: &mut App,
) -> Option<ShotOutcome> {
    let Some(panel) = resolve(backend, window, cx) else {
        return Some(ShotOutcome::NoChart);
    };

    #[cfg(windows)]
    {
        let Some(native) = native else {
            log::warn!("chart shot: no HWND for the chart's window");
            return Some(ShotOutcome::Failed);
        };
        // Geometry is deliberately NOT read here. The caption has to be swapped and drawn first,
        // and by the time that frame exists this one would be several frames stale, so the Windows
        // arm reads it again next to the capture itself.
        caption::begin(backend.clone(), panel, window, native, gpui_window, cx);
        None
    }
    #[cfg(not(windows))]
    {
        // No capture path on this platform, so nothing would ever photograph a substituted
        // caption. Answer immediately instead of swapping a caption and waiting for a frame that
        // will only be thrown away.
        let _ = (native, gpui_window);
        let Some((bounds, scale, device_size)) = panel.read(cx).shot_geometry() else {
            // Painted geometry is published per frame, so a chart in a tab that has never been
            // shown has none. Reported as "no chart" rather than a failure: nothing went wrong.
            return Some(ShotOutcome::NoChart);
        };
        let Some(rect) = rect::slot_capture_rect(bounds, scale, device_size) else {
            return Some(ShotOutcome::NoChart);
        };
        Some(unsupported::capture_to_clipboard(window, rect, cx))
    }
}

/// Choose the chart this window's keystroke means: the last one its pointer entered.
///
/// `Backend::hovered_chart` is deliberately NOT consulted, even though every other cursor-addressed
/// hotkey routes through it. It would answer the same thing and only sometimes: its single writer
/// (`panels::chart::render_input::hover`) records `last_chart[window]` in the very same branch, so a
/// hovered chart always equals this window's entry — while the pointer is inside it. Once the
/// pointer leaves, `hovered_chart` is cleared and this entry is not, which is the whole reason the
/// shot reads this one. A hotkey is not a cursor gesture: it is pressed after the hand has moved to
/// a toolbar, a settings field, or off the chart entirely.
///
/// Scoped BY WINDOW because `hovered_chart` is application-wide while a keystroke is not — the
/// chart hovered most recently anywhere may sit in a window now behind this one, where the pixels
/// belong to whatever covers it.
///
/// A window whose charts have never been hovered at all resolves to nothing and the user is told
/// so. Naming the group's active Main chart instead would need an `Entity<ChartPanel>` that neither
/// `Backend::main_chart_target` nor `AddChartStack` exposes — both answer with a `(core, market)`
/// identity — and inventing that plumbing buys only a state the user leaves by moving the mouse
/// once.
///
/// Args:
///     backend: Application backend holding `last_chart` entries.
///     window: OS window whose most recently entered chart is requested.
///     cx: Application context used to read the backend.
///
/// Returns:
///     The live chart entity for `window`, if one has been entered and not closed.
fn resolve(
    backend: &Entity<Backend>,
    window: AnyWindowHandle,
    cx: &App,
) -> Option<Entity<ChartPanel>> {
    backend
        .read(cx)
        .last_chart
        .get(&window)
        .and_then(|weak| weak.upgrade())
}

/// The Windows arm: capture the rectangle off the desktop and publish it.
///
/// Args:
///     native: Native handle of the window the chart is DRAWN in — not whichever window has focus,
///         so a detached chart is captured against its own client area. Resolved by
///         `copy_active_chart` before this window's update begins, and already checked for absence
///         by `run`: the caption wait would have nothing to arm without it, so the missing-handle
///         refusal happens there rather than being carried down here as an `Option`.
///     rect: Physical client-area rectangle occupied by the chart slot.
///
/// Returns:
///     Whether capture and clipboard publication succeeded.
#[cfg(windows)]
fn capture_windows(native: isize, rect: rect::ShotRect) -> ShotOutcome {
    use windows::Win32::Foundation::HWND;

    let hwnd = HWND(native as *mut std::ffi::c_void);

    let image = match win::capture_client_rect(hwnd, rect) {
        Ok(image) => image,
        Err(error) => {
            log::warn!("chart shot: capture failed: {error:#}");
            return ShotOutcome::Failed;
        }
    };
    match clipboard_win::publish(hwnd, &image) {
        Ok(()) => ShotOutcome::Copied,
        Err(error) => {
            log::warn!("chart shot: clipboard write failed: {error:#}");
            ShotOutcome::Failed
        }
    }
}
