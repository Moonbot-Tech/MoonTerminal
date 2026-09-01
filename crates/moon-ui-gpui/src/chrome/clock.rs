//! Shared terminal clock, normally formatted as `HH:MM:SS CODE` and compacted to `HH:MM CODE` in
//! narrow hosts. It shows the selected zone's wall clock and advances whenever its host rerenders.
//! Clicking it opens a `MoonPopover` listing the curated cities of [`cities`], each with its own
//! current time. A first-run system zone outside that list remains an exact IANA selection and is
//! shown in a checked system row until the user replaces it with a curated city. The selection
//! persists through `Backend::set_header_clock_zone` and is shared by all windows.
//!
//! Summer time needs no handling here: the zone answers every conversion, so a city that has just
//! moved its clocks reads correctly without an update or a restart.
//!
//! The city code is always drawn because it identifies WHICH city's rules drive the clock and is
//! the only visible selection indicator while the picker is closed. Keeping both time and code
//! present also gives the ticker's hand-summed popup offset a stable clock shape to measure.
//!
//! Search is intentionally deferred until this popup is hosted on `Shell`, like
//! `core_settings_content` in `shell/render.rs`. `MoonPopover` takes content eagerly, so the
//! inline design builds every row on every header render, and MoonUI's searchable list requires an
//! `Entity<MoonComboboxState<_>>` created with a `&mut Window`; this free function receives only
//! `&App`. Moving the popup state solves both constraints without a hand-rolled filter input.

use std::rc::Rc;

use chrono::{DateTime, Utc};
use gpui::*;
use moon_ui::{
    MoonMenuItem, MoonMenuSize, MoonPalette, MoonPopover, MoonPopoverPlacement, MoonPopupMenu,
    MoonTooltipView, h_flex,
};
use rust_i18n::t;

use crate::Backend;
use crate::design;

mod cities;

#[cfg(test)]
mod tests;

use cities::City;

/// The current instant, as the type every zone conversion takes.
///
/// Not `Utc::now()`: this crate pins `chrono` without its clock feature. The system clock is read
/// through `moon_core::util::now_unix_ms_i64`, which holds the one copy of that formula, and only a
/// timestamp outside chrono's year range can fail the conversion — the epoch fallback keeps the
/// header drawing rather than panicking a window on it.
fn now_utc() -> DateTime<Utc> {
    DateTime::from_timestamp_millis(moon_core::util::now_unix_ms_i64())
        .unwrap_or(DateTime::UNIX_EPOCH)
}

/// How long a resolved system zone is reused.
///
/// The query is not cheap and not cached by the library: on Windows it activates a WinRT calendar
/// per call, on macOS it resets the process-wide zone cache. The sleep schedule asks ten times a
/// second, and a zone changes when someone travels.
const SYSTEM_ZONE_TTL_MS: i64 = 60_000;

/// The zone this MACHINE is in, as the operating system states it.
///
/// Separate from the header clock's zone on purpose: the clock shows whichever city the operator
/// chose to watch, while some answers — when to fall asleep — are about where the operator
/// actually is.
///
/// Cached, and the last good answer STICKS: a transient failure of the platform query would
/// otherwise move the sleep schedule to the header clock's zone for one tick, and a schedule that
/// jumps by hours for 100 ms clears a live wake-override and puts the terminal back to sleep. Only
/// a machine that has never answered reports `None`.
///
/// Returns:
///     The machine's zone, or `None` when the platform has never named a resolvable one.
pub(crate) fn system_zone() -> Option<chrono_tz::Tz> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<(i64, Option<chrono_tz::Tz>)>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new((0, None)));
    let now = moon_core::util::now_unix_ms_i64();
    {
        let guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        if guard.1.is_some() && now.saturating_sub(guard.0) < SYSTEM_ZONE_TTL_MS {
            return guard.1;
        }
    }
    let fresh = iana_time_zone::get_timezone()
        .ok()
        .and_then(|id| cities::zone_by_id(&id));
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if fresh.is_some() {
        *guard = (now, fresh);
    }
    guard.1
}

