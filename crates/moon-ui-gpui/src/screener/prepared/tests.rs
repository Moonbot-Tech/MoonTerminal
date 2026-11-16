//! Prepared-publication parity and UI selection cost at the heavy Screener scale.

use super::*;
use moon_core::market::ScreenerRow;

/// Independent rows repeat markets across member cores and across two providers.
fn entries(cores: u64, markets: usize) -> Vec<Entry> {
    (0..cores)
        .flat_map(|core| {
            (0..markets).map(move |market| Entry {
                row: ScreenerRow {
                    provider: core / 2,
                    core,
                    market: format!("Xalpha{market}Usdt"),
                    coin: format!("xalpha{market}"),
                    vol_24h: if market == 0 {
                        f64::NAN
                    } else {
                        ((market * 7919) % 100_000) as f64
                    },
                    core_pnl: core as f64,
                    orders: core as u32,
                    ..Default::default()
                }
                .into(),
                core_name: format!("CORE_{core}").into(),
                market_uc: format!("XALPHA{market}USDT").into(),
                coin_uc: format!("XALPHA{market}").into(),
                venue: Default::default(),
            })
        })
        .collect()
}

/// Source membership, provider counts, filter semantics and every sort match the owned pipeline.
#[test]
fn selection_matches_owned_rows_for_all_controls_and_columns() {
    use super::super::{table::COLS, view::tests::filter_and_sort};
    let prepared = Prepared::new(entries(4, 5), "vol24", true);
    for allowed in [
        vec![true; 4],
        vec![false, true, true, false],
        vec![false; 4],
    ] {
        for (coin, volume) in [
            ("", 0.0),
            ("ALPHA1", 0.0),
            ("USDT", 10_000.0),
            ("", f64::NAN),
            ("", f64::INFINITY),
            ("XBETA", 0.0),
        ] {
            let query = &crate::screener::view::CoinQuery::new(coin);
            for key in COLS.iter().map(|column| column.0) {
                for desc in [false, true] {
                    let owned: Vec<_> = entries(4, 5)
                        .into_iter()
                        .filter(|entry| allowed[entry.row.core as usize])
                        .collect();
                    let (expected, total, shown) = filter_and_sort(owned, query, volume, key, desc);
                    let ranks: Vec<_> = prepared
                        .cores
                        .iter()
                        .map(|&core| allowed[core as usize].then_some(core as usize))
                        .collect();
                    let (indices, actual_total, actual_shown) =
                        prepared.select(&ranks, query, volume, key, desc);
                    assert_eq!((actual_total, actual_shown), (total, shown));
                    let identity = |entry: &Entry| (entry.row.core, entry.row.market.clone());
                    assert_eq!(
                        indices
                            .iter()
                            .map(|&i| identity(&prepared.entries[i]))
                            .collect::<Vec<_>>(),
                        expected.iter().map(identity).collect::<Vec<_>>(),
                        "{key}, {desc}, {coin}, {volume}"
                    );
                }
            }
        }
    }
}

/// A filter/sort change applies to the old publication immediately and to its replacement.
#[test]
fn latest_controls_apply_to_both_publications() {
    let old = Prepared::new(entries(4, 5), "vol24", true);
    let replacement = Prepared::new(entries(4, 6), "market", false);
    let allowed = [None, Some(1), None, None];
    for data in [&old, &replacement] {
        let (indices, _, shown) = data.select(
            &allowed,
            &crate::screener::view::CoinQuery::new("ALPHA1"),
            0.0,
            "market",
            false,
        );
        assert_eq!(shown, 1);
        assert_eq!(indices.len(), 1);
        assert_eq!(
            (
                data.entries[indices[0]].row.core,
                data.entries[indices[0]].row.market.as_str()
            ),
            (1, "Xalpha1Usdt")
        );
    }
}

