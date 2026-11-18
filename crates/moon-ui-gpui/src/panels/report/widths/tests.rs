//! Regression coverage for Report natural-width purpose bounds.

use super::{natural_widths_environment, width_bounds};
use crate::design::MonoBodyFontSignature;
use gpui::FontId;

/// Synthetic full Report page with text, quote money, and timestamp formatting.
fn page() -> (Vec<String>, Vec<Vec<rusqlite::types::Value>>) {
    use rusqlite::types::Value;
    let cols = [
        "coin",
        "core_name",
        "comment",
        "profitbtc",
        "profitpct",
        "spentbtc",
        "gainedbtc",
        "buydate",
        "closedate",
        "sellreason",
        "basecurrency",
        "strategyid",
        "isshort",
        "lev",
        "emulator",
    ]
    .map(str::to_string)
    .to_vec();
    let rows = (0..500)
        .map(|i| {
            vec![
                Value::Text(format!("XALPHA{i}")),
                Value::Text("CORE_A".into()),
                Value::Text(format!("UNIT_01 synthetic trade {i}")),
                Value::Real(1.23456789),
                Value::Real(3.25),
                Value::Real(123.456),
                Value::Real(124.69),
                Value::Integer(1_700_000_000 + i),
                Value::Integer(1_700_000_060 + i),
                Value::Text("Sell".into()),
                Value::Integer(0),
                Value::Integer(1),
                Value::Integer(0),
                Value::Integer(10),
                Value::Integer(0),
            ]
        })
        .collect();
    (cols, rows)
}

/// Measure the publication-triggered full-page width refresh before and after incremental caching.
#[gpui::test]
#[ignore = "synthetic reader timing probe"]
fn widths_refresh_scale_probe(cx: &mut gpui::TestAppContext) {
    let _locale = crate::test_locale::force("en");
    let (cols, mut rows) = page();
    cx.update(|cx| {
        moon_ui::MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(
                moon_core::config::UiThemeMode::Light,
                1.0,
            ),
            cx,
        );
        let axis = moon_core::db::ReportAxis::from_measured(Default::default(), chrono_tz::UTC);
        let visible = (0..cols.len()).collect::<Vec<_>>();
        let cores = vec![1; rows.len()];
        let mut cache = super::NaturalWidthsCache::default();
        for round in 0..5 {
            let identities = super::row_identities(&rows, &cores);
            let start = std::time::Instant::now();
            cache.publish(identities, false);
            cache.refresh(
                &cols,
                &rows,
                &cores,
                &visible,
                moon_ui::MoonPalette::default(),
                &axis,
                chrono_tz::UTC,
                cx,
            );
            println!(
                "reader_refresh widths round={round}: {:.3} ms",
                start.elapsed().as_secs_f64() * 1000.0
            );
        }
        rows[7][2] = Value::Text("UNIT_01 update".into());
        rows[7][3] = Value::Real(-1.23456789);
        let identities = super::row_identities(&rows, &cores);
        let start = std::time::Instant::now();
        cache.publish(identities, false);
        cache.refresh(
            &cols,
            &rows,
            &cores,
            &visible,
            MoonPalette::default(),
            &axis,
            chrono_tz::UTC,
            cx,
        );
        println!(
            "reader_refresh widths one_changed: {:.3} ms",
            start.elapsed().as_secs_f64() * 1000.0
        );
    });
}

/// Removing the free-text ceilings in `report::widths::width_bounds` must fail this assertion;
/// otherwise one long comment can consume the standalone Report viewport and hide useful columns.
#[test]
fn free_text_columns_have_larger_but_bounded_widths() {
    let comment = width_bounds("comment");
    let profit = width_bounds("profitpct");

    assert!(
        comment.1 > profit.1,
        "comments may use more room than numbers"
    );
    assert_eq!(comment.1, 360.0, "comments remain capped");
    assert!(
        profit.0 >= 78.0,
        "signed percentages retain a readable floor"
    );
}

