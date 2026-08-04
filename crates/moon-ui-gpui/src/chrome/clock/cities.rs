//! The curated city table behind the header clock, and the pure time math over it.
//!
//! GPUI-free on purpose: the table, the lookups and the formatting are plain data and plain
//! functions, so they are exercised by the sibling unit tests without a window.
//!
//! Each entry pins an IANA zone as a `chrono_tz::Tz` **constant**, never a string, so a mistyped
//! zone is a compile error instead of a runtime fallback to UTC that nobody notices until the
//! clock is silently an hour off. Summer time therefore needs no rules of our own — the embedded
//! tz database answers every conversion, including the transition dates.
//!
//! The persisted selection is the zone's canonical IANA id (`Europe/Warsaw`); the three-letter
//! code is presentation derived from this table, so renaming a code never orphans a saved config.

use chrono::{DateTime, Offset, Timelike, Utc};
use chrono_tz::Tz;

/// One selectable city, pairing the stable presentation key with the zone that answers its time.
/// `code` also keys localization (`city.WAW`), so uniqueness keeps the header and picker aligned.
pub(crate) struct City {
    /// Three-letter uppercase code kept untranslated so it behaves like a stable market ticker.
    pub code: &'static str,
    /// IANA zone stored as a typed constant so a misspelling fails at compile time.
    pub zone: Tz,
}

impl City {
    /// Resolve the picker label through `locales/city.yml`, while the header keeps the stable code.
    pub fn name(&self) -> String {
        rust_i18n::t!(format!("city.{}", self.code)).to_string()
    }
}

/// Return plain UTC as the always-resolvable neutral fallback.
///
/// Returns the table's own first row rather than a second UTC value, so an identity check against
/// a selected city stays meaningful.
pub(crate) fn utc_city() -> &'static City {
    &CITIES[0]
}

/// The selectable cities, ordered by standard UTC offset west to east, with plain UTC pinned first
/// as the neutral default. Standard offset keeps the order stable across summer-time transitions;
/// Dublin therefore belongs among the `+1` cities despite sharing London's wall clock year-round.
///
/// Curated rather than the full `TZ_VARIANTS`: nearly 600 IANA zones, most of them aliases, are
/// not a menu. One entry per zone the desk actually watches, which keeps the codes meaningful and
/// the list scrollable.
///
/// A `static`, not a `const`, and load-bearing: a const is inlined at each use, and the language
/// does not promise one address for its value, so `chrome::clock` comparing a row against the
/// selected city by pointer would be resting on an optimization rather than a guarantee.
#[rustfmt::skip]
pub(crate) static CITIES: &[City] = &[
    City { code: "UTC", zone: Tz::UTC },
    // --- Americas ---
    City { code: "HNL", zone: Tz::Pacific__Honolulu },
    City { code: "ANC", zone: Tz::America__Anchorage },
    City { code: "LAX", zone: Tz::America__Los_Angeles },
    City { code: "DEN", zone: Tz::America__Denver },
    City { code: "MEX", zone: Tz::America__Mexico_City },
    City { code: "CHI", zone: Tz::America__Chicago },
    City { code: "BOG", zone: Tz::America__Bogota },
    City { code: "NYC", zone: Tz::America__New_York },
    City { code: "TOR", zone: Tz::America__Toronto },
    City { code: "SCL", zone: Tz::America__Santiago },
    City { code: "BUE", zone: Tz::America__Argentina__Buenos_Aires },
    City { code: "SAO", zone: Tz::America__Sao_Paulo },
    // --- Europe and Africa ---
    City { code: "LIS", zone: Tz::Europe__Lisbon },
    City { code: "LON", zone: Tz::Europe__London },
    // Ireland uses standard +1 with negative winter time, so Dublin sorts here even though its
    // wall clock always matches London's.
    City { code: "DUB", zone: Tz::Europe__Dublin },
    City { code: "AMS", zone: Tz::Europe__Amsterdam },
    City { code: "BER", zone: Tz::Europe__Berlin },
    City { code: "MAD", zone: Tz::Europe__Madrid },
    City { code: "PAR", zone: Tz::Europe__Paris },
    City { code: "ROM", zone: Tz::Europe__Rome },
    // Warsaw's STANDARD offset is +1 (CET); the +2 it shows for half the year is summer time.
    City { code: "WAW", zone: Tz::Europe__Warsaw },
    City { code: "ZRH", zone: Tz::Europe__Zurich },
    City { code: "ATH", zone: Tz::Europe__Athens },
    City { code: "HEL", zone: Tz::Europe__Helsinki },
    City { code: "KIE", zone: Tz::Europe__Kyiv },
    City { code: "CAI", zone: Tz::Africa__Cairo },
    City { code: "JNB", zone: Tz::Africa__Johannesburg },
    City { code: "IST", zone: Tz::Europe__Istanbul },
    City { code: "MOW", zone: Tz::Europe__Moscow },
    City { code: "RUH", zone: Tz::Asia__Riyadh },
    // --- Asia and Oceania ---
    City { code: "DXB", zone: Tz::Asia__Dubai },
    City { code: "KHI", zone: Tz::Asia__Karachi },
    // Almaty's civic time rules come from the embedded tz database, the authority for transitions.
    City { code: "ALA", zone: Tz::Asia__Almaty },
    City { code: "BOM", zone: Tz::Asia__Kolkata },
    City { code: "BKK", zone: Tz::Asia__Bangkok },
    City { code: "JKT", zone: Tz::Asia__Jakarta },
    City { code: "PER", zone: Tz::Australia__Perth },
    City { code: "SIN", zone: Tz::Asia__Singapore },
    City { code: "HKG", zone: Tz::Asia__Hong_Kong },
    City { code: "SHA", zone: Tz::Asia__Shanghai },
    City { code: "TPE", zone: Tz::Asia__Taipei },
    City { code: "SEL", zone: Tz::Asia__Seoul },
    City { code: "TYO", zone: Tz::Asia__Tokyo },
    City { code: "BNE", zone: Tz::Australia__Brisbane },
    City { code: "SYD", zone: Tz::Australia__Sydney },
    City { code: "AKL", zone: Tz::Pacific__Auckland },
];

