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
            Some(AutoFigurePreset::Levels4h),
            Some(AutoFigurePreset::Squeeze),
            Some(AutoFigurePreset::DayExtremes),
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
            (Some(DetectorKind::Swing), TfSource::Fixed(TfId::H4)),
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
        enabled: true,
        rows: Vec::new(),
    };
    assert_eq!(round_trip(&empty).1, empty);
}

/// An edited row comes back equal, and states only the fields that were changed.
#[test]
fn an_edited_row_writes_only_what_changed() {
    let mut row = AutoFigurePreset::Levels1h.row();
    row.name = "Мои уровни".into();
    row.take.count = 4;
    row.style.thickness = 3.0;
    row.style.color = LabelColor::Fixed(0x123456);
    if let RowDetector::Known(DetectorParams::Swing(p)) = &mut row.detector {
        p.lookback_bars = 400;
    }
    let cfg = AutoFiguresCfg {
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
    assert_eq!(keys, ["detector", "name", "preset", "style", "take", "tf"]);
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
#[test]
fn an_unknown_detector_kind_round_trips_verbatim() {
    let detector = "kind = \"volume_profile\"\nbins = 48\nedge_pct = 0.75\n\
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
    assert_eq!(unknown.kind(), "volume_profile");
    assert!(!row.is_drawn());
    assert!(!doc.auto_figures.is_active());

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
        tolerance_pct: f32::NAN,
        min_touches: 0,
        broken: BrokenPolicy::Keep { bars: u16::MAX },
    }));
    swing.name = format!("  {}  ", "x".repeat(200));
    swing.take.count = 0;
    swing.style.thickness = f32::NAN;
    swing.style.zone = Some(5.0);
    swing.events.approach_pct = -1.0;
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
        ..TrendParams::default()
    }));
    trend.style.thickness = -4.0;
    trend.style.zone = Some(f32::NAN);
    trend.events.approach_pct = f32::NAN;

    let squeeze = AutoFigureRow::new(DetectorParams::Squeeze(SqueezeParams {
        min_trend_touches: 0,
        trend_tolerance_pct: -3.0,
        pressed_pct: 1e9,
        ..SqueezeParams::default()
    }));

    let mut rows = vec![swing, trend, squeeze];
    rows.extend(std::iter::repeat_n(AutoFigurePreset::Levels1h.row(), 100));
    AutoFiguresCfg {
        enabled: true,
        rows,
    }
}

/// `sanitize` lands on values it then leaves alone, NaN included — otherwise the settings
/// signature differs from itself and every notification reads as a change.
#[test]
fn sanitize_is_idempotent() {
    let mut presets = AutoFiguresCfg {
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
    assert_eq!(squeeze.pressed_pct, 10.0);
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