/// Removing locale or the notation from `report/widths.rs:NaturalWidthsEnvironment` must make this
/// fail: a live language or notation change would otherwise reuse widths measured for the previous
/// headers or the previous spelling of the volume cells.
#[test]
fn natural_width_environment_changes_with_locale_notation_or_resolved_font() {
    use moon_core::util::fmt::Notation;
    let environment = |locale: &str, notation: Notation, normal: usize| {
        natural_widths_environment(
            locale,
            notation,
            MonoBodyFontSignature {
                normal: FontId(normal),
                semibold: FontId(2),
                size_bits: 12.0_f32.to_bits(),
            },
        )
    };

    let base = environment("ru", Notation::MoonBot, 1);
    assert_ne!(base, environment("en", Notation::MoonBot, 1));
    assert_ne!(base, environment("ru", Notation::Compact, 1));
    assert_ne!(base, environment("ru", Notation::MoonBot, 3));
}

use super::super::{columns, query};
use super::width_bounds as column_width_bounds;
use crate::design;
use chrono_tz::Tz;
use gpui::FontWeight;
use moon_core::db::ReportAxis;
use moon_ui::MoonPalette;
use rusqlite::types::Value;

/// Frozen full-page measurement oracle.
fn former_natural_widths(
    cols: &[String],
    rows: &[Vec<Value>],
    core_uids: &[u64],
    visible: &[usize],
    p: MoonPalette,
    axis: &ReportAxis,
    display_zone: Tz,
    measurer: &mut design::MonoBodyTextMeasurer<'_>,
) -> std::collections::HashMap<String, f32> {
    visible
        .iter()
        .filter_map(|&column_index| {
            let column = cols.get(column_index)?;
            let header = columns::header_label(column);
            let mut width = measurer.text_width(&header, FontWeight::SEMIBOLD);
            // Generic cells use the same predicate in the renderer, so the two emphasized profit
            // columns cannot be measured light and then clip the wider glyphs they paint. Resolve it
            // once per column because it does not depend on row values.
            let weight = columns::cell_weight(column);
            for (row_index, row) in rows.iter().take(query::MAX_REPORT_ROWS).enumerate() {
                let value = row.get(column_index).unwrap_or(&Value::Null);
                // Measured with the row's own quote, exactly as the renderer formats it: a
                // BTC-denominated row prints eight decimals, and measuring it at two would size the
                // column to clip them.
                let text = columns::cell(
                    column,
                    value,
                    columns::row_quote(cols, row),
                    p,
                    axis,
                    core_uids.get(row_index).copied().unwrap_or(0),
                    display_zone,
                )
                .0;
                width = width.max(measurer.text_width(&text, weight));
            }
            let (floor, ceiling) = column_width_bounds(column);
            Some((column.clone(), (width + 28.0).ceil().clamp(floor, ceiling)))
        })
        .collect()
}

