use super::super::test_bars::{MONDAY_2024_01_01_MS, ace};
use super::*;

/// One hour, ms.
const H1: f64 = 3_600_000.0;
const DAY: f64 = 86_400_000.0;

/// An hourly bar `hours` after Monday 2024-01-01 00:00 UTC spanning `low..high`, closing at
/// `close`, with `qv` of turnover.
fn hb(hours: i64, low: f32, high: f32, close: f32, qv: f32) -> ChartCandle {
    ChartCandle {
        t_open_ms: MONDAY_2024_01_01_MS + hours as f64 * H1,
        open: close,
        high,
        low,
        close,
        volume: 1.0,
        quote_volume: qv,
    }
}

fn params(period: ProfilePeriod, bins: u16, naked: u8) -> VolumeProfileParams {
    VolumeProfileParams {
        period,
        poc: true,
        vah: true,
        val: true,
        value_area_pct: 70.0,
        naked_poc_days: naked,
        bins,
    }
}

fn run(bars: &[ChartCandle], p: VolumeProfileParams, now_ms: f64) -> Vec<Found> {
    VolumeProfile.detect(
        bars,
        &DetectorParams::VolumeProfile(p),
        &DetectCtx {
            tf_ms: 3_600_000,
            now_ms,
        },
    )
}

fn tagged(found: &[Found]) -> Vec<(LineTag, f64)> {
    found
        .iter()
        .map(|f| {
            let FoundShape::Horizontal { price } = f.shape else {
                panic!("a level");
            };
            (f.tag.expect("tagged"), price)
        })
        .collect()
}

/// Four hourly bars from Monday 00:00, read at 04:00, `scale` times the prices below.
///
/// By hand, at `scale = 1`, 10 bins of width 1 over 100..110:
/// - 100..110, 100 traded → 10 in every bin;
/// - 104..106, 60 → 30 into bins 4 and 5;
/// - 105 flat, 40 → all into bin 5;
/// - 101..103, 20 → 10 into bins 1 and 2.
///
/// Bins 0..9: 10 20 20 10 40 80 10 10 10 10, total 220. POC = bin 5, centre 105.5. The value
/// area (70 % = 154) grows from 80: down 40 (vs up 10) → 120; down 10 ties up 10 → the lower side
/// → 130; down 20 → 150; down 20 → 170 ≥ 154. Bins 1..=5: VAL = 101, VAH = 106.
fn four_bars(scale: f32) -> Vec<ChartCandle> {
    let s = |v: f32| v * scale;
    vec![
        hb(0, s(100.0), s(110.0), s(105.0), 100.0),
        hb(1, s(104.0), s(106.0), s(105.0), 60.0),
        hb(2, s(105.0), s(105.0), s(105.0), 40.0),
        hb(3, s(101.0), s(103.0), s(102.0), 20.0),
    ]
}

/// The day's profile by hand: POC, VAH and VAL, each with its bin's volume, its share as strength
/// and its bin as the band.
///
/// Breakage: the POC at a bin edge instead of its centre, a bar's volume not split by overlap, the
/// value area growing toward the smaller side, or a tie broken upward.
#[test]
fn the_days_profile_by_hand() {
    let now = MONDAY_2024_01_01_MS + 4.0 * H1;
    let found = run(&four_bars(1.0), params(ProfilePeriod::Day, 10, 0), now);
    assert_eq!(
        tagged(&found),
        vec![
            (LineTag::Poc, 105.5),
            (LineTag::Vah, 106.0),
            (LineTag::Val, 101.0)
        ]
    );
    assert_eq!(found[0].touch_volume, 80.0);
    assert_eq!(found[2].touch_volume, 20.0);
    assert_eq!(found[0].strength, (80.0f64 / 220.0) as f32);
    assert_eq!(found[0].tolerance_frac, 1.0 / 105.5);
    assert_eq!(found[0].first_ms, MONDAY_2024_01_01_MS);
}

/// The same day at prices under 1e-4 — scaled by 2⁻²¹, a power of two, so every product is exact
/// and the answer is the one above scaled the same way.
///
/// Breakage: a bin edge or a POC rounded through `f32`, or a minimum price step assumed somewhere.
#[test]
fn a_sub_cent_day_scales_exactly() {
    let scale = 1.0 / 2_097_152.0;
    let now = MONDAY_2024_01_01_MS + 4.0 * H1;
    let found = run(&four_bars(scale), params(ProfilePeriod::Day, 10, 0), now);
    let s = f64::from(scale);
    assert_eq!(
        tagged(&found),
        vec![
            (LineTag::Poc, 105.5 * s),
            (LineTag::Vah, 106.0 * s),
            (LineTag::Val, 101.0 * s)
        ]
    );
    assert!(105.5 * s < 1e-4);
}