/// Hiding a provider's first member must move its tied rows behind an earlier visible group.
#[test]
fn hidden_first_member_does_not_reorder_visible_group_ties() {
    let mut rows = entries(3, 1);
    for entry in &mut rows {
        let provider = u64::from(entry.row.core == 1);
        std::sync::Arc::make_mut(&mut entry.row.market_data).provider = provider;
    }
    rows.sort_by_key(|entry| entry.row.provider);
    let prepared = Prepared::new(rows, "market", true);
    let ranks: Vec<_> = prepared
        .cores
        .iter()
        .map(|core| match core {
            0 => None,
            1 => Some(1),
            _ => Some(2),
        })
        .collect();
    let (order, _, _) = prepared.select(
        &ranks,
        &crate::screener::view::CoinQuery::new(""),
        0.0,
        "market",
        true,
    );
    assert_eq!(
        order
            .iter()
            .map(|&index| prepared.entries[index].row.core)
            .collect::<Vec<_>>(),
        [1, 2]
    );
}

/// The Listed list keeps exactly the rows carrying a ticked answer, unknown being one of them.
///
/// Mutation: treat unknown as passing any selection, or as passing none. Ticking the two
/// one-sided answers would then fill with the rows nothing could answer for — which on an
/// exchange with no arbitrage donor is every row — or the unknown rows could never be looked
/// at on their own.
#[test]
fn the_listed_list_keeps_only_the_ticked_answers() {
    use moon_core::market::listing::Listing;
    use std::collections::HashSet;
    let mut rows = entries(1, 4);
    for (entry, listing) in rows.iter_mut().zip([
        Listing::Both,
        Listing::SpotOnly,
        Listing::Unknown,
        Listing::FuturesOnly,
    ]) {
        std::sync::Arc::make_mut(&mut entry.row.market_data).listing = listing;
    }
    let prepared = Prepared::new(rows, "market", false);
    let markets = |query: crate::screener::view::CoinQuery| {
        let (indices, total, shown) = prepared.select(&[Some(0)], &query, 0.0, "market", false);
        let names: Vec<_> = indices
            .iter()
            .map(|&index| prepared.entries[index].row.market.clone())
            .collect();
        (names, total, shown)
    };
    let all = markets(crate::screener::view::CoinQuery::new(""));
    assert_eq!((all.0.len(), all.1, all.2), (4, 4, 4));
    let ticked = |answers: &[Listing]| {
        let selected: HashSet<Listing> = answers.iter().copied().collect();
        markets(crate::screener::view::CoinQuery::new("").listed_in(&selected))
    };
    let one_sided = ticked(&[Listing::SpotOnly, Listing::FuturesOnly]);
    assert_eq!(one_sided.0, ["Xalpha1Usdt", "Xalpha3Usdt"]);
    // The footer's total stays the universe; only "shown" follows the list.
    assert_eq!((one_sided.1, one_sided.2), (4, 2));
    assert_eq!(ticked(&[Listing::Unknown]).0, ["Xalpha2Usdt"]);
    assert_eq!(ticked(&[Listing::Both]).0, ["Xalpha0Usdt"]);
    // Nothing ticked is every answer, the same as the query with no selection at all.
    assert_eq!(ticked(&[]).0.len(), 4);
}

/// The UI pass excludes background market preparation and includes footer counts and sorting.
#[test]
#[ignore = "scale probe: run by hand with --ignored --nocapture"]
fn scale_probe_prepared_selection() {
    let prepared = Prepared::new(entries(10, 800), "vol24", true);
    for coin in ["", "ALPHA1"] {
        let query = &crate::screener::view::CoinQuery::new(coin);
        let start = std::time::Instant::now();
        for _ in 0..100 {
            std::hint::black_box(prepared.select(
                &[
                    Some(0),
                    Some(1),
                    Some(2),
                    Some(3),
                    Some(4),
                    Some(5),
                    Some(6),
                    Some(7),
                    Some(8),
                    Some(9),
                ],
                query,
                0.0,
                "vol24",
                true,
            ));
        }
        println!(
            "[probe] screener rows=8000 filter={coin:?} prepared_ui={:.3}ms",
            start.elapsed().as_secs_f64() * 10.0
        );
    }
}
