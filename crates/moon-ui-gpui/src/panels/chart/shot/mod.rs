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
//! 4. **What is added to it.** The picture is normalized to a bounded final size with a good filter
//!    (`resize.rs`), then a single header strip is BURNT IN with GDI (`paint_win.rs`) - the coin,
//!    the venue, the moment, the timeframe, the chart's own Y-scale badge and the movement
//!    figures, centred, so the picture still explains itself once it has left the application.
//!    The strip is not one run of text: it is three GROUPS - identity, the view, the market - in
//!    two sizes and two weights on a band of its own, ranked by `header.rs` and coloured by
//!    `ink.rs` against a stated contrast floor so it holds in every chart theme.
//! 5. **Where it goes.** The CLIPBOARD, and nowhere else: `CF_DIB` and the registered `"PNG"`
//!    format in a single ownership window (`clipboard_win.rs`). **No step touches the disk and no
//!    step leaves the UI thread.** The shot writes no file — the size rule in `resize.rs` is
//!    therefore the whole of the defence against a messenger's own recompression, which is why it
//!    matters more here than it would beside a file the user could send as a document.
//!
//! Every outcome is reported to the user through a notification, because a hotkey that copies
//! something has no other way to say whether it worked.
//!
//! If capturing an OCCLUDED or off-screen chart ever becomes a requirement, the upgrade path is
//! Windows Graphics Capture — `windows-capture` is already in `Cargo.lock` under `zed-scap`, though
//! it is not built on the Windows target today and would bring an async frame pool with it.

// `pub(crate)` rather than private: the UI-atlas run re-exports `ShotRect` through
// `panels/mod.rs` under `cfg(uidoc)`.
pub(crate) mod rect;
// Platform-neutral like `rect`: pure arithmetic and pure string work, so their unit tests run on
// every platform even though only the Windows arm produces a picture to feed them today. The
// allow is the mirror image of the one on `ShotOutcome::Unsupported` below - on a non-Windows
// build nothing calls into them, and that is the gate working rather than dead code.
#[cfg_attr(not(windows), allow(dead_code))]
mod header;
#[cfg_attr(not(windows), allow(dead_code))]
mod ink;
#[cfg_attr(not(windows), allow(dead_code))]
mod resize;

#[cfg(windows)]
mod caption;
#[cfg(windows)]
mod clipboard_win;
#[cfg(windows)]
mod paint_win;
// `pub(crate)` for the same reason as `rect` above: the UI-atlas re-export needs `DibImage` and
// `capture_client_rect`.
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
    /// The picture is on the clipboard. That is the whole of a success, so this variant carries
    /// nothing.
    ///
    /// An earlier version REFUSED to offer a verdict-free "copied", and it was right to: every
    /// Windows success then attempted a file, so a bare "copied" would have been the hole a later
    /// edit fell into, reporting success while silently writing nothing. **That argument is void
    /// rather than weakened** — its premise is gone. No success attempts a file any more, so there
    /// is nothing left for a bare "copied" to under-report.
    ///
    /// If a file ever comes back it comes back as its own variant carrying its own path. This one
    /// must not be stretched to mean both: that stretching is the failure mode the old comment was
    /// guarding against, and it outlives the thing it guarded.
    #[cfg_attr(not(windows), allow(dead_code))]
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
    /// Two registers, not three. [`Self::Copied`] is the one success and says only that, because
    /// the clipboard is now the only place the picture goes and there is no second artifact whose
    /// fate could differ. The rest stay errors so the user knows not to paste.
    ///
    /// The middle register this used to have — a WARNING for "on the clipboard, but the file
    /// failed" — went with the file. It existed because a shot could half-succeed; nothing can
    /// half-succeed now.
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

/// Everything the burnt-in header states, snapshotted off the chart in ONE read.
///
/// Assembled by [`crate::panels::ChartPanel::shot_inputs`] rather than gathered field by field at
/// the capture site. The chart owns this metadata — the ticker is the renderer's cached caption
/// value, the venue is what the privacy substitution puts on the picture, the movement windows are
/// addressed by a catalogue order only the chart layer knows — and splitting that ownership across
/// the shot's async wait chain is how two of them end up describing different moments.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) struct ShotInputs {
    /// Ticker as the chart's own corner caption spells it. RAW wire text.
    pub(crate) coin: Option<String>,
    /// Exchange caption — the SAME value `caption` substitutes onto the picture.
    pub(crate) venue: String,
    /// Candle timeframe in minutes.
    pub(crate) tf_min: u32,
    /// The chart's background colour.
    pub(crate) bg: [u8; 3],
    /// The chart's supporting-text colour.
    pub(crate) text: [u8; 3],
    /// Unsigned price movement over the last three hours, in percent.
    pub(crate) delta_3h: Option<f64>,
    /// Unsigned price movement over the last hour, in percent.
    pub(crate) delta_1h: Option<f64>,
    /// Unsigned price movement over the last fifteen minutes, in percent.
    pub(crate) delta_15m: Option<f64>,
    /// The chart's own Y-scale badge, as a whole percentage of the visible range.
    ///
    /// The SAME value the chart draws beside its coin badge, read rather than recomputed, so the
    /// burnt-in figure cannot disagree with the badge visible in the picture under it. `None` means
    /// the chart is hiding the badge, which is not the same fact as a zero — see
    /// `header::scale_field`.
    pub(crate) scale_pct: Option<i32>,
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
        Some(unsupported::refuse(window, rect, cx))
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

