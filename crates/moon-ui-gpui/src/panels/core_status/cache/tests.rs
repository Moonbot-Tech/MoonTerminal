//! Synthetic equivalence and performance checks for flat-row snapshots.

use super::{
    FlatViewCache, WarnReadCache, WarnReadKey, card_matches_query, project_rows,
    projection_uniform, replace_snapshots, sort_flat_rows,
};
use crate::backend::core_warn::{LatencySeverity, WarnAxis, WarnEpisode, WarnSnapshot};
use crate::panels::core_status::health::HealthFilter;
use crate::panels::core_status::model::{
    ApiKeyState, CoreStatusRow, ServerKey, ServerStatusGroup, aggregate_servers, ordered_flat_rows,
};
use crate::panels::core_status::ordering::{FlatLine, flat_lines, natural_cmp};
use moon_core::feed::ConnStatus;
use moon_core::feed::CoreEndpoint;
use std::collections::HashMap;

/// Build one closed warning for persisted-read publication tests.
fn warning(id: u64) -> WarnEpisode {
    WarnEpisode {
        id,
        axis: WarnAxis::SysCpu,
        server_ip: Some("10.0.0.1".parse().unwrap()),
        core_id: None,
        start_ms: id as i64,
        end_ms: Some(id as i64 + 1),
        peak: 1,
        snap: WarnSnapshot::default(),
    }
}

/// Build a persisted-read key with one server and the requested cores.
fn warn_key(revision: u64, cores: &[u64]) -> WarnReadKey {
    WarnReadKey::new(
        Some(revision),
        &cores.iter().copied().collect(),
        &std::collections::HashSet::from(["10.0.0.1".parse().unwrap()]),
    )
}

/// An unchanged key must not requery; a revision or effective-scope change must start a new read.
#[test]
fn warning_reads_start_only_for_revision_or_scope_changes() {
    let mut cache = WarnReadCache::default();
    let key = warn_key(4, &[2, 1]);
    assert_eq!(key, warn_key(4, &[1, 2]));
    assert!(cache.begin(key.clone()).is_some());
    assert!(cache.begin(key).is_none());
    assert!(cache.begin(warn_key(5, &[1, 2])).is_some());
    assert!(cache.begin(warn_key(5, &[1, 2, 3])).is_some());
}

/// An older SQLite completion must not replace rows loaded for a newer revision or scope.
#[test]
fn out_of_order_warning_reads_keep_the_newest_snapshot() {
    let mut cache = WarnReadCache::default();
    let (older, _) = cache.begin(warn_key(1, &[1])).unwrap();
    let (newer, _) = cache.begin(warn_key(2, &[1])).unwrap();
    assert!(!cache.apply(older, vec![warning(1)]));
    assert!(cache.persisted().is_empty());
    assert!(cache.apply(newer, vec![warning(2)]));
    assert_eq!(
        cache
            .persisted()
            .iter()
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        vec![2]
    );
}

/// A failed current read must not suppress retries forever; a stale failure must not reset the new key.
#[test]
fn failed_warning_read_retries_only_the_current_sequence() {
    let mut cache = WarnReadCache::default();
    let key = warn_key(1, &[1]);
    let (older, _) = cache.begin(key.clone()).unwrap();
    let (newer, _) = cache.begin(warn_key(2, &[1])).unwrap();
    cache.fail(older);
    assert!(cache.begin(warn_key(2, &[1])).is_none());
    cache.fail(newer);
    assert!(cache.begin(warn_key(2, &[1])).is_some());
}

