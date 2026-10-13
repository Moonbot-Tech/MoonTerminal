//! Whether the reader is using a chart right now, and what that does to its automatic closes.
//!
//! A detect chart leaves on its own in two ways: the strategy's `KeepInChart` timer runs out, or
//! the tab's detect cap picks it as the stalest chart to replace. Neither may take a chart the
//! reader is working on. "Using" is the pointer resting over the chart, or any input the chart saw
//! — move, wheel, pan, click, key while focused — within [`IN_USE_WINDOW`]. The pin stays the
//! explicit hold that needs no interaction.
//!
//! Everything here is pure: the panel feeds it timestamps from its input handlers, and nothing in
//! those handlers repaints for it.

use std::time::{Duration, Instant};

/// How long after the reader's last input a chart still counts as in use.
pub(crate) const IN_USE_WINDOW: Duration = Duration::from_secs(120);

/// How often an expired chart under a parked pointer is looked at again: the pointer leaving
/// sends no input, so the hover alone cannot say when the idle window starts.
pub(crate) const HOVER_RECHECK: Duration = Duration::from_secs(5);

/// The reader's interaction with one chart panel.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct UseTrack {
    /// Whether the pointer is over the chart slot.
    pub(crate) pointer_over: bool,
    /// When the chart last saw input from the reader.
    pub(crate) last_input: Option<Instant>,
}

impl UseTrack {
    /// Record one input. A timestamp write and nothing else, so it is safe at mousemove rate.
    pub(crate) fn note_input(&mut self, now: Instant) {
        self.last_input = Some(now);
    }

    /// How long the chart stays in use from `now`, or `None` when it is idle.
    ///
    /// Under the pointer it is [`HOVER_RECHECK`]; otherwise what is left of [`IN_USE_WINDOW`]
    /// since the last input.
    pub(crate) fn hold_for(&self, now: Instant) -> Option<Duration> {
        if self.pointer_over {
            return Some(HOVER_RECHECK);
        }
        let since = now.saturating_duration_since(self.last_input?);
        IN_USE_WINDOW
            .checked_sub(since)
            .filter(|left| !left.is_zero())
    }
}

/// What the `KeepInChart` timer does next.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum TtlStep {
    /// Look again after this long: the deadline is ahead, or it passed while the chart is in use.
    Wait(Duration),
    /// The deadline passed and the chart is idle: close the expired panes.
    Prune,
}

/// Decide the timer's next step from the nearest unpinned deadline and the chart's use.
///
/// Args:
///     deadline_ms: Nearest TTL deadline in Unix milliseconds, `None` when nothing expires.
///     now_ms: Current Unix time in milliseconds.
///     hold: [`UseTrack::hold_for`] at the same moment.
///
/// Returns:
///     `None` when no timer is needed, else the step to take.
pub(crate) fn ttl_step(
    deadline_ms: Option<f64>,
    now_ms: f64,
    hold: Option<Duration>,
) -> Option<TtlStep> {
    let deadline = deadline_ms?;
    if deadline > now_ms {
        return Some(TtlStep::Wait(Duration::from_millis(
            (deadline - now_ms).max(1.0).ceil() as u64,
        )));
    }
    Some(hold.map_or(TtlStep::Prune, TtlStep::Wait))
}

#[cfg(test)]
mod tests;
