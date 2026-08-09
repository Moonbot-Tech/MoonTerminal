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
//! Its users are the News arrival tint and the Profit Monitor's row highlight, which share both the
//! timing and the tint itself from here. The chart's border flash went further and moved into the
//! chart's own GPU pass (`chartdx::render_state`), where it costs presents instead of view renders
//! — measured over seven live arrivals, `chart_render` did not move at all. That is the better
//! answer whenever the surface HAS an own pass; this module is for the ones that do not.
//!
//! A pulse is decoration: it does not need vblank rate. Everything here drives it from a
//! self-rearming [`PULSE_TICK`] timer instead, so the same flashes cost ~26 redraws. The phase
//! comes from the owner's own `Instant` rather than from an animation's private clock, which also
//! fixes a smaller wart: an element that scrolled into view late used to restart its pulse from
//! zero instead of showing the tail it was already in.

use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder;
use gpui::prelude::ParentElement;
use gpui::{Context, Div, Styled, div};

/// Repaint interval while a pulse is live. 10 Hz reads as smooth for a fade or a slow flash and
/// costs six times less than the vblank rate the animation path used.
pub const PULSE_TICK: Duration = Duration::from_millis(100);

/// How long a just-arrived row or card carries its arrival tint, from full to fully gone.
pub const FLASH: Duration = Duration::from_millis(2000);
/// Share of [`FLASH`] the tint holds at full strength before it starts easing out. The item appears
/// at the same moment and shifts its neighbours, so without a short hold the peak is never seen.
const FLASH_HOLD: f32 = 0.12;
/// Peak opacity of the tint. A tint, not a fill: the text sits ON this plate and has to stay
/// readable, so the colour reads as "this just lit up" rather than covering it.
const FLASH_PEAK: f32 = 0.24;

/// Build the full-bleed arrival tint for an item that arrived at `at`.
///
/// Declared BEFORE the content it belongs to, so it paints underneath. `None` once the pulse is
/// over — that absence is what erases the decoration; a layer at zero opacity would keep every
/// arrival in the element tree forever.
///
/// Both surfaces that flash share this one definition: the News feed and the Profit Monitor are the
/// same promise to the user ("this line is new"), and two copies of the timing drift apart.
///
/// Args:
///     color: Packed theme colour the tint is drawn in, normally `palette.table_selected`.
///     at: When the arrival was observed.
///
/// Returns:
///     The tint layer, or `None` once [`FLASH`] has elapsed.
pub fn arrival_tint(color: u32, at: Instant) -> Option<Div> {
    let delta = phase(at, FLASH)?;
    // Hold, then ease out quadratically: the tail is what reads as "fading", while a linear ramp
    // just switches off.
    let eased = ((delta - FLASH_HOLD) / (1.0 - FLASH_HOLD)).clamp(0.0, 1.0);
    Some(
        div()
            .absolute()
            .inset_0()
            .bg(crate::design::moon_alpha(color, FLASH_PEAK))
            .opacity((1.0 - eased) * (1.0 - eased)),
    )
}

/// Attach [`arrival_tint`] to an element when the item has a live arrival stamp.
///
/// Args:
///     element: Row or card the tint belongs to; it must already be `relative()`.
///     color: Packed theme colour the tint is drawn in.
///     at: Arrival stamp, if the item has one.
///
/// Returns:
///     The element, tinted when the pulse is still running.
pub fn with_arrival_tint(element: Div, color: u32, at: Option<Instant>) -> Div {
    element.when_some(
        at.and_then(|at| arrival_tint(color, at)),
        |element, tint| element.child(tint),
    )
}

/// Which items lit up and when, plus the "a timer is already running" flag that fades them.
///
/// Two surfaces keep an arrival highlight — the News feed keyed by item id, the Profit Monitor
/// keyed by core — and both need the same four things: stamp what just arrived, ask whether
/// anything is still fading, drop what has finished, and remember whether the repaint chain is
/// armed. Sharing the tint and its timing while copying this machine beside it is how the two
/// drifted apart once already.
///
/// It does NOT prune itself, and cannot: only the owner knows when its items changed. Every owner
/// therefore has to call [`Self::prune`] or [`Self::retain_live`] on the same beat it calls
/// [`Self::mark`] — the News feed does it when the feed is rebuilt, the Profit Monitor on each pulse
/// tick. Skip that and the map keeps expired stamps for the life of the view.
pub struct Arrivals<K> {
    /// When each item was observed arriving.
    stamps: HashMap<K, Instant>,
    /// Whether [`arm_with`] already has a chain running for these stamps.
    armed: bool,
}

