//! Regression tests for Screener columns, sorting, and grouped presentation.

mod layout_fixture;

use std::collections::{HashMap, HashSet};

use moon_core::config::TableSortPreference;

use super::{
    COLS, column_title, migrate_legacy_keys, migrate_legacy_sort, migrate_legacy_widths,
    restore_sort,
};

/// Keep long grouped and singleton values inside their default-width columns in both locales.
#[gpui::test]
fn default_view_cells_stay_inside_columns_at_min_width_ru_en(cx: &mut gpui::TestAppContext) {
    use crate::screener::view::MIN_WINDOW_SIZE;
    use gpui::{Modifiers, point, px};
    use layout_fixture::{LONG_COIN, LONG_CORE, LONG_VENUE, MIN_DRAG_WIDTH, bounds, draw, inside};
    use std::time::Duration;

    for locale in ["ru", "en"] {
        let _locale = crate::test_locale::force(locale);
        let clock = cx.background_executor.clone();
        let mut visual = layout_fixture::open(cx, super::DEFAULT_COLS, false);
        draw(&mut visual);

        for row in 0..2 {
            let mut previous_right = px(0.0);
            for (col, key) in super::DEFAULT_COLS.iter().enumerate() {
                let selector = format!("moon-table-cell-{row}-{col}");
                let cell = bounds(&mut visual, selector.clone());
                let content = bounds(&mut visual, format!("{selector}-content"));
                assert!(
                    inside(content, cell),
                    "{locale} row {row} {key}: {content:?} outside {cell:?}"
                );
                assert!(
                    previous_right <= content.left() + px(0.5),
                    "{locale} row {row} {key} overlaps its left neighbor"
                );
                assert!(
                    cell.right() <= MIN_WINDOW_SIZE.width + px(0.5),
                    "{locale} row {row} {key} lies past the window edge"
                );
                previous_right = content.right();
            }
        }

        // Check every default column against its fixture caption. The layout context uses
        // GPUI's deterministic monospace font, so the advance predicts which values must clip.
        let glyph_width = cx.update(|app| {
            let tokens = moon_ui::MoonTheme::active_tokens(app);
            let text = app.text_system();
            let font = text.resolve_font(&gpui::font(tokens.font_family(true)));
            text.layout_width(font, px(tokens.font(10.5)), '0')
        });
        let volumes = [1_234_567.0, 98_765.0, 12_345.0]
            .map(|volume| moon_core::util::fmt::short(volume, moon_core::util::fmt::Form::Label));
        let age = crate::chartdx::auto_figures_sync::caption::age_text(987_654_321.0);
        let auto_figures = rust_i18n::t!("screener.af_stale", age = age);
        for (row, key, full_text, always_overflows) in [
            (0, "market", LONG_COIN, true),
            (0, "exchange", LONG_VENUE, true),
            (1, "core", LONG_CORE, true),
            (0, "vol24", &volumes[0], false),
            (0, "hvol", &volumes[1], false),
            (0, "vol5m", &volumes[2], false),
            (0, "ask", "0.00000002972", true),
            (0, "d24h", "25.3%", false),
            (0, "d1h", "14.3%", false),
            (0, "d15m", "2.3%", false),
            (0, "funding", "-0.08%", false),
            (0, "lev", "5–20 / 20 I/C", false),
            (0, "orders", "297", false),
            (0, "pnl", "-638.55$", false),
            (0, "pos", "32592593", false),
            (0, "auto_figures", auto_figures.as_ref(), false),
        ] {
            let col = super::DEFAULT_COLS
                .iter()
                .position(|&column| column == key)
                .unwrap();
            let cell = bounds(&mut visual, format!("moon-table-cell-{row}-{col}-content"));
            let shaped_width = glyph_width * full_text.chars().count() as f32;
            if !always_overflows && shaped_width.floor() <= cell.size.width {
                continue;
            }
            // Clear the previous tooltip before moving over the next value. Its overlay
            // can otherwise cover the adjacent row in this small fixture.
            visual.simulate_mouse_move(
                point(
                    MIN_WINDOW_SIZE.width - px(1.0),
                    MIN_WINDOW_SIZE.height - px(1.0),
                ),
                None,
                Modifiers::none(),
            );
            draw(&mut visual);
            visual.simulate_mouse_move(cell.center(), None, Modifiers::none());
            draw(&mut visual);
            clock.advance_clock(Duration::from_millis(900));
            draw(&mut visual);
            bounds(&mut visual, format!("moon-table-cell-tooltip:{full_text}"));
        }

        let logo = bounds(&mut visual, "screener-exchange-logo".into());
        assert!(
            (0..2).any(|row| inside(
                logo,
                bounds(&mut visual, format!("moon-table-cell-{row}-1"))
            )),
            "the loaded logo must fit the default Exchange cell"
        );

        let mut narrow = layout_fixture::open(cx, &["exchange"], true);
        draw(&mut narrow);
        let cell = bounds(&mut narrow, "moon-table-cell-0-0".into());
        assert_eq!(cell.size.width, px(MIN_DRAG_WIDTH));
        if let Some(logo) = narrow.debug_bounds("screener-exchange-logo") {
            assert!(
                logo.top() >= cell.bottom(),
                "the logo would leave no room for the ellipsis"
            );
        }
        let caption = bounds(&mut narrow, "moon-table-cell-0-0-content".into());
        narrow.simulate_mouse_move(caption.center(), None, Modifiers::none());
        draw(&mut narrow);
        clock.advance_clock(Duration::from_millis(900));
        draw(&mut narrow);
        bounds(&mut narrow, format!("moon-table-cell-tooltip:{LONG_VENUE}"));

        // These optional columns share the production renderer and must fit when enabled.
        let mut keys = super::DEFAULT_COLS.to_vec();
        keys.extend([super::TYPE_KEY, super::LISTED_KEY, "step"]);
        let mut visual = layout_fixture::open(cx, &keys, false);
        draw(&mut visual);
        for key in [super::TYPE_KEY, super::LISTED_KEY, "step"] {
            let col = COLS
                .iter()
                .filter(|col| keys.contains(&col.0))
                .position(|col| col.0 == key)
                .expect("optional column");
            let selector = format!("moon-table-cell-0-{col}");
            let cell = bounds(&mut visual, selector.clone());
            let content = bounds(&mut visual, format!("{selector}-content"));
            assert!(
                inside(content, cell),
                "{locale} {key}: {content:?} outside {cell:?}"
            );
        }
    }
}

