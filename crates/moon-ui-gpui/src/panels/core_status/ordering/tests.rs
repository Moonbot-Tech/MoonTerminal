//! Tests for flat-row order and exchange-section aggregates.
//!
//! Explicit imports (no `use super::*`) per the crate's test convention: the panel's parent module
//! re-exports `gpui::*`, whose own `test` would shadow the built-in attribute.

use moon_core::session::core_order;
use std::cmp::Ordering;
use std::collections::HashMap;

use moon_core::config::TableSortPreference;
use moon_core::feed::{ConnStatus, CoreTimeOffsetStatus};
use moon_core::session::{CoreStartupStatus, CoreSysStatus};
use moon_core::venue::CoreVenue;

use super::{FlatLine, compare_flat_rows, flat_lines, restore_flat_sort, section_aggregate};
use crate::backend::core_warn::LatencySeverity;
use crate::panels::core_status::model::{ApiKeyState, CoreStatusRow};

/// Build one core row carrying only an API-key state: `Some(days)` is a dated key, `None` is a core
/// nothing is known about (no answer, or an answer with no usable date).
fn row_with_key(id: u64, days: Option<i32>) -> CoreStatusRow {
    CoreStatusRow {
        fault: None,
        mode_suggestion: None,
        id,
        name: format!("Core {id}"),
        status: ConnStatus::Ready,
        sys: CoreSysStatus::default(),
        endpoint: None,
        ping_warn: false,
        ping_sev: LatencySeverity::Normal,
        exch_sev: LatencySeverity::Normal,
        api_key: days.map_or(ApiKeyState::Unknown, ApiKeyState::Days),
        api_warn: false,
        api_notice: false,
        api_quota: None,
        api_quota_warn: false,
        startup: CoreStartupStatus::default(),
        time_offset: CoreTimeOffsetStatus::default(),
        server_version: None,
        server_version_suffix: None,
        version_behind: None,
        update: None,
    }
}

/// Equal numbers order by the letter: missing, then a blank release letter, then a name.
/// The number stays first, so `7.70 R3` stays before `7.71`.
///
/// Breaks when a blank letter collapses into a missing one: a release build and a core that
/// sent no letter would sort as the same row.
#[test]
fn version_orders_the_letter_after_the_number() {
    let mut bare = row_with_key(1, None);
    bare.server_version = Some(771);
    let mut release = row_with_key(4, None);
    release.server_version = Some(771);
    release.server_version_suffix = Some(String::new());
    let mut named = row_with_key(2, None);
    named.server_version = Some(771);
    named.server_version_suffix = Some("R3".to_string());
    let mut older = row_with_key(3, None);
    older.server_version = Some(770);
    older.server_version_suffix = Some("R3".to_string());

    assert_eq!(
        compare_flat_rows(&bare, &release, "version"),
        Ordering::Less
    );
    assert_eq!(
        compare_flat_rows(&release, &named, "version"),
        Ordering::Less
    );
    assert_eq!(compare_flat_rows(&bare, &named, "version"), Ordering::Less);
    assert_eq!(compare_flat_rows(&older, &bare, "version"), Ordering::Less);
}

/// The key column shows text ("9", "∞", "истёк"), but it must sort by the URGENCY behind it.
/// Sorting the rendered string would put 9 after 45 and bury the key that expires first.
#[test]
fn the_key_column_sorts_by_urgency_not_by_its_text() {
    let soon = row_with_key(1, Some(9));
    let later = row_with_key(2, Some(45));

    assert_eq!(
        compare_flat_rows(&soon, &later, "api_key"),
        Ordering::Less,
        "9 days is more urgent than 45"
    );
}

