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

/// Adapt the old parity assertions to the explicit ungrouped selection path.
fn select_ungrouped(
    prepared: &Prepared,
    ranks: &[Option<usize>],
    query: &crate::screener::view::CoinQuery,
    volume: f64,
    key: &str,
    desc: bool,
) -> (Vec<usize>, usize, usize) {
    let selection = prepared.select(ranks, query, volume, key, desc, false);
    let indices = selection
        .rows
        .into_iter()
        .map(|row| {
            assert!(row.group.is_none());
            row.lead
        })
        .collect();
    (indices, selection.total, selection.shown)
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
                        select_ungrouped(&prepared, &ranks, query, volume, key, desc);
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
        let (indices, _, shown) = select_ungrouped(
            data,
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

/// Narrowing and widening must retain the original stable sort and restore matching rows.
#[test]
fn keystrokes_match_the_full_filter_and_sort_in_both_directions() {
    use super::super::view::{CoinQuery, tests::filter_and_sort};

    let prepared = Prepared::new(entries(4, 20), "vol24", true);
    let ranks = [Some(0), Some(1), Some(2), Some(3)];
    for input in [
        "X", "XA", "XALPHA", "XALPHA1", "XALPHA10", "XALPHA1", "XA", "",
    ] {
        let query = CoinQuery::new(input);
        let (selected, total, shown) =
            select_ungrouped(&prepared, &ranks, &query, 0.0, "vol24", true);
        let (expected, old_total, old_shown) =
            filter_and_sort(entries(4, 20), &query, 0.0, "vol24", true);
        let key = |entry: &Entry| (entry.row.core, entry.row.market.clone());
        assert_eq!((total, shown), (old_total, old_shown), "{input}");
        assert_eq!(
            selected
                .iter()
                .map(|&i| key(&prepared.entries[i]))
                .collect::<Vec<_>>(),
            expected.iter().map(key).collect::<Vec<_>>(),
            "{input}",
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
    let (order, _, _) = select_ungrouped(
        &prepared,
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
        let (indices, total, shown) =
            select_ungrouped(&prepared, &[Some(0)], &query, 0.0, "market", false);
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
                false,
            ));
        }
        println!(
            "[probe] screener rows=8000 filter={coin:?} prepared_ui={:.3}ms",
            start.elapsed().as_secs_f64() * 10.0
        );
    }
}

/// A row whose Listed answer is still on its way is found under the "no answer" tick and under
/// no other, and the publication says that something is waiting.
///
/// Mutation: give the waiting state no bit, or the bit of an answer. With none, the rows vanish
/// from under every tick for the first minute after the cores connect; with an answer's bit, a
/// list narrowed to `Futures Only` fills with rows nobody has decided anything about.
#[test]
fn a_waiting_row_goes_with_the_no_answer_tick() {
    use moon_core::market::listing::Listing;
    use std::collections::HashSet;
    let mut rows = entries(1, 3);
    for (entry, listing) in
        rows.iter_mut()
            .zip([Listing::Pending, Listing::FuturesOnly, Listing::Unknown])
    {
        std::sync::Arc::make_mut(&mut entry.row.market_data).listing = listing;
    }
    let prepared = Prepared::new(rows, "market", false);
    assert!(prepared.listing_pending);
    let ticked = |answers: &[Listing]| {
        let selected: HashSet<Listing> = answers.iter().copied().collect();
        let query = crate::screener::view::CoinQuery::new("").listed_in(&selected);
        let (indices, _, _) = select_ungrouped(&prepared, &[Some(0)], &query, 0.0, "market", false);
        indices
            .iter()
            .map(|&index| prepared.entries[index].row.market.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(ticked(&[Listing::Unknown]), ["Xalpha0Usdt", "Xalpha2Usdt"]);
    assert_eq!(ticked(&[Listing::FuturesOnly]), ["Xalpha1Usdt"]);
    assert!(ticked(&[Listing::Both, Listing::SpotOnly]).is_empty());
    // And a publication with nothing waiting says so.
    assert!(!Prepared::new(entries(1, 3), "market", false).listing_pending);
}

/// The empty table may say "still being collected" only when the Listed list alone hides a row
/// that is waiting for its answer.
///
/// Mutation: ask only whether anything in the publication is waiting. A text in the Market box
/// that matches nothing, a core the table does not show, or a selection that admits waiting rows
/// and found none would each put a countdown over a table that is simply, finally empty.
#[test]
fn only_the_listed_list_hiding_a_waiting_row_counts_as_a_wait() {
    use moon_core::market::listing::Listing;
    use std::collections::HashSet;
    let mut rows = entries(2, 2);
    for entry in &mut rows {
        let listing = match entry.row.core {
            0 => Listing::Pending,
            _ => Listing::Both,
        };
        std::sync::Arc::make_mut(&mut entry.row.market_data).listing = listing;
    }
    let prepared = Prepared::new(rows, "market", false);
    let query = |text: &str, answers: &[Listing]| {
        let selected: HashSet<Listing> = answers.iter().copied().collect();
        crate::screener::view::CoinQuery::new(text).listed_in(&selected)
    };
    let both_cores = [Some(0), Some(1)];
    // Narrowed to an answer the waiting rows may yet get: a wait.
    assert!(prepared.hides_waiting_rows(
        &both_cores,
        &query("", &[Listing::FuturesOnly]),
        0.0,
        false
    ));
    // The ticks admit waiting rows: nothing is hidden by them.
    assert!(!prepared.hides_waiting_rows(&both_cores, &query("", &[Listing::Unknown]), 0.0, false));
    assert!(!prepared.hides_waiting_rows(&both_cores, &query("", &[]), 0.0, false));
    // The Market box already drops them.
    assert!(!prepared.hides_waiting_rows(
        &both_cores,
        &query("NOSUCHCOIN", &[Listing::FuturesOnly]),
        0.0,
        false,
    ));
    // So does the Vol. minimum.
    assert!(!prepared.hides_waiting_rows(
        &both_cores,
        &query("", &[Listing::FuturesOnly]),
        1.0e12,
        false
    ));
    // The waiting core is not on screen.
    assert!(!prepared.hides_waiting_rows(
        &[None, Some(0)],
        &query("", &[Listing::FuturesOnly]),
        0.0,
        false,
    ));
}

/// Give each fixture core its own provider while keeping the display venue independently set.
fn set_venue(entry: &mut Entry, label: &'static str) {
    std::sync::Arc::make_mut(&mut entry.row.market_data).provider = entry.row.core;
    entry.venue.label = label.into();
}

/// A grouped market's hidden Listed answer can only come from its visible lead.
#[test]
fn pending_non_lead_does_not_make_empty_state_collecting() {
    let mut rows = entries(3, 2);
    for entry in &mut rows {
        set_venue(entry, "VENUE");
        std::sync::Arc::make_mut(&mut entry.row.market_data).listing = if entry.row.core == 0 {
            Listing::Both
        } else {
            Listing::Pending
        };
    }
    let prepared = Prepared::new(rows, "market", false);
    let coin = super::super::view::CoinQuery::new("").listed_in(&[Listing::FuturesOnly].into());
    let ranks = [Some(0), Some(1), Some(2)];
    assert!(
        prepared
            .select(&ranks, &coin, 0.0, "market", false, true)
            .rows
            .is_empty()
    );
    assert!(!prepared.hides_waiting_rows(&ranks, &coin, 0.0, true));
    assert!(prepared.hides_waiting_rows(&[None, Some(1), Some(2)], &coin, 0.0, true));
}

/// The all-core scope separates venues while preserving one row for each canonical market.
#[test]
fn all_cores_scope_gives_one_row_per_venue_market() {
    let mut rows = entries(3, 2);
    for row in &mut rows {
        row.venue.label = if row.row.core < 2 {
            "VENUE_A"
        } else {
            "VENUE_B"
        }
        .into();
    }
    let prepared = Prepared::new(rows, "market", false);
    let selection = prepared.select(
        &[Some(0), Some(1), Some(2)],
        &crate::screener::view::CoinQuery::new("ALPHA1"),
        0.0,
        "market",
        false,
        true,
    );
    assert_eq!(
        (selection.total, selection.shown, selection.cores_shown),
        (4, 2, 3)
    );
    assert_eq!(
        selection.rows[0].group.as_ref().unwrap().members.as_ref(),
        [1, 3]
    );
    assert_eq!(
        selection.rows[1].group.as_ref().unwrap().members.as_ref(),
        [5]
    );
}

/// Picking one core keeps that core's original entries, including their own account facts.
#[test]
fn single_core_scope_keeps_per_core_rows() {
    let prepared = Prepared::new(entries(2, 2), "market", false);
    let selection = prepared.select(
        &[None, Some(1)],
        &crate::screener::view::CoinQuery::new(""),
        0.0,
        "market",
        false,
        false,
    );
    assert_eq!(
        (selection.total, selection.shown, selection.cores_shown),
        (2, 2, 1)
    );
    assert_eq!(
        selection
            .rows
            .iter()
            .map(|row| row.lead)
            .collect::<Vec<_>>(),
        [2, 3]
    );
    assert!(selection.rows.iter().all(|row| row.group.is_none()));
    assert_eq!(prepared.entries[selection.rows[0].lead].row.core_pnl, 1.0);
}

/// Separate providers of one named venue still produce one row per market.
#[test]
fn per_core_mode_separate_providers_still_group_by_venue() {
    let mut rows = entries(2, 1);
    for row in &mut rows {
        set_venue(row, "VENUE_A");
    }
    let prepared = Prepared::new(rows, "market", false);
    let selection = prepared.select(
        &[Some(0), Some(1)],
        &crate::screener::view::CoinQuery::new(""),
        0.0,
        "market",
        false,
        true,
    );
    assert_eq!(
        (selection.total, selection.shown, selection.cores_shown),
        (1, 1, 2)
    );
    let group = selection.rows[0].group.as_ref().unwrap();
    assert_eq!(group.members.as_ref(), [0, 1]);
    assert_eq!(group.chart_core, 1);
}

/// A hidden lead cannot supply volume, account totals, or a chart for the visible subset.
#[test]
fn partial_core_visibility_reaggregates_subset() {
    let mut rows = entries(3, 1);
    for row in &mut rows {
        set_venue(row, "VENUE_A");
        std::sync::Arc::make_mut(&mut row.row.market_data).vol_24h = 100.0 * row.row.core as f64;
        row.row.core_pnl = if row.row.core == 0 { -100.0 } else { 5.0 };
    }
    rows[1].row.orders = 0;
    let prepared = Prepared::new(rows, "pnl", true);
    let query = crate::screener::view::CoinQuery::new("");
    let ranks = [None, Some(2), Some(1)];
    let selection = prepared.select(&ranks, &query, 50.0, "pnl", true, true);
    assert_eq!(
        (selection.total, selection.shown, selection.cores_shown),
        (1, 1, 2)
    );
    let row = &selection.rows[0];
    assert_eq!(row.lead, 1);
    let group = row.group.as_ref().unwrap();
    assert_eq!(group.members.as_ref(), [1, 2]);
    assert_eq!(group.agg.pnl, 10.0);
    assert_eq!(group.agg.orders, 2);
    assert_eq!(group.chart_core, 2);
    let filtered = prepared.select(&ranks, &query, 150.0, "pnl", true, true);
    assert_eq!(
        (filtered.total, filtered.shown, filtered.cores_shown),
        (1, 0, 0)
    );
}

/// Market and volume edits must reuse the already aggregated partial membership.
#[test]
fn partial_scope_reuses_group_views_across_queries() {
    use super::super::view::CoinQuery;

    let mut rows = entries(3, 2);
    for row in &mut rows {
        set_venue(row, "VENUE_A");
    }
    let prepared = Prepared::new(rows, "vol24", true);
    let ranks = [None, Some(1), Some(2)];
    prepared.prime_partial_groups(&HashMap::from([(1, Some(1)), (2, Some(2))]));
    let primed_members = {
        let cache = prepared.partial_groups.lock().unwrap();
        let scope = cache.as_ref().unwrap();
        let slot = prepared.group_of[3] as usize;
        Arc::clone(&scope.groups[slot].as_ref().unwrap().members)
    };
    let first = prepared.select(&ranks, &CoinQuery::new(""), 0.0, "vol24", true, true);
    let second = prepared.select(&ranks, &CoinQuery::new("XALPHA1"), 1.0, "vol24", true, true);
    assert_eq!((second.total, second.shown), (2, 1));
    let row = &second.rows[0];
    let prior = first
        .rows
        .iter()
        .find(|first| first.lead == row.lead)
        .unwrap();
    let members = &row.group.as_ref().unwrap().members;
    assert_eq!(members.as_ref(), [3, 5]);
    assert!(Arc::ptr_eq(&primed_members, members));
    assert!(Arc::ptr_eq(&prior.group.as_ref().unwrap().members, members));

    let changed = prepared.select(
        &[Some(0), None, Some(2)],
        &CoinQuery::new("XALPHA1"),
        1.0,
        "vol24",
        true,
        true,
    );
    assert_eq!(
        changed.rows[0].group.as_ref().unwrap().members.as_ref(),
        [1, 5]
    );
}

/// An unanswered aggregate stays last, even behind a negative or zero session result.
#[test]
fn grouped_session_sort_keeps_absence_last_in_both_directions() {
    use super::super::view::CoinQuery;

    let mut rows = entries(2, 3);
    rows[0].row.session = None;
    rows[3].row.session = None;
    rows[1].row.session = Some(-4.0);
    rows[4].row.session = Some(1.0);
    rows[2].row.session = Some(0.0);
    rows[5].row.session = None;
    let prepared = Prepared::new(rows, "session_profit", true);
    for (descending, expected) in [(false, [1, 2, 0]), (true, [2, 1, 0])] {
        let selection = prepared.select(
            &[Some(0), Some(1)],
            &CoinQuery::new(""),
            0.0,
            "session_profit",
            descending,
            true,
        );
        let leads: Vec<_> = selection.rows.iter().map(|row| row.lead).collect();
        assert_eq!(leads, expected);
    }
}

/// Every grouped publication sort must agree with an on-demand sort of the same leads.
#[test]
fn cached_group_order_matches_uncached_order_for_every_key_and_direction() {
    use super::super::{table::COLS, view::CoinQuery};

    let rows = || {
        let mut rows = entries(4, 3);
        rows[0].row.session = Some(-3.0);
        rows[2].row.session = Some(0.0);
        rows[5].row.session = Some(5.0);
        rows[6].row.pos_size = 4.0;
        rows[8].row.leverage_x = 10;
        rows[10].row.core_pnl = -10.0;
        rows
    };
    let query = CoinQuery::new("");
    let ranks = [Some(0), Some(1), Some(2), Some(3)];
    for &(key, ..) in COLS {
        for desc in [false, true] {
            let prepared = Prepared::new(rows(), key, desc);
            let mut uncached: Vec<_> = prepared
                .market_groups
                .iter()
                .map(|group| group.members[0])
                .collect();
            if key == AUTO_FIGURES_KEY {
                assert_eq!(prepared.sorted_groups, uncached);
                query.sort_by_scan(&prepared.entries, &mut uncached, desc);
            } else {
                sort_group_leads(&prepared.entries, &mut uncached, key, desc, |lead| {
                    &prepared.market_groups[prepared.group_of[lead] as usize]
                });
                assert_eq!(prepared.sorted_groups, uncached, "{key}, descending={desc}");
            }
            let selected = prepared.select(&ranks, &query, 0.0, key, desc, true);
            let selected: Vec<_> = selected.rows.iter().map(|row| row.lead).collect();
            assert_eq!(selected, uncached, "{key}, descending={desc}");
        }
    }
}

/// The aggregate can reverse the leads' PnL order in both cached and immediate sorts.
#[test]
fn grouped_sort_by_pnl_uses_sum() {
    let mut rows = entries(2, 2);
    for (row, pnl) in rows.iter_mut().zip([9.0, 2.0, -20.0, 3.0]) {
        row.row.core_pnl = pnl;
    }
    let prepared = Prepared::new(rows, "pnl", true);
    let leads = |desc| {
        prepared
            .select(
                &[Some(0), Some(1)],
                &crate::screener::view::CoinQuery::new(""),
                0.0,
                "pnl",
                desc,
                true,
            )
            .rows
            .into_iter()
            .map(|row| row.lead)
            .collect::<Vec<_>>()
    };
    assert_eq!(leads(true), [1, 0]);
    assert_eq!(leads(false), [0, 1]);
}

/// A missing label falls back to provider identity rather than merging unknown venues.
#[test]
fn unidentified_venue_never_merges_across_providers() {
    let prepared = Prepared::new(entries(3, 1), "market", false);
    let selection = prepared.select(
        &[Some(0), Some(1), Some(2)],
        &crate::screener::view::CoinQuery::new(""),
        0.0,
        "market",
        false,
        true,
    );
    assert_eq!(
        (selection.total, selection.shown, selection.cores_shown),
        (2, 2, 3)
    );
    assert_eq!(
        selection.rows[0].group.as_ref().unwrap().members.as_ref(),
        [0, 1]
    );
    assert_eq!(
        selection.rows[1].group.as_ref().unwrap().members.as_ref(),
        [2]
    );
}
