//! How far past a deal's close the tape must go.
//!
//! The terminal's shortest-tail setting and the station's recorded tail are this module, so
//! the two cannot disagree. The floor in force is the clamped setting ([`floor_ms`]);
//! the station holds at least that, the model's own pad, and the
//! margin ([`record_tail_ms`]).

use super::TAIL_MS;
use crate::market::trade_replay::{Coverage, ReplayWindow};

/// The default shortest tail, seconds: a minute (LinKvo, 2026-09-24).
pub const DEFAULT_MIN_TAIL_S: u32 = 60;

/// The longest the setting may ask for — the longest margin the tape store fetches
/// ([`crate::config::storage::MAX_TRADE_MARGIN_S`]). A longer minimum would leave every deal
/// out.
pub const MAX_MIN_TAIL_S: u32 = crate::config::storage::MAX_TRADE_MARGIN_S;

/// The setting, never past [`MAX_MIN_TAIL_S`].
///
/// Args:
///     s: Seconds the caller asked for.
///
/// Returns:
///     `s` capped at the store's longest margin.
pub fn clamp_min_tail_s(s: u32) -> u32 {
    s.min(MAX_MIN_TAIL_S)
}

/// The clamped shortest tail in milliseconds, independent of the terminal's margin.
///
/// Args:
///     min_tail_s: The shortest tail as set, seconds.
///
/// Returns:
///     `clamp_min_tail_s(min_tail_s) * 1000`.
pub fn floor_ms(min_tail_s: u32) -> i64 {
    i64::from(clamp_min_tail_s(min_tail_s)) * 1_000
}

/// Extend the replay focus through the requested post-close tail without widening its lead.
pub fn focus_with_tail(window: &ReplayWindow, tail_ms: i64) -> Coverage {
    let mut focus = window.focus_spans();
    focus.add((
        window.close_ms,
        window.close_ms.saturating_add(tail_ms.max(0)),
    ));
    focus
}

/// Whether a held trail reaches the floor.
///
/// Args:
///     trail_ms: Milliseconds of tape held past the close.
///     floor_ms: The floor from [`floor_ms`], milliseconds.
///
/// Returns:
///     `trail_ms >= floor_ms`.
pub fn reaches(trail_ms: i64, floor_ms: i64) -> bool {
    trail_ms >= floor_ms
}

/// How far past the close the station must hold a deal: at least the margin, the clamped
/// setting, and the model's own tail ([`TAIL_MS`]).
///
/// Args:
///     min_tail_s: The shortest tail as set, seconds.
///     margin_ms: The tape store's margin, milliseconds.
///
/// Returns:
///     `max(margin_ms, clamp_min_tail_s(min_tail_s) * 1000, TAIL_MS)`.
pub fn record_tail_ms(min_tail_s: u32, margin_ms: i64) -> i64 {
    let setting_ms = i64::from(clamp_min_tail_s(min_tail_s)) * 1_000;
    margin_ms.max(setting_ms).max(TAIL_MS)
}

/// The spans a deal must hold once the shortest-tail floor is in force: [`super::required_spans`]
/// united with the tail past the close, clipped to the focus extended through that tail.
///
/// Args:
///     window: The deal's window.
///     floor_ms: The floor, milliseconds. A floor under [`TAIL_MS`] does not shrink the model's
///         own tail.
///
/// Returns:
///     The held coverage a fit deal must include.
pub fn required_with_floor(window: &ReplayWindow, floor_ms: i64) -> Coverage {
    let mut required = super::required_spans(window);
    let tail = floor_ms.max(TAIL_MS);
    required.add((window.close_ms, window.close_ms.saturating_add(tail)));
    required.clip(&focus_with_tail(window, tail))
}

#[cfg(test)]
mod tests;
