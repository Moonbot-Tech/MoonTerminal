//! Filtering regressions independent of the displayed market spelling.

use super::super::table::tests::sort_entries;
use super::{Entry, matches_coin_filter};
use moon_core::market::ScreenerRow;

/// Cached-only control handlers would miss market-only feed updates until another backend event.
/// Pin the refresh wiring as a source contract because these handlers require a native GPUI window.
#[test]
fn controls_resample_market_data_after_immediate_cached_selection() {
    let source = include_str!("../view.rs");
    let input = source
        .split_once("cx.subscribe(input,")
        .unwrap()
        .1
        .split_once(".detach();")
        .unwrap()
        .0;
    assert!(input.contains("this.rebuild(cx);"));
    let sort = source
        .split_once("fn set_sort(")
        .unwrap()
        .1
        .split_once("fn set_source(")
        .unwrap()
        .0;
    assert!(sort.contains("self.rebuild(cx);"));
    let core = source
        .split_once("fn set_source(")
        .unwrap()
        .1
        .split_once("fn toggle_col(")
        .unwrap()
        .0;
    assert!(core.contains("self.rebuild(cx);"));
    let rebuild = source
        .split_once("fn rebuild(")
        .unwrap()
        .1
        .split_once("fn filter_cached(")
        .unwrap()
        .0;
    assert!(rebuild.find("self.filter_cached(cx);").unwrap() < rebuild.find("cx.spawn(").unwrap());
}
use std::collections::HashSet;

/// Cached search forms must allow either market or coin matches without changing display text.
#[test]
fn filter_matches_uppercased_market_or_coin() {
    let entry = Entry {
        row: ScreenerRow {
            market: "alphaUsdt".into(),
            coin: "beta".into(),
            ..Default::default()
        }
        .into(),
        core_name: "synthetic".into(),
        market_uc: "ALPHAUSDT".into(),
        coin_uc: "BETA".into(),
    };
    assert!(matches_coin_filter(&entry, "ALPHA"));
    assert!(matches_coin_filter(&entry, "BETA"));
    assert!(!matches_coin_filter(&entry, "GAMMA"));
}

/// Treating an empty query as absent data would hide every market when the input is cleared.
#[test]
fn empty_filter_keeps_all() {
    let entry = Entry {
        row: ScreenerRow::default().into(),
        core_name: "synthetic".into(),
        market_uc: "".into(),
        coin_uc: "BETA".into(),
    };
    assert!(matches_coin_filter(&entry, ""));
}

/// Scale probe of one market rebuild's pure part — search forms, the coin count and the sort —
/// for 10 cores sharing 800 markets.
#[test]
#[ignore = "scale probe: run by hand with --ignored --nocapture"]
fn scale_probe_market_rebuild() {
    let rows: Vec<ScreenerRow> = (0..10)
        .flat_map(|core| {
            (0..800).map(move |m| ScreenerRow {
                market: format!("Xalpha{m}Usdt"),
                coin: format!("xalpha{m}"),
                vol_24h: f64::from((m * 7919 + core * 31) % 100_000),
                ..Default::default()
            })
        })
        .collect();
    let iters = 20;
    let start = std::time::Instant::now();
    for _ in 0..iters {
        let entries: Vec<Entry> = rows
            .iter()
            .cloned()
            .map(|row| Entry {
                market_uc: row.market.to_uppercase().into(),
                coin_uc: row.coin.to_uppercase().into(),
                row: row.into(),
                core_name: "CORE_A".into(),
            })
            .collect();
        std::hint::black_box(filter_and_sort(entries, "", 0.0, "vol24", true));
    }
    let ms = start.elapsed().as_secs_f64() * 1_000.0 / f64::from(iters);
    println!("[probe] screener rows={} rebuild={ms:.3}ms", rows.len());
}

/// Footer counts are markets per provider; repeated core rows and filters must not change units.
#[test]
fn footer_counts_keep_provider_identity_and_filter_semantics() {
    let entries = || {
        (0..6)
            .map(|i| Entry {
                row: ScreenerRow {
                    core: i,
                    provider: i / 3,
                    market: "XALPHA".into(),
                    vol_24h: i as f64,
                    ..Default::default()
                }
                .into(),
                core_name: "CORE_A".into(),
                market_uc: "XALPHA".into(),
                coin_uc: "XALPHA".into(),
            })
            .collect()
    };
    let (rows, total, shown) = filter_and_sort(entries(), "", 0.0, "vol24", true);
    assert_eq!((rows.len(), total, shown), (6, 2, 2));
    let (rows, total, shown) = filter_and_sort(entries(), "ALPHA", 4.0, "vol24", true);
    assert_eq!((rows.len(), total, shown), (2, 2, 1));
    assert_eq!(
        rows.iter().map(|row| row.row.core).collect::<Vec<_>>(),
        [5, 4]
    );
    let (rows, total, shown) = filter_and_sort(entries(), "XBETA", 0.0, "vol24", true);
    assert_eq!((rows.len(), total, shown), (0, 2, 0));
}