/// The default order must match the rendered schema and contain the agreed 16 columns.
#[test]
fn default_view_is_sixteen_columns_in_order() {
    let expected = [
        "market",
        "exchange",
        "core",
        "vol24",
        "hvol",
        "vol5m",
        "ask",
        "d24h",
        "d1h",
        "d15m",
        "funding",
        "lev",
        "orders",
        "pnl",
        "pos",
        "auto_figures",
    ];
    assert_eq!(super::DEFAULT_COLS, expected);
    let rendered: Vec<_> = COLS
        .iter()
        .map(|column| column.0)
        .filter(|key| super::DEFAULT_COLS.contains(key))
        .collect();
    assert_eq!(rendered, expected);
}

/// Missing and stale saved column sets open with the same columns and Vol. descending.
#[test]
fn fresh_layout_gets_default_cols_and_vol_desc() {
    for saved in [None, Some(vec![]), Some(vec!["retired-column".into()])] {
        let visible = super::restore_visible_columns(saved);
        assert_eq!(
            visible,
            super::DEFAULT_COLS
                .iter()
                .map(|&key| key.to_string())
                .collect()
        );
        assert_eq!(restore_sort(None, &visible), ("vol24".into(), true));
    }
}

/// Install version four once, including on a fresh layout so the marker gets flushed.
#[test]
fn reset_runs_once_and_marker_blocks_second_run() {
    let mut layout = moon_core::config::WindowLayout::default();
    assert!(super::migrate_layout(&mut layout, "screener-table:win"));
    assert_eq!(layout.screener_columns_migration, Some(4));
    assert!(!super::migrate_layout(&mut layout, "screener-table:win"));
}

