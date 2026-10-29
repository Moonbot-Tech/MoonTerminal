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

/// Removing locale from `report/widths.rs:NaturalWidthsEnvironment` must make this assertion fail:
/// a live language change would otherwise reuse widths measured for the previous translated headers.
#[test]
fn natural_width_environment_changes_with_locale_or_resolved_font() {
    let environment = |locale: &str, normal: usize| {
        natural_widths_environment(
            locale,
            MonoBodyFontSignature {
                normal: FontId(normal),
                semibold: FontId(2),
                size_bits: 12.0_f32.to_bits(),
            },
        )
    };

    assert_ne!(environment("ru", 1), environment("en", 1));
    assert_ne!(environment("ru", 1), environment("ru", 3));
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
