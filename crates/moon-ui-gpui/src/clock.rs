//! Clock in the header's right corner, formatted as `HH:MM:SS (UTC+/-N)`. It displays current UTC
//! plus the selected timezone offset and advances once per second with the shell rerender. Clicking
//! it opens a `MoonPopover` listing UTC-12 through UTC+14 plus a fractional system offset when
//! applicable. The selection persists in the layout through
//! `Backend::set_header_clock_offset_min` and is shared by all windows.
//!
//! When the selected offset matches the system timezone, the displayed time is already local, so
//! the `(UTC...)` label is omitted.

use gpui::*;
use moon_ui::{
    MoonMenuItem, MoonMenuSize, MoonPalette, MoonPopover, MoonPopoverPlacement, MoonPopupMenu,
    MoonTooltipView, h_flex,
};
use rust_i18n::t;

use crate::Backend;
use crate::design;

/// Inclusive menu range of whole-hour timezones. A fractional offset such as India +5:30 or Nepal
/// +5:45 is added separately only when it matches the system timezone, because the whole-hour list
/// cannot otherwise select it.
const TZ_MIN_H: i32 = -12;
const TZ_MAX_H: i32 = 14;

/// Returns current UTC as cross-platform epoch seconds without external crates.
fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Formats current UTC plus the minute offset `off_min` as `HH:MM:SS`. `rem_euclid` preserves the
/// correct time of day for negative offsets and midnight crossings.
fn hms(off_min: i32) -> String {
    let day = (now_unix_secs() + off_min as i64 * 60).rem_euclid(86_400);
    let (h, m, s) = (day / 3600, (day % 3600) / 60, day % 60);
    format!("{h:02}:{m:02}:{s:02}")
}

/// Formats a timezone label such as `UTC`, `UTC+3`, or `UTC-5:30`.
fn tz_label(off_min: i32) -> String {
    if off_min == 0 {
        return "UTC".to_string();
    }
    let sign = if off_min > 0 { '+' } else { '-' };
    let a = off_min.abs();
    let (h, m) = (a / 60, a % 60);
    if m == 0 {
        format!("UTC{sign}{h}")
    } else {
        format!("UTC{sign}{h}:{m:02}")
    }
}

/// Gap between the time and the optional timezone label.
///
/// Shared by [`header_clock`] and [`header_clock_width`] rather than written twice: the ticker
/// popup is positioned by summing what sits between its trigger and the window edge, and the clock
/// is part of that span, so a measurement that disagrees with what was drawn lands the popup
/// off its trigger — with nothing to catch it at compile time.
const CLOCK_GAP: f32 = 5.0;

/// Font weight of the clock's time text.
const CLOCK_TIME_WEIGHT: f32 = 600.0;

/// Font weight of the clock's optional timezone label.
const CLOCK_TZ_WEIGHT: f32 = 400.0;

/// The strings the header clock shows: the time, plus the timezone label when it is displayed.
///
/// One source for the renderer and the width measurement, including the label rule — the label is
/// hidden when the selected offset already matches the system one, since then the readout is just
/// local time.
fn clock_parts(off_min: i32, sys_min: i32) -> (String, Option<String>) {
    let tz = (off_min != sys_min).then(|| format!("({})", tz_label(off_min)));
    (hms(off_min), tz)
}

/// Returns the system timezone offset from UTC in minutes, positive eastward. This supports hiding
/// the label when the selected offset matches the system offset.
#[cfg(windows)]
fn system_offset_min() -> i32 {
    use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    // Since windows 0.61, both functions return SYSTEMTIME by value. Adjacent calls keep the hour
    // and minute stable in normal operation.
    let (utc, loc) = unsafe { (GetSystemTime(), GetLocalTime()) };
    let utc_min = utc.wHour as i32 * 60 + utc.wMinute as i32;
    let loc_min = loc.wHour as i32 * 60 + loc.wMinute as i32;
    let mut diff = loc_min - utc_min;
    // Local and UTC dates can differ, for example UTC 23:30 versus local 01:30, so unwrap the day.
    if diff > 14 * 60 {
        diff -= 24 * 60;
    } else if diff < -14 * 60 {
        diff += 24 * 60;
    }
    // Real timezones use 15-minute increments. Rounding suppresses jitter if the calls straddle a minute.
    ((diff as f32 / 15.0).round() as i32) * 15
}

