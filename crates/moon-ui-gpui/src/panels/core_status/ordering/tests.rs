//! Tests for the By IP group sort comparator.
//!
//! Explicit imports (no `use super::*`) per the crate's test convention: the panel's parent module
//! re-exports `gpui::*`, whose own `test` would shadow the built-in attribute.

use std::cmp::Ordering;
use std::collections::HashMap;

use moon_core::config::TableSortPreference;
use moon_core::feed::{ConnStatus, CoreTimeOffsetStatus};
use moon_core::session::{CoreStartupStatus, CoreSysStatus};
use moon_core::venue::CoreVenue;

use super::{
    FlatLine, GroupSortField, compare_flat_rows, compare_groups, flat_lines, restore_flat_sort,
    restore_group_sort,
};
use crate::backend::core_warn::LatencySeverity;
use crate::panels::core_status::model::{
    ApiKeyState, CoreStatusRow, GroupVersion, ServerConnectivity, ServerKey, ServerStatusGroup,
    TzOffsetGroup,
};

/// Build one core row carrying only an API-key state: `Some(days)` is a dated key, `None` is a core
/// nothing is known about (no answer, or an answer with no usable date).
fn row_with_key(id: u64, days: Option<i32>) -> CoreStatusRow {
    CoreStatusRow {
        fault: None,
        id,
        name: format!("Core {id}"),
        status: ConnStatus::Ready,
        sys: CoreSysStatus::default(),
        endpoint: None,
        ping_warn: false,
        exch_warn: false,
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
        version_behind: None,
    }
}

/// Build a minimal server group with the fields the comparator reads; `rtts` sets each core's
/// round-trip (and whether it is Ready), so `worst_latency`'s Ready gate can be exercised.
fn group(
    name: &str,
    cpu: Option<u8>,
    proc_mb: Option<u64>,
    free_mb: Option<u16>,
    rtts: &[(u32, bool)],
) -> ServerStatusGroup {
    let cores = rtts
        .iter()
        .enumerate()
        .map(|(i, &(rtt, ready))| CoreStatusRow {
            fault: None,
            id: i as u64,
            name: format!("c{i}"),
            status: if ready {
                ConnStatus::Ready
            } else {
                ConnStatus::Disconnected
            },
            sys: CoreSysStatus {
                round_trip_ms: Some(rtt),
                ..CoreSysStatus::default()
            },
            endpoint: None,
            ping_warn: false,
            exch_warn: false,
            ping_sev: LatencySeverity::Normal,
            exch_sev: LatencySeverity::Normal,
            api_key: ApiKeyState::Unknown,
            api_warn: false,
            api_notice: false,
            api_quota: None,
            api_quota_warn: false,
            startup: CoreStartupStatus::default(),
            time_offset: CoreTimeOffsetStatus::default(),
            server_version: None,
            version_behind: None,
        })
        .collect::<Vec<_>>();
    let ready_count = cores
        .iter()
        .filter(|c| c.status == ConnStatus::Ready)
        .count();
    ServerStatusGroup {
        key: ServerKey::Unknown(0),
        display_name: name.to_string(),
        cpu_warn: false,
        mem_warn: false,
        conn_warn: false,
        ping_warn: false,
        exch_warn: false,
        api_warn: false,
        api_notice: false,
        api_key: ApiKeyState::Unknown,
        version: GroupVersion::Absent,
        version_behind: None,
        tz_offset: TzOffsetGroup::Absent,
        address: None,
        cores,
        ready_count,
        connectivity: ServerConnectivity::Online,
        system_cpu_percent: cpu,
        process_memory_mb: proc_mb,
        free_physical_memory_mb: free_mb,
        logical_cpu_count: None,
        startup: None,
    }
}

/// CPU sorts by the system percentage, and an absent percentage sorts below any value (so unknown
/// servers group at the ascending end).
#[test]
fn cpu_orders_by_system_percent() {
    let hot = group("hot", Some(80), None, None, &[]);
    let cool = group("cool", Some(20), None, None, &[]);
    let unknown = group("unknown", None, None, None, &[]);

    assert_eq!(
        compare_groups(&hot, &cool, GroupSortField::Cpu),
        Ordering::Greater
    );
    assert_eq!(
        compare_groups(&unknown, &cool, GroupSortField::Cpu),
        Ordering::Less,
        "no CPU reading sorts below a known one"
    );
}

