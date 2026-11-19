//! Regression tests for restoring Screener header sorting.

use std::collections::{HashMap, HashSet};

use moon_core::config::TableSortPreference;

use super::{
    COLS, add_new_columns, column_title, migrate_legacy_keys, migrate_legacy_sort,
    migrate_legacy_widths, restore_sort,
};

/// A stored visible set gains the columns it predates, once, and nothing else changes.
///
/// Mutation: skip the migration. Everyone who ever arranged their Screener columns holds an
/// explicit list without `coin`, and the new column never appears for them. Mutation the other
/// way: fill an empty set, and "no stored choice" becomes a one-column table.
#[test]
fn a_stored_column_set_gains_the_late_column() {
    let mut arranged = vec!["market".to_string(), "vol24".to_string()];
    add_new_columns(&mut arranged, 0);
    assert_eq!(
        arranged,
        [
            "market",
            "vol24",
            "coin",
            "exchange",
            "type",
            "listed",
            "auto_figures"
        ]
    );

    // Already carrying them — a set written after the columns existed — stays as it is.
    add_new_columns(&mut arranged, 0);
    assert_eq!(
        arranged,
        [
            "market",
            "vol24",
            "coin",
            "exchange",
            "type",
            "listed",
            "auto_figures"
        ]
    );

    let mut empty: Vec<String> = Vec::new();
    add_new_columns(&mut empty, 0);
    assert!(empty.is_empty());
}

/// A later migration adds ITS columns and leaves an earlier one's alone.
///
/// Mutation: add every late column whenever the version moves. A user whose layout had already
/// been offered `coin` and who hid it gets it back the day three unrelated columns arrive — the
/// defect the per-column version exists for, found before it shipped (2026-11-16).
#[test]
fn a_later_migration_does_not_put_back_an_earlier_column_the_user_hid() {
    let mut coin_hidden = vec!["market".to_string(), "vol24".to_string()];
    add_new_columns(&mut coin_hidden, 1);
    assert_eq!(
        coin_hidden,
        [
            "market",
            "vol24",
            "exchange",
            "type",
            "listed",
            "auto_figures"
        ]
    );

    let mut up_to_date = vec!["market".to_string()];
    add_new_columns(&mut up_to_date, super::COLUMNS_MIGRATION);
    assert_eq!(up_to_date, ["market"]);
}

/// Every late column is a real column, and the newest of them is what the layout is migrated to.
///
/// Mutation: add a column under a version above `COLUMNS_MIGRATION`, or bump the constant with no
/// column behind it. In the first case the guard in the view returns before the column is ever
/// offered; in the second every layout is rewritten for nothing.
#[test]
fn the_late_columns_exist_and_end_at_the_current_migration() {
    for (_, key) in super::COLS_ADDED_LATE {
        assert!(COLS.iter().any(|column| column.0 == *key), "{key}");
    }
    let newest = super::COLS_ADDED_LATE.iter().map(|(version, _)| *version);
    assert_eq!(newest.max(), Some(super::COLUMNS_MIGRATION));
    // Oldest first, so a set gains them in the order they were introduced.
    assert!(
        super::COLS_ADDED_LATE
            .windows(2)
            .all(|pair| pair[0].0 <= pair[1].0)
    );
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
            brand: None,
            kind,
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

/// The auto-figures column ships ON for everyone: a layout with no stored set shows every `COLS`
/// key, and a set stored before the column existed gains it once, as every late column does. A
/// layout already migrated past it — whoever has since hidden the column — is left alone.
///
/// Mutation: drop the column from `COLS` (a fresh layout never shows it), drop it from
/// `COLS_ADDED_LATE` (everyone who ever arranged their columns never sees it), or file it under an
/// old version (a layout already offered the location columns never gets it).
#[test]
fn the_auto_figures_column_ships_on_for_a_stored_set_once() {
    use super::super::auto_figures::AUTO_FIGURES_KEY;
    assert!(COLS.iter().any(|column| column.0 == AUTO_FIGURES_KEY));
    let mut arranged = vec!["market".to_string(), "vol24".to_string()];
    add_new_columns(&mut arranged, 2);
    assert_eq!(arranged, ["market", "vol24", "auto_figures"]);
    let mut hidden_since = vec!["market".to_string(), "vol24".to_string()];
    add_new_columns(&mut hidden_since, super::COLUMNS_MIGRATION);
    assert_eq!(hidden_since, ["market", "vol24"]);
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