/// Omitting the publish generation bump keeps stale sorted rows after a telemetry rebuild.
#[test]
fn publishing_snapshots_invalidates_flat_sort() {
    let (rows, groups) = fixture();
    let mut cached_rows = std::rc::Rc::new(rows);
    let mut cached_groups = std::rc::Rc::new(groups);
    let mut generation = 7;
    let mut flat = FlatViewCache::default();
    let old = flat.get_or_build(generation, None, None, "", false, || {
        sort_flat_rows(&cached_rows, None, &cached_groups)
    });
    let (mut rows, groups) = fixture();
    rows[0].name = "Changed telemetry".into();
    replace_snapshots(
        &mut cached_rows,
        &mut cached_groups,
        &mut generation,
        rows,
        groups,
    );
    assert_eq!(generation, 8);
    let fresh = flat.get_or_build(generation, None, None, "", false, || {
        sort_flat_rows(&cached_rows, None, &cached_groups)
    });
    assert!(!std::rc::Rc::ptr_eq(&old, &fresh));
    assert!(fresh.iter().any(|row| row.name == "Changed telemetry"));
    let source: String = include_str!("../cache.rs")
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    assert!(source.contains("replace_snapshots(&mutself.cached_rows,&mutself.cached_groups,&mutself.rows_generation,rows,groups"));
}

/// Mixed readiness and names make attention order and stable natural-name ties observable.
fn fixture() -> (Vec<CoreStatusRow>, Vec<ServerStatusGroup>) {
    let rows: Vec<_> = (0..200)
        .rev()
        .map(|id| CoreStatusRow {
            id,
            name: format!("Core {id}"),
            status: if id % 3 == 0 {
                ConnStatus::Disconnected
            } else {
                ConnStatus::Ready
            },
            sys: Default::default(),
            startup: Default::default(),
            time_offset: Default::default(),
            fault: None,
            mode_suggestion: None,
            endpoint: None,
            ping_warn: false,
            ping_sev: LatencySeverity::Normal,
            exch_sev: LatencySeverity::Normal,
            api_key: ApiKeyState::Unknown,
            api_warn: false,
            api_notice: false,
            api_quota: None,
            api_quota_warn: false,
            server_version: None,
            server_version_suffix: None,
            version_behind: None,
            update: None,
        })
        .collect();
    let mut groups = aggregate_servers(&rows, None);
    for (n, group) in groups.iter_mut().enumerate() {
        group.display_name = format!("Synthetic Server {}", n % 17);
    }
    groups.pop();
    (rows, groups)
}

/// Frozen clone-based comparator is the independent pre-optimization ordering oracle.
fn reference(
    rows: &[CoreStatusRow],
    groups: &[ServerStatusGroup],
    ascending: bool,
) -> Vec<CoreStatusRow> {
    let names: HashMap<ServerKey, String> = groups
        .iter()
        .map(|g| (g.key, g.display_name.clone()))
        .collect();
    let mut out = ordered_flat_rows(rows);
    out.sort_by(|a, b| {
        let a = names
            .get(&ServerKey::for_row(a))
            .cloned()
            .unwrap_or_default();
        let b = names
            .get(&ServerKey::for_row(b))
            .cloned()
            .unwrap_or_default();
        let cmp = natural_cmp(&a, &b);
        if ascending { cmp } else { cmp.reverse() }
    });
    out
}

/// Borrowed name sorting must retain missing-name placement and stable ties in both directions.
#[test]
fn server_column_sort_matches_reference_order() {
    let (rows, groups) = fixture();
    for ascending in [false, true] {
        let sort = ("server".to_string(), ascending);
        let actual = sort_flat_rows(&rows, Some(&sort), &groups);
        let expected = reference(&rows, &groups, ascending);
        assert_eq!(
            actual.iter().map(|r| r.id).collect::<Vec<_>>(),
            expected.iter().map(|r| r.id).collect::<Vec<_>>()
        );
    }
}