/// Shared market halves must keep member account values, search text and chart identity separate.
#[test]
fn shared_entries_preserve_member_values_and_normalize_each_market_once() {
    use moon_core::market::SharedScreenerRow;
    use std::sync::Arc;
    let market = ScreenerRow {
        provider: 1,
        market: "xAlphaUSDT".into(),
        coin: "xAlpha".into(),
        vol_24h: 25.0,
        ..Default::default()
    };
    let first: SharedScreenerRow = market.into();
    let mut second = first.clone();
    second.core = 2;
    second.orders = 7;
    second.core_pnl = -3.0;
    second.session = Some(4.0);
    let names = std::collections::HashMap::from([(0, "CORE_A".into()), (2, "CORE_B".into())]);
    let entries = super::entries_for(vec![first, second], &names);
    assert!(Arc::ptr_eq(&entries[0].market_uc, &entries[1].market_uc));
    assert!(Arc::ptr_eq(&entries[0].coin_uc, &entries[1].coin_uc));
    assert_eq!(entries[0].market_uc.as_ref(), "XALPHAUSDT");
    assert_eq!(entries[1].row.market, "xAlphaUSDT");
    assert_eq!(
        (
            entries[0].row.core,
            entries[0].row.orders,
            entries[0].row.core_pnl
        ),
        (0, 0, 0.0)
    );
    assert_eq!(
        (
            entries[1].row.core,
            entries[1].row.orders,
            entries[1].row.core_pnl,
            entries[1].row.session
        ),
        (2, 7, -3.0, Some(4.0))
    );
    assert_eq!(entries[1].core_name.as_ref(), "CORE_B");
}

/// Time the synchronous shared-source rebuild at the same ten-member, 800-market scale.
#[test]
#[ignore = "scale probe: run by hand with --ignored --nocapture"]
fn scale_probe_shared_market_rebuild() {
    use moon_core::market::SharedScreenerRow;
    let markets: Vec<SharedScreenerRow> = (0..800)
        .map(|m| {
            ScreenerRow {
                market: format!("Xalpha{m}Usdt"),
                coin: format!("xalpha{m}"),
                vol_24h: f64::from((m * 7919) % 100_000),
                ..Default::default()
            }
            .into()
        })
        .collect();
    let rows: Vec<_> = (0..10)
        .flat_map(|core| {
            markets.iter().cloned().map(move |mut row| {
                row.core = core;
                row
            })
        })
        .collect();
    let names = (0..10).map(|core| (core, "CORE_A".into())).collect();
    let iters = 20;
    let start = std::time::Instant::now();
    for _ in 0..iters {
        let entries = super::entries_for(rows.clone(), &names);
        let (entries, total, shown) = filter_and_sort(entries, "", 0.0, "vol24", true);
        assert_eq!((entries.len(), total, shown), (8_000, 800, 800));
        std::hint::black_box(entries);
    }
    println!(
        "[probe] screener rows={} shared_rebuild={:.3}ms",
        rows.len(),
        start.elapsed().as_secs_f64() * 1_000.0 / f64::from(iters)
    );
}

/// Count the distinct markets among per-core rows, for the footer's coin count.
///
/// Rows on one exchange repeat every market once per core, and two exchanges may list the same
/// market name, so the key is the market together with its exchange — `row.provider`, the core
/// its market half was read from. That is what the footer counted before rows went per-core.
///
/// Args:
///     entries: The rows to count over.
///
/// Returns:
///     The number of distinct `(provider, market)` pairs across the entries.
fn distinct_markets(entries: &[Entry]) -> usize {
    entries
        .iter()
        .map(|e| (e.row.provider, e.row.market.as_str()))
        .collect::<HashSet<_>>()
        .len()
}

/// Apply the current filters and order, reusing the total when no filter can remove a market.
pub(in crate::screener) fn filter_and_sort(
    mut entries: Vec<Entry>,
    coin_filter: &str,
    min_vol: f64,
    key: &str,
    desc: bool,
) -> (Vec<Entry>, usize, usize) {
    let total = distinct_markets(&entries);
    if !coin_filter.is_empty() {
        entries.retain(|entry| matches_coin_filter(entry, coin_filter));
    }
    if min_vol > 0.0 {
        entries.retain(|entry| entry.row.vol_24h >= min_vol);
    }
    let shown = if coin_filter.is_empty() && min_vol <= 0.0 {
        total
    } else {
        distinct_markets(&entries)
    };
    sort_entries(&mut entries, key, desc);
    (entries, total, shown)
}
