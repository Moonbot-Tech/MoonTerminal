//! Waiting for the substituted caption to reach the screen, then taking the picture.
//!
//! The shot names the EXCHANGE where the chart normally names the core. The core name is the
//! user's own account label and these pictures get shared; the exchange is the part that means
//! something to a stranger. Only the CAPTURED frame changes - the chart the user is looking at
//! keeps its core name a fraction of a second later.
//!
//! Because the capture reads the composited desktop rather than the chart's own render target
//! (`super::win`), the substitution cannot be applied to the bitmap afterwards. It has to be
//! DRAWN and then photographed, which makes this an asynchronous sequence rather than the
//! straight-line one the hotkey used to run:
//!
//! 1. Arm the override on the engine and force an order sync, so the venue caption is current.
//! 2. Wait, one frame callback at a time, until the renderer-side pre-capture proof is ready.
//!    **Never capture on a timer alone** - a pane that is not presentable draws nothing, and
//!    photographing it blind would publish the account name.
//! 3. Cross a compositor boundary with `DwmFlush`, so the presented frame is on the desktop the
//!    capture reads.
//! 4. Capture, restore the caption, and only then tell the user - a notification pushed any
//!    earlier would be standing in the picture it is announcing.
//!
//! What the renderer can and cannot promise is the sharp edge of this design. There is no
//! post-present hook anywhere in the fork, so "presented" is inferred rather than observed:
//! `chartdx` counts completed text passes that drew the substituted caption and only answers yes
//! at two of them, restarting the count whenever the DirectX device generation moves. That closes
//! the concrete hole - a device recovery makes the renderer skip `draw` entirely for one frame
//! while the text pass still runs, so a one-frame proof could photograph the previous frame and
//! its account name.
//!
//! **The residual is real and is stated rather than hidden:** `Present` failures are logged and
//! swallowed inside the fork's window, so no signal reaches here. Two drawn frames plus a
//! compositor flush make a leak require two consecutive silently-failed presents, which is a much
//! smaller window than one, but it is not zero. Closing it properly needs a present-completion
//! signal in MoonUI, and that is where the fix belongs if this ever proves reachable.
//!
//! This module is Windows-only on purpose. `DwmFlush` is a Windows compositor primitive and the
//! capture below it is the Windows arm; platforms without a capture path answer `Unsupported`
//! immediately in `super::run` and never arm a caption they cannot photograph.

use std::time::{Duration, Instant};

use gpui::{AnyWindowHandle, App, Entity};
use moon_ui::MoonWindowExt as _;
use windows::Win32::Graphics::Dwm::DwmFlush;

use super::{ShotOutcome, rect};
use crate::Backend;
use crate::panels::ChartPanel;

/// How long the renderer keeps the substituted caption before restoring it by itself.
///
/// The watchdog behind the explicit restore below, not the mechanism: it only matters when this
/// module's callback chain never finishes - a window closed mid-shot, a stalled machine. Generous
/// next to [`WAIT_TICKS`] so the deadline can never expire while the wait is still legitimately
/// running.
const SHOT_CAPTION_TTL: Duration = Duration::from_secs(1);

/// How many frame callbacks the wait spends before giving up.
///
/// About 200 ms at 60 Hz. It has to cover more than the one frame the swap itself needs: arming
/// forces an order sync, which lands on a GPUI render pass, and the canvas tick that draws the new
/// caption comes after it. A chart that is merely slow gets its picture; one that is not drawing
/// at all is refused rather than photographed blind.
const WAIT_TICKS: u32 = 12;

/// What the wait should do after one frame callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WaitStep {
    /// The renderer-side pre-capture proof is ready; take the picture.
    Capture,
    /// Not yet, and there is budget left.
    Wait,
    /// Not drawn and out of budget. Refuse rather than capture.
    GiveUp,
}

/// Decide the next move from the renderer's proof and the remaining budget.
///
/// Split out as a free function because it is the one part of this file that can be tested without
/// a window, and because it is the part that must never get "helpful": there is no branch here
/// that captures without `drawn`, and that absence is the privacy guarantee.
///
/// Args:
///     drawn: Whether the renderer-side pre-capture proof has completed.
///     ticks_left: Frame callbacks still available.
///
/// Returns:
///     The next step for the wait chain.
pub(super) fn wait_step(drawn: bool, ticks_left: u32) -> WaitStep {
    match (drawn, ticks_left) {
        (true, _) => WaitStep::Capture,
        (false, 0) => WaitStep::GiveUp,
        (false, _) => WaitStep::Wait,
    }
}

