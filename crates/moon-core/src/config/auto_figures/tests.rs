use super::*;

use crate::config::chart_defaults::ChartTabKind;
use crate::config::chart_labels::LabelColor;
use crate::config::layout::WindowLayout;
use crate::figures::LineKind;

/// The chart sections of a real `layout.toml`, copied verbatim from a profile written before auto
/// figures existed (candle view, graphics, captions and all three per-kind default tables). Not
/// produced by today's writer, which would contain the new key and prove nothing.
const OLD_LAYOUT: &str = include_str!("fixtures/layout_before_auto_figures.toml");

/// The kinds that are not Main.
const OTHER_KINDS: [ChartTabKind; 3] = [
    ChartTabKind::AddTo,
    ChartTabKind::Compare,
    ChartTabKind::Trade,
];

/// A document holding one configuration, for the tests that read and write the value alone.
#[derive(serde::Serialize, serde::Deserialize)]
struct Doc {
    #[serde(default)]
    auto_figures: AutoFiguresCfg,
}

/// Write `cfg` as a TOML document and read it back.
fn round_trip(cfg: &AutoFiguresCfg) -> (String, AutoFiguresCfg) {
    let text = toml::to_string(&Doc {
        auto_figures: cfg.clone(),
    })
    .expect("a configuration always writes");
    let back: Doc = toml::from_str(&text).expect("what was written reads back");
    (text, back.auto_figures)
}

/// The `auto_figures` table of a written document, as plain TOML.
fn written_table(text: &str) -> toml::Table {
    let doc: toml::Table = toml::from_str(text).expect("valid TOML");
    doc.get("auto_figures")
        .and_then(toml::Value::as_table)
        .cloned()
        .unwrap_or_default()
}

/// Assert `cfg` is the shipped four-row set, spelled out here rather than rebuilt from `presets`
/// so a changed preset list fails this test instead of agreeing with itself.
fn assert_shipped_rows(cfg: &AutoFiguresCfg) {
    let presets: Vec<_> = cfg.rows.iter().map(|r| r.preset).collect();
    assert_eq!(
        presets,
        vec![
            Some(AutoFigurePreset::Levels1h),
            Some(AutoFigurePreset::TrendLines1h),
            Some(AutoFigurePreset::Squeeze),
            Some(AutoFigurePreset::Extremes24h),
        ]
    );
    let shape: Vec<_> = cfg
        .rows
        .iter()
        .map(|r| (r.detector.known().map(DetectorParams::kind), r.tf))
        .collect();
    assert_eq!(
        shape,
        vec![
            (Some(DetectorKind::Swing), TfSource::Fixed(TfId::H1)),
            (Some(DetectorKind::Trend), TfSource::Fixed(TfId::H1)),
            (Some(DetectorKind::Squeeze), TfSource::Fixed(TfId::M15)),
            (Some(DetectorKind::PeriodExtreme), TfSource::Fixed(TfId::M5)),
        ]
    );
    assert!(cfg.rows.iter().all(|r| r.visible));
}

/// A profile from before this feature opens with auto figures ON, with the four shipped rows, on
/// Main — and switched OFF everywhere else — while everything the file did state still loads.
///
/// Breaks if the field loses its default, if the default stops being the ON preset, or if a
/// non-Main kind stops shipping its own disabled set and falls through to Main's.
#[test]
fn an_old_layout_without_the_key_opens_with_the_shipped_set_on_main_only() {
    let layout: WindowLayout = toml::from_str(OLD_LAYOUT).expect("the real old file loads");
    // The file was read, not defaulted: its own per-kind captions are there.
    assert!(layout.chart_defaults_addto.chart_labels.is_some());
    assert!(layout.chart_defaults_compare.chart_labels.is_some());

    let main = layout.auto_figures_for(ChartTabKind::Main);
    assert!(main.enabled, "a new display feature ships ON");
    assert_shipped_rows(main);
    for kind in OTHER_KINDS {
        let cfg = layout.auto_figures_for(kind);
        assert!(
            !cfg.enabled,
            "{kind:?} draws auto figures only once switched on"
        );
        assert_shipped_rows(cfg);
        assert_eq!(
            layout.stored_auto_figures(kind),
            None,
            "{kind:?} stores nothing yet"
        );
    }
}

/// The shipped set is a DEFAULT, not a value: writing an untouched profile states nothing about
/// it, so a later build that improves the presets reaches every reader who never edited them.
#[test]
fn an_untouched_configuration_writes_nothing_and_reads_back_equal() {
    let shipped = AutoFiguresCfg::default();
    let (text, back) = round_trip(&shipped);
    assert_eq!(back, shipped);
    assert!(
        written_table(&text).is_empty(),
        "the shipped set was frozen into the file:\n{text}"
    );

    // The whole layout too: one profile save must not freeze it either.
    let text = toml::to_string(&WindowLayout::default()).expect("the layout writes");
    assert!(written_table(&text).is_empty(), "{text}");
}

/// What the user DID store comes back exactly, and the file states only that.
#[test]
fn a_stored_value_survives_a_save_and_load_untouched() {
    let off = AutoFiguresCfg {
        opacity: AF_OPACITY_DEFAULT,
        enabled: false,
        ..AutoFiguresCfg::default()
    };
    let (text, back) = round_trip(&off);
    assert_eq!(back, off);
    assert_eq!(
        written_table(&text),
        toml::from_str::<toml::Table>("enabled = false").unwrap()
    );

    // Through the real layout file as well.
    let mut layout = WindowLayout::default();
    layout.auto_figures = off.clone();
    let text = toml::to_string(&layout).expect("the layout writes");
    let back: WindowLayout = toml::from_str(&text).expect("and reads back");
    assert_eq!(back.auto_figures, off);

    // An emptied table is the reader's choice, not an absent setting.
    let empty = AutoFiguresCfg {
        opacity: AF_OPACITY_DEFAULT,
        enabled: true,
        rows: Vec::new(),
    };
    assert_eq!(round_trip(&empty).1, empty);
}

/// An edited row comes back equal, and states only the fields that were changed.
///
/// Built on the 4h levels preset, whose detector differs from the swing default only in the
/// lookback this test overwrites — so every detector key written is one the edit made.
#[test]
fn an_edited_row_writes_only_what_changed() {
    let mut row = AutoFigurePreset::Levels4h.row();
    row.name = "Мои уровни".into();
    row.take.count = 4;
    row.style.thickness = 3.0;
    row.style.color = LabelColor::Fixed(0x123456);
    if let RowDetector::Known(DetectorParams::Swing(p)) = &mut row.detector {
        p.lookback_bars = 400;
    }
    let cfg = AutoFiguresCfg {
        opacity: AF_OPACITY_DEFAULT,
        enabled: true,
        rows: vec![row],
    };
    let (text, back) = round_trip(&cfg);
    assert_eq!(back, cfg);

    let table = written_table(&text);
    let rows = table["rows"].as_array().expect("a list of rows");
    let row = rows[0].as_table().expect("a row table");
    let mut keys: Vec<&str> = row.keys().map(String::as_str).collect();
    keys.sort_unstable();
    // `caption`: a preset row shows its captions on hover, which a file states — absent reads as
    // the type's `Always`.
    assert_eq!(
        keys,
        [
            "caption", "detector", "name", "preset", "style", "take", "tf"
        ]
    );
    let detector = row["detector"].as_table().unwrap();
    assert_eq!(
        detector,
        &toml::from_str::<toml::Table>("kind = \"swing\"\nlookback_bars = 400").unwrap()
    );
    let mut style: Vec<&str> = row["style"]
        .as_table()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    style.sort_unstable();
    assert_eq!(style, ["color", "thickness"]);
}