/// This column is scanned for the key that dies soonest, so the states with NO number trail the
/// counts instead of heading them — the opposite of what a plain `Option` sort would do. And the
/// two of them are not interchangeable: a dash may still hide a dying key, an infinity cannot.
#[test]
fn the_states_without_a_number_trail_the_counts() {
    let dated = row_with_key(1, Some(300));
    let unknown = row_with_key(2, None);
    let unlimited = CoreStatusRow {
        api_key: ApiKeyState::Perpetual,
        ..row_with_key(3, None)
    };

    assert_eq!(
        compare_flat_rows(&dated, &unknown, "api_key"),
        Ordering::Less,
        "a real count outranks 'nothing known', however distant the date"
    );
    assert_eq!(
        compare_flat_rows(&unknown, &unlimited, "api_key"),
        Ordering::Less,
        "'nothing known' outranks 'cannot expire' — it may still hide a problem"
    );
}

/// An expired key is the most urgent thing this column can show, so it leads every live one instead
/// of grouping with the keys that carry no number at all.
#[test]
fn an_expired_key_sorts_ahead_of_every_live_one() {
    let expired = row_with_key(1, Some(-3));
    let last_day = row_with_key(2, Some(0));

    assert_eq!(
        compare_flat_rows(&expired, &last_day, "api_key"),
        Ordering::Less,
        "expired leads the ascending order"
    );
}

/// `ordering.rs:restore_flat_sort` must retain the version key and direction, and reject retired
/// keys. Removing `"version"` from `KEYS` silently discards a user's selected build sort at restart.
///
/// Mutation: drop `"version"` from `KEYS`. Flat mode would reopen in attention order, and the
/// retained-version assertion reddens without relying on `KEYS`' implementation length.
#[test]
fn flat_sort_restore_validates_key_and_direction() {
    assert_eq!(
        restore_flat_sort(Some(TableSortPreference {
            column: "version".to_string(),
            ascending: false,
        })),
        Some(("version".to_string(), false))
    );
    assert_eq!(
        restore_flat_sort(Some(TableSortPreference {
            column: "retired".to_string(),
            ascending: true,
        })),
        None
    );
}

/// Median ping is the middle Ready sample. An even count is the half-up mean of the two middle
/// samples: 10, 20, 30, 40 becomes 25, not the upper-middle 30.
///
/// Breaks when the median stays on index `len / 2` for an even count, or sorts the wrong way
/// and averages the wrong pair.
#[test]
fn section_aggregate_median_averages_the_two_middle_ready_pings() {
    let odd = [
        ping_row(1, ConnStatus::Ready, Some(30)),
        ping_row(2, ConnStatus::Ready, Some(10)),
        ping_row(3, ConnStatus::Ready, Some(20)),
    ];
    let odd_refs: Vec<_> = odd.iter().collect();
    let odd_agg = section_aggregate(&odd_refs);
    assert_eq!(odd_agg.cores, 3);
    assert_eq!(odd_agg.median_ping_ms, Some(20));
    assert_eq!(odd_agg.not_connected, 0);

    let even = [
        ping_row(1, ConnStatus::Ready, Some(10)),
        ping_row(2, ConnStatus::Ready, Some(40)),
        ping_row(3, ConnStatus::Ready, Some(20)),
        ping_row(4, ConnStatus::Ready, Some(30)),
    ];
    let even_refs: Vec<_> = even.iter().collect();
    assert_eq!(section_aggregate(&even_refs).median_ping_ms, Some(25));
}

/// Unknown pings and pings on cores that are not Ready stay out of the median. Those cores still
/// count as not connected.
///
/// Breaks when a disconnected core's stale ping becomes the median, or a Ready core with no ping
/// is reported as not connected.
#[test]
fn section_aggregate_skips_unknown_pings_and_counts_cores_that_are_not_ready() {
    let rows = [
        ping_row(1, ConnStatus::Ready, None),
        ping_row(2, ConnStatus::Ready, Some(15)),
        ping_row(3, ConnStatus::Disconnected, Some(999)),
        ping_row(4, ConnStatus::Failed("down".to_string()), None),
    ];
    let refs: Vec<_> = rows.iter().collect();
    let agg = section_aggregate(&refs);
    assert_eq!(agg.cores, 4);
    assert_eq!(agg.median_ping_ms, Some(15));
    assert_eq!(agg.not_connected, 2);

    let unknown = [ping_row(1, ConnStatus::Ready, None)];
    let unknown_refs: Vec<_> = unknown.iter().collect();
    assert_eq!(section_aggregate(&unknown_refs).median_ping_ms, None);
}