/// Resolve the exact IANA zone represented by the visible header clock.
///
/// Args:
///     zone_id: Persisted IANA zone id, if the user selected one.
///
/// Returns:
///     Saved curated or uncurated IANA zone, or UTC under the header's fallback policy.
pub(crate) fn resolved_header_clock_zone(zone_id: Option<&str>) -> chrono_tz::Tz {
    zone_id
        .and_then(cities::zone_by_id)
        .unwrap_or(chrono_tz::Tz::UTC)
}

/// Resolve the saved zone shown by the header clock.
///
/// Args:
///     backend: Shared state containing the persisted header-clock zone.
///     cx: Application context used to read the backend entity.
///
/// Returns:
///     Matching curated or uncurated zone, or UTC when the saved value is invalid.
fn selected_zone(backend: &Entity<Backend>, cx: &App) -> chrono_tz::Tz {
    resolved_header_clock_zone(backend.read(cx).header_clock_zone())
}

/// Gap between the time and the city code.
///
/// Shared by [`header_clock`] and [`header_clock_width`] rather than written twice: the ticker
/// popup is positioned by summing what sits between its trigger and the window edge, and the clock
/// is part of that span, so a measurement that disagrees with what was drawn lands the popup
/// off its trigger — with nothing to catch it at compile time.
const CLOCK_GAP: f32 = 5.0;

const CLOCK_TIME_WEIGHT: f32 = 600.0;

const CLOCK_TZ_WEIGHT: f32 = 400.0;

/// Column width the offset label is right-padded to inside a picker row's trailing text.
///
/// `mono(true)` on the popup makes every row monospace, so a fixed width keeps the sign and
/// digits aligned down the whole list. Sized for the widest label the table can produce today —
/// `"-12:45"`, six glyphs — rather than the common two- or three-glyph case, so a future
/// quarter-hour zone at the far edge of the table does not outgrow it and break alignment.
const OFFSET_LABEL_WIDTH: usize = 6;

/// Compose a picker row's trailing text: the city's wall clock, and its offset from UTC.
///
/// UTC is always resolvable, unlike the machine's own zone, so every row carries an offset
/// column — there is no "reference unresolved" fallback left to handle.
///
/// Args:
///     zone: City zone the row shows.
///     now: Instant both the time and the offset are read at.
///
/// Returns:
///     `"HH:MM:SS  <offset>"`, offset right-padded to [`OFFSET_LABEL_WIDTH`].
fn picker_right_label(zone: chrono_tz::Tz, now: DateTime<Utc>) -> String {
    let time = cities::local_hms(zone, now);
    let offset = cities::offset_label(zone, chrono_tz::Tz::UTC, now);
    format!("{time}  {offset:>OFFSET_LABEL_WIDTH$}")
}

/// Time precision requested by one clock host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ClockPrecision {
    /// Hours, minutes, and seconds for normal-width headers.
    Seconds,
    /// Hours and minutes for narrow hosts.
    Minutes,
}

/// The two strings one clock host shows: the selected-zone time and its compact code.
///
/// One source for the renderer and the width measurement, so they cannot drift apart.
///
/// Args:
///     zone: Selected IANA zone that determines the wall clock.
///     now: UTC instant converted into the selected zone.
///     precision: Whether the host has room for seconds.
///
/// Returns:
///     Local time text and curated city code, or the zone's current abbreviation when uncurated.
fn clock_parts(
    zone: chrono_tz::Tz,
    now: DateTime<Utc>,
    precision: ClockPrecision,
) -> (String, String) {
    let mut time = cities::local_hms(zone, now);
    if precision == ClockPrecision::Minutes {
        time.truncate(5);
    }
    let code = cities::by_zone_id(zone.name())
        .map(|city| city.code.to_string())
        .unwrap_or_else(|| now.with_timezone(&zone).format("%Z").to_string());
    (time, code)
}

