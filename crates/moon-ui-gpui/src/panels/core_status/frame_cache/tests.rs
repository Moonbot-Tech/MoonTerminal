//! Regressions for the per-render memo: a repeat render with an unchanged snapshot must answer
//! every projection from the memo, and the memoized figures must equal a fresh computation.
//!
//! Synthetic data only: 63 cores on `10.0.x.y`, 500 alarm starts, 2 000 history records.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::time::Instant;

use moon_core::feed::UpdateTarget;
use moon_core::session::core_update::{CoreUpdateOutcome, CoreUpdateRecord};

use super::super::layout_fixture;
use super::super::model::{CoreStatusRow, aggregate_servers};
use super::super::{health, ordering, servers_view, table, updates_list};
use super::*;

const NOW_MS: i64 = 1_800_000_000_000;

/// 63 cores, three per server, cycling through the fixture's chip-lighting shapes.
fn fleet() -> Vec<CoreStatusRow> {
    let shapes = layout_fixture::cores();
    (0..63u64)
        .map(|i| {
            let mut row = shapes[(i as usize) % shapes.len()].clone();
            row.id = 1_000 + i;
            row.name = format!("CORE-{i:02}");
            let address = IpAddr::from([10, 0, (i / 3) as u8, 1]);
            if let Some(endpoint) = row.endpoint.as_mut() {
                endpoint.address = address;
            }
            row
        })
        .collect()
}

/// 500 alarm starts spread over the last 48 hours, so about half fall inside the window.
fn alarm_starts() -> Vec<i64> {
    (0..500i64)
        .map(|i| NOW_MS - i * (2 * health::DAY_MS / 500))
        .collect()
}

/// 2 000 closed records, newest first, cycling through the fleet's cores.
fn history() -> Vec<CoreUpdateRecord> {
    (0..2_000u64)
        .map(|i| CoreUpdateRecord {
            core: 1_000 + i % 63,
            core_name: format!("CORE-{:02}", i % 63),
            lane_addr: IpAddr::from([10, 0, ((i % 63) / 3) as u8, 1]),
            from: Some(770),
            from_suffix: None,
            started_ms: 1,
            ended_ms: 2,
            target: UpdateTarget::Release,
            outcome: CoreUpdateOutcome::Succeeded {
                from: Some(770),
                to: 771,
                to_suffix: None,
            },
        })
        .collect()
}

/// A second lookup under an unchanged key must return the stored value and never run its
/// builder. If a future edit keyed a memo on something that moves every frame (a clock, a fresh
/// allocation's address), every hover would rebuild again and the lists would stutter as before.
#[test]
fn a_repeat_lookup_with_an_unchanged_key_is_a_hit_and_skips_the_builder() {
    let mut memo: Memo<u64, Vec<u32>> = Memo::default();
    let calls = Cell::new(0);
    let build = || {
        calls.set(calls.get() + 1);
        vec![1, 2, 3]
    };
    let first = memo.get_or_build(7, build);
    let second = memo.get_or_build(7, || {
        calls.set(calls.get() + 1);
        vec![9]
    });
    assert_eq!(calls.get(), 1, "the builder ran on a hit");
    assert!(
        Rc::ptr_eq(&first, &second),
        "a hit must hand back the stored Rc"
    );
    assert_eq!((memo.hits, memo.builds), (1, 1));
    // A new snapshot generation is a miss and builds exactly once more.
    let third = memo.get_or_build(8, || vec![4]);
    assert_eq!(*third, vec![4]);
    assert_eq!((memo.hits, memo.builds), (1, 2));
}