/// One core row with a chosen connection and core ping. Other fields stay at the quiet default.
fn ping_row(id: u64, status: ConnStatus, ping: Option<u32>) -> CoreStatusRow {
    let mut row = row_with_key(id, None);
    row.status = status;
    row.sys.round_trip_ms = ping;
    row
}

/// `ordering.rs:flat_lines` must retain every source row once while partitioning by venue identity.
///
/// Mutation: remove `lines.extend(members.into_iter().map(FlatLine::Core));`. A core would silently
/// vanish from the operator's fleet list, or a future broken partition could duplicate it.
#[test]
fn flat_lines_partitions_each_input_row_once_by_venue_identity() {
    let _locale = crate::test_locale::force("en");
    let rows = (0..6).map(|id| row_with_key(id, None)).collect::<Vec<_>>();
    let venues = HashMap::from([
        (1, CoreVenue::identify(200, "", None)),
        (2, CoreVenue::identify(2, "", Some("Bybit legacy spelling"))),
        (
            3,
            CoreVenue::identify(2, "", Some("Bybit current spelling")),
        ),
        (4, CoreVenue::identify(13, "dex-a", Some("Hyperliquid"))),
        (5, CoreVenue::identify(13, "dex-b", Some("Hyperliquid"))),
    ]);

    let lines = flat_lines(&rows, &venues);
    let core_indices = lines
        .iter()
        .filter_map(|line| match line {
            FlatLine::Core(index) => Some(*index),
            FlatLine::Section(_) => None,
        })
        .collect::<Vec<_>>();
    let sections = lines
        .iter()
        .filter_map(|line| match line {
            FlatLine::Section(section) => Some((section.section, section.members)),
            FlatLine::Core(_) => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        core_indices,
        (0..rows.len()).collect::<Vec<_>>(),
        "the flattened member indices must cover each input row exactly once"
    );
    assert_eq!(
        sections,
        vec![
            (core_order::ExchangeSection::Unidentified, 2),
            (core_order::ExchangeSection::Venue(venues[&2].id), 2,),
            (core_order::ExchangeSection::Venue(venues[&4].id), 1,),
            (core_order::ExchangeSection::Venue(venues[&5].id), 1,),
        ],
        "unidentified cores lead, shared venue identities merge, and HIP-3 DEX identities stay distinct"
    );
}

/// Ascending by quota puts the emptiest budget first, and the cores that publish NO quota last.
/// `Option`'s own ordering would do the opposite — `None` sorts below every `Some` — and would fill
/// the head of the column with the twenty cores that never had a quota to run out of.
#[test]
fn an_absent_quota_sorts_behind_every_real_one() {
    let mut low = row_with_key(1, None);
    low.api_quota = Some(900);
    let mut high = row_with_key(2, None);
    high.api_quota = Some(1_065_447);
    let none = row_with_key(3, None);

    assert_eq!(compare_flat_rows(&low, &high, "api_quota"), Ordering::Less);
    assert_eq!(
        compare_flat_rows(&high, &none, "api_quota"),
        Ordering::Less,
        "a full quota still outranks no quota at all"
    );
    assert_eq!(
        compare_flat_rows(&none, &low, "api_quota"),
        Ordering::Greater
    );
}

/// The column has to survive a restart: a sort saved on it must be restored, which only happens if
/// its key is in the allow-list `restore_flat_sort` filters against.
#[test]
fn the_quota_column_is_a_restorable_sort() {
    let restored = restore_flat_sort(Some(TableSortPreference {
        column: "api_quota".to_string(),
        ascending: true,
    }));

    assert_eq!(restored, Some(("api_quota".to_string(), true)));
}