/// A detector a newer build wrote is kept and written back as it was, every parameter included,
/// and the row it sits in is listed but never drawn.
///
/// The kind is an invented one (`density_level`, the planned order-book levels): this test used
/// `volume_profile` until that kind became known, and a kind this build runs is read, not kept.
#[test]
fn an_unknown_detector_kind_round_trips_verbatim() {
    let detector = "kind = \"density_level\"\nbins = 48\nedge_pct = 0.75\n\
                    sides = [\"buy\", \"sell\"]\nnested = { depth = 3 }\n";
    let file = format!(
        "[[auto_figures.rows]]\nname = \"From a newer build\"\n\
         [auto_figures.rows.detector]\n{detector}"
    );
    let doc: Doc = toml::from_str(&file).expect("a newer build's file loads");
    let row = &doc.auto_figures.rows[0];
    let RowDetector::Unknown(unknown) = &row.detector else {
        panic!(
            "an unknown kind must not be read as a known one: {:?}",
            row.detector
        );
    };
    assert_eq!(unknown.kind(), "density_level");
    assert!(!row.is_drawn());
    assert!(!doc.auto_figures.shows(None));

    let (text, back) = round_trip(&doc.auto_figures);
    assert_eq!(back, doc.auto_figures);
    let written = written_table(&text)["rows"][0]["detector"].clone();
    assert_eq!(
        written,
        toml::Value::Table(toml::from_str(detector).unwrap()),
        "the detector changed on the way through:\n{text}"
    );
    // And it settles: the second write is the first one, byte for byte.
    assert_eq!(round_trip(&back).0, text);
}

/// A hand-broken row costs only what is broken in it, and an unusable table costs only the auto
/// figures — never the layout around them.
#[test]
fn a_broken_row_costs_only_itself() {
    let file = "chart_book_width_migrated = true\n\
        [auto_figures]\n\
        rows = [\n\
          \"not a row\",\n\
          { detector = { kind = \"swing\", lookback_bars = \"many\", pivot_left = 5 } },\n\
          { tf = { mode = \"fixed\", tf = \"h2\" }, detector = { kind = \"trend\" } },\n\
          { detector = { lookback_bars = 10 } },\n\
          { name = \"kept\", take = { count = \"two\" }, caption = { parts = [\"price\", \"wat\", \"touches\"] } },\n\
        ]\n";
    let layout: WindowLayout = toml::from_str(file).expect("the layout survives");
    assert!(layout.chart_book_width_migrated);
    let rows = &layout.auto_figures.rows;
    assert_eq!(
        rows.len(),
        3,
        "a string and a kind-less detector are dropped: {rows:#?}"
    );

    // The unreadable field fell back alone; its readable neighbour was kept.
    let Some(DetectorParams::Swing(swing)) = rows[0].detector.known() else {
        panic!("{:?}", rows[0].detector);
    };
    assert_eq!(swing.pivot_left, 5);
    assert_eq!(swing.lookback_bars, 200);

    // A timeframe this build has no name for reads as Auto, and the row stays.
    assert_eq!(rows[1].tf, TfSource::Auto);
    assert_eq!(
        rows[1].detector.known().map(DetectorParams::kind),
        Some(DetectorKind::Trend)
    );

    // A broken table and an unknown caption part cost themselves only.
    assert_eq!(rows[2].name, "kept");
    assert_eq!(rows[2].take.count, 2);
    assert_eq!(
        rows[2].caption.parts,
        [CaptionPart::Price, CaptionPart::Touches]
    );

    let layout: WindowLayout =
        toml::from_str("chart_book_width_migrated = true\nauto_figures = \"garbage\"\n")
            .expect("the layout survives");
    assert!(layout.chart_book_width_migrated);
    assert!(layout.auto_figures.enabled);
    assert_shipped_rows(&layout.auto_figures);
}

/// Saving can never fail on a value that `sanitize` has not seen yet: TOML has no null, and both a
/// NaN parameter and a null inside a newer build's detector would otherwise become one.
#[test]
fn an_unsanitized_value_still_saves() {
    let mut row = AutoFigurePreset::Squeeze.row();
    if let RowDetector::Known(DetectorParams::Squeeze(p)) = &mut row.detector {
        p.pressed_pct = f32::NAN;
    }
    let from_json: AutoFigureRow = serde_json::from_str(
        r#"{"detector": {"kind": "fib_cluster", "levels": [0.5, null], "anchor": null}}"#,
    )
    .expect("a JSON copy of a row reads");
    let cfg = AutoFiguresCfg {
        opacity: AF_OPACITY_DEFAULT,
        enabled: true,
        rows: vec![row, from_json],
    };
    let (_, back) = round_trip(&cfg);
    let Some(DetectorParams::Squeeze(p)) = back.rows[0].detector.known() else {
        panic!("{:?}", back.rows[0].detector);
    };
    assert_eq!(p.pressed_pct, SqueezeParams::default().pressed_pct);
    let RowDetector::Unknown(unknown) = &back.rows[1].detector else {
        panic!("{:?}", back.rows[1].detector);
    };
    assert_eq!(unknown.kind(), "fib_cluster");
}

/// A configuration stuffed with every impossible value `sanitize` knows about.
fn hostile() -> AutoFiguresCfg {
    let mut swing = AutoFigureRow::new(DetectorParams::Swing(SwingParams {
        lookback_bars: 0,
        pivot_left: 0,
        pivot_right: 255,
        tolerance_unit: ToleranceUnit::Atr,
        tolerance_pct: f32::NAN,
        tolerance_atr: f32::NAN,
        min_touches: 0,
        broken: BrokenPolicy::Keep { bars: u16::MAX },
    }));
    swing.name = format!("  {}  ", "x".repeat(200));
    swing.take.count = 0;
    swing.style.thickness = f32::NAN;
    swing.style.zone = Some(5.0);
    swing.events.approach_pct = -1.0;
    swing.events.stop_hunt_pct = f32::NAN;
    swing.events.confluence_pct = 1e9;
    swing.caption.parts = vec![
        CaptionPart::Price,
        CaptionPart::Tf,
        CaptionPart::Price,
        CaptionPart::Tf,
    ];

    let mut trend = AutoFigureRow::new(DetectorParams::Trend(TrendParams {
        min_touches: 1,
        min_slope_pct_per_bar: 5.0,
        max_slope_pct_per_bar: -1.0,
        tolerance_pct: f32::INFINITY,
        tolerance_atr: 1e9,
        ..TrendParams::default()
    }));
    trend.style.thickness = -4.0;
    trend.style.zone = Some(f32::NAN);
    trend.events.approach_pct = f32::NAN;
    trend.events.stop_hunt_pct = -1.0;
    trend.events.confluence_pct = f32::INFINITY;

    let squeeze = AutoFigureRow::new(DetectorParams::Squeeze(SqueezeParams {
        min_trend_touches: 0,
        trend_tolerance_pct: -3.0,
        trend_tolerance_atr: -1.0,
        pressed_pct: 1e9,
        ..SqueezeParams::default()
    }));

    let profile = AutoFigureRow::new(DetectorParams::VolumeProfile(VolumeProfileParams {
        value_area_pct: f32::NAN,
        naked_poc_days: u8::MAX,
        bins: 1,
        ..VolumeProfileParams::default()
    }));
    let mut profile_wide = AutoFigureRow::new(DetectorParams::VolumeProfile(VolumeProfileParams {
        value_area_pct: 400.0,
        bins: u16::MAX,
        ..VolumeProfileParams::default()
    }));
    // A profile cannot answer on four-hour bars; sanitize pulls the pin down to H1.
    profile_wide.tf = TfSource::Fixed(TfId::H4);
    let round = AutoFigureRow::new(DetectorParams::RoundLevels(RoundLevelsParams {
        grid: RoundGrid::Minor,
        range_pct: f32::NEG_INFINITY,
    }));
    let round_wide = AutoFigureRow::new(DetectorParams::RoundLevels(RoundLevelsParams {
        grid: RoundGrid::Major,
        range_pct: 1e6,
    }));
    let mut day_open = AutoFigureRow::new(DetectorParams::PeriodLevel(PeriodLevelParams {
        day_open: true,
        ..PeriodLevelParams::default()
    }));
    day_open.tf = TfSource::Fixed(TfId::D1);

    let formation = AutoFigureRow::new(DetectorParams::Formation(FormationParams {
        lookback_bars: 40,
        pivot_left: 0,
        pivot_right: 99,
        tolerance_pct: f32::NAN,
        tolerance_atr: -2.0,
        min_touches: 1,
        min_bars: 0,
        parallel_width_pct: f32::INFINITY,
        impulse_pct: f32::NAN,
        impulse_bars: 0,
        flag_max_bars: u32::MAX,
        double_tolerance_pct: 1e9,
        double_tolerance_atr: f32::NAN,
        double_depth_pct: -1.0,
        broken: BrokenPolicy::Keep { bars: u16::MAX },
        ..FormationParams::default()
    }));

    let mut rows = vec![
        swing,
        trend,
        squeeze,
        profile,
        profile_wide,
        round,
        round_wide,
        day_open,
        formation,
    ];
    rows.extend(std::iter::repeat_n(AutoFigurePreset::Levels1h.row(), 100));
    AutoFiguresCfg {
        opacity: AF_OPACITY_DEFAULT,
        enabled: true,
        rows,
    }
}