/// Rendered width of the header clock, in the units the header lays its children out with.
///
/// `shell::ticker` positions the rate ticker's popup by summing everything between the ticker and
/// the window's right edge, and because the ticker sits to the left of the clock, the clock is part
/// of that span. Reads [`clock_parts`] and the `CLOCK_*` constants, exactly so it cannot disagree
/// with what [`header_clock`] draws.
///
/// Glyph advances only, no kerning (see `design::ui_text_width`) — a close estimate, not an exact
/// measurement.
///
/// Args:
///     backend: Shared state containing the selected display zone.
///     cx: Application context used to measure the active typography.
///
/// Returns:
///     Full `HH:MM:SS CODE` trigger width used by the main-header ticker offset.
pub(crate) fn header_clock_width(backend: &Entity<Backend>, cx: &App) -> f32 {
    let (time, code) = clock_parts(
        selected_zone(backend, cx),
        now_utc(),
        ClockPrecision::Seconds,
    );
    design::mono_body_text_width(cx, &time, CLOCK_TIME_WEIGHT)
        + design::ui_value(cx, CLOCK_GAP)
        + design::mono_body_text_width(cx, &code, CLOCK_TZ_WEIGHT)
}

/// Write a picker city into the layout, deriving its compatibility offset from the same instant.
///
/// The click path's writer; the startup path pairs the same two fields through
/// [`cities::reconcile_target`]. `Backend` cannot derive an offset from a zone — the city table
/// lives up here — so the pairing has to happen on this side, and both paths derive it the one
/// way, through `cities::current_offset_min`. Any new writer goes through one of these two.
///
/// Args:
///     b: Shared Backend whose layout stores the selection.
///     city: Curated city selected by the user.
///     now: UTC instant used to derive the compatibility offset mirror.
///     cx: Backend context that publishes the display-time revision.
///
/// Returns:
///     Nothing; layout state and observers are updated in place.
fn commit_city(b: &mut Backend, city: &City, now: DateTime<Utc>, cx: &mut Context<Backend>) {
    b.set_header_clock_zone(
        city.zone.name(),
        cities::current_offset_min(city.zone, now),
        cx,
    );
}

/// Settle both persisted clock fields once at startup, before any window draws.
///
/// A profile with neither field configured calls the OS detector and persists its exact IANA id.
/// Old nonzero offsets migrate without consulting the OS, while every already-saved valid IANA id
/// wins and only refreshes its derived compatibility offset. A failed detection leaves the profile
/// untouched, so this startup path can retry after the next reboot instead of persisting fallback
/// UTC as a manual choice.
///
/// Args:
///     backend: Shared state whose loaded layout is reconciled and marked for persistence.
///     cx: Application context used to update Backend before window construction.
///
/// Returns:
///     Nothing; a valid saved, migrated, or detected zone is applied through the shared setter.
pub(crate) fn reconcile_clock_zone(backend: &Entity<Backend>, cx: &mut App) {
    backend.update(cx, |b, bcx| {
        let now = now_utc();
        let target = cities::reconcile_target(
            b.layout.header_clock_zone.as_deref(),
            b.layout.header_clock_offset_min,
            now,
            || iana_time_zone::get_timezone().ok(),
        );
        if let Some(target) = target {
            b.set_header_clock_zone(&target.zone_id, target.offset_min, bcx);
        }
    });
}

/// Render the full shared terminal clock and its city-selection popover.
///
/// Both the drawn strings and [`header_clock_width`] are derived through [`clock_parts`].
///
/// Args:
///     backend: Shared state containing the selected display zone.
///     p: Active palette.
///     cx: Application context used to build the shared popover.
///
/// Returns:
///     `HH:MM:SS CODE` trigger backed by the shared city picker.
pub(crate) fn header_clock(backend: &Entity<Backend>, p: MoonPalette, cx: &App) -> AnyElement {
    render_header_clock(backend, p, ClockPrecision::Seconds, cx)
}