/// Arm the caption substitution and start waiting for it to reach the screen.
///
/// Args:
///     backend: Application backend, re-consulted before the capture to confirm the chart still
///         belongs to this window.
///     panel: The chart being photographed.
///     window: Handle of the OS window the keystroke arrived at.
///     native: That window's native handle, read by the caller while it still held the `&mut
///         Window`.
///     gpui_window: The live window, used to schedule the first frame callback.
///     cx: Application context.
pub(super) fn begin(
    backend: Entity<Backend>,
    panel: Entity<ChartPanel>,
    window: AnyWindowHandle,
    native: isize,
    gpui_window: &mut gpui::Window,
    cx: &mut App,
) {
    panel.update(cx, |p, cx| {
        p.arm_shot_caption(Some(Instant::now() + SHOT_CAPTION_TTL), cx)
    });
    // Read AFTER arming: this is the generation this chain owns, and any later value means another
    // press has taken the caption over.
    let generation = panel.read(cx).shot_caption_gen();
    schedule(
        backend,
        panel,
        window,
        native,
        generation,
        WAIT_TICKS,
        gpui_window,
    );
}

/// Queue one more frame callback in the wait chain.
///
/// Args:
///     backend: Application backend rechecked before the eventual capture.
///     panel: Chart whose caption substitution this chain owns.
///     window: OS window whose chart slot will be captured.
///     native: Native handle for that window.
///     generation: Caption arming generation this chain owns.
///     ticks_left: Frame callbacks remaining before the chain refuses the capture.
///     gpui_window: Live window used to schedule the next frame callback.
#[allow(clippy::too_many_arguments)]
fn schedule(
    backend: Entity<Backend>,
    panel: Entity<ChartPanel>,
    window: AnyWindowHandle,
    native: isize,
    generation: u64,
    ticks_left: u32,
    gpui_window: &mut gpui::Window,
) {
    gpui_window.on_next_frame(move |gpui_window, cx| {
        hop(
            backend,
            panel,
            window,
            native,
            generation,
            ticks_left,
            gpui_window,
            cx,
        );
    });
}

/// One step of the wait: check the proof, then capture, wait again, or refuse.
///
/// A frame callback fires at the START of a tick, before that tick's canvas pass and its present
/// (`moon-gpui/src/window.rs:1488-1497`). So this never infers the proof from callback count - it
/// asks the renderer every time.
///
/// Args:
///     backend: Application backend rechecked before the eventual capture.
///     panel: Chart whose caption substitution this chain owns.
///     window: OS window whose chart slot will be captured.
///     native: Native handle for that window.
///     generation: Caption arming generation this chain owns.
///     ticks_left: Frame callbacks remaining before the chain refuses the capture.
///     gpui_window: Live window that receives notification after the outcome.
///     cx: Application context used to read and update entities.
#[allow(clippy::too_many_arguments)]
fn hop(
    backend: Entity<Backend>,
    panel: Entity<ChartPanel>,
    window: AnyWindowHandle,
    native: isize,
    generation: u64,
    ticks_left: u32,
    gpui_window: &mut gpui::Window,
    cx: &mut App,
) {
    // A second press re-armed the caption and zeroed the frame count this chain was waiting on.
    // Stand down SILENTLY: the newer chain owns the caption, the restore and the notification, and
    // reporting a failure here would announce one for a shot that was merely replaced.
    if panel.read(cx).shot_caption_gen() != generation {
        return;
    }
    let drawn = panel.read(cx).shot_caption_drawn();
    match wait_step(drawn, ticks_left) {
        WaitStep::Wait => schedule(
            backend,
            panel,
            window,
            native,
            generation,
            ticks_left - 1,
            gpui_window,
        ),
        WaitStep::GiveUp => {
            // Deliberately NOT a capture. The caption the user would get is the account name, and
            // a shot that silently leaks it is worse than a shot that failed.
            log::warn!("chart shot: caption override never reached a frame");
            finish(&panel, ShotOutcome::Failed, gpui_window, cx);
        }
        WaitStep::Capture => {
            let outcome = capture(&backend, &panel, window, native, cx);
            finish(&panel, outcome, gpui_window, cx);
        }
    }
}

