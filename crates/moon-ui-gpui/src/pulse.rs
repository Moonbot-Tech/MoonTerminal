//! Cheap repaint driver for short-lived visual pulses (arrival highlights, tints).
//!
//! GPUI's `with_animation` requests a frame from `request_animation_frame`, which is
//! `cx.notify(current_view)` — the OWNER VIEW, not the element. `mark_view_dirty` then marks that
//! view and every ancestor, so an animation re-renders its owner in full, every vblank, for its
//! whole duration.
//!
//! What contains the damage is MoonUI's dock: `PanelView::render_panel` wraps every panel in
//! `.cached(size_full())`, and a cached view whose own subtree is clean is reused. Sibling panels
//! are therefore safe. The owner is not — it re-renders in full, ~120 times per second.
//!
//! The only user left is the News arrival tint. The chart's border flash went further and moved
//! into the chart's own GPU pass (`chartdx::render_state`), where it costs presents instead of view
//! renders — measured over seven live arrivals, `chart_render` did not move at all. That is the
//! better answer whenever the surface HAS an own pass; this module is for the ones that do not.
//!
//! A pulse is decoration: it does not need vblank rate. Everything here drives it from a
//! self-rearming [`PULSE_TICK`] timer instead, so the same flashes cost ~26 redraws. The phase
//! comes from the owner's own `Instant` rather than from an animation's private clock, which also
//! fixes a smaller wart: an element that scrolled into view late used to restart its pulse from
//! zero instead of showing the tail it was already in.

use std::time::{Duration, Instant};

use gpui::Context;

/// Repaint interval while a pulse is live. 10 Hz reads as smooth for a fade or a slow flash and
/// costs six times less than the vblank rate the animation path used.
pub const PULSE_TICK: Duration = Duration::from_millis(100);

/// Progress of a pulse that started at `at` and runs for `total`, normalised to `0.0..1.0`.
///
/// `None` once it is over, so the caller draws nothing — that is what ENDS the pulse. A version
/// saturating at `1.0` instead would leave the finished decoration on screen forever.
pub fn phase(at: Instant, total: Duration) -> Option<f32> {
    let elapsed = at.elapsed();
    (elapsed < total).then(|| elapsed.as_secs_f32() / total.as_secs_f32())
}

/// Repaint `cx`'s view every [`PULSE_TICK`] for as long as `live` reports a pulse in flight.
///
/// `armed` borrows the owner's "a timer is already running" flag, so calling this on every arrival
/// cannot stack timers. The first tick where `live` is false still repaints — that is the frame
/// which erases the finished pulse — and only then does the chain stop.
///
/// Call it where a pulse STARTS, never from `render`: arming from render would let the view keep
/// itself awake through its own repaints.
pub fn arm<T: 'static>(
    this: &mut T,
    cx: &mut Context<T>,
    armed: fn(&mut T) -> &mut bool,
    live: fn(&T) -> bool,
) {
    if *armed(this) || !live(this) {
        return;
    }
    *armed(this) = true;
    cx.spawn(async move |handle, cx| {
        let executor = cx.update(|cx| cx.background_executor().clone());
        executor.timer(PULSE_TICK).await;
        let _ = cx.update(|cx| {
            handle
                .update(cx, |this, cx| {
                    crate::diag::bump(&crate::diag::PULSE_TICK);
                    *armed(this) = false;
                    cx.notify();
                    arm(this, cx, armed, live);
                })
                .is_ok()
        });
    })
    .detach();
}

#[cfg(test)]
mod tests;
