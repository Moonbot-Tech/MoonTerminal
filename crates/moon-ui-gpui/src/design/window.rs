//! Header visibility thresholds and window decoration choices.

use super::*;

/// Ceiling for a header selector label (core, manual strategy).
///
/// Those pills size to their content and both names are arbitrary user text, so without a ceiling
/// one long name pushes the right-hand cluster — clock and window controls included — off the
/// window. Matches the other selectors' ceiling; the full name stays in the open menu.
/// This is the unscaled width passed through [`font_w`].
pub const HEADER_LABEL_MAX_W: f32 = 260.0;

/// Window widths at which the header sheds its ambient readouts, in a fixed order of sacrifice:
/// the brand's WORDMARK first, then the ticker's per-window deltas, then the ticker itself, then
/// the balance's " / total USDT" tail, and the clock last.
///
/// The order is by how much each one is worth once space runs out. The wordmark identifies an
/// application the operator is already looking at, so it buys nothing and goes first; the plane
/// icon beside it never goes, because it is what the operator grabs to move the window (#1082).
/// The clock is the only readout here whose absence cannot be reconstructed from anything else on
/// screen, so it goes last. The account total sits just above it: the free amount stays — it is
/// the figure an order is sized against — and the total is one double-click away in Assets and in
/// the balance's tooltip. The ticker sits between them and collapses in two steps rather than
/// clipping: the readout is monospaced and its informative part is the TAIL, so a character-level
/// clip eats the deltas and then the price digits, turning "61 333$" into "61 33" — a plausible
/// WRONG price stated as fact.
///
/// Every threshold is a window width and must stay ordered — wordmark highest, clock lowest — or
/// two readouts would disappear at the same width and the priority above would be a comment rather
/// than a behaviour. Usable only because [`HEADER_LABEL_MAX_W`] bounds the clusters that would
/// otherwise grow without limit.
///
/// These are TUNED window widths, not values derived from what the row holds, so the brand's own
/// width does not enter them: the MoonTerminal lockup is 36px wider than the Moonbot one these
/// were set against, and the `flex_1` drag spacer absorbs the difference. Re-tune them by looking
/// at the row, not by arithmetic on the mark.
const HEADER_WORDMARK_MIN_W: f32 = 1400.0;
const TICKER_DELTAS_MIN_W: f32 = 1200.0;
const TICKER_MIN_W: f32 = 1000.0;
const HEADER_BALANCE_TOTAL_MIN_W: f32 = 900.0;
const HEADER_CLOCK_MIN_W: f32 = 820.0;
// The order of sacrifice above, held by the compiler rather than by the comment.
const _: () = assert!(
    HEADER_WORDMARK_MIN_W > TICKER_DELTAS_MIN_W
        && TICKER_DELTAS_MIN_W > TICKER_MIN_W
        && TICKER_MIN_W > HEADER_BALANCE_TOTAL_MIN_W
        && HEADER_BALANCE_TOTAL_MIN_W > HEADER_CLOCK_MIN_W
);

/// Return whether the header's full lockup — plane icon AND wordmark — fits at `chrome_width`.
///
/// Below this width the header draws the icon alone ([`super::header_mark`]), never nothing: the
/// drag region the brand lives in sizes to its content, so an empty region is a zero-width hit
/// target and a narrow window would keep no titlebar to grab (#1082).
pub fn header_wordmark_visible(cx: &App, chrome_width: f32) -> bool {
    chrome_width >= font_w(cx, HEADER_WORDMARK_MIN_W)
}

/// Return whether the balance's " / total USDT" tail fits at `chrome_width`.
///
/// Below it the header keeps only the free amount; the full line moves into the balance tooltip.
/// The free figure itself never yields to width: it is what an order is sized against.
pub fn header_balance_total_visible(cx: &App, chrome_width: f32) -> bool {
    chrome_width >= font_w(cx, HEADER_BALANCE_TOTAL_MIN_W)
}

/// Return whether the header clock fits at `chrome_width`.
///
/// Last to go, and below [`ticker_visible`]'s threshold on purpose: by the width the clock yields,
/// the ticker is already gone, so the hand-summed ticker-popup offset (`shell::ticker`) can never
/// be computed against a header that has a clock in one frame and not the next.
pub fn header_clock_visible(cx: &App, chrome_width: f32) -> bool {
    chrome_width >= font_w(cx, HEADER_CLOCK_MIN_W)
}

/// Return whether the header ticker fits at `chrome_width`.
///
/// ONE predicate for the header that renders it and the popup layer that must not outlive its
/// trigger. Scaled by the UI font: everything beside the ticker is text, so a larger font claims
/// proportionally more of the same window and the ticker has to yield sooner.
pub fn ticker_visible(cx: &App, chrome_width: f32) -> bool {
    chrome_width >= font_w(cx, TICKER_MIN_W)
}

/// Return whether the ticker's 1h/24h deltas fit at `chrome_width`.
///
/// Uses the same font-scaled width policy as [`ticker_visible`], with a higher threshold so the
/// price remains visible after the deltas collapse.
pub fn ticker_deltas_visible(cx: &App, chrome_width: f32) -> bool {
    chrome_width >= font_w(cx, TICKER_DELTAS_MIN_W)
}

/// Transparent macOS titlebars keep native traffic-light buttons over the client
/// area. Keep terminal chrome content and drag hitboxes out of that strip.
pub fn titlebar_leading_inset() -> f32 {
    if cfg!(target_os = "macos") {
        76.0
    } else {
        HEADER_PAD_X
    }
}

pub fn show_custom_window_controls() -> bool {
    !cfg!(target_os = "macos")
}

pub fn platform_window_decorations() -> Option<WindowDecorations> {
    if cfg!(target_os = "linux") {
        Some(WindowDecorations::Client)
    } else {
        None
    }
}
