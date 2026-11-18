//! Filtering regressions independent of the displayed market spelling.

use super::super::table::tests::sort_entries;
use super::{CoinQuery, Entry, matches_coin_filter};
use moon_core::market::ScreenerRow;

/// Measure the cached publication filter after the Market field gains a character.
#[test]
#[ignore = "bench: S-smooth"]
fn bench_s_screener_keystroke() {
    use super::super::prepared::Prepared;
    use crate::test_support::{
        bench,
        fixtures::{PanelFixture, core_store},
    };

    let fixture = PanelFixture::new(20, 100, 5000, 0, 0);
    let symbols = core_store(1, 0, 0, 3000, 0);
    let catalogue = &symbols.core(1).unwrap().assets.rows;
    let entries: Vec<_> = fixture
        .sessions
        .iter()
        .flat_map(|session| {
            catalogue.iter().enumerate().map(move |(i, symbol)| Entry {
                row: ScreenerRow {
                    provider: session.id,
                    core: session.id,
                    market: symbol.market.clone(),
                    coin: symbol.coin.clone(),
                    vol_24h: ((i * 7919) % 100_000) as f64,
                    ..Default::default()
                }
                .into(),
                core_name: session.name.clone().into(),
                market_uc: symbol.market.clone().into(),
                coin_uc: symbol.coin.clone().into(),
                venue: Default::default(),
            })
        })
        .collect();
    assert_eq!(entries.len(), 60_000);
    let publication = Prepared::new(entries, "vol24", true);
    let ranks: Vec<_> = (0..20).map(Some).collect();
    let query = CoinQuery::new("XA");
    bench::measure("screener_keystroke_warm_filtered_cached_sort", 24, || {
        let (indices, total, shown) = publication.select(&ranks, &query, 0.0, "vol24", true);
        assert_eq!(total, 60_000);
        assert_eq!(shown, 59_980);
        assert_eq!(indices.len(), shown);
        (indices, total, shown)
    });
}

/// The same revision reuses its snapshot; changing a core or replacing it retires that snapshot.
#[test]
fn order_count_memo_tracks_table_revision_and_fleet_epoch() {
    use std::sync::Arc;

    let mut memo = super::OrderCountMemo::default();
    let store = crate::test_support::fixtures::core_store(1, 0, 1, 0, 0);
    let orders = &store.core(1).unwrap().orders;
    let market = orders[0].market.as_str();
    let old = memo.counts(1, 1, orders);
    let other = memo.counts(2, 1, orders);
    assert_eq!(old[market], 1);
    assert!(Arc::ptr_eq(&old, &memo.counts(1, 1, orders)));
    let empty = memo.counts(1, 2, &[]);
    assert!(empty.is_empty());
    assert_eq!(old[market], 1);
    assert!(Arc::ptr_eq(&other, &memo.counts(2, 1, orders)));
    memo.update_fleet_epoch(1);
    assert_eq!(memo.counts(1, 2, orders)[market], 1);
}

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
        .split_once("fn toggle_core(")
        .unwrap()
        .0;
    assert!(sort.contains("self.rebuild(cx);"));
    // Every change of the core filter — one core, an exchange row, a saved group — lands in the
    // picker host's one hook, so that hook is where the resample must be.
    let core = source
        .split_once("fn after_core_selection_change(")
        .unwrap()
        .1
        .split_once("\n    }")
        .unwrap()
        .0;
    assert!(core.contains("self.rebuild(cx);"));
    for toggle in ["fn toggle_core(", "fn toggle_exchange_cores("] {
        let body = source
            .split_once(toggle)
            .unwrap()
            .1
            .split_once("\n    }")
            .unwrap()
            .0;
        assert!(body.contains("self.after_core_selection_change(cx);"));
    }
    let rebuild = source
        .split_once("fn rebuild(")
        .unwrap()
        .1
        .split_once("fn filter_cached(")
        .unwrap()
        .0;
    assert!(rebuild.find("self.filter_cached(cx);").unwrap() < rebuild.find("cx.spawn(").unwrap());
}