/// Every incremental measurement equals a full measurement, with widths growing between format changes.
#[gpui::test]
fn widths_refresh_cache_matches_full_measurement(cx: &mut gpui::TestAppContext) {
    let _locale = crate::test_locale::force("en");
    let (cols, mut rows) = page();
    cx.update(|cx| {
        moon_ui::MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(
                moon_core::config::UiThemeMode::Light,
                1.0,
            ),
            cx,
        );
        let axis = moon_core::db::ReportAxis::from_measured(Default::default(), chrono_tz::UTC);
        let visible = (0..cols.len()).collect::<Vec<_>>();
        let cores = vec![1; rows.len()];
        let mut cache = super::NaturalWidthsCache::default();
        let mut expected = std::collections::HashMap::<String, f32>::new();
        for round in 0..4 {
            if round == 2 {
                rows[7][2] = rusqlite::types::Value::Text("UNIT_01 ".repeat(80));
                rows[7][3] = rusqlite::types::Value::Real(-123456.12345678);
            }
            if round == 3 {
                rows.swap(7, 499);
            }
            cache.publish(super::row_identities(&rows, &cores), round == 0);
            cache.refresh(
                &cols,
                &rows,
                &cores,
                &visible,
                moon_ui::MoonPalette::default(),
                &axis,
                chrono_tz::UTC,
                cx,
            );
            let mut measurer = crate::design::MonoBodyTextMeasurer::new(cx);
            for (column, width) in former_natural_widths(
                &cols,
                &rows,
                &cores,
                &visible,
                moon_ui::MoonPalette::default(),
                &axis,
                chrono_tz::UTC,
                &mut measurer,
            ) {
                expected
                    .entry(column)
                    .and_modify(|previous| *previous = previous.max(width))
                    .or_insert(width);
            }
            assert_eq!(cache.widths, expected, "round={round}");
        }
        rows[7][2] = rusqlite::types::Value::Text("short".into());
        cache.publish(super::row_identities(&rows, &cores), false);
        rust_i18n::set_locale("ru");
        let shifted = moon_core::db::ReportAxis::from_measured(
            std::collections::HashMap::from([(
                1,
                vec![moon_core::db::OffsetSegment {
                    from_utc: 0,
                    offset_secs: 3600,
                }],
            )]),
            chrono_tz::Europe::Warsaw,
        );
        cache.refresh(
            &cols,
            &rows,
            &cores,
            &visible,
            moon_ui::MoonPalette::default(),
            &shifted,
            chrono_tz::Europe::Warsaw,
            cx,
        );
        let mut measurer = crate::design::MonoBodyTextMeasurer::new(cx);
        assert_eq!(
            cache.widths,
            former_natural_widths(
                &cols,
                &rows,
                &cores,
                &visible,
                moon_ui::MoonPalette::default(),
                &shifted,
                chrono_tz::Europe::Warsaw,
                &mut measurer
            )
        );
        // A column hidden across several publications must measure all of its current inputs.
        cache.clear();
        cache.refresh(
            &cols,
            &rows,
            &cores,
            &[0],
            moon_ui::MoonPalette::default(),
            &axis,
            chrono_tz::UTC,
            cx,
        );
        rows[3][2] = rusqlite::types::Value::Text("UNIT_01 ".repeat(50));
        cache.publish(super::row_identities(&rows, &cores), false);
        cache.refresh(
            &cols,
            &rows,
            &cores,
            &[0],
            moon_ui::MoonPalette::default(),
            &axis,
            chrono_tz::UTC,
            cx,
        );
        cache.publish(super::row_identities(&rows, &cores), false);
        cache.refresh(
            &cols,
            &rows,
            &cores,
            &visible,
            moon_ui::MoonPalette::default(),
            &axis,
            chrono_tz::UTC,
            cx,
        );
        let mut measurer = crate::design::MonoBodyTextMeasurer::new(cx);
        assert_eq!(
            cache.widths,
            former_natural_widths(
                &cols,
                &rows,
                &cores,
                &visible,
                moon_ui::MoonPalette::default(),
                &axis,
                chrono_tz::UTC,
                &mut measurer
            )
        );
    });
}