/// `sanitize` lands on values it then leaves alone, NaN included — otherwise the settings
/// signature differs from itself and every notification reads as a change.
#[test]
fn sanitize_is_idempotent() {
    let mut presets = AutoFiguresCfg {
        opacity: AF_OPACITY_DEFAULT,
        enabled: true,
        rows: AutoFigurePreset::ALL.map(AutoFigurePreset::row).to_vec(),
    };
    let before = presets.clone();
    presets.sanitize();
    assert_eq!(presets, before, "a preset must already be a repaired value");

    let mut once = hostile();
    once.sanitize();
    let mut twice = once.clone();
    twice.sanitize();
    assert_eq!(twice, once);
}

/// What `sanitize` repairs each impossible value TO.
#[test]
fn sanitize_repairs_impossible_values() {
    let mut cfg = hostile();
    cfg.sanitize();
    assert_eq!(cfg.rows.len(), AF_ROWS_MAX);

    let swing_row = &cfg.rows[0];
    assert_eq!(swing_row.name.chars().count(), 48);
    assert!(!swing_row.name.starts_with(' '));
    assert_eq!(swing_row.take.count, 1);
    assert_eq!(swing_row.style.thickness, 1.0);
    assert_eq!(swing_row.style.zone, Some(1.0));
    assert_eq!(swing_row.events.approach_pct, 0.0);
    assert_eq!(
        swing_row.caption.parts,
        [CaptionPart::Price, CaptionPart::Tf]
    );
    let Some(DetectorParams::Swing(swing)) = swing_row.detector.known() else {
        panic!();
    };
    assert_eq!((swing.pivot_left, swing.pivot_right), (1, 20));
    // At least one pivot of these widths must fit: 1 + 1 + 20 bars.
    assert_eq!(swing.lookback_bars, 22);
    assert_eq!(swing.tolerance_pct, 0.25);
    assert_eq!(swing.min_touches, 1);
    assert_eq!(swing.broken, BrokenPolicy::Keep { bars: 1_000 });

    let trend_row = &cfg.rows[1];
    assert_eq!(trend_row.style.thickness, 0.5);
    assert_eq!(trend_row.style.zone, None);
    assert_eq!(trend_row.events.approach_pct, 0.3);
    let Some(DetectorParams::Trend(trend)) = trend_row.detector.known() else {
        panic!();
    };
    assert_eq!(trend.min_touches, 2);
    assert_eq!(trend.tolerance_pct, 0.15);
    assert_eq!(
        (trend.min_slope_pct_per_bar, trend.max_slope_pct_per_bar),
        (1.0, 5.0)
    );

    let Some(DetectorParams::Squeeze(squeeze)) = cfg.rows[2].detector.known() else {
        panic!();
    };
    assert_eq!(squeeze.min_trend_touches, 2);
    assert_eq!(squeeze.trend_tolerance_pct, 0.01);
    assert_eq!(squeeze.trend_tolerance_atr, AF_TOLERANCE_ATR_MIN);
    assert_eq!(squeeze.pressed_pct, 10.0);
}

/// What `sanitize` repairs each impossible value of the fields added on 19.11.2026 TO: the ATR
/// shares, the new event distances, and the volume-profile and round-price parameters — NaN to
/// the default, the rest into the documented bounds — and the timeframe pins the new kinds cannot
/// answer on.
///
/// Breakage: a new field left out of the repair (a NaN tolerance reaching a detector), a NaN
/// clamped instead of replaced (`f32::clamp` passes it through), or a bound swapped.
#[test]
fn sanitize_repairs_the_new_fields() {
    let mut cfg = hostile();
    cfg.sanitize();

    let swing_row = &cfg.rows[0];
    assert_eq!(swing_row.events.stop_hunt_pct, 0.3);
    assert_eq!(swing_row.events.confluence_pct, AF_PCT_MAX);
    let Some(DetectorParams::Swing(swing)) = swing_row.detector.known() else {
        panic!();
    };
    assert_eq!(swing.tolerance_unit, ToleranceUnit::Atr);
    assert_eq!(swing.tolerance_atr, 0.5);

    let trend_row = &cfg.rows[1];
    assert_eq!(trend_row.events.stop_hunt_pct, AF_TOLERANCE_MIN);
    assert_eq!(trend_row.events.confluence_pct, 0.3);
    let Some(DetectorParams::Trend(trend)) = trend_row.detector.known() else {
        panic!();
    };
    assert_eq!(trend.tolerance_atr, AF_TOLERANCE_ATR_MAX);

    let Some(DetectorParams::VolumeProfile(narrow)) = cfg.rows[3].detector.known() else {
        panic!("{:?}", cfg.rows[3].detector);
    };
    assert_eq!(narrow.value_area_pct, 70.0);
    assert_eq!(narrow.naked_poc_days, AF_NAKED_POC_DAYS_MAX);
    assert_eq!(narrow.bins, AF_PROFILE_BINS_MIN);
    let Some(DetectorParams::VolumeProfile(wide)) = cfg.rows[4].detector.known() else {
        panic!("{:?}", cfg.rows[4].detector);
    };
    assert_eq!(wide.value_area_pct, AF_VALUE_AREA_MAX);
    assert_eq!(wide.bins, AF_PROFILE_BINS_MAX);
    assert_eq!(cfg.rows[4].tf, TfSource::Fixed(TfId::H1));

    let Some(DetectorParams::RoundLevels(round)) = cfg.rows[5].detector.known() else {
        panic!("{:?}", cfg.rows[5].detector);
    };
    assert_eq!(round.range_pct, 10.0);
    assert_eq!(round.grid, RoundGrid::Minor);
    let Some(DetectorParams::RoundLevels(round)) = cfg.rows[6].detector.known() else {
        panic!("{:?}", cfg.rows[6].detector);
    };
    assert_eq!(round.range_pct, AF_ROUND_RANGE_MAX);

    // A day's open is never a closed daily bar: the pin comes down to H4.
    assert_eq!(cfg.rows[7].tf, TfSource::Fixed(TfId::H4));
}

/// A row as the previous build wrote it — `take` and `events` whole, a swing detector without a
/// tolerance unit — loads with every field added since at its default, keeps what it stated, and
/// is written back stating only what differs.
///
/// Spelled out by hand rather than produced by today's writer, which already knows the new fields
/// and would prove nothing. Breakage: a new field without a default (the old row fails to load or
/// is dropped), or the take and event tables written whole again, freezing today's defaults of
/// the new fields into every edited row.
#[test]
fn a_row_written_before_the_new_fields_loads_with_their_defaults() {
    let file = r#"
[[auto_figures.rows]]
name = "old"

[auto_figures.rows.take]
count = 3
side = "above"
order = "strongest"

[auto_figures.rows.events]
approach = true
squeeze = true
breakout = false
false_break = true
bounce = true
approach_pct = 0.5

[auto_figures.rows.detector]
kind = "swing"
tolerance_pct = 0.375
"#;
    let doc: Doc = toml::from_str(file).expect("an old row loads");
    let row = &doc.auto_figures.rows[0];
    assert_eq!(row.name, "old");
    assert_eq!(
        row.take,
        Take {
            count: 3,
            side: TakeSide::Above,
            order: TakeOrder::Strongest,
            strength: StrengthBasis::Touches,
        }
    );
    assert!(!row.events.breakout);
    assert_eq!(row.events.approach_pct, 0.5);
    assert!(row.events.role_flip && row.events.stop_hunt && row.events.confluence);
    assert_eq!(row.events.stop_hunt_pct, 0.3);
    assert_eq!(row.events.confluence_pct, 0.3);
    let Some(DetectorParams::Swing(swing)) = row.detector.known() else {
        panic!("{:?}", row.detector);
    };
    assert_eq!(swing.tolerance_pct, 0.375);
    assert_eq!(swing.tolerance_unit, ToleranceUnit::Pct);
    assert_eq!(swing.tolerance_atr, 0.5);

    let (text, back) = round_trip(&doc.auto_figures);
    assert_eq!(back, doc.auto_figures);
    let written = &written_table(&text)["rows"][0];
    let table = |s: &str| toml::Value::Table(toml::from_str(s).unwrap());
    assert_eq!(
        written["take"],
        table(
            r#"count = 3
side = "above"
order = "strongest""#
        )
    );
    assert_eq!(
        written["events"],
        table("breakout = false\napproach_pct = 0.5")
    );
    assert_eq!(
        written["detector"],
        table(
            r#"kind = "swing"
tolerance_pct = 0.375"#
        )
    );
}

