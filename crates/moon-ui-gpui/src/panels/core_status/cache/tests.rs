//! Synthetic equivalence and performance checks for flat-row snapshots.

use super::{FlatViewCache, replace_snapshots, sort_flat_rows};
use crate::backend::core_warn::LatencySeverity;
use crate::panels::core_status::model::{
    ApiKeyState, CoreStatusRow, ServerKey, ServerStatusGroup, aggregate_servers, ordered_flat_rows,
};
use crate::panels::core_status::ordering::{FlatLine, flat_lines, natural_cmp};
use moon_core::feed::ConnStatus;
use std::collections::HashMap;

/// Omitting the publish generation bump keeps stale sorted rows after a telemetry rebuild.
#[test]
fn publishing_snapshots_invalidates_flat_sort() {
    let (rows, groups) = fixture();
    let mut cached_rows = std::rc::Rc::new(rows);
    let mut cached_groups = std::rc::Rc::new(groups);
    let mut generation = 7;
    let mut flat = FlatViewCache::default();
    let old = flat.get_or_build(generation, None, || {
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
    let fresh = flat.get_or_build(generation, None, || {
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
            exch_warn: false,
            ping_sev: LatencySeverity::Normal,
            exch_sev: LatencySeverity::Normal,
            api_key: ApiKeyState::Unknown,
            api_warn: false,
            api_notice: false,
            api_quota: None,
            api_quota_warn: false,
            server_version: None,
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
        let sorted = cache.get_or_build(7, Some(&sort), || {
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
        cache.get_or_build(generation, sort, || {
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
        let sorted = cache.get_or_build(7, Some(&sort), || {
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
            let sorted = cache.get_or_build(1, Some(&sort), || {
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