/// Dropping either key component would leave stale row order; rebuilding on hits wastes sorts.
#[test]
fn flat_view_builds_once_per_generation_and_sort() {
    let (rows, groups) = fixture();
    let sort = ("server".to_string(), true);
    let reverse = ("server".to_string(), false);
    let mut cache = FlatViewCache::default();
    let builds = std::cell::Cell::new(0);
    let mut previous = None;
    for _ in 0..100 {
        let sorted = cache.get_or_build(7, Some(&sort), None, "", false, || {
            builds.set(builds.get() + 1);
            sort_flat_rows(&rows, Some(&sort), &groups)
        });
        if let Some(ref previous) = previous {
            assert!(std::rc::Rc::ptr_eq(previous, &sorted));
        }
        previous = Some(sorted);
    }
    assert_eq!(builds.get(), 1);
    for (generation, sort) in [(8, Some(&sort)), (8, Some(&reverse)), (8, None)] {
        cache.get_or_build(generation, sort, None, "", false, || {
            builds.set(builds.get() + 1);
            sort_flat_rows(&rows, sort, &groups)
        });
    }
    assert_eq!(builds.get(), 4);
}

/// Compare every heading field and member index without changing production line types.
fn line_signature(lines: &[FlatLine]) -> Vec<String> {
    lines
        .iter()
        .map(|line| match line {
            FlatLine::Core(index) => format!("core:{index}"),
            FlatLine::Section(section) => format!(
                "section:{:?}:{}:{:?}:{}",
                section.section, section.label, section.brand, section.members
            ),
        })
        .collect()
}

/// Caching heading lines would freeze venue membership and translated captions on a key hit.
#[test]
fn flat_view_refreshes_venues_and_locale_with_fixed_sort_key() {
    let (rows, groups) = fixture();
    let sort = ("server".to_string(), true);
    let mut cache = FlatViewCache::default();
    let mut venues = HashMap::new();
    let mut previous_lines = None;
    let mut first_rows = None;
    for (locale, add_venue) in [("en", false), ("en", true), ("ru", true)] {
        let _locale = crate::test_locale::force(locale);
        if add_venue {
            venues.insert(
                1,
                moon_core::venue::CoreVenue::identify(2, "", Some("Synthetic venue")),
            );
        }
        let sorted = cache.get_or_build(7, Some(&sort), None, "", false, || {
            sort_flat_rows(&rows, Some(&sort), &groups)
        });
        if let Some(ref first) = first_rows {
            assert!(std::rc::Rc::ptr_eq(first, &sorted));
        } else {
            first_rows = Some(sorted.clone());
        }
        let actual = line_signature(&flat_lines(&sorted, &venues));
        let uncached = reference(&rows, &groups, true);
        assert_eq!(actual, line_signature(&flat_lines(&uncached, &venues)));
        if let Some(previous) = previous_lines {
            assert_ne!(actual, previous);
        }
        previous_lines = Some(actual);
    }
}

/// Measure the server sort including row copies and the name index.
#[test]
#[ignore]
fn bench_core_status_server_sort_200() {
    let (rows, groups) = fixture();
    let sort = ("server".to_string(), true);
    let iterations = 2000;
    let start = std::time::Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(sort_flat_rows(
            std::hint::black_box(&rows),
            Some(&sort),
            &groups,
        ));
    }
    println!(
        "bench_core_status_server_sort_200 ns/iter={}",
        start.elapsed().as_nanos() / iterations
    );
}

/// Measure 100 renders on one generation, including fresh heading construction each time.
#[test]
#[ignore]
fn bench_core_status_flat_view_200() {
    let _locale = crate::test_locale::force("en");
    let (rows, groups) = fixture();
    let sort = ("server".to_string(), true);
    let venues = HashMap::new();
    let iterations = 200;
    let start = std::time::Instant::now();
    for _ in 0..iterations {
        let mut cache = FlatViewCache::default();
        for _ in 0..100 {
            let sorted = cache.get_or_build(1, Some(&sort), None, "", false, || {
                sort_flat_rows(&rows, Some(&sort), &groups)
            });
            std::hint::black_box(flat_lines(&sorted, &venues));
        }
    }
    println!(
        "bench_core_status_flat_view_200 ns/100-renders={}",
        start.elapsed().as_nanos() / iterations
    );
}