/// The Windows arm: capture the slot, burn the strip in, normalize the size and publish.
///
/// The pipeline, in this order and for these reasons:
///
/// 1. `win::capture_client_rect` — untouched, still the lossless 1:1 read of exactly the slot.
/// 2. `to_rgb_top_down` → `resize::normalize` — the ONE downscale, taken with a good filter so the
///    messenger's own resampler has nothing left to do.
/// 3. `win::rgb_to_dib` → `paint_win::draw_strips` — the strip is drawn AFTER the resize, at final
///    resolution. Drawing it first and resampling afterwards would blur the very text this feature
///    exists to add.
/// 4. `clipboard_win::publish` — the picture goes to the clipboard, and that is the end of it.
///    The PNG encode is INSIDE that call rather than a step here: it exists only to serve the
///    registered `"PNG"` clipboard format, so the module that owns the hand-off owns the bytes.
///    It briefly lived out here, when a file needed the same bytes and encoding twice would have
///    doubled the slowest step in the shot; there is no second consumer any more.
///
/// Every intermediate failure logs the step that failed and answers [`ShotOutcome::Failed`]. There
/// is no partial success: a picture with no strip, or a strip with no picture, is not what the
/// user asked for and would be harder to notice than an honest failure.
///
/// Args:
///     native: Native handle of the window the chart is DRAWN in — not whichever window has focus,
///         so a detached chart is captured against its own client area. Resolved by
///         `copy_active_chart` before this window's update begins, and already checked for absence
///         by `run`: the caption wait would have nothing to arm without it, so the missing-handle
///         refusal happens there rather than being carried down here as an `Option`.
///     rect: Physical client-area rectangle occupied by the chart slot.
///     inputs: What the chart knew at this instant, snapshotted in one read.
///     when_ms: The capture instant, as stated in the header.
///
/// Returns:
///     The outcome to announce.
#[cfg(windows)]
fn capture_windows(
    native: isize,
    rect: rect::ShotRect,
    inputs: &ShotInputs,
    when_ms: i64,
) -> ShotOutcome {
    use windows::Win32::Foundation::HWND;

    let hwnd = HWND(native as *mut std::ffi::c_void);

    macro_rules! fail {
        ($step:literal, $error:expr) => {{
            log::warn!("chart shot: {} failed: {:#}", $step, $error);
            return ShotOutcome::Failed;
        }};
    }

    let captured = match win::capture_client_rect(hwnd, rect) {
        Ok(image) => image,
        Err(error) => fail!("capture", error),
    };

    let normalized = resize::normalize(resize::RgbFrame {
        width: captured.width,
        height: captured.height,
        rgb: captured.to_rgb_top_down(),
    });
    let Some(body) = win::rgb_to_dib(normalized.width, normalized.height, &normalized.rgb) else {
        fail!(
            "repacking the normalized picture",
            anyhow::anyhow!(
                "{}x{} does not match {} bytes",
                normalized.width,
                normalized.height,
                normalized.rgb.len()
            )
        )
    };

    let zone = crate::chartdx::axes::display_zone();
    let composed = match paint_win::draw_strips(
        &body,
        &header::header_strip(&header::HeaderInputs {
            coin: inputs.coin.clone(),
            venue: inputs.venue.clone(),
            when_ms,
            zone,
            tf_min: inputs.tf_min,
            scale_pct: inputs.scale_pct,
            delta_3h: inputs.delta_3h,
            delta_1h: inputs.delta_1h,
            delta_15m: inputs.delta_15m,
        }),
        &paint_win::ShotStyle {
            bg: inputs.bg,
            text: inputs.text,
        },
    ) {
        Ok(image) => image,
        Err(error) => fail!("drawing the header", error),
    };

    if let Err(error) = clipboard_win::publish(hwnd, &composed) {
        fail!("clipboard write", error);
    }

    ShotOutcome::Copied
}
