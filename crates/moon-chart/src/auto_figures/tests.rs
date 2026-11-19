use super::test_bars::{M5_MS, from_hl};
use super::*;
use moon_core::config::auto_figures::{SqueezeParams, SwingParams};

/// A found level reports the money traded on its touching bars and the tolerance it grouped them
/// by: three equal highs at 100, with the touching bars' turnover set apart from the rest, give
/// the sum of exactly those three bars and `tolerance_pct / 100`.
///
/// Breakage: `touch_volume` summing the whole window (or the pivot's neighbours), counting a bar
/// twice, or `tolerance_frac` left in % — the zone band would be a hundred times too wide.
#[test]
fn a_level_reports_its_touch_volume_and_tolerance() {
    let mut bars = from_hl(&[
        (90.0, 85.0),
        (100.0, 95.0),
        (90.0, 85.0),
        (91.0, 86.0),
        (100.0, 95.0),
        (91.0, 86.0),
        (92.0, 87.0),
        (100.0, 95.0),
        (92.0, 87.0),
    ]);
    for b in &mut bars {
        b.quote_volume = 1.0;
    }
    bars[1].quote_volume = 100.0;
    bars[4].quote_volume = 200.0;
    bars[7].quote_volume = 400.0;
    let ctx = DetectCtx {
        tf_ms: M5_MS,
        now_ms: 0.0,
    };
    let swing = DetectorParams::Swing(SwingParams {
        pivot_left: 1,
        pivot_right: 1,
        min_touches: 3,
        tolerance_pct: 0.5,
        ..SwingParams::default()
    });
    let found = detect(&swing, &bars, &ctx);
    let level = found
        .iter()
        .find(|f| f.shape == FoundShape::Horizontal { price: 100.0 })
        .expect("the three highs make a level");
    assert_eq!(level.touch_volume, 700.0);
    assert_eq!(level.tolerance_frac, 0.005);

    // A touch on a bar the input does not hold adds nothing; one bar touched twice counts once.
    let touches = [
        Touch {
            t_ms: bars[1].t_open_ms,
            price: 100.0,
        },
        Touch {
            t_ms: bars[1].t_open_ms,
            price: 95.0,
        },
        Touch {
            t_ms: bars[8].t_open_ms + 1.0,
            price: 92.0,
        },
    ];
    assert_eq!(touch_volume(&bars, &touches), 100.0);
}

/// A squeeze reports its LEVEL's tolerance — the band drawn around the level — not its sloped
/// half's. Breakage: the trend tolerance reported instead, drawing the zone at the line's width.
#[test]
fn a_squeeze_reports_its_level_tolerance() {
    let p = SqueezeParams::default();
    assert_ne!(p.level.tolerance_pct, p.trend_tolerance_pct);
    // The default squeeze's search on the committed bench finds formations (the squeeze module's
    // own bench test pins that); each must carry the level's band.
    let params = DetectorParams::Squeeze(SqueezeParams {
        level: SwingParams {
            lookback_bars: 2_000,
            ..p.level
        },
        ..p
    });
    let mut formations = 0;
    for kind in [1u32, 5] {
        let bars = test_bars::ace(kind);
        let ctx = DetectCtx {
            tf_ms: i64::from(kind) * 60_000,
            now_ms: bars.last().map_or(0.0, |b| b.t_open_ms),
        };
        for f in detect(&params, &bars, &ctx) {
            assert_eq!(f.tolerance_frac, f64::from(p.level.tolerance_pct) / 100.0);
            formations += 1;
        }
    }
    assert!(formations > 0, "the bench holds a squeeze");
}

/// Breakage: a detector missing from the table makes `detect` silently return nothing for a whole
/// kind; a duplicate row makes the second one unreachable.
#[test]
fn registry_has_exactly_one_row_per_kind() {
    for kind in DetectorKind::ALL {
        let rows = REGISTRY.iter().filter(|d| d.kind() == kind).count();
        assert_eq!(rows, 1, "{kind:?} must have exactly one registry row");
    }
    assert_eq!(REGISTRY.len(), DetectorKind::ALL.len());
}

/// `detect` must reach the detector the parameters name: three equal highs are a `Swing` level,
/// and the same bars handed `PeriodExtreme` parameters give the day's high and low instead.
///
/// Breakage: dispatching by position instead of by kind.
#[test]
fn detect_dispatches_by_the_parameters_kind() {
    let bars = from_hl(&[
        (90.0, 85.0),
        (100.0, 95.0),
        (90.0, 85.0),
        (91.0, 86.0),
        (100.0, 95.0),
        (91.0, 86.0),
        (92.0, 87.0),
        (100.0, 95.0),
        (92.0, 87.0),
    ]);
    let ctx = DetectCtx {
        tf_ms: M5_MS,
        now_ms: 0.0,
    };
    let swing = DetectorParams::Swing(SwingParams {
        pivot_left: 1,
        pivot_right: 1,
        min_touches: 3,
        tolerance_pct: 0.1,
        ..SwingParams::default()
    });
    let found = detect(&swing, &bars, &ctx);
    assert!(
        found
            .iter()
            .any(|f| f.shape == FoundShape::Horizontal { price: 100.0 }),
        "{found:?}"
    );
    let day = DetectorKind::PeriodExtreme.default_params();
    let shapes: Vec<FoundShape> = detect(&day, &bars, &ctx).iter().map(|f| f.shape).collect();
    assert_eq!(
        shapes,
        vec![
            FoundShape::Horizontal { price: 100.0 },
            FoundShape::Horizontal { price: 85.0 }
        ]
    );
}

/// A line is linear in time and refuses to hand out a non-positive price.
///
/// Breakage: interpolating by bar index, dividing by a zero span, or returning a negative price a
/// caller then divides by.
#[test]
fn line_price_is_linear_in_time_and_positive() {
    assert_eq!(line_price_at(0.0, 100.0, 10.0, 110.0, 5.0), Some(105.0));
    assert_eq!(line_price_at(0.0, 100.0, 10.0, 110.0, 20.0), Some(120.0));
    assert_eq!(line_price_at(0.0, 100.0, 0.0, 110.0, 5.0), None);
    assert_eq!(line_price_at(0.0, 100.0, 10.0, 50.0, 30.0), None);
    assert_eq!(line_price_at(0.0, f64::NAN, 10.0, 50.0, 3.0), None);
    let squeeze = FoundShape::Squeeze {
        level: 100.0,
        t0_ms: 0.0,
        p0: 90.0,
        t1_ms: 10.0,
        p1: 95.0,
        pressed: false,
    };
    assert_eq!(squeeze.price_at(1e12), Some(100.0));
}

/// Empty and too-short input never panics and finds nothing, for every detector.
///
/// Breakage: an unchecked index on an empty or 1-bar slice inside any detector.
#[test]
fn every_detector_survives_empty_and_tiny_input() {
    let ctx = DetectCtx {
        tf_ms: M5_MS,
        now_ms: 0.0,
    };
    let one = from_hl(&[(100.0, 99.0)]);
    let nan = vec![test_bars::bar(0, f32::NAN, f32::NAN, f32::NAN, f32::NAN); 20];
    for kind in DetectorKind::ALL {
        let params = kind.default_params();
        assert!(detect(&params, &[], &ctx).is_empty(), "{kind:?} on nothing");
        assert!(
            detect(&params, &nan, &ctx).is_empty(),
            "{kind:?} on NaN bars"
        );
        if kind != DetectorKind::PeriodExtreme {
            assert!(
                detect(&params, &one, &ctx).is_empty(),
                "{kind:?} on one bar"
            );
        }
    }
}