/// Uses UTC as the system timezone outside Windows because `moon-chart` intentionally provides no
/// platform-specific timezone lookup. The clock still works, but the `(UTC)` label remains visible.
#[cfg(not(windows))]
fn system_offset_min() -> i32 {
    0
}

/// The corresponding top-right header clock shows the time and an optional timezone label; clicking
/// it opens the timezone-selection popup.
/// Rendered width of the header clock, in the units the header lays its children out with.
///
/// `shell::ticker` positions the rate ticker's popup by summing everything between the ticker and
/// the window's right edge, and because the ticker sits to the left of the clock, the clock is part
/// of that span. Reads [`clock_parts`] and the `CLOCK_*` constants, exactly so it cannot disagree
/// with what [`header_clock`] draws.
///
/// Glyph advances only, no kerning (see `design::ui_text_width`) — a close estimate, not an exact
/// measurement.
pub fn header_clock_width(backend: &Entity<Backend>, cx: &App) -> f32 {
    let off = backend.read(cx).header_clock_offset_min();
    let (time, tz) = clock_parts(off, system_offset_min());
    let mut width = design::mono_body_text_width(cx, &time, CLOCK_TIME_WEIGHT);
    if let Some(tz) = tz {
        width += design::ui_value(cx, CLOCK_GAP)
            + design::mono_body_text_width(cx, &tz, CLOCK_TZ_WEIGHT);
    }
    width
}

/// Render the header clock and its timezone-selection popover.
///
/// The timezone label is omitted when the selected offset matches the system offset; both the
/// rendered strings and [`header_clock_width`] are derived through [`clock_parts`].
pub fn header_clock(backend: &Entity<Backend>, p: MoonPalette, cx: &App) -> impl IntoElement {
    let off = backend.read(cx).header_clock_offset_min();
    let sys = system_offset_min();
    let (time, tz) = clock_parts(off, sys);

    // List whole-hour offsets plus a fractional system timezone not already present, allowing India
    // or Nepal system zones to be selected exactly and thereby hide the label.
    let mut offsets: Vec<i32> = (TZ_MIN_H..=TZ_MAX_H).map(|h| h * 60).collect();
    if !offsets.contains(&sys) {
        offsets.push(sys);
        offsets.sort_unstable();
    }

    // All zone labels use the menu's mono font, so character count identifies the widest one.
    // Measure only that candidate because text measurement is uncached and this path renders on
    // every 1 Hz clock tick. The fixed-width placeholder matches every `hms` value.
    let widest_tz = offsets
        .iter()
        .map(|&m| tz_label(m))
        .max_by_key(|s| s.chars().count())
        .unwrap_or_default();
    let menu_w = design::menu_fit_width_2col(cx, &widest_tz, "00:00:00", 156.0);

    let mut items = Vec::with_capacity(offsets.len());
    for m in offsets {
        let backend = backend.clone();
        items.push(
            MoonMenuItem::with_key(format!("tz-{m}"), tz_label(m))
                // Snapshot the time in this zone; it advances with shell rerenders while the popup is open.
                .right_label(hms(m))
                .selected(m == off)
                .checked(m == off)
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        b.set_header_clock_offset_min(m);
                        bcx.notify();
                    });
                }),
        );
    }

    let mut row = h_flex()
        .id("header-clock")
        .flex_none()
        .items_center()
        .gap(design::ui_px(cx, CLOCK_GAP))
        .font_family(design::mono())
        .text_size(design::t_body(cx))
        .cursor_pointer()
        .tooltip(|_w, cx| {
            cx.new(|_| MoonTooltipView::new(t!("header.clock_tip").to_string()))
                .into()
        })
        .child(
            div()
                .text_color(rgb(p.text))
                .font_weight(FontWeight(CLOCK_TIME_WEIGHT))
                .child(time),
        );
    if let Some(tz) = tz {
        row = row.child(
            div()
                .text_color(rgb(p.text_soft))
                .font_weight(FontWeight(CLOCK_TZ_WEIGHT))
                .child(tz),
        );
    }

    MoonPopover::new("header-clock-popover")
        .placement(MoonPopoverPlacement::BottomEnd)
        .width(design::popover_outer_width(cx, menu_w))
        .close_on_content_click(true)
        .trigger(row)
        .content(
            MoonPopupMenu::new("header-clock-menu")
                .width(menu_w)
                .size(MoonMenuSize::Compact)
                .mono(true)
                // Show about 11 of the 27 or more zones at once and scroll the remainder.
                .max_height(300.0)
                .items(items)
                .render(),
        )
        .into_any_element()
}
