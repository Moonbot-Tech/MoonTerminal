//! The editor's field table, its kind switch and its preview calculation.

use moon_core::config::ChartTheme;
use moon_core::config::auto_figures::{
    AutoFigurePreset, AutoFigureRow, BrokenPolicy, DetectorKind, DetectorParams, ExtremePeriod,
    PeriodExtremeParams, RowDetector, TfId, TfSource, ToleranceUnit,
};
use moon_core::figures::LineKind;

use super::fields::{NumField, parse_number, unit_of_mut};
use super::preview::{MAX_BARS, compute, dash_pattern, dashes, demo_bars, preview_tf};
use super::{row_title, switch_detector};

/// A fixed Wednesday noon, UTC: well inside a day and a week, so "today" and "this week" both
/// have closed bars.
const NOON: f64 = 1_763_553_600_000.0;

fn row_of(kind: DetectorKind) -> AutoFigureRow {
    AutoFigureRow::new(kind.default_params())
}

/// `kind`'s default row with its touch tolerance in ATR — the same row for a kind without one.
fn atr_row_of(kind: DetectorKind) -> AutoFigureRow {
    let mut row = row_of(kind);
    if let RowDetector::Known(d) = &mut row.detector
        && let Some(unit) = unit_of_mut(d)
    {
        *unit = ToleranceUnit::Atr;
    }
    row
}

/// Which numbers each detector shows is the editor's whole layout rule: a field shown on a kind
/// that ignores it is a control that changes nothing — and a tolerance shown in the unit the row
/// does not read is one too. Mutation: let `BodyBreaks` answer for a swing, a period extreme answer
/// a lookback, or the `%` tolerance answer on a row in ATR.
#[test]
fn each_detector_shows_exactly_its_own_numbers() {
    let shown = |row: AutoFigureRow| {
        NumField::DETECTOR
            .into_iter()
            .filter(|f| f.get(&row).is_some())
            .collect::<Vec<_>>()
    };
    use NumField::*;
    assert_eq!(
        shown(row_of(DetectorKind::Swing)),
        vec![Lookback, PivotLeft, PivotRight, Tolerance, MinTouches]
    );
    assert_eq!(
        shown(atr_row_of(DetectorKind::Swing)),
        vec![Lookback, PivotLeft, PivotRight, ToleranceAtr, MinTouches]
    );
    assert_eq!(
        shown(row_of(DetectorKind::PeriodExtreme)),
        Vec::<NumField>::new()
    );
    assert_eq!(
        shown(row_of(DetectorKind::Trend)),
        vec![
            Lookback, PivotLeft, PivotRight, Tolerance, MinTouches, BodyBreaks, SlopeMin, SlopeMax
        ]
    );
    assert_eq!(
        shown(atr_row_of(DetectorKind::Trend)),
        vec![
            Lookback,
            PivotLeft,
            PivotRight,
            ToleranceAtr,
            MinTouches,
            BodyBreaks,
            SlopeMin,
            SlopeMax
        ]
    );
    assert_eq!(
        shown(row_of(DetectorKind::Squeeze)),
        vec![
            Lookback,
            PivotLeft,
            PivotRight,
            Tolerance,
            MinTouches,
            TrendTouches,
            TrendTolerance,
            Pressed
        ]
    );
    // One unit for both halves of the formation.
    assert_eq!(
        shown(atr_row_of(DetectorKind::Squeeze)),
        vec![
            Lookback,
            PivotLeft,
            PivotRight,
            ToleranceAtr,
            MinTouches,
            TrendTouches,
            TrendToleranceAtr,
            Pressed
        ]
    );
    assert_eq!(
        shown(row_of(DetectorKind::VolumeProfile)),
        vec![ValueArea, NakedPocDays, Bins]
    );
    assert_eq!(
        shown(row_of(DetectorKind::PeriodLevel)),
        Vec::<NumField>::new()
    );
    assert_eq!(shown(row_of(DetectorKind::RoundLevels)), vec![RoundRange]);
}