/// A column the user hides after reset must stay hidden on reopen.
#[test]
fn user_choice_after_reset_survives_reopen() {
    let id = "screener-table:win";
    let mut layout = moon_core::config::WindowLayout::default();
    super::migrate_layout(&mut layout, id);
    layout
        .table_visible_columns
        .get_mut(id)
        .unwrap()
        .retain(|column| column != "auto_figures");
    assert!(!super::migrate_layout(&mut layout, id));
    let visible = super::restore_visible_columns(layout.table_visible_columns.get(id).cloned());
    assert_eq!(visible.len(), 15);
    assert!(!visible.contains("auto_figures"));
}

/// Replace the previous full-column layout, including its custom widths and dragged order.
#[test]
fn reset_replaces_29_column_market_desc_layout() {
    let id = "screener-table:win";
    let mut layout = moon_core::config::WindowLayout::default();
    let previous: Vec<_> = COLS.iter().map(|column| column.0.to_string()).collect();
    assert_eq!(previous.len(), 29);
    layout.table_visible_columns.insert(id.into(), previous);
    layout.screener_columns = Some(vec!["market".into()]);
    layout.screener_columns_migration = Some(3);
    layout.table_sorts.insert(
        id.into(),
        TableSortPreference {
            column: "market".into(),
            ascending: false,
        },
    );
    layout
        .table_column_widths
        .insert(id.into(), HashMap::from([("market".into(), 140.0)]));
    layout
        .table_column_order
        .insert(id.into(), vec!["market".into(), "ask".into()]);
    assert!(super::migrate_layout(&mut layout, id));
    assert_eq!(layout.table_visible_columns[id], super::DEFAULT_COLS);
    assert!(layout.screener_columns.is_none());
    assert_eq!(layout.table_sorts[id].column, "vol24");
    assert!(!layout.table_sorts[id].ascending);
    assert!(!layout.table_column_widths.contains_key(id));
    assert!(!layout.table_column_order.contains_key(id));
}

/// The initial volume text must activate a one-million-dollar volume floor.
#[test]
fn default_min_vol_parses_to_one_million() {
    assert_eq!(super::parse_vol(super::DEFAULT_MIN_VOL), 1_000_000.0);
}

/// The three venue columns order rows by what they print, not by the float fallback.
///
/// Mutation: drop one of the arms in `sort_indices`. That column then sorts every row as equal,
/// the header arrow turns and nothing moves.
#[test]
fn venue_columns_sort_by_exchange_kind_and_listing() {
    use super::{Entry, EntryVenue};
    use moon_core::market::ScreenerRow;
    use moon_core::market::listing::Listing;
    use moon_core::venue::MarketKind;
    let entry = |core: u64, label: &str, kind: Option<MarketKind>, listing: Listing| Entry {
        row: ScreenerRow {
            core,
            listing,
            ..Default::default()
        }
        .into(),
        core_name: "".into(),
        market_uc: "".into(),
        coin_uc: "".into(),
        venue: EntryVenue {
            label: label.to_string().into(),
            kind,
            ..Default::default()
        },
    };
    let rows = || {
        vec![
            entry(0, "Gate Spot", Some(MarketKind::Spot), Listing::FuturesOnly),
            entry(
                1,
                "Binance Futures",
                Some(MarketKind::Futures),
                Listing::Both,
            ),
            entry(2, "", None, Listing::Unknown),
            entry(
                3,
                "Binance Quarterly",
                Some(MarketKind::Quarterly),
                Listing::SpotOnly,
            ),
        ]
    };
    let order = |key: &str| {
        let mut sorted = rows();
        sort_entries(&mut sorted, key, false);
        sorted.iter().map(|e| e.row.core).collect::<Vec<_>>()
    };
    assert_eq!(order("exchange"), [2, 1, 3, 0]);
    assert_eq!(order("type"), [0, 1, 3, 2]);
    assert_eq!(order("listed"), [2, 1, 3, 0]);
}