/// Two renders of one snapshot, through every memo `render` reads, in the keys the view builds
/// them with: the second render must be all hits and call no builder. Dropping a memo from the
/// render path, or keying one on per-frame state, fails here before it reaches a hover.
#[test]
fn a_second_render_with_unchanged_revisions_recomputes_nothing() {
    let rows = fleet();
    let groups = aggregate_servers(&rows, Some(771));
    let starts = alarm_starts();
    let mut frame = FrameCache::default();
    let builds = Cell::new(0u32);
    let generation = 3u64;
    let locale = "ru".to_string();
    let render = |frame: &mut FrameCache| {
        if frame
            .strip
            .cached_while(&generation, |model| NOW_MS < model.valid_until_ms)
            .is_none()
        {
            builds.set(builds.get() + 1);
            frame.strip.store(
                generation,
                strip_model(&rows, &starts, groups.len(), NOW_MS),
            );
        }
        frame.server_names.get_or_build(generation, || {
            builds.set(builds.get() + 1);
            groups
                .iter()
                .map(|group| (group.key, group.display_name.clone()))
                .collect()
        });
        frame
            .servers
            .get_or_build((generation, String::new(), false), || {
                builds.set(builds.get() + 1);
                servers_view::card_order(&groups)
                    .into_iter()
                    .cloned()
                    .collect()
            });
        let columns_key = ColumnsKey {
            generation,
            hidden: HashSet::new(),
            uniform: Vec::new(),
            dragged: HashMap::new(),
            measured_w_bits: 1100.0f32.to_bits(),
            locale: locale.clone(),
        };
        frame.columns.get_or_build(columns_key, || {
            builds.set(builds.get() + 1);
            table::projected_column_keys(
                &rows,
                &HashSet::new(),
                &[],
                &HashMap::new(),
                1100.0,
                &HashMap::new(),
            )
        });
    };
    render(&mut frame);
    let (hits_after_first, builds_after_first) = (frame.hits(), frame.builds());
    assert_eq!(builds.get(), 4);
    render(&mut frame);
    assert_eq!(builds.get(), 4, "a builder ran on the second render");
    assert_eq!(frame.builds(), builds_after_first);
    assert_eq!(
        frame.hits(),
        hits_after_first + 4,
        "every memo must count a hit"
    );
}

/// The memoized strip must equal a fresh `health_counts` at every instant it is served, and must
/// expire exactly when a start crosses the 24 h edge. Serving it a millisecond too long would
/// leave the alarms chip one count off while the operator hovers a quiet panel.
#[test]
fn the_strip_memo_stays_exact_until_the_alarm_window_moves() {
    let rows = fleet();
    let starts = vec![NOW_MS - health::DAY_MS + 5_000, NOW_MS - 10, NOW_MS + 7_000];
    let model = strip_model(&rows, &starts, 21, NOW_MS);
    // The oldest counted start leaves the window 5 s from now; the future start joins in 7 s.
    assert_eq!(model.valid_until_ms, NOW_MS + 5_000);
    for now in [NOW_MS, NOW_MS + 4_999] {
        assert_eq!(
            health::health_counts(&rows, &starts, now),
            model.counts,
            "stale at {now}"
        );
    }
    assert_ne!(
        health::health_counts(&rows, &starts, model.valid_until_ms),
        model.counts,
        "the bound must be the first instant the count actually changes"
    );
    let verdict = health::verdict(&rows, &model.counts, 21);
    assert_eq!(model.verdict, verdict);
    assert_eq!(
        model.segments,
        health::segment_counts(&verdict, &model.counts, 21)
    );
}

/// With no start that can move, the strip never expires on its own.
#[test]
fn a_strip_with_only_old_alarms_never_expires() {
    let model = strip_model(&fleet(), &[NOW_MS - 2 * health::DAY_MS], 21, NOW_MS);
    assert_eq!(model.valid_until_ms, i64::MAX);
}

/// The footer counts must match what `fleet_update_plan` enqueues: the selection narrows only
/// when it is non-empty. Counting the whole candidate list under a selection would label the
/// button with cores the press does not reach.
#[test]
fn plan_counts_follow_the_selection_only_when_one_exists() {
    let lane = IpAddr::from([10, 0, 0, 1]);
    let candidates = [
        UpdateCandidate {
            core: 1,
            lane,
            behind: true,
        },
        UpdateCandidate {
            core: 2,
            lane,
            behind: false,
        },
        UpdateCandidate {
            core: 3,
            lane,
            behind: true,
        },
    ];
    assert_eq!(plan_counts(&candidates, None), (3, 2));
    let only_two = |core: CoreId| core == 2;
    assert_eq!(plan_counts(&candidates, Some(&only_two)), (1, 0));
}