/// Two bins tied for the most volume: the POC is the one whose centre is nearest the day's last
/// close, and at an equal distance the lower one.
///
/// By hand: 100..110 at 100 (10 per bin), plus 30 into bin 2 (102..103) and 30 into bin 7
/// (107..108): bins 2 and 7 both hold 40, centres 102.5 and 107.5. Last close 107 → 107.5; last
/// close 105, 2.5 from both → 102.5.
///
/// Breakage: the first maximum always winning, or the last one.
#[test]
fn a_tied_poc_goes_to_the_close_then_down() {
    let now = MONDAY_2024_01_01_MS + 3.0 * H1;
    let day = |close: f32| {
        vec![
            hb(0, 102.0, 103.0, 102.5, 30.0),
            hb(1, 107.0, 108.0, 107.5, 30.0),
            hb(2, 100.0, 110.0, close, 100.0),
        ]
    };
    let poc_of = |bars: &[ChartCandle]| {
        let p = VolumeProfileParams {
            vah: false,
            val: false,
            ..params(ProfilePeriod::Day, 10, 0)
        };
        tagged(&run(bars, p, now))
    };
    assert_eq!(poc_of(&day(107.0)), vec![(LineTag::Poc, 107.5)]);
    assert_eq!(poc_of(&day(105.0)), vec![(LineTag::Poc, 102.5)]);
}

/// No turnover, no profile; a hole in the day, or a day not covered from its start, no profile.
///
/// Breakage: a profile of zero volume drawn at the range's bottom, or a partial day drawn as if it
/// were the day.
#[test]
fn no_volume_and_holes_give_nothing() {
    let now = MONDAY_2024_01_01_MS + 4.0 * H1;
    let p = params(ProfilePeriod::Day, 10, 0);
    let mut dry = four_bars(1.0);
    for b in &mut dry {
        b.quote_volume = 0.0;
    }
    assert!(run(&dry, p, now).is_empty());

    // The two middle hours missing: two hours of nothing between 01:00 and 03:00.
    let mut holed = four_bars(1.0);
    holed.drain(1..3);
    assert!(run(&holed, p, now).is_empty());

    // The day's first two hours missing: more than an hour of nothing before its first bar. One
    // missing hour is a quiet start, not a hole.
    assert!(run(&four_bars(1.0)[2..], p, now).is_empty());
    assert!(!run(&four_bars(1.0)[1..], p, now).is_empty());
}

/// A complete day of 24 hourly bars, each spanning 100..110 with 1 traded, except hour `extra`,
/// which trades 51 inside 104..105: that bin holds 51 + 23 × 0.1, the POC is its centre, 104.5.
fn full_day(day: i64, extra: i64) -> Vec<ChartCandle> {
    (0..24)
        .map(|h| {
            let qv = if h == extra { 51.0 } else { 1.0 };
            let (lo, hi) = if h == extra {
                (104.0, 105.0)
            } else {
                (100.0, 110.0)
            };
            hb(day * 24 + h, lo, hi, 105.0, qv)
        })
        .collect()
}

/// The previous day's profile needs the day complete to its end, within the hour of nothing a
/// quiet market may leave.
///
/// Breakage: a finished day missing its evening profiled anyway, or a quiet last hour rejected.
#[test]
fn the_previous_day_needs_its_end() {
    let now = MONDAY_2024_01_01_MS + DAY + 2.0 * H1;
    let p = VolumeProfileParams {
        vah: false,
        val: false,
        ..params(ProfilePeriod::PrevDay, 10, 0)
    };
    let bars = full_day(0, 7);
    assert_eq!(tagged(&run(&bars, p, now)), vec![(LineTag::Poc, 104.5)]);
    // The last hour quiet is still a whole day; the last two hours missing are a hole.
    assert_eq!(
        tagged(&run(&bars[..23], p, now)),
        vec![(LineTag::Poc, 104.5)]
    );
    assert!(run(&bars[..22], p, now).is_empty());
}

/// Naked POCs. Monday's POC (104.5) with Tuesday's six hours trading 106..108 above it is naked;
/// one Tuesday bar reaching down to 104 trades at it, so it is not; Tuesday with two of its six
/// hours missing is too holed to say.
///
/// Breakage: "naked" decided from the profiled day's own bars, a touch read from the close instead
/// of the range, or a hole after the day read as "never traded since".
#[test]
fn naked_pocs() {
    let now = MONDAY_2024_01_01_MS + DAY + 6.0 * H1;
    let p = VolumeProfileParams {
        poc: false,
        vah: false,
        val: false,
        ..params(ProfilePeriod::Day, 10, 3)
    };
    let tuesday = |lows: [f32; 6]| {
        lows.iter()
            .enumerate()
            .map(|(h, &lo)| hb(24 + h as i64, lo, 108.0, 107.0, 1.0))
            .collect::<Vec<_>>()
    };
    let mut bars = full_day(0, 7);
    bars.extend(tuesday([106.0; 6]));
    let found = run(&bars, p, now);
    assert_eq!(tagged(&found), vec![(LineTag::NakedPoc, 104.5)]);
    assert_eq!(found[0].first_ms, MONDAY_2024_01_01_MS);

    let mut touched = full_day(0, 7);
    touched.extend(tuesday([106.0, 106.0, 104.0, 106.0, 106.0, 106.0]));
    assert!(run(&touched, p, now).is_empty());

    let mut holed = full_day(0, 7);
    let mut tue = tuesday([106.0; 6]);
    tue.drain(2..4);
    holed.extend(tue);
    assert!(run(&holed, p, now).is_empty());
}