/// A value of a take or event field this build cannot read costs that field alone — the reason
/// "strongest by volume" is a field and not a third order. Breakage: `take` read as one struct
/// again, where a newer build's `strength` would reset the row's count and side too.
#[test]
fn an_unreadable_take_or_event_field_costs_only_itself() {
    let file = r#"
[[auto_figures.rows]]
take = { count = 4, side = "below", strength = "liquidity" }
events = { stop_hunt = false, confluence_pct = "near", approach_pct = 0.75 }
detector = { kind = "swing" }
"#;
    let doc: Doc = toml::from_str(file).expect("loads");
    let row = &doc.auto_figures.rows[0];
    assert_eq!(row.take.count, 4);
    assert_eq!(row.take.side, TakeSide::Below);
    assert_eq!(row.take.strength, StrengthBasis::Touches);
    assert!(!row.events.stop_hunt);
    assert_eq!(row.events.confluence_pct, 0.3);
    assert_eq!(row.events.approach_pct, 0.75);
}

/// Every new detector kind, and every new option of the old ones, survives a save and a load —
/// and each table states only what differs from its defaults.
///
/// Breakage: a kind missing from `DetectorParams` (the row would be kept as unknown and never
/// drawn), a renamed serde tag (an existing profile would lose its rows' kind), or a new field
/// that does not round-trip. The values are exact in `f32`, so the written numbers compare
/// exactly.
#[test]
fn the_new_kinds_and_options_round_trip() {
    let mut swing_atr = AutoFigureRow::new(DetectorParams::Swing(SwingParams {
        tolerance_unit: ToleranceUnit::Atr,
        tolerance_atr: 0.75,
        ..SwingParams::default()
    }));
    swing_atr.take.order = TakeOrder::Strongest;
    swing_atr.take.strength = StrengthBasis::Volume;
    swing_atr.events.role_flip = false;
    swing_atr.events.stop_hunt_pct = 0.625;
    swing_atr.events.confluence_pct = 0.125;
    let mut squeeze = SqueezeParams::default();
    squeeze.level.tolerance_unit = ToleranceUnit::Atr;
    squeeze.trend_tolerance_atr = 0.25;
    let profile = DetectorParams::VolumeProfile(VolumeProfileParams {
        period: ProfilePeriod::PrevDay,
        vah: false,
        value_area_pct: 68.0,
        naked_poc_days: 3,
        bins: 150,
        ..VolumeProfileParams::default()
    });
    let level = DetectorParams::PeriodLevel(PeriodLevelParams {
        prev_day_close: false,
        day_open: true,
        week_open: true,
        prev_week_close: true,
    });
    let round = DetectorParams::RoundLevels(RoundLevelsParams {
        grid: RoundGrid::Major,
        range_pct: 4.0,
    });
    let cfg = AutoFiguresCfg {
        opacity: AF_OPACITY_DEFAULT,
        enabled: true,
        rows: vec![
            swing_atr,
            AutoFigureRow::new(DetectorParams::Squeeze(squeeze)),
            AutoFigureRow::new(profile),
            AutoFigureRow::new(level),
            AutoFigureRow::new(round),
            AutoFigureRow::new(DetectorKind::RoundLevels.default_params()),
        ],
    };
    let mut sanitized = cfg.clone();
    sanitized.sanitize();
    assert_eq!(sanitized, cfg, "the test values are already in range");

    let (text, back) = round_trip(&cfg);
    assert_eq!(back, cfg);
    let rows = written_table(&text)["rows"].clone();
    let detector = |ix: usize| rows[ix]["detector"].clone();
    let table = |s: &str| toml::Value::Table(toml::from_str(s).unwrap());
    assert_eq!(
        detector(0),
        table(
            r#"kind = "swing"
tolerance_unit = "atr"
tolerance_atr = 0.75"#
        )
    );
    assert_eq!(
        detector(2),
        table(
            r#"kind = "volume_profile"
period = "prev_day"
vah = false
value_area_pct = 68.0
naked_poc_days = 3
bins = 150"#
        )
    );
    assert_eq!(
        detector(3),
        table(
            r#"kind = "period_level"
prev_day_close = false
day_open = true
prev_week_close = true"#
        )
    );
    assert_eq!(
        detector(4),
        table(
            r#"kind = "round_levels"
grid = "major"
range_pct = 4.0"#
        )
    );
    // A kind at its defaults states nothing but itself.
    assert_eq!(detector(5), table(r#"kind = "round_levels""#));
    assert_eq!(
        rows[0]["take"],
        table(
            r#"order = "strongest"
strength = "volume""#
        )
    );
    assert_eq!(
        rows[0]["events"],
        table("role_flip = false\nstop_hunt_pct = 0.625\nconfluence_pct = 0.125")
    );
}

/// A new kind's table with one unreadable field is repaired field by field like an old kind's:
/// the bad field takes its default, its readable neighbours stay.
///
/// Breakage: the new kinds missing from `DetectorKind`'s serde names, which would send the table
/// to "unknown" and keep it undrawn instead of repairing it.
#[test]
fn a_new_kind_with_a_broken_field_is_repaired_not_dropped() {
    let file = r#"
[[auto_figures.rows]]
detector = { kind = "volume_profile", bins = "many", naked_poc_days = 3 }
[[auto_figures.rows]]
detector = { kind = "round_levels", grid = "tiny", range_pct = 5.0 }
"#;
    let doc: Doc = toml::from_str(file).expect("loads");
    let Some(DetectorParams::VolumeProfile(p)) = doc.auto_figures.rows[0].detector.known() else {
        panic!("{:?}", doc.auto_figures.rows[0].detector);
    };
    assert_eq!(p.bins, 100);
    assert_eq!(p.naked_poc_days, 3);
    let Some(DetectorParams::RoundLevels(r)) = doc.auto_figures.rows[1].detector.known() else {
        panic!("{:?}", doc.auto_figures.rows[1].detector);
    };
    assert_eq!(r.grid, RoundGrid::Half);
    assert_eq!(r.range_pct, 5.0);
}

/// None of the presets added on 19.11.2026 joins the shipped set (LinKvo's four rows), and each
/// runs the detector its name promises.
///
/// Breakage: a new preset slipped into `DEFAULT_SET`, which would switch it on for every profile
/// that never edited its rows; a naked-POC preset that also draws the day's own profile.
#[test]
fn the_new_presets_are_offered_not_shipped() {
    let new = [
        (AutoFigurePreset::Profile24h, DetectorKind::VolumeProfile),
        (AutoFigurePreset::PeriodLevels, DetectorKind::PeriodLevel),
        (AutoFigurePreset::RoundLevels, DetectorKind::RoundLevels),
    ];
    for (preset, kind) in new {
        assert!(AutoFigurePreset::ALL.contains(&preset), "{preset:?}");
        assert!(
            !AutoFigurePreset::DEFAULT_SET.contains(&preset),
            "{preset:?}"
        );
        let row = preset.row();
        assert_eq!(row.detector.known().map(DetectorParams::kind), Some(kind));
    }
    let Some(DetectorParams::VolumeProfile(naked)) =
        AutoFigurePreset::NakedPoc.row().detector.known().copied()
    else {
        panic!();
    };
    assert!(naked.naked_poc_days > 0 && !naked.poc && !naked.vah && !naked.val);
}

/// The rows that need days of history are offered and never shipped, and the two that need more
/// than the cache holds for most markets are trimmed (LinKvo, 19.11.2026): the naked POCs leave the
/// "Add" menu, and the period levels keep yesterday's close with the week's open off.
///
/// Breakage: `NakedPoc` back in `ALL`, or the period-level preset reaching back a week again.
#[test]
fn the_day_deep_presets_stay_out_of_the_shipped_set() {
    assert!(!AutoFigurePreset::ALL.contains(&AutoFigurePreset::NakedPoc));
    for preset in [AutoFigurePreset::Levels4h, AutoFigurePreset::Profile24h] {
        assert!(AutoFigurePreset::ALL.contains(&preset), "{preset:?}");
        assert!(
            !AutoFigurePreset::DEFAULT_SET.contains(&preset),
            "{preset:?}"
        );
    }
    let Some(DetectorParams::PeriodLevel(p)) = AutoFigurePreset::PeriodLevels
        .row()
        .detector
        .known()
        .copied()
    else {
        panic!();
    };
    assert!(p.prev_day_close && !p.week_open && !p.day_open && !p.prev_week_close);
}

/// Every row of the shipped set needs no more history than one core ring holds: 500 sealed
/// 5-minute candles (moonproto `candles_5m`), less the partial first bar of the row's timeframe
/// that the bar assembly drops. The ring is the one history the terminal has for every market; a
/// row deeper than it reports "not enough history" almost everywhere.
///
/// Breakage: a shipped row back on the generic 200-bar lookback (50 h at 15 minutes, 200 h at an
/// hour), or a day-deep preset slipped into the set.
#[test]
fn the_shipped_set_fits_one_core_ring() {
    let ring_ms: i64 = 500 * 5 * 60_000;
    for preset in AutoFigurePreset::DEFAULT_SET {
        let row = preset.row();
        let TfSource::Fixed(tf) = row.tf else {
            panic!("{preset:?}: a shipped row names its timeframe");
        };
        let needed = row
            .detector
            .known()
            .expect("a preset runs a known detector")
            .bars_needed(tf);
        let span = i64::from(needed) * tf.ms();
        assert!(
            span <= ring_ms - tf.ms(),
            "{preset:?} needs {needed} bars of {tf:?}, {} h, past the ring",
            span / 3_600_000
        );
    }
}

/// A row created from the naked-POC preset before it left the "Add" menu still loads, with its
/// preset — the name it is shown under — and its parameters, and writes back the same.
///
/// Breakage: the variant deleted or renamed (the row would lose its name), or the lenient preset
/// read turned strict (the row would be dropped).
#[test]
fn a_stored_naked_poc_row_still_loads() {
    let file = r#"
[[auto_figures.rows]]
preset = "naked_poc"
detector = { kind = "volume_profile", poc = false, vah = false, val = false, naked_poc_days = 5 }
"#;
    let doc: Doc = toml::from_str(file).expect("loads");
    let rows = &doc.auto_figures.rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].preset, Some(AutoFigurePreset::NakedPoc));
    assert_eq!(rows[0].title_key(), Some("auto_figures.preset.naked_poc"));
    let Some(DetectorParams::VolumeProfile(p)) = rows[0].detector.known() else {
        panic!("{:?}", rows[0].detector);
    };
    assert_eq!(p.naked_poc_days, 5);
    let (_, back) = round_trip(&doc.auto_figures);
    assert_eq!(back, doc.auto_figures);
}