/// Memory sorts by the FREE share of the reconstructed total (process RAM + free), not the raw free
/// megabytes: 100 MB free of 200 total (50%) ranks above 150 MB free of 600 total (25%).
#[test]
fn mem_orders_by_free_percentage() {
    let roomy = group("roomy", Some(0), Some(100), Some(100), &[]); // 100/(100+100) = 50%
    let tight = group("tight", Some(0), Some(450), Some(150), &[]); // 150/(450+150) = 25%

    assert_eq!(
        compare_groups(&roomy, &tight, GroupSortField::Mem),
        Ordering::Greater,
        "higher free share ranks higher despite fewer absolute free MB elsewhere"
    );
}

/// Ping sorts by the WORST round-trip among READY cores only: a disconnected core's high stale RTT
/// must not count.
#[test]
fn ping_uses_worst_ready_core_only() {
    // Ready 120 ms, plus a disconnected core stuck at 9000 ms that must be ignored.
    let a = group("a", None, None, None, &[(120, true), (9000, false)]);
    // Ready 300 ms.
    let b = group("b", None, None, None, &[(300, true)]);

    assert_eq!(
        compare_groups(&a, &b, GroupSortField::Ping),
        Ordering::Less,
        "a's worst READY ping (120) is below b's (300); the stale 9000 is ignored"
    );
}

/// Name uses natural order, and any field falls back to the name when it ties, so equal metrics keep
/// a stable order instead of reshuffling.
#[test]
fn name_is_natural_and_the_tiebreak() {
    let s2 = group("Server 2", Some(50), None, None, &[]);
    let s10 = group("Server 10", Some(50), None, None, &[]);

    assert_eq!(
        compare_groups(&s2, &s10, GroupSortField::Name),
        Ordering::Less,
        "Server 2 < Server 10 in natural order"
    );
    // Equal CPU → the comparator falls back to the natural name order.
    assert_eq!(
        compare_groups(&s2, &s10, GroupSortField::Cpu),
        Ordering::Less,
        "equal CPU breaks the tie by name"
    );
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

/// The server row sorts by the very key it displays — the one aggregation already picked — so the
/// column cannot order by one number while showing another.
#[test]
fn a_server_sorts_by_the_key_it_displays() {
    let mut urgent = group("a", None, None, None, &[]);
    urgent.api_key = ApiKeyState::Days(3);
    let mut relaxed = group("b", None, None, None, &[]);
    relaxed.api_key = ApiKeyState::Days(20);

    assert_eq!(
        compare_groups(&urgent, &relaxed, GroupSortField::ApiKey),
        Ordering::Less,
        "3 days beats the other server's soonest (20)"
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

/// `ordering.rs:restore_group_sort` must preserve a valid choice and default unknown keys to Name.
///
/// Mutation: let `from_key` fall through to CPU or discard the stored direction. By-IP would
/// restart on a different column/order, and an exact tuple assertion reddens.
#[test]
fn by_ip_sort_restore_keeps_valid_choice_and_historical_default() {
    assert_eq!(
        restore_group_sort(Some(TableSortPreference {
            column: "api_key".to_string(),
            ascending: false,
        })),
        (GroupSortField::ApiKey, false)
    );
    assert_eq!(
        restore_group_sort(Some(TableSortPreference {
            column: "retired".to_string(),
            ascending: false,
        })),
        (GroupSortField::Name, true)
    );
}

/// `ordering.rs:flat_lines` must retain every source row once while partitioning by venue identity.
///
/// Mutation: remove `lines.extend(members.into_iter().map(FlatLine::Core));`. A core would silently
/// vanish from the operator's fleet list, or a future broken partition could duplicate it.
#[test]
fn flat_lines_partitions_each_input_row_once_by_venue_identity() {
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
            (crate::core_order::ExchangeSection::Unidentified, 2),
            (crate::core_order::ExchangeSection::Venue(venues[&2].id), 2,),
            (crate::core_order::ExchangeSection::Venue(venues[&4].id), 1,),
            (crate::core_order::ExchangeSection::Venue(venues[&5].id), 1,),
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