/// One synthetic core. Callers set the field the assertion reads.
fn sample(id: u64, name: &str, status: ConnStatus) -> CoreStatusRow {
    CoreStatusRow {
        id,
        name: name.to_string(),
        status,
        sys: Default::default(),
        startup: Default::default(),
        time_offset: Default::default(),
        fault: None,
        mode_suggestion: None,
        endpoint: None,
        ping_warn: false,
        ping_sev: LatencySeverity::Normal,
        exch_sev: LatencySeverity::Normal,
        api_key: ApiKeyState::Unknown,
        api_warn: false,
        api_notice: false,
        api_quota: None,
        api_quota_warn: false,
        server_version: None,
        server_version_suffix: None,
        version_behind: None,
        update: None,
    }
}

/// A ping chip keeps the core that is over baseline and drops the quiet one.
#[test]
fn health_filter_keeps_only_the_matching_cores() {
    let mut hot = sample(1, "CORE-A1", ConnStatus::Ready);
    hot.ping_warn = true;
    let quiet = sample(2, "CORE-A2", ConnStatus::Ready);
    let rows = vec![hot, quiet];
    let groups = aggregate_servers(&rows, None);
    let kept = project_rows(
        &rows,
        &groups,
        Some(HealthFilter::Ping),
        "",
        false,
        None,
        &["core"],
    );
    assert_eq!(kept.iter().map(|row| row.id).collect::<Vec<_>>(), vec![1]);
}

/// Search matches the core name, the server display name, and an unmasked address.
#[test]
fn search_matches_name_server_and_unmasked_address() {
    let mut core = sample(1, "CORE-A1", ConnStatus::Ready);
    core.endpoint = Some(CoreEndpoint {
        address: "10.0.0.8".parse().unwrap(),
        port: 1,
    });
    let rows = vec![core];
    let mut groups = aggregate_servers(&rows, None);
    groups[0].display_name = "Synthetic Host".to_string();
    let columns = ["server", "core"];
    for query in ["core-a1", "host", "10.0.0.8"] {
        let matched = project_rows(&rows, &groups, None, query, false, None, &columns);
        assert_eq!(matched.len(), 1, "{query}");
    }
}

/// An address query matches nothing once the column is masked.
#[test]
fn ip_query_does_not_match_while_addresses_are_masked() {
    let mut core = sample(1, "CORE-A1", ConnStatus::Ready);
    core.endpoint = Some(CoreEndpoint {
        address: "10.0.0.8".parse().unwrap(),
        port: 1,
    });
    let rows = vec![core];
    let mut groups = aggregate_servers(&rows, None);
    groups[0].display_name = "Synthetic Host".to_string();
    let matched = project_rows(
        &rows,
        &groups,
        None,
        "10.0.0.8",
        true,
        None,
        &["server", "core"],
    );
    assert!(matched.is_empty());
}

/// A server card matches its name, a core name, and its address, and not the address while masked.
/// An empty query keeps the card.
#[test]
fn card_filter_matches_name_core_and_unmasked_ip() {
    let mut core = sample(1, "CORE-A1", ConnStatus::Ready);
    core.endpoint = Some(CoreEndpoint {
        address: "10.1.2.3".parse().unwrap(),
        port: 9,
    });
    let rows = vec![core];
    let mut groups = aggregate_servers(&rows, None);
    groups[0].display_name = "Host Nine".to_string();
    let card = &groups[0];
    assert!(card_matches_query(card, "", false));
    assert!(card_matches_query(card, "host", false));
    assert!(card_matches_query(card, "core-a1", false));
    assert!(card_matches_query(card, "10.1.2.3", false));
    assert!(!card_matches_query(card, "10.1.2.3", true));
    assert!(!card_matches_query(card, "other", false));
}