/// The 4h levels are told apart from the 1h ones at a glance: another colour and a heavier line.
#[test]
fn the_two_level_scales_never_look_alike() {
    let h1 = AutoFigurePreset::Levels1h.row().style;
    let h4 = AutoFigurePreset::Levels4h.row().style;
    assert_ne!(h1.color, h4.color);
    assert!(h4.thickness > h1.thickness);
    // Colours are references, never a literal the theme cannot reach: the 1h line follows it.
    assert_eq!(h1.color, LabelColor::Theme);
    assert_eq!(h1.line_kind, LineKind::Solid);
}

/// Setting Main's default leaves every other kind on its OWN shipped set — nothing is frozen on a
/// copy — and a kind's stored value is told apart from one it merely follows.
#[test]
fn per_kind_storage_never_freezes_a_follower() {
    let mut layout = WindowLayout::default();
    let off = AutoFiguresCfg {
        opacity: AF_OPACITY_DEFAULT,
        enabled: false,
        rows: Vec::new(),
    };
    assert!(layout.set_auto_figures_default(ChartTabKind::Main, off.clone()));
    assert_eq!(layout.auto_figures_for(ChartTabKind::Main), &off);
    for kind in OTHER_KINDS {
        assert_eq!(
            layout.stored_auto_figures(kind),
            None,
            "{kind:?} was frozen"
        );
        assert_shipped_rows(layout.auto_figures_for(kind));
    }

    let on = AutoFiguresCfg::shipped();
    assert!(layout.set_auto_figures_default(ChartTabKind::AddTo, on.clone()));
    assert_eq!(layout.stored_auto_figures(ChartTabKind::AddTo), Some(&on));
    assert!(layout.auto_figures_for(ChartTabKind::AddTo).enabled);
    assert_eq!(layout.auto_figures_for(ChartTabKind::Main), &off);
    assert_eq!(layout.stored_auto_figures(ChartTabKind::Compare), None);
    // Storing the same value again moves nothing.
    assert!(!layout.set_auto_figures_default(ChartTabKind::AddTo, on));

    // The stored value travels through the file, and the follower stays a follower.
    let text = toml::to_string(&layout).expect("the layout writes");
    let back: WindowLayout = toml::from_str(&text).expect("and reads back");
    assert!(back.auto_figures_for(ChartTabKind::AddTo).enabled);
    assert_eq!(back.stored_auto_figures(ChartTabKind::Trade), None);
    assert_eq!(back.auto_figures_for(ChartTabKind::Main), &off);

    // A reset empties the slot, which hands the kind its own built-in set back.
    assert!(layout.reset_auto_figures_default(ChartTabKind::AddTo));
    assert_eq!(layout.stored_auto_figures(ChartTabKind::AddTo), None);
    assert!(!layout.auto_figures_for(ChartTabKind::AddTo).enabled);
}

/// Every kind answers a broken policy: its own for the pivot detectors (`Squeeze` through its
/// level), the default for a period extreme.
///
/// Breakage: the trend or squeeze arm returning the default, so a row's `Remove` never reaches the
/// event layer's false-break window.
#[test]
fn broken_policy_answers_for_every_kind() {
    let remove = BrokenPolicy::Remove;
    let swing = DetectorParams::Swing(SwingParams {
        broken: remove,
        ..SwingParams::default()
    });
    let trend = DetectorParams::Trend(TrendParams {
        broken: remove,
        ..TrendParams::default()
    });
    let mut squeeze_params = SqueezeParams::default();
    squeeze_params.level.broken = remove;
    let squeeze = DetectorParams::Squeeze(squeeze_params);
    for params in [swing, trend, squeeze] {
        assert_eq!(params.broken_policy(), remove, "{:?}", params.kind());
    }
    assert_eq!(
        DetectorKind::PeriodExtreme.default_params().broken_policy(),
        BrokenPolicy::Keep { bars: 3 }
    );
}

/// A trend row written before `broken` existed loads with the default policy, and `sanitize`
/// bounds a trend's `Keep` like a level's.
///
/// Breakage: the new field without a default (every old trend row fails to load), or the trend
/// policy left out of the repair.
#[test]
fn trend_broken_policy_loads_and_is_sanitized() {
    let old: DetectorParams = toml::from_str(
        "kind = \"trend\"
lookback_bars = 120
",
    )
    .expect("an old trend row");
    let DetectorParams::Trend(t) = old else {
        panic!("a trend row");
    };
    assert_eq!(t.lookback_bars, 120);
    assert_eq!(t.broken, BrokenPolicy::Keep { bars: 3 });

    let mut row = AutoFigureRow::new(DetectorParams::Trend(TrendParams {
        broken: BrokenPolicy::Keep { bars: u16::MAX },
        ..TrendParams::default()
    }));
    row.sanitize();
    assert_eq!(
        row.detector.known().map(DetectorParams::broken_policy),
        Some(BrokenPolicy::Keep {
            bars: AF_BROKEN_KEEP_MAX
        })
    );
}