/// Integer keys must reproduce the standard total float order for arbitrary NaN payloads too.
#[test]
fn cached_float_keys_match_standard_total_order_across_bit_patterns() {
    let mut bits = 7_u64;
    let mut direct: Vec<_> = (0..10_000)
        .map(|_| {
            bits = bits.wrapping_mul(6364136223846793005).wrapping_add(1);
            f64::from_bits(bits)
        })
        .collect();
    let mut cached = direct.clone();
    direct.sort_by(f64::total_cmp);
    cached.sort_by_key(|value| super::float_sort_key(*value));
    assert_eq!(
        direct
            .iter()
            .map(|value| value.to_bits())
            .collect::<Vec<_>>(),
        cached
            .iter()
            .map(|value| value.to_bits())
            .collect::<Vec<_>>()
    );
}

/// The previous direct comparator is the oracle for every column, stable tie, NaN and signed zero.
#[test]
fn index_sort_matches_direct_rows_for_every_column_and_direction() {
    use super::{Entry, lev_sort, num_key};
    use moon_core::market::ScreenerRow;
    let entries = || {
        [
            f64::NAN,
            -0.0,
            0.0,
            2.0,
            2.0,
            f64::NEG_INFINITY,
            f64::INFINITY,
            -f64::NAN,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, value)| Entry {
            row: ScreenerRow {
                core: i as u64,
                market: if i % 2 == 0 { "XALPHA" } else { "XBETA" }.into(),
                vol_24h: value,
                vol_1h: value,
                vol_1m: value,
                vol_3m: value,
                vol_5m: value,
                ask: value,
                high_1h: value,
                max_order: value,
                d_24h: value,
                d_3h: value,
                d_1h: value,
                d_15m: value,
                d_1m: value,
                d_72h: value,
                funding_pct: value,
                mark_delta_pct: (i % 3 != 0).then_some(value),
                price_step: value,
                core_pnl: value,
                session: (i % 3 != 0).then_some(value),
                pos_size: value,
                orders: (i % 3) as u32,
                leverage_x: (i % 3) as i32,
                max_leverage: (i % 4) as i32,
                ..Default::default()
            }
            .into(),
            core_name: if i % 2 == 0 { "CORE_A" } else { "CORE_B" }.into(),
            market_uc: "".into(),
            coin_uc: "".into(),
            venue: Default::default(),
        })
        .collect::<Vec<_>>()
    };
    for key in COLS.iter().map(|column| column.0).chain(["unknown"]) {
        for desc in [false, true] {
            let mut direct = entries();
            direct.sort_by(|a, b| {
                let order = match key {
                    "market" => a.row.market.cmp(&b.row.market),
                    "core" => a.core_name.cmp(&b.core_name),
                    "lev" => lev_sort(a).total_cmp(&lev_sort(b)),
                    _ => num_key(a, key).total_cmp(&num_key(b, key)),
                };
                if desc { order.reverse() } else { order }
            });
            let mut actual = entries();
            sort_entries(&mut actual, key, desc);
            assert_eq!(
                actual
                    .iter()
                    .map(|entry| entry.row.core)
                    .collect::<Vec<_>>(),
                direct
                    .iter()
                    .map(|entry| entry.row.core)
                    .collect::<Vec<_>>(),
                "{key} desc={desc}"
            );
        }
    }
}

/// The footer labels must resolve from the same schema as the Market and Vol. headers.
///
/// Mutation: restore independent `Coin` or `DVol` footer literals. The footer would again disagree
/// with the headers and the source assertion below would redden.
#[test]
fn screener_filter_labels_reuse_matching_column_titles() {
    assert_eq!(column_title("market"), "Market");
    assert_eq!(column_title("vol24"), "Vol.");

    let view_source = include_str!("../view.rs");
    assert!(view_source.contains("label(column_title(\"market\"))"));
    assert!(view_source.contains("label(column_title(\"vol24\"))"));
    assert!(!view_source.contains("label(\"Coin\")"));
    assert!(!view_source.contains("label(\"DVol\")"));
}