/// The column migration runs once: guard first, the stored sets next, then the marker, then an
/// unconditional dirty mark so the marker reaches disk.
///
/// Mutation: drop the marker write, or mark the layout dirty only when a set changed. Either way
/// the guard never short-circuits on a later open, and a Coin column the user has hidden comes
/// back every time the window opens. Pinned as a source contract because the block runs inside a
/// view constructor, which needs a native GPUI window.
#[test]
fn the_column_migration_records_its_completion() {
    let source = include_str!("../view.rs");
    let block = source
        .split_once("b.layout.screener_columns_migration.unwrap_or(0)")
        .expect("the migration must be guarded by its marker")
        .1
        .split_once("});")
        .expect("the migration runs in one backend update")
        .0;
    let guard = block
        .find("return;")
        .expect("an applied migration must not run again");
    let rewrite = block
        .find("add_new_columns(keys, applied)")
        .expect("the stored sets must gain the late columns");
    let marker = block
        .find("b.layout.screener_columns_migration = Some(SCREENER_COLUMNS_MIGRATION);")
        .expect("the migration must record its completion");
    let dirty = block
        .find("b.layout_dirty = true;")
        .expect("the marker must be flushed");
    assert!(guard < rewrite && rewrite < marker && marker < dirty);
    // Past the last stored set, nothing may open a new block: the marker and the dirty mark sit at
    // the closure's own level, so neither can end up under a condition on whether a set changed.
    let last_rewrite = block
        .rfind("add_new_columns(keys, applied)")
        .expect("checked above");
    assert!(
        !block[last_rewrite..dirty].contains('{'),
        "the marker and the dirty mark must not depend on whether a set was rewritten"
    );
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
        venue: Default::default(),
    };
    assert!(matches_coin_filter(&entry, &CoinQuery::new("ALPHA")));
    assert!(matches_coin_filter(&entry, &CoinQuery::new("BETA")));
    assert!(!matches_coin_filter(&entry, &CoinQuery::new("GAMMA")));
}

/// Treating an empty query as absent data would hide every market when the input is cleared.
#[test]
fn empty_filter_keeps_all() {
    let entry = Entry {
        row: ScreenerRow::default().into(),
        core_name: "synthetic".into(),
        market_uc: "".into(),
        coin_uc: "BETA".into(),
        venue: Default::default(),
    };
    assert!(matches_coin_filter(&entry, &CoinQuery::new("")));
}

/// Trimming before mode detection would retain substring coins and lose contract-tail semantics.
#[test]
fn trailing_space_selects_exact_ticker_or_contract_tail_in_either_search_string() {
    let entries = super::entries_for(
        ["WAG", "GMWAGMI", "WAGMI", "1000WAG", "WAG_0925", "WAG_RP"]
            .into_iter()
            .map(|coin| {
                ScreenerRow {
                    market: format!("USDT-{coin}"),
                    coin: coin.to_string(),
                    ..Default::default()
                }
                .into()
            })
            .chain(std::iter::once(
                ScreenerRow {
                    market: "WAG_1225".into(),
                    coin: "OTHER".into(),
                    ..Default::default()
                }
                .into(),
            ))
            .collect(),
        &Default::default(),
    );
    let selected = |raw: &str| {
        let query = CoinQuery::new(raw);
        entries
            .iter()
            .filter(|entry| matches_coin_filter(entry, &query))
            .map(|entry| entry.row.coin.as_str())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        selected("wag"),
        [
            "WAG", "GMWAGMI", "WAGMI", "1000WAG", "WAG_0925", "WAG_RP", "OTHER"
        ]
    );
    assert_eq!(selected("wAg "), ["WAG", "WAG_0925", "WAG_RP", "OTHER"]);
    assert_eq!(selected(" WAG"), selected("WAG"));
    assert!(selected("WA G").is_empty());
    assert_eq!(
        selected("USDT-"),
        ["WAG", "GMWAGMI", "WAGMI", "1000WAG", "WAG_0925", "WAG_RP"]
    );
    assert!(selected("USDT- ").is_empty());
    for raw in ["", " ", "   ", "\t"] {
        assert_eq!(selected(raw).len(), entries.len());
    }
}