/// The bars a row needs: the lookback for the pivot detectors, and for a period extreme the whole
/// period from its start up to the latest reference time plus one bar, rounded up.
///
/// Breakage: a period extreme asking for its lookback (it has none) and the caller fetching zero
/// bars; a floor instead of a ceiling coming one bar short on a timeframe that does not divide the
/// day.
#[test]
fn bars_needed_per_kind() {
    let extreme = |period| {
        DetectorParams::PeriodExtreme(PeriodExtremeParams {
            period,
            which: ExtremeWhich::Both,
        })
    };
    // 288 five-minute bars in a day, 24 hourly, 1 daily; 6 four-hour bars a day.
    assert_eq!(extreme(ExtremePeriod::Day).bars_needed(TfId::M5), 289);
    assert_eq!(extreme(ExtremePeriod::PrevDay).bars_needed(TfId::M5), 577);
    assert_eq!(extreme(ExtremePeriod::Week).bars_needed(TfId::H1), 169);
    assert_eq!(extreme(ExtremePeriod::PrevWeek).bars_needed(TfId::H4), 85);
    assert_eq!(extreme(ExtremePeriod::Day).bars_needed(TfId::D1), 2);

    let swing = DetectorParams::Swing(SwingParams {
        lookback_bars: 321,
        ..SwingParams::default()
    });
    assert_eq!(swing.bars_needed(TfId::H1), 321);
    let mut squeeze = SqueezeParams::default();
    squeeze.level.lookback_bars = 654;
    assert_eq!(DetectorParams::Squeeze(squeeze).bars_needed(TfId::M15), 654);
    let trend = DetectorParams::Trend(TrendParams {
        lookback_bars: 77,
        ..TrendParams::default()
    });
    assert_eq!(trend.bars_needed(TfId::M1), 77);
}

/// The bars the kinds added on 19.11.2026 need, and the ATR's floor under a short lookback.
///
/// Breakage: an ATR row fetching fewer than `ATR_PERIOD + 1` bars (the ATR never forms and the row
/// draws nothing for ever); the naked-POC days left out of a profile's count; a period level sized
/// by its first flag instead of its longest.
#[test]
fn bars_needed_for_the_new_kinds() {
    let short = |unit| SwingParams {
        lookback_bars: 10,
        tolerance_unit: unit,
        ..SwingParams::default()
    };
    assert_eq!(
        DetectorParams::Swing(short(ToleranceUnit::Pct)).bars_needed(TfId::H1),
        10
    );
    assert_eq!(
        DetectorParams::Swing(short(ToleranceUnit::Atr)).bars_needed(TfId::H1),
        ATR_PERIOD + 1
    );
    let squeeze = SqueezeParams {
        level: short(ToleranceUnit::Atr),
        ..SqueezeParams::default()
    };
    assert_eq!(
        DetectorParams::Squeeze(squeeze).bars_needed(TfId::M5),
        ATR_PERIOD + 1
    );
    let trend = DetectorParams::Trend(TrendParams {
        lookback_bars: 12,
        tolerance_unit: ToleranceUnit::Atr,
        ..TrendParams::default()
    });
    assert_eq!(trend.bars_needed(TfId::M5), ATR_PERIOD + 1);
    // A long lookback is not cut down to the ATR's.
    let long = DetectorParams::Swing(SwingParams {
        lookback_bars: 300,
        tolerance_unit: ToleranceUnit::Atr,
        ..SwingParams::default()
    });
    assert_eq!(long.bars_needed(TfId::H1), 300);

    let profile = |period, naked_poc_days| {
        DetectorParams::VolumeProfile(VolumeProfileParams {
            period,
            naked_poc_days,
            ..VolumeProfileParams::default()
        })
    };
    // 288 five-minute bars a day, 24 hourly.
    assert_eq!(profile(ProfilePeriod::Day, 0).bars_needed(TfId::M5), 289);
    assert_eq!(
        profile(ProfilePeriod::PrevDay, 0).bars_needed(TfId::M5),
        577
    );
    assert_eq!(profile(ProfilePeriod::Day, 5).bars_needed(TfId::H1), 145);

    let level = |prev_day_close, day_open, week_open, prev_week_close| {
        DetectorParams::PeriodLevel(PeriodLevelParams {
            prev_day_close,
            day_open,
            week_open,
            prev_week_close,
        })
    };
    assert_eq!(level(false, true, false, false).bars_needed(TfId::H1), 25);
    assert_eq!(level(true, true, false, false).bars_needed(TfId::H1), 49);
    assert_eq!(level(true, false, true, false).bars_needed(TfId::H4), 43);
    assert_eq!(level(false, false, false, true).bars_needed(TfId::D1), 15);
    assert_eq!(level(false, false, false, false).bars_needed(TfId::M5), 1);

    assert_eq!(
        DetectorKind::RoundLevels
            .default_params()
            .bars_needed(TfId::M1),
        1
    );
}

/// The new kinds have no break policy of their own and answer the default, and each caps the
/// timeframe it can answer on: a profile at H1, a day's open at H4, the rest not at all.
///
/// Breakage: a profile allowed on D1 (one bar a day is no profile), or a day-open row on D1, whose
/// current day never closes.
#[test]
fn the_new_kinds_policy_and_coarsest_timeframe() {
    for kind in [
        DetectorKind::VolumeProfile,
        DetectorKind::PeriodLevel,
        DetectorKind::RoundLevels,
    ] {
        assert_eq!(
            kind.default_params().broken_policy(),
            BrokenPolicy::Keep { bars: 3 }
        );
    }
    assert_eq!(
        DetectorKind::VolumeProfile.default_params().coarsest_tf(),
        TfId::H1
    );
    assert_eq!(
        DetectorKind::PeriodLevel.default_params().coarsest_tf(),
        TfId::D1
    );
    let day_open = DetectorParams::PeriodLevel(PeriodLevelParams {
        day_open: true,
        ..PeriodLevelParams::default()
    });
    assert_eq!(day_open.coarsest_tf(), TfId::H4);
    assert_eq!(
        DetectorKind::RoundLevels.default_params().coarsest_tf(),
        TfId::D1
    );
}

/// `marks` answers each event from its own switch. Breakage: a new event wired to a neighbour's
/// switch, so hiding "stop hunt" hides "role flip" instead.
#[test]
fn every_event_reads_its_own_switch() {
    let all = [
        AutoEvent::Approach,
        AutoEvent::Squeeze,
        AutoEvent::Breakout,
        AutoEvent::FalseBreak,
        AutoEvent::Bounce,
        AutoEvent::RoleFlip,
        AutoEvent::StopHunt,
        AutoEvent::Confluence,
    ];
    for off in all {
        let mut marks = EventMarks::default();
        let switch = match off {
            AutoEvent::Approach => &mut marks.approach,
            AutoEvent::Squeeze => &mut marks.squeeze,
            AutoEvent::Breakout => &mut marks.breakout,
            AutoEvent::FalseBreak => &mut marks.false_break,
            AutoEvent::Bounce => &mut marks.bounce,
            AutoEvent::RoleFlip => &mut marks.role_flip,
            AutoEvent::StopHunt => &mut marks.stop_hunt,
            AutoEvent::Confluence => &mut marks.confluence,
        };
        *switch = false;
        for event in all {
            assert_eq!(marks.marks(event), event != off, "{off:?} off, {event:?}");
        }
    }
}

/// A `Day` extreme on D1 reads only closed bars and so could never see the forming day; sanitize
/// pulls such a fixed timeframe down to H4, and leaves a `PrevDay` row on D1 alone — its day is
/// closed.
#[test]
fn a_timeframe_the_detector_cannot_answer_on_is_pulled_down() {
    let mut day = AutoFigureRow::new(DetectorParams::PeriodExtreme(PeriodExtremeParams {
        period: ExtremePeriod::Day,
        which: ExtremeWhich::Both,
    }));
    day.tf = TfSource::Fixed(TfId::D1);
    day.sanitize();
    assert_eq!(day.tf, TfSource::Fixed(TfId::H4));

    let mut prev = AutoFigureRow::new(DetectorParams::PeriodExtreme(PeriodExtremeParams {
        period: ExtremePeriod::PrevDay,
        which: ExtremeWhich::Both,
    }));
    prev.tf = TfSource::Fixed(TfId::D1);
    prev.sanitize();
    assert_eq!(prev.tf, TfSource::Fixed(TfId::D1));
}

/// The "Levels 4h" preset — offered in the "Add" menu, no longer in the shipped set — is built from
/// the 5-minute cache, so its lookback (plus the partial first bucket the bar assembly drops) must
/// fit what that cache keeps. At the generic 200 bars (33 days) against a 15-day cache, a row added
/// from it would report "not enough history" for ever.
#[test]
fn the_offered_4h_levels_fit_the_candle_cache() {
    let row = AutoFigurePreset::Levels4h.row();
    let needed = row
        .detector
        .known()
        .expect("a preset runs a known detector")
        .bars_needed(TfId::H4);
    let cache_4h_bars = crate::market::kline_cache::retention_days(5) * 24 / 4;
    assert!(
        i64::from(needed) < cache_4h_bars,
        "4h row needs {needed} bars, the 5-minute cache holds {cache_4h_bars}"
    );
}