/// Look a city up by the IANA zone id persisted in `layout.toml`.
///
/// An id that names no curated city returns `None`; the caller falls back to [`utc_city`] rather
/// than inventing a code for a zone the table cannot label.
pub(crate) fn by_zone_id(id: &str) -> Option<&'static City> {
    CITIES.iter().find(|c| c.zone.name() == id)
}

/// The city's wall clock at `now_utc`, as `HH:MM:SS`.
///
/// The single place a zone is resolved: the header and every menu row go through it, so they
/// cannot disagree about a transition. Formatted by hand rather than through `%H:%M:%S`, because
/// strftime re-lexes its format string on every call and this runs once per menu row per frame.
pub(crate) fn local_hms(zone: Tz, now_utc: DateTime<Utc>) -> String {
    let t = now_utc.with_timezone(&zone).time();
    format!("{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second())
}

/// Resolve the zone's current UTC offset in minutes, including summer time, so migration preserves
/// the displayed wall clock and the compatibility mirror matches what the city currently reads.
pub(crate) fn current_offset_min(zone: Tz, now_utc: DateTime<Utc>) -> i32 {
    now_utc
        .with_timezone(&zone)
        .offset()
        .fix()
        .local_minus_utc()
        / 60
}

/// Valid range of a compatibility seed; the fixed-offset schema rejects values outside it.
const LEGACY_OFFSETS: std::ops::RangeInclusive<i32> = (-12 * 60)..=(14 * 60);

/// Map a fixed `header_clock_offset_min` compatibility seed onto a curated city.
///
/// Zero returns `None` on purpose: it is the untouched default of every config that never opened
/// the picker, so treating it as a choice would relabel every one of those headers. `None` leaves
/// the clock on UTC. A value outside [`LEGACY_OFFSETS`] is not a valid saved choice and gets the
/// same answer.
///
/// The match uses the CURRENT offset, not the standard one, because migration must preserve the
/// wall clock at the migration instant. Matching standard offsets can map a saved `+2` to Athens
/// while Athens currently reads `+3`. Nearest wins when nothing matches exactly, so a valid edge
/// such as `+14` shifts by as little as possible instead of collapsing fourteen hours to UTC.
///
/// It stays lossy in one direction and cannot be otherwise: the saved integer never named a place,
/// so a `+2` reads as any city currently at `+2`. Its clock is right; its name is a guess, and one
/// click settles it.
pub(crate) fn migrate_offset_min(off_min: i32, now_utc: DateTime<Utc>) -> Option<&'static City> {
    if off_min == 0 || !LEGACY_OFFSETS.contains(&off_min) {
        return None;
    }
    CITIES
        .iter()
        .min_by_key(|c| (current_offset_min(c.zone, now_utc) - off_min).abs())
}

/// The city and its current offset that `chrome::clock::reconcile_clock_zone` must persist this
/// call, given the zone id already saved in the layout (if any) and the compatibility offset mirror.
///
/// Pure counterpart of `reconcile_clock_zone`, extracted here so it can be unit tested without a
/// `Backend` — a GPUI entity too large to construct in a unit test. The caller must apply this
/// on EVERY reconcile, never skipping it because a zone is already saved: the offset paired with
/// an already-chosen city is a DERIVED value that goes stale at that city's next summer-time
/// transition. Keeping it fresh preserves the clock for compatibility readers that consume only
/// the offset field.
pub(crate) fn reconcile_target(
    zone_id: Option<&str>,
    legacy_offset_min: i32,
    now_utc: DateTime<Utc>,
) -> Option<(&'static City, i32)> {
    let city = match zone_id {
        Some(id) => by_zone_id(id),
        None => migrate_offset_min(legacy_offset_min, now_utc),
    }?;
    Some((city, current_offset_min(city.zone, now_utc)))
}

#[cfg(test)]
mod tests;
