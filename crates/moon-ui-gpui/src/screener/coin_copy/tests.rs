//! Regressions for the coin lists the Screener's copy entries write.

use moon_core::market::ScreenerRow;
use moon_core::session::CoreId;

use super::super::table::Entry;
use super::{coin_lists, unique_coins};

/// One listed row of `core` for `market`, printing `coin`.
fn entry(core: CoreId, market: &str, coin: &str) -> Entry {
    Entry {
        row: ScreenerRow {
            provider: core,
            core,
            market: market.into(),
            coin: coin.into(),
            ..Default::default()
        }
        .into(),
        core_name: "synthetic".into(),
        market_uc: market.to_uppercase().into(),
        coin_uc: coin.to_uppercase().into(),
        venue: Default::default(),
    }
}

/// The same coin on two cores is written once, where it first appears, in table order.
///
/// Mutation: drop the de-duplication, and a two-core Screener copies `BTC,BTC,ETH`, which a
/// strategy coin list takes as a duplicate. Mutation the other way: keep the LAST occurrence, or
/// sort the set, and the order stops being the one the user sorted the table into.
#[test]
fn a_coin_listed_on_two_cores_is_copied_once_where_it_first_appears() {
    assert_eq!(
        unique_coins(["ETH", "BTC", " ETH", "", "SOL", "BTC"]),
        ["ETH", "BTC", "SOL"]
    );
}

/// The lists follow the table's order, not the publication's, and the clicked row is found by
/// core and market.
///
/// Mutation: read `entries` in their own order instead of through `rows`, and the copy ignores
/// the user's sort. Mutation: match the clicked row by market alone, and a right click on the
/// second core's `ETHUSDT` stops at the first core's line. Mutation: take `last` rows instead of
/// `last + 1`, and the clicked coin itself is missing from "down to this row".
#[test]
fn down_to_this_row_follows_the_sort_and_includes_the_clicked_row() {
    let entries = vec![
        entry(1, "BTCUSDT", "BTC"),
        entry(1, "ETHUSDT", "ETH"),
        entry(2, "ETHUSDT", "ETH"),
        entry(2, "SOLUSDT", "SOL"),
        entry(1, "DOGEUSDT", "DOGE"),
    ];
    // Table order: SOL@2, ETH@1, DOGE@1, ETH@2, BTC@1.
    let rows = [3, 1, 4, 2, 0];

    let lists = coin_lists(&entries, &rows, (2, "ETHUSDT"));
    assert_eq!(lists.on_screen, ["SOL", "ETH", "DOGE", "BTC"]);
    assert_eq!(
        lists.through_clicked.as_deref(),
        Some(&["SOL", "ETH", "DOGE"].map(String::from)[..])
    );

    let lists = coin_lists(&entries, &rows, (1, "DOGEUSDT"));
    assert_eq!(
        lists.through_clicked.as_deref(),
        Some(&["SOL", "ETH", "DOGE"].map(String::from)[..])
    );

    let lists = coin_lists(&entries, &rows, (1, "ETHUSDT"));
    assert_eq!(
        lists.through_clicked.as_deref(),
        Some(&["SOL", "ETH"].map(String::from)[..])
    );
}

/// A row a newer publication filtered out offers no "down to this row": its line is gone, and
/// guessing one would copy a prefix the user never pointed at.
///
/// Mutation: fall back to the whole list, or to an empty prefix, when the row is missing.
#[test]
fn a_row_no_longer_listed_offers_only_the_whole_list() {
    let entries = vec![entry(1, "BTCUSDT", "BTC"), entry(1, "ETHUSDT", "ETH")];
    let lists = coin_lists(&entries, &[1], (1, "BTCUSDT"));
    assert_eq!(lists.on_screen, ["ETH"]);
    assert!(lists.through_clicked.is_none());
}

/// A grouped menu finds the lead for copy even if another member owns the chart.
#[test]
fn non_lead_chart_core_keeps_copy_through_grouped_row() {
    use super::super::{prepared::Prepared, table::MarketTarget, view::CoinQuery};
    let mut members = vec![
        entry(1, "XALPHAUSDT", "XALPHA"),
        entry(1, "XBETAUSDT", "XBETA"),
        entry(2, "XBETAUSDT", "XBETA"),
        entry(1, "XGAMMAUSDT", "XGAMMA"),
    ];
    for member in &mut members {
        member.venue.id = Some(moon_core::feed::ExchangeId::new(4));
        member.venue.label = "VENUE".into();
    }
    members[2].row.orders = 1;
    let prepared = Prepared::new(members, "market", false);
    let selection = prepared.select(
        &[Some(0), Some(1)],
        &CoinQuery::new(""),
        0.0,
        "market",
        false,
        true,
    );
    let clicked = &selection.rows[1];
    assert_eq!(clicked.chart_entry(&prepared.entries).row.core, 2);
    let target = MarketTarget::new(clicked, &prepared.entries);
    let leads: Vec<_> = selection.rows.iter().map(|row| row.lead).collect();
    let lists = coin_lists(&prepared.entries, &leads, target.copy_identity());
    assert_eq!(lists.on_screen, ["XALPHA", "XBETA", "XGAMMA"]);
    assert_eq!(lists.through_clicked.unwrap(), ["XALPHA", "XBETA"]);
}