/// `screener/table.rs:restore_sort` must translate MoonUI ascending into the existing `desc` flag.
///
/// Mutation: copy `ascending` directly into `desc`. A saved ascending Core sort would reopen with a
/// descending row order while MoonUI draws an up arrow, and this assertion reddens.
#[test]
fn screener_restore_translates_ascending_to_descending_flag() {
    let visible = COLS.iter().map(|column| column.0.to_string()).collect();
    assert_eq!(
        restore_sort(
            Some(TableSortPreference {
                column: "core".to_string(),
                ascending: true,
            }),
            &visible
        ),
        ("core".to_string(), false)
    );
}

/// `screener/table.rs:restore_sort` must retain Vol.-descending for absent or retired preferences.
///
/// Mutation: accept an unknown key or default to ascending. Existing users would reopen on a
/// non-column or reversed familiar order, and one of these assertions reddens.
#[test]
fn screener_unknown_sort_keeps_the_historical_default() {
    let visible = COLS.iter().map(|column| column.0.to_string()).collect();
    assert_eq!(restore_sort(None, &visible), ("vol24".to_string(), true));
    assert_eq!(
        restore_sort(
            Some(TableSortPreference {
                column: "retired".to_string(),
                ascending: true,
            }),
            &visible
        ),
        ("vol24".to_string(), true)
    );
}

/// `screener/table.rs:restore_sort` must never restore an active key hidden from the user.
///
/// Mutation: omit the visibility check. A saved Core sort would remain active after Core was
/// hidden, leaving no visible header or arrow that could explain or reverse the order.
#[test]
fn screener_hidden_sort_uses_a_visible_fallback() {
    let visible = HashSet::from(["market".to_string(), "ask".to_string()]);
    assert_eq!(
        restore_sort(
            Some(TableSortPreference {
                column: "core".to_string(),
                ascending: true,
            }),
            &visible,
        ),
        ("market".to_string(), true)
    );
}

/// The two profit columns carry MoonBot's own titles: `pnl` is its `PnL`, `session_profit` its
/// `Session`; the retired `session` key names nothing.
///
/// Mutation: title `pnl` "Session" again, or drop one of the keys. The Screener would print the
/// core's PnL counter under the name of the chart's Session caption — the confusion this pair was
/// split to end — and one of these assertions reddens.
#[test]
fn screener_names_both_profit_counters_as_moonbot_does() {
    assert_eq!(column_title("pnl"), "PnL");
    assert_eq!(column_title("session_profit"), "Session");
    assert_eq!(
        column_title("session"),
        "",
        "the retired key must name no column"
    );
}

/// `screener/table.rs:migrate_legacy_keys` must rename the retired `session` entry to `pnl` and
/// leave every current key — the real Session column's `session_profit` included — alone.
///
/// Mutation: guess "is this list pre-rename" from the presence of `pnl`. A user who hid PnL and
/// kept Session after the rename would have the two swapped on every reopen; that is why the old
/// key is retired rather than reused, and why the second assertion holds a list without `pnl`.
#[test]
fn screener_saved_session_key_always_means_the_pnl_column() {
    let legacy = vec!["market".to_string(), "session".to_string()];
    assert_eq!(
        migrate_legacy_keys(legacy),
        vec!["market".to_string(), "pnl".to_string()]
    );
    let pnl_hidden = vec!["market".to_string(), "session_profit".to_string()];
    assert_eq!(migrate_legacy_keys(pnl_hidden.clone()), pnl_hidden);
    let hidden_both = vec!["market".to_string()];
    assert_eq!(migrate_legacy_keys(hidden_both.clone()), hidden_both);
}