/// Folding after trimming would turn Russian-layout exact IO into a substring that admits BIO.
#[test]
fn russian_layout_preserves_exact_mode() {
    let entries = super::entries_for(
        ["IO", "BIO", "IO_0925"]
            .into_iter()
            .map(|coin| {
                ScreenerRow {
                    market: format!("{coin}USDT"),
                    coin: coin.into(),
                    ..Default::default()
                }
                .into()
            })
            .collect(),
        &Default::default(),
    );
    let query = CoinQuery::new("шщ ");
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches_coin_filter(entry, &query))
            .map(|entry| entry.row.coin.as_str())
            .collect::<Vec<_>>(),
        ["IO", "IO_0925"]
    );
}

/// Folding every Cyrillic query to the Latin keys made a Russian coin name unreachable: `офз`
/// became `JAP` and missed `ОФЗ 26254` (#1303). Both spellings must find their own rows, and the
/// Russian-layout ticker must still find its Latin coin.
#[test]
fn cyrillic_query_matches_a_russian_coin_name_and_the_layout_fold_still_works() {
    let entries = super::entries_for(
        [("SU26254RMFS1", "ОФЗ 26254"), ("SBER", "SBER")]
            .into_iter()
            .map(|(market, coin)| {
                ScreenerRow {
                    market: market.into(),
                    coin: coin.into(),
                    ..Default::default()
                }
                .into()
            })
            .collect(),
        &Default::default(),
    );
    let selected = |raw: &str| {
        let query = CoinQuery::new(raw);
        entries
            .iter()
            .filter(|entry| matches_coin_filter(entry, &query))
            .map(|entry| entry.row.market.as_str())
            .collect::<Vec<_>>()
    };
    assert_eq!(selected("офз"), ["SU26254RMFS1"]);
    assert_eq!(selected("ыиук"), ["SBER"]);
    assert_eq!(selected("su26254"), ["SU26254RMFS1"]);
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
                venue: Default::default(),
                row: row.into(),
                core_name: "CORE_A".into(),
            })
            .collect();
        std::hint::black_box(filter_and_sort(
            entries,
            &CoinQuery::new(""),
            0.0,
            "vol24",
            true,
        ));
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
                venue: Default::default(),
            })
            .collect()
    };
    let (rows, total, shown) = filter_and_sort(entries(), &CoinQuery::new(""), 0.0, "vol24", true);
    assert_eq!((rows.len(), total, shown), (6, 2, 2));
    let (rows, total, shown) =
        filter_and_sort(entries(), &CoinQuery::new("ALPHA"), 4.0, "vol24", true);
    assert_eq!((rows.len(), total, shown), (2, 2, 1));
    assert_eq!(
        rows.iter().map(|row| row.row.core).collect::<Vec<_>>(),
        [5, 4]
    );
    let (rows, total, shown) =
        filter_and_sort(entries(), &CoinQuery::new("XBETA"), 0.0, "vol24", true);
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
        let (entries, total, shown) =
            filter_and_sort(entries, &CoinQuery::new(""), 0.0, "vol24", true);
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
    coin_filter: &super::CoinQuery,
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

/// A list's selection: a tick toggles, the All row clears, and every choice ticked IS "all".
///
/// Mutation: keep a complete selection as an explicit full set. The All row then stands unticked
/// over a table that shows everything, and the button reads a count instead of "all".
#[test]
fn a_choice_list_toggles_and_collapses_a_full_selection_to_all() {
    let mut selected: HashSet<u8> = HashSet::new();
    super::toggle_choice(&mut selected, Some(1), 3);
    assert_eq!(selected, HashSet::from([1]));
    super::toggle_choice(&mut selected, Some(2), 3);
    assert_eq!(selected, HashSet::from([1, 2]));
    super::toggle_choice(&mut selected, Some(1), 3);
    assert_eq!(selected, HashSet::from([2]), "a second click unticks");
    super::toggle_choice(&mut selected, Some(1), 3);
    super::toggle_choice(&mut selected, Some(3), 3);
    assert!(selected.is_empty(), "every choice ticked is all");
    super::toggle_choice(&mut selected, Some(2), 3);
    super::toggle_choice(&mut selected, None, 3);
    assert!(selected.is_empty(), "the All row clears");
}