/// Every field a row shows writes the number it then reads back, for every kind and both
/// tolerance units — the table's `get` and `set` name the same struct field — and a field the row
/// does not show writes nothing. Mutation: point one `set` arm at a neighbour (the squeeze's
/// tolerance into its sloped half's, an ATR share into the `%` field), or let `set` write a
/// tolerance of the unit the row does not read.
#[test]
fn every_shown_field_writes_what_it_reads() {
    for kind in DetectorKind::ALL {
        for row in [row_of(kind), atr_row_of(kind)] {
            for field in NumField::ALL {
                let mut row = row.clone();
                row.style.zone = Some(0.2);
                if field.get(&row).is_none() {
                    let before = row.clone();
                    assert!(!field.set(&mut row, 7.0), "{field:?} wrote into {kind:?}");
                    assert_eq!(row, before, "{field:?} changed {kind:?}");
                    continue;
                }
                assert!(field.set(&mut row, 7.0), "{field:?} on {kind:?}");
                assert_eq!(field.get(&row), Some(7.0), "{field:?} on {kind:?}");
            }
        }
    }
}

/// A number is written as typed and only repaired on the way out: "1" on the way to "150" must not
/// snap to the lookback floor under the reader's fingers. Mutation: sanitize inside `set`.
#[test]
fn a_typed_number_is_kept_until_the_row_is_sanitized() {
    let mut row = row_of(DetectorKind::Swing);
    assert!(NumField::Lookback.set(&mut row, 1.0));
    assert_eq!(NumField::Lookback.get(&row), Some(1.0));
    row.sanitize();
    assert!(NumField::Lookback.get(&row).unwrap() >= 10.0);
}

/// Mutation: drop the comma replacement — a Russian or Ukrainian keyboard types "0,25".
#[test]
fn a_decimal_comma_parses_and_garbage_does_not() {
    assert_eq!(parse_number(" 0,25 ", false), Some(0.25));
    assert_eq!(parse_number("12", true), Some(12.0));
    assert_eq!(parse_number("abc", false), None);
    assert_eq!(parse_number("inf", false), None);
}

/// A broken-bar count exists only under `Keep`. Mutation: answer it under `Remove`.
#[test]
fn the_broken_bar_count_follows_the_policy() {
    let mut row = row_of(DetectorKind::Swing);
    assert!(NumField::BrokenBars.get(&row).is_some());
    if let moon_core::config::auto_figures::RowDetector::Known(DetectorParams::Swing(p)) =
        &mut row.detector
    {
        p.broken = BrokenPolicy::Remove;
    }
    assert_eq!(NumField::BrokenBars.get(&row), None);
}

/// Switching to a day's extremes pulls a daily pin down to a timeframe the detector can answer
/// on, so the dropdown never shows a choice the new kind refuses. Mutation: skip the cap.
#[test]
fn switching_kinds_caps_a_pinned_timeframe() {
    let mut row = row_of(DetectorKind::Swing);
    row.tf = TfSource::Fixed(TfId::D1);
    switch_detector(&mut row, DetectorKind::PeriodExtreme);
    assert_eq!(row.tf, TfSource::Fixed(TfId::H4));
    assert_eq!(
        row.detector.known().map(DetectorParams::kind),
        Some(DetectorKind::PeriodExtreme)
    );
}

/// The user's name wins over the preset's, which wins over the detector's.
#[test]
fn a_row_is_titled_by_its_own_name_first() {
    // The dictionary's first lookup needs a deep stack; warm it on a thread that has one.
    crate::test_locale::warm();
    let mut row = AutoFigurePreset::Levels4h.row();
    assert_eq!(
        row_title(&row),
        rust_i18n::t!("auto_figures.preset.levels_4h")
    );
    row.name = "Mine".to_string();
    assert_eq!(row_title(&row), "Mine");
    let plain = row_of(DetectorKind::Trend);
    assert_eq!(row_title(&plain), rust_i18n::t!("auto_figures.kind.trend"));
}