/// `screener/table.rs:migrate_legacy_sort` and `migrate_legacy_widths` must bring the retired key
/// current the same way the column list does, and keep a width already saved under `pnl`.
///
/// Mutation: rename only the list. A sort or a width saved on the old PnL column would be dropped
/// by `restore_sort`'s known-column check or left on a key no column reads, and the user's PnL
/// order and width would silently reset.
#[test]
fn screener_saved_session_sort_and_width_follow_the_pnl_column() {
    let on_session = Some(TableSortPreference {
        column: "session".to_string(),
        ascending: false,
    });
    assert_eq!(
        migrate_legacy_sort(on_session).map(|p| p.column),
        Some("pnl".to_string())
    );
    let on_current = Some(TableSortPreference {
        column: "session_profit".to_string(),
        ascending: true,
    });
    assert_eq!(
        migrate_legacy_sort(on_current).map(|p| p.column),
        Some("session_profit".to_string())
    );
    assert_eq!(migrate_legacy_sort(None), None);

    let legacy = HashMap::from([("session".to_string(), 90.0_f32)]);
    assert_eq!(
        migrate_legacy_widths(legacy),
        HashMap::from([("pnl".to_string(), 90.0_f32)])
    );
    let both = HashMap::from([("session".to_string(), 90.0_f32), ("pnl".to_string(), 70.0)]);
    assert_eq!(
        migrate_legacy_widths(both),
        HashMap::from([("pnl".to_string(), 70.0_f32)])
    );
}

/// Retain the owned-row projection only as the test oracle for the index-based UI publication.
pub(in crate::screener) fn sort_entries(entries: &mut [super::Entry], key: &str, desc: bool) {
    let mut order: Vec<_> = (0..entries.len()).collect();
    super::sort_indices(entries, &mut order, key, desc);
    let mut destinations = vec![0; entries.len()];
    for (destination, source) in order.into_iter().enumerate() {
        destinations[source] = destination;
    }
    for source in 0..entries.len() {
        while destinations[source] != source {
            let destination = destinations[source];
            entries.swap(source, destination);
            destinations.swap(source, destination);
        }
    }
}

/// Type and Listed use their short cell captions in both English and Russian.
#[test]
fn type_and_listed_cells_use_short_captions_ru_en() {
    for (locale, kinds, listed) in [
        ("en", ["Spot", "Fut", "Quart"], ["Both", "Spot", "Fut"]),
        ("ru", ["Spot", "Fut", "Quart"], ["Обе", "Спот", "Фьюч"]),
    ] {
        let _locale = crate::test_locale::force(locale);
        assert_eq!(
            super::short_kind_captions()
                .each_ref()
                .map(|label| label.as_ref()),
            kinds
        );
        assert_eq!(
            super::short_listed_captions()
                .each_ref()
                .map(|label| label.as_ref()),
            listed
        );
    }
}

/// Hiding the auto-figures column while the table is sorted by it goes back to the sort STORED
/// before it, and stores nothing: that sort is the user's, and the column's own sort was never
/// written (session-only answers). Hiding any other sorted column still falls back and stores.
///
/// By hand: stored `d1h` ascending, sorted by `auto_figures`, which is then hidden — the table
/// sorts by `d1h` ascending again, nothing written. Sorted by `d1h` and `d1h` hidden — the usual
/// `vol24` descending fallback, written.
///
/// Breakage: the old path, which ran the hidden auto sort through the fallback (`vol24`) and
/// stored that as "no preference", erasing `d1h`.
#[test]
fn hiding_the_auto_figures_sort_restores_the_stored_one() {
    use super::super::auto_figures::AUTO_FIGURES_KEY;
    let stored = Some(TableSortPreference {
        column: "d1h".to_string(),
        ascending: true,
    });
    let visible: HashSet<String> = ["market", "vol24", "d1h"]
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(
        super::sort_after_hiding(AUTO_FIGURES_KEY, true, stored.clone(), &visible),
        (("d1h".to_string(), false), false)
    );
    let without_d1h: HashSet<String> = ["market", "vol24"]
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(
        super::sort_after_hiding("d1h", false, stored, &without_d1h),
        (("vol24".to_string(), true), true)
    );
}