/// The button names what the list is narrowed to: "all", the one choice, or how many.
///
/// Mutation: print the count for a single tick, or the first caption for several. A list
/// narrowed to `Spot` would read `Type: 1`, and one narrowed to two answers would name only one
/// of them — a table missing half its rows under a button that does not say so.
#[test]
fn a_choice_list_button_reads_all_one_caption_or_a_count() {
    let choices = [(1u8, "Spot".to_string()), (2, "Futures".to_string())];
    let summary = |ticked: &[u8]| {
        let selected: HashSet<u8> = ticked.iter().copied().collect();
        super::choice_summary("Type", "All", &choices, &selected)
    };
    assert_eq!(summary(&[]), "Type: All");
    assert_eq!(summary(&[2]), "Type: Futures");
    assert_eq!(summary(&[1, 2]), "Type: 2");
}

/// The button is sized for every caption it can show, and for none it cannot.
///
/// Mutation: size it for the choice captions alone. The button then clips "all" or a count
/// whenever that is the longest thing it prints — in a language whose word for "all" outruns
/// its market kinds — and, being fixed, never grows to show it.
#[test]
fn a_choice_list_button_is_sized_for_every_caption_it_can_show() {
    let choices = [
        (1u8, "Spot".to_string()),
        (2, "Futures".to_string()),
        (3, "Quarterly".to_string()),
        (4, "?".to_string()),
    ];
    let mut captions = super::choice_captions("Type", "All", &choices);
    captions.sort();
    // Four ticks collapse to "all" and one tick prints its caption, so the counts are 2 and 3.
    assert_eq!(
        captions,
        [
            "Type: 2",
            "Type: 3",
            "Type: ?",
            "Type: All",
            "Type: Futures",
            "Type: Quarterly",
            "Type: Spot",
        ]
    );
}

/// The Type list decides by market kind, and an exchange this build cannot name is a kind of
/// its own rather than a wildcard.
///
/// Mutation: let an unnamed exchange through every selection, or fold coin-margined contracts
/// into `Futures`. Ticking `Spot` would then still list every core of an unnamed exchange, and
/// the `Quarterly` tick would select nothing the `Futures` tick does not.
#[test]
fn the_type_list_admits_a_kind_only_by_its_own_tick() {
    use moon_core::venue::MarketKind;
    let ticked = |kinds: &[Option<MarketKind>]| -> HashSet<Option<MarketKind>> {
        kinds.iter().copied().collect()
    };
    let every_kind = [
        Some(MarketKind::Spot),
        Some(MarketKind::Futures),
        Some(MarketKind::Quarterly),
        None,
    ];
    for kind in every_kind {
        assert!(
            super::type_admits(&ticked(&[]), kind),
            "nothing ticked is all"
        );
        for other in every_kind {
            assert_eq!(
                super::type_admits(&ticked(&[other]), kind),
                kind == other,
                "{kind:?} under a list narrowed to {other:?}"
            );
        }
    }
    let derivatives = ticked(&[Some(MarketKind::Futures), Some(MarketKind::Quarterly)]);
    assert!(super::type_admits(
        &derivatives,
        Some(MarketKind::Quarterly)
    ));
    assert!(!super::type_admits(&derivatives, None));
}

/// The "still being collected" line counts whole seconds up, and never reaches zero.
///
/// Mutation: round down, or let it reach zero. The line is drawn while a row is still waiting,
/// and the book that settles it lands a few seconds after the moment the count runs out — the
/// table would read "0 s left" for those seconds, over rows that have not arrived.
#[test]
fn the_collecting_line_counts_whole_seconds_and_never_reads_zero() {
    let now = 5_000_000;
    assert_eq!(super::collecting_seconds(Some(now + 42_000), now), 42);
    assert_eq!(super::collecting_seconds(Some(now + 41_001), now), 42);
    assert_eq!(super::collecting_seconds(Some(now + 1), now), 1);
    assert_eq!(super::collecting_seconds(Some(now), now), 1);
    assert_eq!(super::collecting_seconds(Some(now - 4_000), now), 1);
    assert_eq!(super::collecting_seconds(None, now), 1);
}