/// The demo series is a pure function of its arguments and ends on the last CLOSED bar — a
/// preview that shifted between two renders would flicker. Mutation: seed the jitter from a clock.
#[test]
fn the_demo_series_is_deterministic_and_closed() {
    let a = demo_bars(TfId::M15, 200, NOON);
    let b = demo_bars(TfId::M15, 200, NOON);
    assert_eq!(a, b);
    let last = a.last().unwrap();
    assert!(last.t_open_ms + TfId::M15.ms() as f64 <= NOON);
    assert!(
        a.iter()
            .all(|c| c.low <= c.open.min(c.close) && c.high >= c.open.max(c.close))
    );
}

/// The demo candles are built so that every shipped detector finds something: a preview that
/// draws nothing for the default rows would show the reader nothing to judge — the volume, period
/// and round-price presets included. Mutation: flatten
/// the demo series, drop the take selection, or take the turnover off the demo bars (the volume
/// presets then have no profile).
#[test]
fn every_default_detector_finds_something_on_the_demo_candles() {
    let theme = ChartTheme::default();
    for preset in AutoFigurePreset::ALL {
        let mut row = preset.row();
        row.sanitize();
        let data = compute(&row, NOON, &theme).expect("a known detector");
        assert!(data.found > 0, "{preset:?} found nothing");
        assert!(data.kept > 0 && data.kept <= data.found, "{preset:?}");
        assert!(!data.figures.is_empty(), "{preset:?} drew nothing");
    }
}

/// A week's extremes on five-minute demo bars would need thousands of them; the preview steps up
/// until the period fits. Mutation: use the row's own timeframe for a period extreme.
#[test]
fn a_long_period_previews_on_a_timeframe_that_fits_it() {
    let mut row = AutoFigureRow::new(DetectorParams::PeriodExtreme(PeriodExtremeParams {
        period: ExtremePeriod::PrevWeek,
        ..PeriodExtremeParams::default()
    }));
    row.tf = TfSource::Fixed(TfId::M5);
    let params = *row.detector.known().unwrap();
    let tf = preview_tf(&row, &params);
    assert!(tf > TfId::M5);
    assert!(params.bars_needed(tf) as usize <= MAX_BARS);
}

/// A dashed segment is mostly ink and partly gap, a solid one is one piece. Mutation: return the
/// whole segment for every pattern, or swap on and off.
#[test]
fn dashes_cover_the_on_fraction_of_the_segment() {
    assert_eq!(dashes((0.0, 0.0), (100.0, 0.0), &[]).len(), 1);
    let pattern = dash_pattern(LineKind::Dash, 1.0);
    let parts = dashes((0.0, 0.0), (100.0, 0.0), &pattern);
    let ink: f32 = parts.iter().map(|(a, b)| b.0 - a.0).sum();
    // 6 on, 4 off: 60 % ink, give or take the cut at the end.
    assert!((55.0..=65.0).contains(&ink), "ink {ink}");
    assert!(parts.len() >= 9);
}

/// Breakage: "1,000" in a bar count read as a decimal comma — one bar, then silently clamped.
#[test]
fn thousands_separators_group_in_a_whole_number() {
    for typed in ["1,000", "1 000", "1\u{a0}000", "1_000", " 1,000 "] {
        assert_eq!(parse_number(typed, true), Some(1000.0), "{typed:?}");
    }
}

/// Breakage: every comma in a fraction treated alike — either "0,25" stops being a quarter, or
/// "1,000.5" reads as one.
#[test]
fn a_comma_is_decimal_only_when_it_is_the_only_separator() {
    assert_eq!(parse_number("0,25", false), Some(0.25));
    assert_eq!(parse_number("1,000.5", false), Some(1000.5));
    assert_eq!(parse_number("1,000,000", false), Some(1_000_000.0));
    assert_eq!(parse_number("1 000,5", false), Some(1000.5));
}