/// The previous day's POC that is also naked is ONE line, tagged naked — not the POC and its naked
/// twin drawn on top of each other.
///
/// Breakage: the naked pass adding a second line for the day the profile already drew.
#[test]
fn a_naked_previous_day_poc_is_drawn_once() {
    let now = MONDAY_2024_01_01_MS + DAY + 6.0 * H1;
    let p = VolumeProfileParams {
        vah: false,
        val: false,
        ..params(ProfilePeriod::PrevDay, 10, 1)
    };
    let mut bars = full_day(0, 7);
    bars.extend((0..6).map(|h| hb(24 + h, 106.0, 108.0, 107.0, 1.0)));
    assert_eq!(
        tagged(&run(&bars, p, now)),
        vec![(LineTag::NakedPoc, 104.5)]
    );
}

/// On the real `chart-ace` 5-minute candles: the previous day's POC is the centre of the most
/// traded of 100 bins as an independent brute-force spread computes it, the value area holds at
/// least 70 % of the day, and VAL ≤ POC ≤ VAH. On a day the cache recorded with holes (day 20674,
/// whose first bar is at 07:15) there is no profile.
///
/// Breakage: the profile built from the wrong day's bars, a value area short of its share, or a
/// partial day profiled.
#[test]
fn bench_profile_matches_a_brute_force_spread() {
    let bars = ace(5);
    let now = 20_680.0 * DAY + 3_600_000.0;
    let upto: Vec<ChartCandle> = bars.iter().filter(|b| b.t_open_ms < now).copied().collect();
    let ctx = DetectCtx {
        tf_ms: 300_000,
        now_ms: now,
    };
    let p = params(ProfilePeriod::PrevDay, 100, 0);
    let found = VolumeProfile.detect(&upto, &DetectorParams::VolumeProfile(p), &ctx);
    let got = tagged(&found);
    assert_eq!(got.len(), 3, "{got:?}");
    let (poc, vah, val) = (got[0].1, got[1].1, got[2].1);
    assert!(val <= poc && poc <= vah);

    // Independent spread: every bin against every bar of day 20679.
    let day: Vec<&ChartCandle> = upto
        .iter()
        .filter(|b| b.t_open_ms >= 20_679.0 * DAY && b.t_open_ms < 20_680.0 * DAY)
        .collect();
    assert_eq!(day.len(), 288, "a complete bench day");
    let lo = day
        .iter()
        .map(|b| f64::from(b.low))
        .fold(f64::MAX, f64::min);
    let hi = day
        .iter()
        .map(|b| f64::from(b.high))
        .fold(f64::MIN, f64::max);
    let w = (hi - lo) / 100.0;
    let mut vols = [0.0f64; 100];
    for b in &day {
        let (l, h, v) = (
            f64::from(b.low),
            f64::from(b.high),
            f64::from(b.quote_volume),
        );
        for (i, slot) in vols.iter_mut().enumerate() {
            let (a, z) = (lo + i as f64 * w, lo + (i + 1) as f64 * w);
            if h > l {
                *slot += v * ((h.min(z) - l.max(a)).max(0.0) / (h - l));
            } else if l >= a && (l < z || i == 99) {
                *slot += v;
            }
        }
    }
    let total: f64 = vols.iter().sum();
    let best = vols.iter().cloned().fold(f64::MIN, f64::max);
    let poc_bin = ((poc - lo) / w).floor() as usize;
    assert!(
        (vols[poc_bin] - best).abs() <= best * 1e-9,
        "POC bin is a maximum"
    );
    let in_va: f64 = vols
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            let c = lo + (*i as f64 + 0.5) * w;
            c > val && c < vah
        })
        .map(|(_, v)| v)
        .sum();
    assert!(in_va >= 0.7 * total * (1.0 - 1e-9), "{in_va} of {total}");

    let now = 20_674.0 * DAY + 80_000_000.0;
    let upto: Vec<ChartCandle> = bars.iter().filter(|b| b.t_open_ms < now).copied().collect();
    let ctx = DetectCtx {
        tf_ms: 300_000,
        now_ms: now,
    };
    let day = params(ProfilePeriod::Day, 100, 0);
    assert!(
        VolumeProfile
            .detect(&upto, &DetectorParams::VolumeProfile(day), &ctx)
            .is_empty()
    );
}