/// Render the minute-precision shared clock and its unchanged city-selection popover.
///
/// Args:
///     backend: Shared state containing the selected display zone.
///     p: Active palette.
///     cx: Application context used to build the shared popover.
///
/// Returns:
///     `HH:MM CODE` trigger backed by the same picker as the full clock.
pub(crate) fn compact_header_clock(
    backend: &Entity<Backend>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    render_header_clock(backend, p, ClockPrecision::Minutes, cx)
}

/// Render one clock precision through the single selected-zone and popover implementation.
///
/// Args:
///     backend: Shared state containing the selected display zone.
///     p: Active palette.
///     precision: Time fields retained by the host.
///     cx: Application context used to build the shared popover.
///
/// Returns:
///     Clock trigger and eagerly prepared city-selection popover.
fn render_header_clock(
    backend: &Entity<Backend>,
    p: MoonPalette,
    precision: ClockPrecision,
    cx: &App,
) -> AnyElement {
    let selected = selected_zone(backend, cx);
    // One instant for the header and every menu row, so no row can straddle a second and disagree
    // with the clock above it. `MoonPopover` builds its content eagerly, so this loop runs whether
    // the popup is open or not — reading the system clock per row would be that cost per frame.
    let now = now_utc();
    let (time, code) = clock_parts(selected, now, precision);

    // One shared handle for all 47 rows: cloning an `Entity` takes a read lock on the entity map,
    // so a clone per row is not the refcount bump it reads as.
    let backend = Rc::new(backend.clone());
    let selected_curated = cities::by_zone_id(selected.name());
    let mut items =
        Vec::with_capacity(cities::CITIES.len() + usize::from(selected_curated.is_none()));
    if selected_curated.is_none() {
        items.push(
            MoonMenuItem::with_key(
                "tz-system",
                t!("header.clock_system", zone = selected.name()).to_string(),
            )
            .right_label(picker_right_label(selected, now))
            .selected(true)
            .checked(true)
            .disabled(true),
        );
    }
    for city in cities::CITIES {
        let is_selected = city.zone == selected;
        let backend = backend.clone();
        items.push(
            MoonMenuItem::with_key(
                format!("tz-{}", city.code),
                format!("{}  {}", city.code, city.name()),
            )
            .right_label(picker_right_label(city.zone, now))
            .selected(is_selected)
            .checked(is_selected)
            .on_click(move |_, _, cx| {
                backend.update(cx, |b, bcx| {
                    // `now_utc()` again rather than the captured snapshot: a popup left open across
                    // a transition would otherwise mirror an offset that has since moved.
                    commit_city(b, city, now_utc(), bcx);
                    bcx.notify();
                });
            }),
        );
    }

    let row = h_flex()
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
        )
        .child(
            div()
                .text_color(rgb(p.text_soft))
                .font_weight(FontWeight(CLOCK_TZ_WEIGHT))
                .child(code),
        );

    MoonPopover::new("header-clock-popover")
        .placement(MoonPopoverPlacement::BottomEnd)
        .fit_content()
        .close_on_content_click(true)
        .trigger(row)
        .content(
            MoonPopupMenu::new("header-clock-menu")
                // One compact row: the caption is a single line, and the pinned header is now
                // held out of the scrolling region, so its height has to be declared for the row
                // list's budget. MoonUI floors it at the menu's own row height regardless.
                .header(20.0, t!("header.clock_pick").to_string())
                .fit_width(200.0, 560.0)
                .size(MoonMenuSize::Compact)
                .mono(true)
                // Cap the viewport at about 11 cities so the full curated list cannot overrun a
                // short window; the remaining rows stay reachable by scrolling.
                .max_height_ui(300.0)
                .items(items)
                .render(),
        )
        .into_any_element()
}