/// Cross the compositor boundary, confirm the chart is still ours, and take the picture.
///
/// Every early return here leaves the caption armed; [`finish`] is what restores it, and it runs on
/// all three outcomes.
///
/// Args:
///     backend: Application backend used to confirm that `window` still owns `panel`.
///     panel: Chart being captured.
///     window: OS window whose client area is captured.
///     native: Native handle for `window`.
///     cx: Application context used to read chart state.
///
/// Returns:
///     The outcome, after the compositor barrier and the ownership check.
fn capture(
    backend: &Entity<Backend>,
    panel: &Entity<ChartPanel>,
    window: AnyWindowHandle,
    native: isize,
    cx: &mut App,
) -> ShotOutcome {
    // The frame selected by the renderer-side proof can still be queued rather than composited on
    // the desktop this capture reads. `DwmFlush` blocks until the compositor has finished a pass,
    // which is the barrier between the old account-name pixels and the read below - so a FAILURE
    // here is not cosmetic. Continuing past it can photograph the previous frame, complete with
    // the core name.
    // Races harmlessly with the fork's own vsync thread, which calls `DwmFlush` in its own loop
    // (`moon-gpui-windows/src/vsync.rs`): it is a stateless OS call over no Rust-shared state, so
    // the two callers are serialized by the compositor rather than by anything here.
    if let Err(error) = unsafe { DwmFlush() } {
        log::warn!("chart shot: DwmFlush failed, refusing to capture: {error:#}");
        return ShotOutcome::Failed;
    }

    // A detached chart keeps the same entity when it moves to a new window
    // (`chart_tabs/windows.rs`), and `Backend::last_chart` is keyed by window. If the chart this
    // shot armed is no longer the one THIS window owns, its geometry now describes a different
    // window's client area and applying it to our HWND would copy whatever happens to sit there.
    let still_ours = super::resolve(backend, window, cx)
        .is_some_and(|current| current.entity_id() == panel.entity_id());
    if !still_ours {
        log::info!("chart shot: the chart moved to another window mid-shot");
        return ShotOutcome::NoChart;
    }

    // Re-read the geometry rather than reusing what the hotkey saw: several frames have passed and
    // the slot may have been resized, scrolled into a different pane split, or hidden.
    let Some((bounds, scale, device_size)) = panel.read(cx).shot_geometry() else {
        return ShotOutcome::NoChart;
    };
    let Some(rect) = rect::slot_capture_rect(bounds, scale, device_size) else {
        return ShotOutcome::NoChart;
    };
    // Read HERE, beside the capture, rather than when the hotkey was pressed: a dozen frame
    // callbacks have passed while the substituted caption reached the screen, and the burnt-in
    // header must describe the frame that was actually photographed.
    let inputs = panel.read(cx).shot_inputs(backend.read(cx));
    let when_ms = chrono::Utc::now().timestamp_millis();
    super::capture_windows(native, rect, &inputs, when_ms)
}

/// Restore the caption, then finish the shot and tell the user.
///
/// Ordered, and every half matters.
///
/// **The restore runs FIRST and SYNCHRONOUSLY, on every path.** That is what keeps the exchange
/// caption from being left standing on the user's own screen; the renderer's deadline is only the
/// backstop for a chain that never got here at all. Nothing after it is unbounded any more — the
/// shot ends at the clipboard — so the restore and the notification are one synchronous pair, and
/// there is no longer any asynchronous tail the restore could be deferred into.
///
/// **The notification comes last**, because the capture is already taken by this point - pushed
/// any earlier it would be standing inside the picture it announces.
///
/// Args:
///     panel: Chart whose caption must be restored.
///     outcome: What the capture achieved.
///     gpui_window: Live window that shows the result notification.
///     cx: Application context used to update the panel and notification layer.
fn finish(
    panel: &Entity<ChartPanel>,
    outcome: ShotOutcome,
    gpui_window: &mut gpui::Window,
    cx: &mut App,
) {
    panel.update(cx, |p, cx| p.arm_shot_caption(None, cx));
    if let ShotOutcome::Failed | ShotOutcome::NoChart = outcome {
        log::info!("chart shot: {outcome:?}");
    }
    gpui_window.push_notification(outcome.notification(), cx);
}

#[cfg(test)]
mod tests;