/// Version agreement follows the searched rows. A core the search hides cannot veto the column,
/// and it cannot be hidden by cores the search excluded.
#[test]
fn projection_uniform_follows_the_search() {
    // Uniform facts translate; the first lookup must not run on the 2 MiB test stack.
    crate::test_locale::warm();
    let mut alpha_a = sample(1, "alpha-1", ConnStatus::Ready);
    let mut alpha_b = sample(2, "alpha-2", ConnStatus::Ready);
    let mut beta = sample(3, "beta", ConnStatus::Ready);
    alpha_a.server_version = Some(771);
    alpha_b.server_version = Some(771);
    beta.server_version = Some(770);
    let rows = vec![alpha_a, alpha_b, beta];
    let groups = aggregate_servers(&rows, None);
    let searched = projection_uniform(&rows, &groups, None, "alpha", false);
    assert!(searched.iter().any(|fact| fact.key == "version"));
    let all = projection_uniform(&rows, &groups, None, "", false);
    assert!(all.iter().all(|fact| fact.key != "version"));
}

/// A saved API-key sort whose column is hidden stays on attention order.
///
/// Unknown sorts before Perpetual, so a visible column would put the Ready core first.
/// Attention order puts the disconnected core first instead.
#[test]
fn hidden_api_key_sort_keeps_attention_order() {
    let ready = sample(1, "CORE-A1", ConnStatus::Ready);
    let mut down = sample(2, "CORE-A2", ConnStatus::Disconnected);
    down.api_key = ApiKeyState::Perpetual;
    let rows = vec![ready, down];
    let groups = aggregate_servers(&rows, None);
    let sort = ("api_key".to_string(), true);
    let hidden = project_rows(
        &rows,
        &groups,
        None,
        "",
        false,
        Some(&sort),
        &["core", "status"],
    );
    assert_eq!(hidden[0].id, 2);
    let shown = project_rows(
        &rows,
        &groups,
        None,
        "",
        false,
        Some(&sort),
        &["core", "api_key"],
    );
    assert_eq!(shown[0].id, 1);
}

/// A user-hidden name column does not apply a descending name sort.
#[test]
fn hidden_core_sort_keeps_attention_order() {
    let ready = sample(1, "BBB", ConnStatus::Ready);
    let down = sample(2, "AAA", ConnStatus::Disconnected);
    let rows = vec![ready, down];
    let groups = aggregate_servers(&rows, None);
    let sort = ("core".to_string(), false);
    let hidden = project_rows(&rows, &groups, None, "", false, Some(&sort), &["status"]);
    assert_eq!(hidden[0].id, 2);
    let shown = project_rows(&rows, &groups, None, "", false, Some(&sort), &["core"]);
    assert_eq!(shown[0].id, 1);
}

/// Filter, query, and the address mask are part of the cache key.
#[test]
fn flat_cache_rebuilds_when_the_filter_query_or_mask_changes() {
    let (rows, groups) = fixture();
    let mut cache = FlatViewCache::default();
    let builds = std::cell::Cell::new(0);
    let first = cache.get_or_build(1, None, None, "", false, || {
        builds.set(builds.get() + 1);
        sort_flat_rows(&rows, None, &groups)
    });
    let again = cache.get_or_build(1, None, None, "", false, || {
        builds.set(builds.get() + 1);
        sort_flat_rows(&rows, None, &groups)
    });
    assert!(std::rc::Rc::ptr_eq(&first, &again));
    assert_eq!(builds.get(), 1);
    let _ = cache.get_or_build(1, None, Some(HealthFilter::Ping), "", false, || {
        builds.set(builds.get() + 1);
        Vec::new()
    });
    let _ = cache.get_or_build(1, None, Some(HealthFilter::Ping), "core", false, || {
        builds.set(builds.get() + 1);
        Vec::new()
    });
    let _ = cache.get_or_build(1, None, Some(HealthFilter::Ping), "core", true, || {
        builds.set(builds.get() + 1);
        Vec::new()
    });
    assert_eq!(builds.get(), 4);
}
