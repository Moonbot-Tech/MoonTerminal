//! Physical wheel notches from a platform's line-based scroll delta.
//!
//! gpui reports a mouse wheel as `ScrollDelta::Lines`, and every platform derives those lines
//! from the notch its own way: Windows multiplies `raw delta / 120` by the user's "lines per
//! notch" setting (or "characters per notch" for a horizontal delta), X11 and Wayland by a fixed
//! three, and macOS hands over its own speed-accelerated line count. The chart's time zoom and
//! pan are measured in notches — one classic click is one doubling, the way MoonBot answers one
//! click — so this module undoes each platform's scaling. Banking lines against a threshold
//! instead made a slow turn do nothing and a fast one lose notches (#1461).

/// Lines gpui's Linux backends report per wheel notch (`SCROLL_LINES` in moon-gpui-linux), and
/// the fallback when a platform setting cannot be read.
const DEFAULT_LINES_PER_NOTCH: f32 = 3.0;

/// Most notches one event may apply.
///
/// A fast spin on Windows was seen to carry three notches in one message. The cap exists only so
/// a driver that reports a page scroll or garbage cannot turn one event into a jump across the
/// whole history.
pub(crate) const MAX_NOTCHES_PER_EVENT: f32 = 8.0;

/// How a platform's line delta relates to the wheel's physical notches.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum LineScale {
    /// The line count is the notch count times this many lines.
    PerNotch(f32),
    /// The line count is acceleration, not distance: every event is one notch in its sign.
    ///
    /// macOS sends one non-precise scroll event per detent and scales its value with the speed
    /// of the turn, so reading that value as notches would turn a fast flick into a 1000x jump,
    /// and reading it against a threshold would ignore a slow turn. Built only where macOS uses
    /// it and for the tests that pin it, so no other platform carries a variant it never makes.
    #[cfg(any(target_os = "macos", test))]
    OneNotchPerEvent,
}

/// Notches one wheel event turned.
///
/// Args:
///     lines: The event's line delta as gpui reports it; positive is wheel up.
///     scale: How this platform derived the lines from notches.
///
/// Returns:
///     Signed notches, fractional for a high-resolution wheel, capped at
///     [`MAX_NOTCHES_PER_EVENT`]. Zero for a zero or non-finite delta. A `PerNotch` that is not a
///     positive finite number falls back to [`DEFAULT_LINES_PER_NOTCH`].
pub(crate) fn notches(lines: f32, scale: LineScale) -> f32 {
    if lines == 0.0 || !lines.is_finite() {
        return 0.0;
    }
    let notches = match scale {
        LineScale::PerNotch(per) if per.is_finite() && per > 0.0 => lines / per,
        LineScale::PerNotch(_) => lines / DEFAULT_LINES_PER_NOTCH,
        #[cfg(any(target_os = "macos", test))]
        LineScale::OneNotchPerEvent => lines.signum(),
    };
    notches.clamp(-MAX_NOTCHES_PER_EVENT, MAX_NOTCHES_PER_EVENT)
}

/// This platform's line scale for one wheel event.
///
/// Args:
///     horizontal: Whether the delta came from the X axis. Windows scales those by its
///         "characters per notch" setting (Shift+wheel and a tilt wheel) and the vertical wheel
///         by "lines per notch"; the other platforms use one scale for both.
///
/// Returns:
///     The scale [`notches`] needs to turn this event's lines back into notches.
pub(crate) fn platform_line_scale(horizontal: bool) -> LineScale {
    #[cfg(windows)]
    {
        LineScale::PerNotch(windows_lines_per_notch(horizontal))
    }
    #[cfg(target_os = "macos")]
    {
        let _ = horizontal;
        LineScale::OneNotchPerEvent
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = horizontal;
        LineScale::PerNotch(DEFAULT_LINES_PER_NOTCH)
    }
}

/// The Windows setting gpui multiplied this event's notches by.
///
/// Read on every event rather than cached: it is a user32 lookup of an in-memory value, and
/// reading it live follows a change in the mouse settings without a restart, as gpui does.
///
/// Args:
///     horizontal: `SPI_GETWHEELSCROLLCHARS` for an X delta, `SPI_GETWHEELSCROLLLINES` otherwise
///         — the same choice gpui's `handle_mouse_wheel_msg` makes.
///
/// Returns:
///     Lines (or characters) per notch, or 0 when the setting cannot be read, which [`notches`]
///     treats as the default three.
#[cfg(windows)]
fn windows_lines_per_notch(horizontal: bool) -> f32 {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETWHEELSCROLLCHARS, SPI_GETWHEELSCROLLLINES, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
        SystemParametersInfoW,
    };
    let action = if horizontal {
        SPI_GETWHEELSCROLLCHARS
    } else {
        SPI_GETWHEELSCROLLLINES
    };
    let mut value: u32 = 0;
    // SAFETY: both actions write one UINT through `pvparam`, which points at a live local u32.
    let read = unsafe {
        SystemParametersInfoW(
            action,
            0,
            Some((&mut value as *mut u32).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS::default(),
        )
    };
    if read.is_ok() { value as f32 } else { 0.0 }
}

#[cfg(test)]
mod tests;
