//! Per-panel click counting, derived from the window-global native click count.
//!
//! The platform counts double clicks per WINDOW, from timing and cursor distance alone — see
//! `ClickState::update` in the GPUI fork's `moon-gpui-windows/src/window.rs`, which compares each
//! press against `GetDoubleClickTime` and `SM_CXDOUBLECLK` and never looks at which element was
//! hit. So a press landing on a chart can carry `click_count == 2` even though the FIRST press of
//! that pair went somewhere else entirely: the × that closed a chart, a sibling chart the stack
//! reflow has since moved away, or this slot before it took a new coin. Trading reads that count —
//! `LeftDouble` places an order by default — which turns closing charts one after another on the
//! same spot into an unrequested order on whatever slid under the cursor.
//!
//! This tracks the series one panel actually witnessed. A press extends the panel's own count only
//! when it continues the press this panel saw last: same button, native count exactly one higher,
//! and within the platform's own double-click time and distance. The last two are what make the
//! previous observation identifiable as the pair's first press — a panel clicked once minutes ago
//! also sits at native 1, and the native count alone would let a stranger's press chain onto it.

use gpui::MouseButton;

/// Cursor travel still counted as the same spot, in the screen-space logical pixels every position
/// here is expressed in — see `render_input::press_count`, the only producer of them.
///
/// Known limit: those pixels come from each window's own scale factor, so across monitors of
/// DIFFERENT DPI the spaces do not line up. The cost is bounded and one-sided — two chart windows
/// on mixed-DPI screens can have one gesture read as another's residue for [`CLOSE_RESIDUE_MS`] —
/// so it is not worth threading a physical-pixel conversion through for.
///
/// Deliberately wider than the platform's own `SM_CXDOUBLECLK` tolerance (4 physical pixels by
/// default, which is 4 logical at 100% scaling and fewer above it): the native count has already
/// applied the exact test, so this only has to reject a press that plainly happened ELSEWHERE, and
/// a tolerance narrower than the platform's would drop double clicks the user really made.
const SAME_SPOT_PX: f32 = 8.0;

/// The system double-click interval in milliseconds.
///
/// Read per press rather than cached: it changes from Control Panel at any time, and the call is a
/// user32 lookup, not a computation.
#[cfg(windows)]
fn double_click_ms() -> f64 {
    f64::from(unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() })
}

/// Windows' own default, used where the platform's value is not readable.
#[cfg(not(windows))]
fn double_click_ms() -> f64 {
    500.0
}

/// How long presses on the very spot a chart was closed from still count as part of that closing.
///
/// Only presses that never moved off that spot are affected, so this is not a trading blackout —
/// anywhere else on the chart, and this same spot after the interval, trade immediately.
const CLOSE_RESIDUE_MS: f64 = 3_000.0;

/// Whether this press is left over from closing a chart rather than aimed at the chart under it.
///
/// Per-panel counting rejects the press that lands right after a close — the panel the reflow moved
/// under the cursor never saw press one. It cannot reject the press AFTER that: two presses on one
/// spot are a real pair as far as that panel can tell, even while the user is still stabbing at a ×
/// that has moved. What identifies the whole chain is that it never leaves the pixel the closing
/// was aimed at, which no single panel can know — hence the mark on the shared backend.
///
/// Args:
///     close: Time and screen position of the last chart close, if a click caused one.
///     now_ms: Unix-millisecond time of this press.
///     pos: This press's position in screen logical pixels.
///
/// Returns:
///     Whether a trading gesture must refuse this press.
pub(super) fn press_is_close_residue(
    close: Option<(f64, (f32, f32))>,
    now_ms: f64,
    pos: (f32, f32),
) -> bool {
    let Some((at_ms, close_pos)) = close else {
        return false;
    };
    // A range test, not `< CLOSE_RESIDUE_MS`: both stamps are wall clock, and a backwards step
    // (NTP, a resume from sleep) would otherwise latch this on until the clock caught up.
    (0.0..CLOSE_RESIDUE_MS).contains(&(now_ms - at_ms)) && same_spot(pos, close_pos)
}

/// Whether two window positions are close enough to count as the same spot.
fn same_spot(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).abs() <= SAME_SPOT_PX && (a.1 - b.1).abs() <= SAME_SPOT_PX
}

/// One press this panel saw, as the platform counted it and as this panel counted it.
#[derive(Clone, Copy)]
struct Seen {
    button: MouseButton,
    native: usize,
    own: usize,
    at_ms: f64,
    pos: (f32, f32),
}

/// Click-series state for one chart panel.
#[derive(Default)]
pub(super) struct ClickSeries {
    last: Option<Seen>,
}

impl ClickSeries {
    /// Record a press and return the click count that belongs to THIS panel.
    ///
    /// Args:
    ///     button: Mouse button of the press.
    ///     native: Click count the platform reported, counted per window.
    ///     at_ms: Unix-millisecond time of the press.
    ///     pos: Press position in screen logical pixels.
    ///
    /// Returns:
    ///     1 for a press that starts a series here, or one more than the previous press this panel
    ///     saw when this press continues that same series.
    pub(super) fn observe(
        &mut self,
        button: MouseButton,
        native: usize,
        at_ms: f64,
        pos: (f32, f32),
    ) -> usize {
        let own = match self.last {
            Some(prev) if prev.continues_into(button, native, at_ms, pos) => prev.own + 1,
            _ => 1,
        };
        self.last = Some(Seen {
            button,
            native,
            own,
            at_ms,
            pos,
        });
        own
    }

    /// Where the last press landed, if it is recent enough to be the one being handled now.
    ///
    /// Used to place the close mark at the pixel the × was clicked at: a close that no press
    /// explains — Escape, a TTL expiry, a window teardown — must leave no mark at all, and a stale
    /// position would put one at a pixel the user has long left.
    ///
    /// The window matches the mark's own lifetime rather than the double-click interval, because a
    /// button fires on mouse UP: a × held down longer than that interval would otherwise be treated
    /// as a close no press explains and leave the following presses unguarded.
    ///
    /// Args:
    ///     now_ms: Unix-millisecond time being handled.
    ///
    /// Returns:
    ///     The press position, or `None` when the last press is too old or none was seen.
    pub(super) fn fresh_press_pos(&self, now_ms: f64) -> Option<(f32, f32)> {
        let last = self.last?;
        (0.0..=CLOSE_RESIDUE_MS)
            .contains(&(now_ms - last.at_ms))
            .then_some(last.pos)
    }

    /// Forget the current series because this panel no longer shows what was clicked.
    ///
    /// A vacated stack slot keeps its panel and takes the next coin, so without this the press that
    /// landed on the previous market could chain into a double click that trades the new one.
    pub(super) fn reset(&mut self) {
        self.last = None;
    }
}

impl Seen {
    /// Whether a press is the next one in the series this observation belongs to.
    fn continues_into(
        &self,
        button: MouseButton,
        native: usize,
        at_ms: f64,
        pos: (f32, f32),
    ) -> bool {
        button == self.button
            && native == self.native + 1
            // Range rather than `<=`, so a backwards clock step cannot make an ancient press look
            // like this one's immediate predecessor.
            && (0.0..=double_click_ms()).contains(&(at_ms - self.at_ms))
            && same_spot(pos, self.pos)
    }
}

#[cfg(test)]
mod tests;