impl<K> Default for Arrivals<K> {
    /// Start with nothing lit and no timer.
    fn default() -> Self {
        Self {
            stamps: HashMap::new(),
            armed: false,
        }
    }
}

impl<K: Eq + Hash> Arrivals<K> {
    /// Borrow the armed flag for [`arm`] / [`arm_with`].
    ///
    /// Returns:
    ///     The owner's "a timer is already running" flag, so several arrivals in one update cannot
    ///     stack several chains.
    pub fn armed(&mut self) -> &mut bool {
        &mut self.armed
    }

    /// Stamp every key as having arrived now.
    ///
    /// Args:
    ///     keys: Items that just appeared or changed.
    pub fn mark(&mut self, keys: impl IntoIterator<Item = K>) {
        let at = Instant::now();
        self.stamps.extend(keys.into_iter().map(|key| (key, at)));
    }

    /// Return one item's arrival stamp, if it is still recorded.
    ///
    /// Args:
    ///     key: Item being drawn.
    ///
    /// Returns:
    ///     Its stamp, which [`arrival_tint`] turns into an opacity or into nothing.
    pub fn get(&self, key: &K) -> Option<Instant> {
        self.stamps.get(key).copied()
    }

    /// Whether any recorded arrival is still inside [`FLASH`].
    ///
    /// Returns:
    ///     Whether a highlight still has something to draw — the `live` predicate every pulse
    ///     chain ends on.
    pub fn live(&self) -> bool {
        self.stamps.values().any(|at| at.elapsed() < FLASH)
    }

    /// Drop every finished stamp.
    pub fn prune(&mut self) {
        self.stamps.retain(|_, at| at.elapsed() < FLASH);
    }

    /// Drop finished stamps and anything the caller no longer shows.
    ///
    /// A surface whose items come and go — a feed that rotates ids, a table whose rows change with
    /// the period — would otherwise keep stamps for items nobody can see.
    ///
    /// Args:
    ///     keep: Whether an item is still on screen.
    pub fn retain_live(&mut self, keep: impl Fn(&K) -> bool) {
        self.stamps
            .retain(|key, at| at.elapsed() < FLASH && keep(key));
    }

    /// Copy the stamps out for a render that cannot borrow their owner.
    ///
    /// A view building its children while `cx` is borrowed mutably cannot also hold `&self`. The
    /// copy is as big as the map, which is as big as the owner's pruning lets it be — see the type
    /// doc: nothing here expires a stamp on its own.
    ///
    /// Returns:
    ///     An owned snapshot of the current stamps.
    pub fn snapshot(&self) -> HashMap<K, Instant>
    where
        K: Clone,
    {
        self.stamps.clone()
    }

    /// Forget every stamp, ending the fade immediately.
    ///
    /// Used where the highlight stops being meaningful rather than finishing: the feature switched
    /// off, or the question the surface was answering changed.
    pub fn clear(&mut self) {
        self.stamps.clear();
    }
}

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
    arm_with(this, cx, armed, live, |_this, _cx| {});
}

/// [`arm`] with per-tick work of the owner's own, run before the repaint.
///
/// A view whose pulse is drawn inside a `.cached(..)` CHILD needs this: marking only the owner
/// leaves that child clean, so GPUI reuses the still-tinted cached subtree and the fade never
/// moves. `on_tick` is where such an owner invalidates its child — and routing it through here
/// rather than hand-rolling the chain is what keeps every pulse on one timer and one diag counter.
///
/// Args:
///     this: Pulse owner.
///     cx: Owner's view context.
///     armed: Accessor for the owner's "a timer is already running" flag.
///     live: Whether any pulse still has something to draw.
///     on_tick: Owner work performed on each tick, before the repaint request.
pub fn arm_with<T: 'static>(
    this: &mut T,
    cx: &mut Context<T>,
    armed: fn(&mut T) -> &mut bool,
    live: fn(&T) -> bool,
    on_tick: fn(&mut T, &mut Context<T>),
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
                    on_tick(this, cx);
                    cx.notify();
                    arm_with(this, cx, armed, live, on_tick);
                })
                .is_ok()
        });
    })
    .detach();
}

#[cfg(test)]
mod tests;
