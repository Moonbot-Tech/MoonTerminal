//! The chart shot on platforms that have no capture path yet.
//!
//! Everything above this arm is platform-neutral — the action, the binding, the settings row, the
//! rectangle arithmetic — so the hotkey is bindable and visible on macOS and Linux exactly as it is
//! on Windows. What is missing is only the last step, and saying so out loud beats a key that
//! quietly does nothing. The Windows path reads the composited desktop through GDI; the equivalents
//! are `CGWindowListCreateImage` on macOS and a compositor-specific portal on Linux, neither of
//! which this goal built.

use gpui::AnyWindowHandle;

use super::ShotOutcome;
use super::rect::ShotRect;

/// Refuse the capture, naming the platform rather than failing silently.
///
/// Args:
///     window: The window the chart is drawn in, unused here.
///     rect: The chart slot, unused here.
///     cx: Application context, unused here.
///
/// Returns:
///     Always [`ShotOutcome::Unsupported`].
pub(super) fn capture_to_clipboard(
    _window: AnyWindowHandle,
    _rect: ShotRect,
    _cx: &mut gpui::App,
) -> ShotOutcome {
    ShotOutcome::Unsupported
}
