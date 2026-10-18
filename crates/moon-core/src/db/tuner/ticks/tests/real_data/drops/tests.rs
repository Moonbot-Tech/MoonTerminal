//! Snapshot routing contracts avoid touching global data-root state in the test process.

use super::{scratch_root, snapshot_root};

/// Neutral deal input isolates timestamp scoring from tape and parameter selection.
fn row() -> super::Replay {
    let mut deal = crate::db::tuner::ticks::tests::deal();
    deal.buy_ms = 70000;
    deal.buy_set_ms = Some(70000);
    super::Replay {
        deal,
        ticks: Vec::new(),
        ui_ticks: None,
        extended_covered: false,
        current: Default::default(),
        legacy: None,
        values: Default::default(),
        dump_ms: None,
        previous_ms: None,
        covered: super::Coverage::none(),
        ui_spans: None,
    }
}

/// A close detect cannot rescue a late order; a close order can follow an earlier detect.
#[test]
fn timing_scores_placement_and_reports_detector_separately() {
    let row = row();
    let mut metrics = super::Metrics::default();
    for (detect_ms, order_ms, expected_hits) in [(68000, 73000, 0), (60000, 71000, 1)] {
        metrics.observe_trace(
            &row,
            crate::db::tuner::ticks::drops::DropsTrace {
                detect_ms: Some(detect_ms),
                order_ms: Some(order_ms),
                fill: Some(crate::db::tuner::ticks::Fill {
                    t_ms: order_ms,
                    price: row.deal.buy_price,
                }),
                ..Default::default()
            },
            None,
        );
        assert_eq!(metrics.detect, expected_hits);
        assert_eq!(metrics.joint, expected_hits);
    }
    assert_eq!(metrics.n, 2);
    assert_eq!(metrics.detect, 1);
    assert_eq!(metrics.detector, 1);
    assert_eq!(metrics.joint, 1);
    assert_eq!(metrics.dt, vec![-2.0, -10.0]);
    assert_eq!(metrics.placement_dt, vec![3.0, 1.0]);
}

/// A candidate with unavailable inputs must lose agreement without shrinking its cohort.
#[test]
fn candidate_refusal_and_degeneracy_remain_paired_misses() {
    let row = row();
    let mut metrics = super::Metrics::default();
    for (refused, degenerate) in [(true, false), (false, true)] {
        metrics.observe_trace(
            &row,
            crate::db::tuner::ticks::drops::DropsTrace {
                refused,
                degenerate,
                ..Default::default()
            },
            None,
        );
    }
    assert_eq!(metrics.n, 2);
    assert_eq!(metrics.joint, 0);
    assert_eq!(metrics.candidate_refused, 1);
    assert_eq!(metrics.candidate_degenerate, 1);
}

/// The override receives the copy, including hot-database companions and read-side settings.
#[test]
fn snapshot_override_routes_to_copy_before_readers() {
    let source_guard = scratch_root().unwrap();
    let source = source_guard.0.clone();
    for (relative, bytes) in [
        ("data/a.sqlite", "database"),
        ("data/a.sqlite-wal", "wal"),
        ("data/a.sqlite-shm", "shm"),
        ("cfg/layout.json", "settings"),
        ("logs/read.log", "log"),
    ] {
        let path = source.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    let copied = snapshot_root(&source, |override_root| {
        assert_ne!(override_root, source);
        for relative in [
            "data/a.sqlite",
            "data/a.sqlite-wal",
            "data/a.sqlite-shm",
            "cfg/layout.json",
            "logs/read.log",
        ] {
            assert_eq!(
                std::fs::read(override_root.join(relative)).unwrap(),
                std::fs::read(source.join(relative)).unwrap()
            );
        }
        std::fs::write(override_root.join("data/a.sqlite"), "copy recovery").unwrap();
        true
    })
    .unwrap();
    assert_ne!(copied.0, source);
    assert_eq!(
        std::fs::read_to_string(source.join("data/a.sqlite")).unwrap(),
        "database"
    );
    let copied_path = copied.0.clone();
    drop(copied);
    assert!(!copied_path.exists());
    drop(source_guard);
    assert!(!source.exists());
}

/// The cached optimizer must agree with direct zones across grid edges and missing warm-up.
#[test]
fn unit_grid_cache_matches_direct_complete_zone_readings() {
    use crate::db::tuner::ticks::drops::{DropsDetect, units::Units};
    use crate::feed::types::{Side, Tick};
    let mut r = row();
    r.deal.buy_set_ms = Some(300000);
    r.deal.drops_detect = Some(DropsDetect {
        x_price_delta_pct: 3.0,
        price_is_low: false,
    });
    let ticks: Vec<_> = (0..=300)
        .map(|i| Tick {
            time_ms: f64::from(i) * 1000.0,
            price: (100 + i % 17) as f32,
            qty: 1.0,
            side: Side::Buy,
        })
        .collect();
    for tape in [&ticks[..], &ticks[200..], &ticks[290..]] {
        let cache = super::units::Cached::new(&r, tape);
        for w in [1, 2, 10, 120] {
            for k in [1, 2, 20] {
                for j in [0, 1, 6] {
                    let u = Units {
                        explicit: Some((w as u32 * 2, k, j)),
                        ..Default::default()
                    };
                    let direct = u
                        .reading(&r.current, tape, 300000)
                        .map_or(f64::INFINITY, |v| (v.drop_pct - 3.0).abs());
                    let cached = cache.error(w, k, j);
                    assert!(direct == cached || (direct - cached).abs() < 1e-10);
                }
            }
        }
    }
}