/// What `sanitize` repairs each impossible formation value TO: NaN to the default, the rest into
/// the documented bounds, and the span bounds into the lookback the row searches.
///
/// By hand, from `hostile`'s formation row: the pivot widths clamp to 1 and 20, so the 40-bar
/// lookback stays (it holds 1 + 1 + 20 bars); `min_bars` 0 rises to the 3-bar floor and
/// `flag_max_bars` falls to the lookback, 40; one touch a side rises to two.
///
/// Breakage: a NaN clamped instead of replaced, a span bound left above the lookback (a row that
/// can never find a flag), or the formation left out of the repair altogether.
#[test]
fn sanitize_repairs_the_formation_fields() {
    let mut cfg = hostile();
    cfg.sanitize();
    let Some(DetectorParams::Formation(f)) = cfg.rows[8].detector.known() else {
        panic!("{:?}", cfg.rows[8].detector);
    };
    let d = FormationParams::default();
    assert_eq!((f.pivot_left, f.pivot_right), (1, 20));
    assert_eq!(f.lookback_bars, 40);
    assert_eq!(f.tolerance_pct, d.tolerance_pct);
    assert_eq!(f.tolerance_atr, AF_TOLERANCE_ATR_MIN);
    assert_eq!(f.min_touches, 2);
    assert_eq!(f.min_bars, AF_FORMATION_BARS_MIN);
    // Infinite is not finite: the default, like a NaN.
    assert_eq!(f.parallel_width_pct, d.parallel_width_pct);
    assert_eq!(f.impulse_pct, d.impulse_pct);
    assert_eq!(f.impulse_bars, 1);
    assert_eq!(f.flag_max_bars, 40);
    assert_eq!(f.double_tolerance_pct, AF_PCT_MAX);
    assert_eq!(f.double_tolerance_atr, d.double_tolerance_atr);
    assert_eq!(f.double_depth_pct, AF_TOLERANCE_MIN);
    assert_eq!(
        f.broken,
        BrokenPolicy::Keep {
            bars: AF_BROKEN_KEEP_MAX
        }
    );
    // The kind switches are choices, not damage: all on stays all on.
    assert!(f.triangle && f.wedge && f.channel && f.flag && f.double);
}

