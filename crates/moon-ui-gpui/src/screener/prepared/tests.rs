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
            for key in COLS.iter().map(|column| column.0) {
                for desc in [false, true] {
                    let owned: Vec<_> = entries(4, 5)
                        .into_iter()
                        .filter(|entry| allowed[entry.row.core as usize])
                        .collect();
                    let (expected, total, shown) = filter_and_sort(owned, coin, volume, key, desc);
                    let ranks: Vec<_> = prepared
                        .cores
                        .iter()
                        .map(|&core| allowed[core as usize].then_some(core as usize))
                        .collect();
                    let (indices, actual_total, actual_shown) =
                        prepared.select(&ranks, coin, volume, key, desc);
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
        let (indices, _, shown) = data.select(&allowed, "ALPHA1", 0.0, "market", false);
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
    let (order, _, _) = prepared.select(&ranks, "", 0.0, "market", true);
    assert_eq!(
        order
            .iter()
            .map(|&index| prepared.entries[index].row.core)
            .collect::<Vec<_>>(),
        [1, 2]
    );
}

/// The UI pass excludes background market preparation and includes footer counts and sorting.
#[test]
#[ignore = "scale probe: run by hand with --ignored --nocapture"]
fn scale_probe_prepared_selection() {
    let prepared = Prepared::new(entries(10, 800), "vol24", true);
    for coin in ["", "ALPHA1"] {
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
                coin,
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
