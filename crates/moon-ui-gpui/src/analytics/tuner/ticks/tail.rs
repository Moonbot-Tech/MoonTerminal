//! The shortest tape past the close a deal must hold to be worked on: a row whose held trail
//! (`DealRow::held`) is shorter is not fit (`DealRow::fit`), so the fact's baseline, the variant
//! columns and the search all run on the same deals, and the sample's exit horizon — the shortest
//! trail among them (`search::common_horizon_ms`) — is never under this.
//!
//! One deal with a 30 s tail cut every variant's exit 30 s past its close: a variant that fills a
//! hair off the fact's price reaches its take or its Price Down line later than the fact did, is
//! left open at the cut, and refuses the whole point (`search::closing`). The tape of an older
//! trade cannot be fetched again — its exchange no longer serves it — so the deal is left out
//! instead (LinKvo, 2026-09-24).
//!
//! Held for the whole process like the model's settings (`model_cfg`): every path that asks
//! whether a row is fit reads it here. Seeded from the saved layout when an analytics view
//! opens; written by the model popover's "Sample" section.

use std::sync::atomic::{AtomicU32, Ordering};

use gpui::*;
use moon_ui::{MoonInput, MoonInputEvent, MoonInputState, MoonPalette};
use rust_i18n::t;

use super::super::super::AnalyticsView;
use super::state::TicksData;
use crate::design;

#[cfg(test)]
mod tests;

/// The default shortest tail, seconds: a minute (LinKvo, 2026-09-24).
const DEFAULT_MIN_TAIL_S: u32 = 60;

/// The longest the setting may ask for — the longest margin the tape store fetches
/// (`moon_core::config::storage::MAX_TRADE_MARGIN_S`): a longer one would leave every deal out.
const MAX_MIN_TAIL_S: u32 = moon_core::config::storage::MAX_TRADE_MARGIN_S;

/// The shortest tail in force, seconds.
static MIN_TAIL_S: AtomicU32 = AtomicU32::new(DEFAULT_MIN_TAIL_S);

/// The input box's cache key in the axis' inputs.
const INPUT_ID: &str = "tail:min";

/// The shortest tail as set, seconds.
pub(in crate::analytics::tuner) fn current_s() -> u32 {
    MIN_TAIL_S.load(Ordering::Relaxed)
}

/// The shortest tail in force, seconds: the setting, but never past the tape store's margin
/// (`[trade_replay] margin`, the Storage tab) — a row holds at most that much past its close,
/// and a minimum above it would leave every deal out.
pub(in crate::analytics::tuner) fn effective_s() -> u32 {
    let margin_s = moon_core::market::trade_replay::margin_ms() / 1_000;
    current_s().min(u32::try_from(margin_s.max(0)).unwrap_or(u32::MAX))
}

/// Put a saved value in force; `None` is the default.
pub(in crate::analytics) fn replace(secs: Option<u32>) {
    MIN_TAIL_S.store(
        secs.unwrap_or(DEFAULT_MIN_TAIL_S).min(MAX_MIN_TAIL_S),
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
    held.is_some_and(|(_, trail_ms)| trail_ms >= i64::from(min_s) * 1_000)
}

/// A typed value in seconds: a whole number, clamped to the store's longest margin; anything
/// else is refused.
fn parse_s(text: &str) -> Option<u32> {
    text.trim()
        .parse::<u32>()
        .ok()
        .map(|s| s.min(MAX_MIN_TAIL_S))
}

impl TicksData {
    /// Rows the model reproduces, with their tape, left out only for a tail shorter than the
    /// setting — what the footer counts.
    fn short_tail(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| {
                r.tape == super::state::TapeStatus::Covered
                    && r.verdict
                        .as_ref()
                        .is_some_and(moon_core::db::tuner::ticks::fit_for_search)
                    && !holds(r.held)
            })
            .count()
    }
}

impl AnalyticsView {
    /// The footer's note on the rows the tail left out, with its leading separator; empty when
    /// none.
    pub(super) fn ticks_short_tail_note(&self) -> String {
        match self.ticks.data.data().map(|d| d.short_tail()) {
            Some(n) if n > 0 => format!(
                " · {}",
                t!("analytics.ticks.tail_short", n = n, s = effective_s())
            ),
            _ => String::new(),
        }
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
