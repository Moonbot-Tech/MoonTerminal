//! The chart shot on platforms that have no capture path yet.
//!
//! Everything above this arm is platform-neutral — the action, the binding, the settings row, the
//! rectangle arithmetic — so the hotkey is bindable and visible on macOS and Linux exactly as it is
//! on Windows. What is missing is only the last step, and saying so out loud beats a key that
//! quietly does nothing. The Windows path reads the composited desktop through GDI; the equivalents
//! are `CGWindowListCreateImage` on macOS and a compositor-specific portal on Linux, neither of
//! which this goal built.
//!
//! What that means CONCRETELY on macOS and Linux: nothing is captured, nothing reaches the
//! clipboard, and no header strip is drawn. Every one of those is a product of a capture that does
//! not exist here, not a separate feature that could ship without one.

use gpui::AnyWindowHandle;

use super::ShotOutcome;
use super::rect::ShotRect;

/// Refuse the shot, naming the platform rather than failing silently.
///
/// Named `refuse` rather than after the clipboard: this arm never copied anything, so naming it
/// after the one thing it does not do would describe the Windows path instead of this one. The
/// name survives the file's removal unchanged, because it was never the file it was avoiding
/// naming — it was the refusal it states.
///
/// Args:
///     window: The window the chart is drawn in, unused here.
///     rect: The chart slot, unused here.
///     cx: Application context, unused here.
///
/// Returns:
///     Always [`ShotOutcome::Unsupported`].
pub(super) fn refuse(
    _window: AnyWindowHandle,
    _rect: ShotRect,
    _cx: &mut gpui::App,
) -> ShotOutcome {
    ShotOutcome::Unsupported
}
