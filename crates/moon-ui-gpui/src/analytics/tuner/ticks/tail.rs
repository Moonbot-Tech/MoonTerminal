//! The shortest tape past the close a deal must hold to be worked on: a row whose held trail
//! (`DealRow::held`) is shorter is not fit (`DealRow::fit`), so the fact's baseline, the variant
//! columns and the search all run on the same deals. Each deal's own held trail is its exit
//! horizon and is never under this floor.
//!
//! A deal with a 30 s tail cuts its variant's exit 30 s past its close: a variant that fills a
//! hair off the fact's price reaches its take or its Price Down line later than the fact did, is
//! left open at the cut, and the point is never the answer (`search::closing`) — an exit search
//! refuses it outright, an entry search can only steer through it. Short tails can be extended
//! by the existing fetch job while REST or a daily archive still serves the missing time;
//! otherwise the deal remains outside the sample.
//!
//! A tape with a hole between the buy and the close (`Deal::gap`) is left out the same way. A
//! variant whose rules reach into the hole ends there unanswered (`ExitKind::InGap`), which the
//! search counts as left open, and one such deal refused every point of a search: over six
//! MainShotS strategies (118 deals with a 120 s tail, 18 of them holed, 2026-10-01) the entry
//! search answered the strategies as they stand after one pass; with the holed deals out it
//! moved.
//!
//! Held for the whole process like the model's settings (`model_cfg`): every path that asks
//! whether a row is fit reads it here. Seeded from the saved layout at app startup;
//! written by the model popover's "Sample" section.

use std::sync::atomic::{AtomicU32, Ordering};

use gpui::*;
use moon_core::db::tuner::ticks::tail::{
    DEFAULT_MIN_TAIL_S, clamp_min_tail_s, reaches as core_reaches,
};
use moon_ui::{MoonInput, MoonInputEvent, MoonInputState, MoonPalette};
use rust_i18n::t;

use super::super::super::AnalyticsView;
use crate::design;

#[cfg(test)]
mod tests;

/// The shortest tail in force, seconds.
static MIN_TAIL_S: AtomicU32 = AtomicU32::new(DEFAULT_MIN_TAIL_S);

/// The input box's cache key in the axis' inputs.
const INPUT_ID: &str = "tail:min";

/// The shortest tail as set, seconds.
pub(crate) fn current_s() -> u32 {
    MIN_TAIL_S.load(Ordering::Relaxed)
}

/// The clamped shortest tail in force, independent of the terminal's tape margin.
pub(in crate::analytics::tuner) fn effective_s() -> u32 {
    clamp_min_tail_s(current_s())
}

/// Put a saved value in force; `None` is the default.
pub(in crate::analytics) fn replace(secs: Option<u32>) {
    MIN_TAIL_S.store(
        clamp_min_tail_s(secs.unwrap_or(DEFAULT_MIN_TAIL_S)),
        Ordering::Relaxed,
    );
}

/// Whether a row's held coverage reaches far enough past the close.
///
/// Args:
///     held: The row's `(lead_ms, trail_ms)` ([`super::state::DealRow::held`]); `None` holds
///         nothing.
pub(in crate::analytics::tuner) fn holds(held: Option<(i64, i64)>) -> bool {
    reaches(held, effective_s())
}

/// Whether `held` reaches `min_s` seconds past the close.
fn reaches(held: Option<(i64, i64)>, min_s: u32) -> bool {
    held.is_some_and(|(_, trail_ms)| core_reaches(trail_ms, i64::from(min_s) * 1_000))
}

/// A typed value in seconds: a whole number, clamped to the store's longest margin; anything
/// else is refused.
fn parse_s(text: &str) -> Option<u32> {
    text.trim().parse::<u32>().ok().map(clamp_min_tail_s)
}

impl AnalyticsView {
    /// Seed the saved floor before station jobs can read it, without constructing a view.
    pub(crate) fn seed_tape_min_tail_s(secs: Option<u32>) {
        replace(secs);
    }

    /// The model popover's "Sample" section: its heading and the shortest tail's box.
    pub(super) fn ticks_tail_rows(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> [AnyElement; 2] {
        let input = self.ticks_tail_input(window, cx);
        [
            super::cfg::popup_section(t!("analytics.ticks.model_sec_sample").to_string(), p, cx),
            super::cfg::popup_row(
                t!("analytics.ticks.tail_min").to_string(),
                Some(t!("analytics.ticks.tail_min_tip").to_string()),
                div()
                    .w(design::font_w_px(cx, 76.0))
                    .flex_none()
                    .font_family(design::mono())
                    .child(
                        MoonInput::new("an-ticks-m-tail")
                            .state(&input)
                            .size(design::INPUT_SIZE),
                    )
                    .into_any_element(),
                p,
                cx,
            ),
        ]
    }

    /// The shortest tail's box. Taken on Enter or when the box loses focus, never per keystroke:
    /// each commit reloads the axis — the rows a shorter tail left out lost their tape to the
    /// memory cap and are read again. A value it cannot take puts the box back.
    fn ticks_tail_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        if let Some(state) = self.ticks.inputs.get(INPUT_ID) {
            return state.clone();
        }
        let state =
            cx.new(|cx| MoonInputState::new(window, cx).default_value(current_s().to_string()));
        cx.subscribe_in(
            &state,
            window,
            move |this, state, ev: &MoonInputEvent, _window, cx| {
                if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                    return;
                }
                let typed = state.read(cx).value().to_string();
                if let Some(secs) = parse_s(&typed).filter(|&secs| secs != current_s()) {
                    MIN_TAIL_S.store(secs, Ordering::Relaxed);
                    this.backend
                        .update(cx, |backend, cx| backend.station_min_tail_sync(secs, cx));
                    this.persist_ticks_settings(cx);
                    this.reload_ticks(cx);
                }
                // A value clamped or refused: the box is recreated from the value in force on
                // the next frame, so it never shows what is not applied.
                if typed.trim() != current_s().to_string() {
                    this.ticks.inputs.remove(INPUT_ID);
                }
                cx.notify();
            },
        )
        .detach();
        self.ticks
            .inputs
            .insert(INPUT_ID.to_string(), state.clone());
        state
    }
}