/// Putting a pixel ceiling back on the date columns in `width_bounds` must fail this: the text is
/// measured under the live font while a ceiling is a constant, so the column would be sized under
/// its own stamp and print `12:50:3…` (#1391). Checked at the default body and with the font
/// slider raised, because a ceiling that happens to clear one size cuts the next.
#[gpui::test]
fn a_date_column_is_always_as_wide_as_its_own_stamp(cx: &mut gpui::TestAppContext) {
    let _locale = crate::test_locale::force("en");
    let (cols, rows) = page();
    let cores = vec![1; rows.len()];
    let axis = ReportAxis::from_measured(Default::default(), Tz::UTC);
    let visible = (0..cols.len()).collect::<Vec<_>>();
    for font_delta in [design::DESIGN_FONT_DELTA, design::DESIGN_FONT_DELTA + 6.0] {
        cx.update(|cx| {
            moon_ui::MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(
                    moon_core::config::UiThemeMode::Light,
                    1.0,
                )
                .with_font_delta(font_delta),
                cx,
            );
            let mut cache = super::NaturalWidthsCache::default();
            cache.publish(super::row_identities(&rows, &cores), true);
            cache.refresh(
                &cols,
                &rows,
                &cores,
                &visible,
                MoonPalette::default(),
                &axis,
                Tz::UTC,
                cx,
            );
            let mut measurer = design::MonoBodyTextMeasurer::new(cx);
            // The first fixture row opens at 2023-11-14 22:13:20 UTC; the fixed format makes every
            // other row the same length.
            let stamp = measurer.text_width("2023-11-14 22:13:20", FontWeight::NORMAL);
            assert!(stamp > 0.0, "the test font must measure real glyphs");
            for column in ["buydate", "closedate"] {
                let natural = cache.widths[column];
                assert!(
                    natural >= stamp + 20.0,
                    "{column} at font delta {font_delta}: natural width {natural} cannot hold a \
                     {stamp} px stamp inside the table's 20 px cell padding"
                );
                // A column the first drag froze at its natural width keeps its seconds; the 150
                // every date column was capped at before does not.
                let frozen = std::collections::HashMap::from([(column.to_string(), natural)]);
                assert!(cache.dates_without_room_for_seconds(&frozen).is_empty());
                let old_cap = std::collections::HashMap::from([(column.to_string(), 150.0)]);
                assert_eq!(cache.dates_without_room_for_seconds(&old_cap), [column]);
            }
        });
    }
}

/// Mutations in `NaturalWidthsCache::dates_without_room_for_seconds`: comparing against the
/// natural width with no slack flips a column to minutes when a fractional display scale rounds
/// its frozen width one pixel under; treating an undragged column as narrow drops the seconds from
/// a table nobody resized; and testing a column other than the two that carry seconds would let a
/// narrow `coin` change the dates.
#[test]
fn only_a_dragged_date_column_narrower_than_its_seconds_falls_back_to_minutes() {
    let mut cache = super::NaturalWidthsCache::default();
    cache.widths.insert("buydate".into(), 188.0);
    cache.widths.insert("closedate".into(), 188.0);
    cache.widths.insert("coin".into(), 90.0);
    let dragged = |pairs: &[(&str, f32)]| {
        pairs
            .iter()
            .map(|(column, width)| (column.to_string(), *width))
            .collect::<std::collections::HashMap<_, _>>()
    };

    assert!(
        cache
            .dates_without_room_for_seconds(&dragged(&[]))
            .is_empty(),
        "an untouched table is rendered at its natural widths or wider"
    );
    assert!(
        cache
            .dates_without_room_for_seconds(&dragged(&[("buydate", 187.0), ("closedate", 400.0)]))
            .is_empty(),
        "a pixel of rounding under the natural width still holds the text"
    );
    assert_eq!(
        cache.dates_without_room_for_seconds(&dragged(&[("buydate", 160.0), ("coin", 40.0)])),
        ["buydate"],
        "the dragged date falls back; the untouched one and the narrow coin do not"
    );
    assert_eq!(
        cache.dates_without_room_for_seconds(&dragged(&[("buydate", 120.0), ("closedate", 120.0)])),
        ["buydate", "closedate"]
    );
    cache.clear();
    assert!(
        cache
            .dates_without_room_for_seconds(&dragged(&[("buydate", 120.0)]))
            .is_empty(),
        "a hidden, unmeasured column has no cell to decide for"
    );
}

/// Hash collisions cannot reuse a width token for different cell contents.
#[test]
fn width_row_identity_resolves_hash_collisions_exactly() {
    let identity = |byte| {
        std::sync::Arc::new(super::RowIdentity {
            hash: 0,
            bytes: vec![byte],
        })
    };
    let mut cache = super::NaturalWidthsCache::default();
    cache.publish(vec![identity(1), identity(2)], false);
    assert_ne!(cache.row_tokens[0], cache.row_tokens[1]);
    let first = cache.row_tokens.clone();
    cache.publish(vec![identity(2), identity(1)], false);
    assert_eq!(cache.row_tokens, vec![first[1], first[0]]);
    cache.publish(vec![identity(3)], false);
    assert!(!first.contains(&cache.row_tokens[0]));
    assert_eq!(cache.identities.len(), 1);
}