/// Keep Russian count inflection and English plurality while showing a singleton's name.
#[test]
fn core_count_plural_forms_ru_en() {
    let name = gpui::SharedString::from("CORE_A");
    for (locale, expected) in [
        (
            "ru",
            [
                "CORE_A",
                "2 ядра",
                "5 ядер",
                "11 ядер",
                "21 ядро",
                "22 ядра",
            ],
        ),
        (
            "en",
            [
                "CORE_A", "2 cores", "5 cores", "11 cores", "21 cores", "22 cores",
            ],
        ),
    ] {
        let _locale = crate::test_locale::force(locale);
        for (n, expected) in [1, 2, 5, 11, 21, 22].into_iter().zip(expected) {
            assert_eq!(super::core_label(&name, n).as_ref(), expected);
        }
    }
}

/// Create a venue market with a distinct name, leverage and PnL on each core.
fn member(core: u64) -> super::Entry {
    super::Entry {
        row: moon_core::market::ScreenerRow {
            core,
            provider: core,
            market: "XALPHAUSDT".into(),
            coin: "XALPHA".into(),
            leverage_x: core as i32 + 1,
            max_leverage: 20,
            isolated: Some(true),
            core_pnl: -(core as f64 + 0.5),
            orders: u32::from(core == 1),
            ..Default::default()
        }
        .into(),
        core_name: format!("CORE_{core}").into(),
        market_uc: "XALPHAUSDT".into(),
        coin_uc: "XALPHA".into(),
        venue: super::EntryVenue {
            id: Some(moon_core::feed::ExchangeId::new(4)),
            label: "VENUE".into(),
            ..Default::default()
        },
    }
}

/// Show eight member accounts in order and a localized remainder for longer lists.
#[test]
fn core_tooltip_caps_at_eight_plus_more() {
    let entries: Vec<_> = (0..10).map(member).collect();
    let members: Vec<_> = (0..entries.len()).collect();
    for (locale, more) in [("en", "+2 more"), ("ru", "ещё 2")] {
        let _locale = crate::test_locale::force(locale);
        let text = super::core_tooltip(&entries, &members);
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 9);
        assert_eq!(lines[0], "CORE_0 · 1 / 20 I · -0.50$");
        assert_eq!(lines[7], "CORE_7 · 8 / 20 I · -7.50$");
        assert_eq!(lines[8], more);
        assert_eq!(
            super::core_tooltip(&entries, &members[..8]).lines().count(),
            8
        );
    }
}

/// Use the same compact leverage style for single accounts and grouped summaries.
#[test]
fn leverage_prints_short_mode_letters() {
    use super::{LevSummary, Mode, lev_text};
    let row = member(9).row;
    assert_eq!(
        lev_text(super::aggregate([super::account(&row)]).lev),
        "10 / 20 I"
    );
    assert_eq!(
        lev_text(LevSummary::Same {
            x: 10,
            max: 10,
            mode: Mode::C
        }),
        "10 / 10 C"
    );
    for (lo, hi, max, mode, text) in [
        (3, 5, 5, Mode::I, "3–5 / 5 I"),
        (5, 20, 20, Mode::Mixed, "5–20 / 20 I/C"),
    ] {
        assert_eq!(lev_text(LevSummary::Range { lo, hi, max, mode }), text);
    }
    assert_eq!(lev_text(LevSummary::Max(20)), "20");
    assert_eq!(lev_text(LevSummary::None), "—");
}

/// Market clicks must follow the active member even when another member leads the group.
#[test]
fn market_cell_click_target_resolves_to_chart_core() {
    let prepared = super::Prepared::new(vec![member(0), member(1)], "market", false);
    let coin = super::super::view::CoinQuery::new("");
    let selection = prepared.select(&[Some(0), Some(1)], &coin, 0.0, "market", false, true);
    let target = super::MarketTarget::new(&selection.rows[0], &prepared.entries);
    assert_eq!(target.core, 1);
    assert_eq!(target.copy_identity(), (0, "XALPHAUSDT"));
    assert_eq!(target.members.as_deref(), Some(&[0, 1][..]));
}