/// Only the first record of each core in a newest-first history may offer a retry.
#[test]
fn history_newest_marks_the_first_record_of_each_core() {
    let mut records = history();
    records.truncate(65);
    let newest = history_newest(&records);
    assert!(newest[..63].iter().all(|flag| *flag));
    assert!(!newest[63] && !newest[64]);
}

/// Per-render cost of each projection before (builder every frame) and after (memo hit), on the
/// 63-core / 500-alarm / 2 000-record fixture. Timing only; run with
/// `cargo test -p moon-ui-gpui --bin moonterminal frame_cache_bench -- --ignored --nocapture`.
#[test]
#[ignore = "timing benchmark, prints a table"]
fn frame_cache_bench() {
    // `flat_lines` overflows the default test-thread stack in a debug build.
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(bench_body)
        .expect("spawn the bench thread")
        .join()
        .expect("bench thread panicked");
}

/// Body of [`frame_cache_bench`], run on a thread with a debug-sized stack.
fn bench_body() {
    const N: u32 = 2_000;
    let rows = fleet();
    let groups = aggregate_servers(&rows, Some(771));
    let starts = alarm_starts();
    let records = history();
    let core_ids: HashSet<CoreId> = rows.iter().map(|row| row.id).collect();
    let venues = HashMap::new();
    let time = |label: &str, f: &mut dyn FnMut()| {
        let start = Instant::now();
        for _ in 0..N {
            f();
        }
        let us = start.elapsed().as_secs_f64() * 1e6 / f64::from(N);
        println!("{label:<44} {us:>9.2} us");
    };
    time("strip: counts+verdict+segments (before)", &mut || {
        std::hint::black_box(strip_model(&rows, &starts, groups.len(), NOW_MS));
    });
    time("flat: projected_column_keys (before)", &mut || {
        std::hint::black_box(table::projected_column_keys(
            &rows,
            &HashSet::new(),
            &[],
            &HashMap::new(),
            1100.0,
            &HashMap::new(),
        ));
    });
    time("flat: flat_lines (before)", &mut || {
        std::hint::black_box(ordering::flat_lines(&rows, &venues));
    });
    time("servers: search filter + card sort (before)", &mut || {
        let shown = servers_view::shown_server_groups(&groups, "", false);
        std::hint::black_box(servers_view::card_order(&shown).len());
    });
    time("updates: versions + history filter (before)", &mut || {
        std::hint::black_box(updates_list::version_distribution(&rows));
        let kept: Vec<&CoreUpdateRecord> = records
            .iter()
            .filter(|record| core_ids.contains(&record.core))
            .collect();
        std::hint::black_box(kept.len());
        std::hint::black_box(history_newest(&records));
    });
    let mut memo: Memo<u64, StripModel> = Memo::default();
    memo.store(1, strip_model(&rows, &starts, groups.len(), NOW_MS));
    time("any memo hit (after)", &mut || {
        std::hint::black_box(memo.cached_while(&1, |model| NOW_MS < model.valid_until_ms));
    });
}

/// The selection figures must equal the old per-frame walk: selected-and-visible, then the plan
/// over the selection, or the whole visible plan when nothing is selected. A selected core that
/// left the view must count for nothing, or the footer names a core its press cannot reach.
#[test]
fn selection_counts_match_the_visible_walk() {
    let model = UpdatesModel {
        visible: HashMap::from([(1, Some(true)), (2, Some(false)), (3, None)]),
        unselected_counts: (2, 1),
        summary: String::new(),
        shares: Vec::new(),
        lags: false,
        newest: String::new(),
        live_rows: Rc::new(Vec::new()),
        history: Rc::new(Vec::new()),
        history_newest: Rc::new(Vec::new()),
        phases: Rc::new(HashMap::new()),
        server_names: Rc::new(HashMap::new()),
        zone: chrono_tz::UTC,
    };
    assert_eq!(selection_counts(&model, &[]), (0, 2, 1));
    // 3 is visible but not offerable; 9 is not visible at all.
    assert_eq!(selection_counts(&model, &[1, 3, 9]), (2, 1, 1));
    assert_eq!(selection_counts(&model, &[2]), (1, 1, 0));
}