/// A formation row writes only what differs from its kind's defaults and reads back equal; a
/// formation table from a build that knew fewer of its fields loads with the rest at their
/// defaults, and one unreadable field costs only itself.
///
/// Breakage: a renamed serde tag (`formation`) or field, a field without a default (an older
/// formation row fails to load), or the field-by-field repair skipping the new kind.
#[test]
fn a_formation_row_round_trips_and_loads_partially() {
    let mut row = AutoFigureRow::new(DetectorParams::Formation(FormationParams {
        wedge: false,
        double: false,
        tolerance_unit: ToleranceUnit::Atr,
        tolerance_atr: 0.75,
        impulse_pct: 4.5,
        flag_max_bars: 24,
        broken: BrokenPolicy::Remove,
        ..FormationParams::default()
    }));
    row.preset = Some(AutoFigurePreset::Triangles);
    let cfg = AutoFiguresCfg {
        opacity: AF_OPACITY_DEFAULT,
        enabled: true,
        rows: vec![
            row,
            AutoFigureRow::new(DetectorKind::Formation.default_params()),
        ],
    };
    let mut sanitized = cfg.clone();
    sanitized.sanitize();
    assert_eq!(sanitized, cfg, "the test values are already in range");

    let (text, back) = round_trip(&cfg);
    assert_eq!(back, cfg);
    let rows = written_table(&text)["rows"].clone();
    let table = |s: &str| toml::Value::Table(toml::from_str(s).unwrap());
    assert_eq!(
        rows[0]["detector"],
        table(
            r#"kind = "formation"
wedge = false
double = false
tolerance_unit = "atr"
tolerance_atr = 0.75
impulse_pct = 4.5
flag_max_bars = 24
broken = { mode = "remove" }"#
        )
    );
    assert_eq!(rows[0]["preset"].as_str(), Some("triangles"));
    assert_eq!(rows[1]["detector"], table(r#"kind = "formation""#));

    let file = r#"
[[auto_figures.rows]]
detector = { kind = "formation", channel = false, lookback_bars = 90 }
[[auto_figures.rows]]
detector = { kind = "formation", min_touches = "three", double_depth_pct = 2.5 }
"#;
    let doc: Doc = toml::from_str(file).expect("loads");
    let Some(DetectorParams::Formation(older)) = doc.auto_figures.rows[0].detector.known() else {
        panic!("{:?}", doc.auto_figures.rows[0].detector);
    };
    assert_eq!(
        *older,
        FormationParams {
            channel: false,
            lookback_bars: 90,
            ..FormationParams::default()
        }
    );
    let Some(DetectorParams::Formation(typo)) = doc.auto_figures.rows[1].detector.known() else {
        panic!("{:?}", doc.auto_figures.rows[1].detector);
    };
    assert_eq!(typo.min_touches, FormationParams::default().min_touches);
    assert_eq!(typo.double_depth_pct, 2.5);
}

/// The formation presets are offered in the "Add" menu, never shipped, and each switches on the
/// kinds its name promises and no other.
///
/// Breakage: a formation preset slipped into `DEFAULT_SET` (it would switch on for every profile
/// that never edited its rows), or a preset that also finds what another one is for.
#[test]
fn the_formation_presets_are_offered_not_shipped() {
    let expect = [
        (
            AutoFigurePreset::Triangles,
            [true, true, false, false, false],
        ),
        (
            AutoFigurePreset::Channels,
            [false, false, true, false, false],
        ),
        (AutoFigurePreset::Flags, [false, false, false, true, false]),
        (
            AutoFigurePreset::DoubleTops,
            [false, false, false, false, true],
        ),
    ];
    for (preset, kinds) in expect {
        assert!(AutoFigurePreset::ALL.contains(&preset), "{preset:?}");
        assert!(
            !AutoFigurePreset::DEFAULT_SET.contains(&preset),
            "{preset:?}"
        );
        let Some(DetectorParams::Formation(p)) = preset.row().detector.known().copied() else {
            panic!("{preset:?}");
        };
        assert_eq!(
            [p.triangle, p.wedge, p.channel, p.flag, p.double],
            kinds,
            "{preset:?}"
        );
        assert!(preset.locale_key().starts_with("auto_figures.preset."));
    }
}

/// The formation kind's policy, coarsest timeframe and bar need: its own `broken`, any timeframe,
/// and its lookback raised to the ATR's `ATR_PERIOD + 1` bars under an ATR tolerance.
///
/// Breakage: the policy arm returning the default (a row's `Remove` never reaching the event
/// layer), or the ATR need forgotten (a short lookback fetches too few bars for its ATR).
#[test]
fn the_formation_policy_timeframe_and_bars() {
    let remove = DetectorParams::Formation(FormationParams {
        broken: BrokenPolicy::Remove,
        ..FormationParams::default()
    });
    assert_eq!(remove.broken_policy(), BrokenPolicy::Remove);
    assert_eq!(remove.coarsest_tf(), TfId::D1);
    assert_eq!(remove.kind(), DetectorKind::Formation);
    assert_eq!(DetectorKind::ALL.last(), Some(&DetectorKind::Formation));

    let short = |unit| {
        DetectorParams::Formation(FormationParams {
            lookback_bars: 10,
            tolerance_unit: unit,
            ..FormationParams::default()
        })
    };
    assert_eq!(short(ToleranceUnit::Pct).bars_needed(TfId::M5), 10);
    assert_eq!(
        short(ToleranceUnit::Atr).bars_needed(TfId::M5),
        ATR_PERIOD + 1
    );
    assert_eq!(
        DetectorKind::Formation
            .default_params()
            .bars_needed(TfId::H1),
        150
    );
}

/// The block's opacity: absent from a file is the default (40 %), the default is never written,
/// a stored value comes back exactly, and the repair lands NaN on the default and the rest inside
/// the bounds — idempotently.
///
/// Breakage: the field losing its serde default (an old profile fails or reads 0 — the lines
/// vanish), the default frozen into every saved file, or a NaN kept (the settings signature then
/// differs from itself on every notification).
#[test]
fn the_block_opacity_defaults_round_trips_and_is_repaired() {
    let old: Doc = toml::from_str("[auto_figures]\nenabled = true\n").expect("an old table loads");
    assert_eq!(old.auto_figures.opacity, 0.4);
    let (text, _) = round_trip(&AutoFiguresCfg::default());
    assert!(!text.contains("opacity"), "{text}");

    let stored = AutoFiguresCfg {
        opacity: 0.25,
        ..AutoFiguresCfg::default()
    };
    let (text, back) = round_trip(&stored);
    assert_eq!(back.opacity, 0.25);
    assert_eq!(
        written_table(&text),
        toml::from_str::<toml::Table>("opacity = 0.25").unwrap()
    );

    for (raw, repaired) in [
        (f32::NAN, 0.4),
        (f32::INFINITY, 0.4),
        (0.0, 0.1),
        (-1.0, 0.1),
        (7.0, 1.0),
        (0.45, 0.45),
    ] {
        let mut cfg = AutoFiguresCfg {
            opacity: raw,
            ..AutoFiguresCfg::default()
        };
        cfg.sanitize();
        assert_eq!(cfg.opacity, repaired, "{raw}");
        let once = cfg.clone();
        cfg.sanitize();
        assert_eq!(cfg, once, "{raw}: the repair is idempotent");
    }
    // A hand-edited value out of bounds is repaired on read.
    let hand: Doc = toml::from_str("[auto_figures]\nopacity = 5.0\n").expect("loads");
    assert_eq!(hand.auto_figures.opacity, 1.0);
}

/// Every preset prints its captions on hover, and a row stored before that — its caption absent
/// from the file — keeps printing it always. The stored row is a real "Levels 1h" row: its preset
/// in the file is the variant's serde name (`levels1h`, not the locale key's `levels_1h`), and it is
/// recognised as that preset — the case the rule is about, not an unnamed row.
///
/// Breakage: a preset left on `Always`, or `LineCaption`'s own default moved to `OnHover`, which
/// would silently switch every stored row the reader never touched.
#[test]
fn presets_caption_on_hover_and_stored_rows_keep_theirs() {
    for preset in AutoFigurePreset::ALL {
        assert_eq!(
            preset.row().caption.show,
            CaptionShow::OnHover,
            "{preset:?}"
        );
    }
    let file = r#"
[[auto_figures.rows]]
preset = "levels1h"
detector = { kind = "swing" }
"#;
    let doc: Doc = toml::from_str(file).expect("loads");
    assert_eq!(
        doc.auto_figures.rows[0].preset,
        Some(AutoFigurePreset::Levels1h)
    );
    assert_eq!(doc.auto_figures.rows[0].caption.show, CaptionShow::Always);
}

/// A preset row is named by what it reads NOW: the profile and extremes presets by their period —
/// 24 hours by default, and whatever the reader switched it to; the hour-named levels and trend
/// rows by their timeframe; a row switched to another kind by its detector (no preset name). Rows
/// saved before 20.11.2026 keep their stored period and are named by it: the yesterday profile
/// stored `period = "prev_day"`, and a day-extremes row stored nothing (the calendar day is the
/// kind's default).
///
/// Breakage: the name read from the preset alone (a "yesterday" caption over today's lines, LinKvo
/// 20.11.2026), a preset back on a calendar day, a wire name changed so old rows lose their preset,
/// or an old day-extremes row silently turned into a 24-hour one.
#[test]
fn a_preset_row_is_named_by_what_it_reads_now() {
    let mut profile = AutoFigurePreset::Profile24h.row();
    let Some(DetectorParams::VolumeProfile(p)) = profile.detector.known() else {
        panic!("{:?}", profile.detector);
    };
    assert_eq!(p.period, ProfilePeriod::Last24h);
    assert_eq!(
        profile.title_key(),
        Some("auto_figures.preset.profile_last24h")
    );
    if let RowDetector::Known(DetectorParams::VolumeProfile(p)) = &mut profile.detector {
        p.period = ProfilePeriod::Day;
    }
    assert_eq!(profile.title_key(), Some("auto_figures.preset.day_profile"));

    let mut extremes = AutoFigurePreset::Extremes24h.row();
    assert_eq!(
        extremes.title_key(),
        Some("auto_figures.preset.extremes_last24h")
    );
    if let RowDetector::Known(DetectorParams::PeriodExtreme(p)) = &mut extremes.detector {
        p.period = ExtremePeriod::Week;
    }
    assert_eq!(
        extremes.title_key(),
        Some("auto_figures.preset.week_extremes")
    );
    extremes.detector = RowDetector::Known(DetectorParams::Swing(SwingParams::default()));
    assert_eq!(extremes.title_key(), None);

    let mut levels = AutoFigurePreset::Levels1h.row();
    assert_eq!(levels.title_key(), Some("auto_figures.preset.levels_1h"));
    levels.tf = TfSource::Fixed(TfId::H4);
    assert_eq!(levels.title_key(), Some("auto_figures.preset.levels_4h"));
    levels.tf = TfSource::Auto;
    assert_eq!(levels.title_key(), Some("auto_figures.kind.swing"));

    let file = r#"
[[auto_figures.rows]]
preset = "prev_day_profile"
detector = { kind = "volume_profile", period = "prev_day" }

[[auto_figures.rows]]
preset = "day_extremes"
tf = { mode = "fixed", tf = "m5" }
detector = { kind = "period_extreme" }
"#;
    let doc: Doc = toml::from_str(file).expect("loads");
    let rows = &doc.auto_figures.rows;
    assert_eq!(rows[0].preset, Some(AutoFigurePreset::Profile24h));
    assert_eq!(
        rows[0].title_key(),
        Some("auto_figures.preset.prev_day_profile")
    );
    assert_eq!(rows[1].preset, Some(AutoFigurePreset::Extremes24h));
    let Some(DetectorParams::PeriodExtreme(p)) = rows[1].detector.known() else {
        panic!("{:?}", rows[1].detector);
    };
    assert_eq!(p.period, ExtremePeriod::Day);
    assert_eq!(
        rows[1].title_key(),
        Some("auto_figures.preset.day_extremes")
    );
}

/// The rolling windows' needs and caps, by hand: a window of `h` hours on bars of `tf` is
/// `h·60/tf + 1` bars (24 h of M5: 288 + 1; 48 h of H1: 48 + 1; 4 h of M15: 16 + 1), a rolling
/// profile the same, and a rolling profile with naked POCs at least the naked days plus today
/// (24 h with three naked days: four days of H1, 96 + 1). A rolling extreme stops at H1 — M15 for
/// four hours — however long the window: its start falls between bar boundaries and coverage needs
/// a bar within an hour of it.
///
/// Breakage: hours read as days (a 24-hour window asking for 24 days), the naked POCs ignored on a
/// rolling profile, or the 48-hour window let onto H4 (uncovered three times in four).
#[test]
fn rolling_windows_need_their_hours_and_stop_at_an_hour() {
    let extreme = |period| {
        DetectorParams::PeriodExtreme(PeriodExtremeParams {
            period,
            which: ExtremeWhich::Both,
        })
    };
    let profile = |period, naked_poc_days| {
        DetectorParams::VolumeProfile(VolumeProfileParams {
            period,
            naked_poc_days,
            ..VolumeProfileParams::default()
        })
    };
    assert_eq!(extreme(ExtremePeriod::Last24h).bars_needed(TfId::M5), 289);
    assert_eq!(extreme(ExtremePeriod::Last48h).bars_needed(TfId::H1), 49);
    assert_eq!(extreme(ExtremePeriod::Last4h).bars_needed(TfId::M15), 17);
    assert_eq!(
        profile(ProfilePeriod::Last12h, 0).bars_needed(TfId::M5),
        145
    );
    assert_eq!(profile(ProfilePeriod::Last24h, 3).bars_needed(TfId::H1), 97);

    assert_eq!(extreme(ExtremePeriod::Last4h).coarsest_tf(), TfId::M15);
    for period in [
        ExtremePeriod::Last12h,
        ExtremePeriod::Last24h,
        ExtremePeriod::Last48h,
    ] {
        assert_eq!(extreme(period).coarsest_tf(), TfId::H1, "{period:?}");
    }
}
