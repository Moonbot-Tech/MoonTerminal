//! Clock in the header's right corner, formatted as `HH:MM:SS CODE`. It shows the selected city's
//! wall clock and advances once per second with the shell rerender. Clicking it opens a
//! `MoonPopover` listing the curated cities of [`cities`], each with its own current time. The
//! selection persists in the layout as the city's IANA zone id through
//! `Backend::set_header_clock_zone` and is shared by all windows.
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

/// Resolve the saved zone to a curated city, falling back to UTC so an unknown hand-edited zone
/// cannot leave the header without a stable label or clock.
fn selected_city(backend: &Entity<Backend>, cx: &App) -> &'static City {
    backend
        .read(cx)
        .header_clock_zone()
        .and_then(cities::by_zone_id)
        .unwrap_or_else(cities::utc_city)
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

/// The two strings the header clock shows: the city's time and its code.
///
/// One source for the renderer and the width measurement, so they cannot drift apart.
fn clock_parts(city: &City, now: DateTime<Utc>) -> (String, &'static str) {
    (cities::local_hms(city.zone, now), city.code)
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
pub(crate) fn header_clock_width(backend: &Entity<Backend>, cx: &App) -> f32 {
    let (time, code) = clock_parts(selected_city(backend, cx), now_utc());
    design::mono_body_text_width(cx, &time, CLOCK_TIME_WEIGHT)
        + design::ui_value(cx, CLOCK_GAP)
        + design::mono_body_text_width(cx, code, CLOCK_TZ_WEIGHT)
}

/// Write a city into the layout, deriving its compatibility offset mirror from the same instant.
///
/// The click path's writer; the startup path pairs the same two fields through
/// [`cities::reconcile_target`]. `Backend` cannot derive an offset from a zone — the city table
/// lives up here — so the pairing has to happen on this side, and both paths derive it the one
/// way, through `cities::current_offset_min`. Any new writer goes through one of these two.
fn commit_city(b: &mut Backend, city: &City, now: DateTime<Utc>) {
    b.set_header_clock_zone(city.zone.name(), cities::current_offset_min(city.zone, now));
}

/// Settle both persisted clock fields once at startup, before any window draws.
///
/// A layout with only the compatibility offset receives a city here rather than during rendering,
/// because lazy resolution would let later city-table ordering decide a persisted selection. A
/// layout that names a city also gets its offset mirror refreshed: the mirror is DERIVED and can
/// become an hour stale at the city's next summer-time transition. Both cases are decided by
/// [`cities::reconcile_target`], so this must run even when a zone is already saved.
pub(crate) fn reconcile_clock_zone(backend: &Entity<Backend>, cx: &mut App) {
    backend.update(cx, |b, _| {
        let now = now_utc();
        let target = cities::reconcile_target(
            b.layout.header_clock_zone.as_deref(),
            b.layout.header_clock_offset_min,
            now,
        );
        if let Some((city, offset_min)) = target {
            b.set_header_clock_zone(city.zone.name(), offset_min);
        }
    });
}

/// Render the header clock and its city-selection popover.
///
/// Both the drawn strings and [`header_clock_width`] are derived through [`clock_parts`].
pub(crate) fn header_clock(
    backend: &Entity<Backend>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    let selected = selected_city(backend, cx);
    // One instant for the header and every menu row, so no row can straddle a second and disagree
    // with the clock above it. `MoonPopover` builds its content eagerly, so this loop runs whether
    // the popup is open or not — reading the system clock per row would be that cost per frame.
    let now = now_utc();
    let (time, code) = clock_parts(selected, now);

    // One shared handle for all 47 rows: cloning an `Entity` takes a read lock on the entity map,
    // so a clone per row is not the refcount bump it reads as.
    let backend = Rc::new(backend.clone());
    let mut items = Vec::with_capacity(cities::CITIES.len());
    for city in cities::CITIES {
        // Both rows and the header come from the same `CITIES` slice, so identity IS the selection
        // test — no string comparison stands in for it.
        let is_selected = std::ptr::eq(city, selected);
        let backend = backend.clone();
        items.push(
            MoonMenuItem::with_key(
                format!("tz-{}", city.code),
                format!("{}  {}", city.code, city.name()),
            )
            .right_label(cities::local_hms(city.zone, now))
            .selected(is_selected)
            .checked(is_selected)
            .on_click(move |_, _, cx| {
                backend.update(cx, |b, bcx| {
                    // `now_utc()` again rather than the captured snapshot: a popup left open across
                    // a transition would otherwise mirror an offset that has since moved.
                    commit_city(b, city, now_utc());
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
                .header(t!("header.clock_pick").to_string())
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
