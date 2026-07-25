use super::*;
use crate::feed::news::NewsSnapshot;
use crate::session::CoreId;

/// A fixed "now" every test collects against, well past the item stamps below.
const NOW: i64 = 1_000_000;

/// Build a minimal item: id, publication time, tickers, tags.
fn item(id: &str, time_ms: i64, coins: &[&str], tags: &[&str]) -> NewsItem {
    NewsItem {
        id: id.to_string(),
        time_ms,
        coins: coins.iter().map(|c| c.to_string()).collect(),
        tags: tags.iter().map(|t| t.to_string()).collect(),
        en: format!("body {id}"),
        is_original: true,
        ..NewsItem::default()
    }
}

fn store_with(cores: &[(CoreId, Vec<NewsItem>)]) -> CoreStore {
    let mut store = CoreStore::default();
    for (id, items) in cores {
        store.ensure(*id);
        if let Some(core) = store.core_mut(*id) {
            core.news = NewsSnapshot {
                items: items.clone(),
                catalog: Vec::new(),
            };
        }
    }
    store
}

/// Collect against the fixed clock and reduce to ids, which is what every ordering/filter test
/// asserts on.
fn ids(store: &CoreStore, market: &str, settings: &NewsTagSettings) -> Vec<String> {
    collect(store, market, settings, NOW)
        .into_iter()
        .map(|(item, _)| item.id)
        .collect()
}

#[test]
fn keeps_only_the_charts_coin() {
    let store = store_with(&[(
        1,
        vec![
            item("btc", 1_000, &["BTC"], &[]),
            item("eth", 2_000, &["ETH"], &[]),
            item("both", 3_000, &["ETH", "BTC"], &[]),
        ],
    )]);
    assert_eq!(
        ids(&store, "BTCUSDT", &NewsTagSettings::default()),
        ["btc", "both"]
    );
}

#[test]
fn coin_match_ignores_case_and_contract_suffix() {
    let store = store_with(&[(1, vec![item("a", 1_000, &["btc_rp"], &[])])]);
    assert_eq!(ids(&store, "BTCUSDT", &NewsTagSettings::default()), ["a"]);
}

#[test]
fn merges_cores_by_id_preferring_the_translation() {
    let mut translated = item("dup", 1_000, &["BTC"], &[]);
    translated.is_original = false;
    translated.ru = "перевод".to_string();
    let store = store_with(&[
        (1, vec![item("dup", 1_000, &["BTC"], &[])]),
        (2, vec![translated]),
    ]);
    let marks = collect(&store, "BTCUSDT", &NewsTagSettings::default(), NOW);
    assert_eq!(marks.len(), 1, "one row per meta.id across cores");
    assert_eq!(marks[0].0.ru, "перевод");
}

#[test]
fn hidden_tag_removes_the_mark() {
    let store = store_with(&[(
        1,
        vec![
            item("hidden", 1_000, &["BTC"], &["listing"]),
            item("mixed", 2_000, &["BTC"], &["listing", "hack"]),
        ],
    )]);
    let mut settings = NewsTagSettings::default();
    settings.set_hidden("listing", true);
    // "mixed" survives: only news whose EVERY tag is hidden disappears.
    assert_eq!(ids(&store, "BTCUSDT", &settings), ["mixed"]);
}

#[test]
fn hide_untagged_removes_tagless_marks() {
    let store = store_with(&[(
        1,
        vec![
            item("bare", 1_000, &["BTC"], &[]),
            item("tagged", 2_000, &["BTC"], &["listing"]),
        ],
    )]);
    let mut settings = NewsTagSettings::default();
    settings.set_hide_untagged(true);
    assert_eq!(ids(&store, "BTCUSDT", &settings), ["tagged"]);
}

#[test]
fn mark_time_walks_the_whole_fallback_chain() {
    let mut it = item("a", 0, &["BTC"], &[]);
    assert_eq!(
        mark_time_ms(&it),
        None,
        "no usable stamp keeps it off chart"
    );
    it.recv_terminal_ms = Some(9_000);
    assert_eq!(mark_time_ms(&it), Some(9_000), "terminal receipt is last");
    it.send_time_ms = Some(8_000);
    assert_eq!(mark_time_ms(&it), Some(8_000), "service send beats it");
    it.recv_time_ms = Some(7_000);
    assert_eq!(mark_time_ms(&it), Some(7_000), "service receive beats that");
    it.time_ms = 5_000;
    assert_eq!(mark_time_ms(&it), Some(5_000), "publication wins");
}

#[test]
fn timeless_item_is_not_marked() {
    let store = store_with(&[(1, vec![item("a", 0, &["BTC"], &[])])]);
    assert!(ids(&store, "BTCUSDT", &NewsTagSettings::default()).is_empty());
}

#[test]
fn a_stamp_from_the_future_is_rejected() {
    let store = store_with(&[(
        1,
        vec![
            item("sane", NOW - 1_000, &["BTC"], &[]),
            // A rescaled stamp (seconds multiplied by 1000 twice) lands far ahead of now.
            item("rescaled", NOW * 1_000, &["BTC"], &[]),
            // Ordinary clock skew between the service and this machine still passes.
            item("skewed", NOW + 30_000, &["BTC"], &[]),
        ],
    )]);
    assert_eq!(
        ids(&store, "BTCUSDT", &NewsTagSettings::default()),
        ["sane", "skewed"]
    );
}

#[test]
fn cap_drops_the_oldest_and_keeps_chronological_order() {
    let items: Vec<NewsItem> = (0..MAX_CHART_MARKS + 5)
        .map(|i| item(&format!("n{i:03}"), 1_000 + i as i64, &["BTC"], &[]))
        .collect();
    let store = store_with(&[(1, items)]);
    let marks = collect(&store, "BTCUSDT", &NewsTagSettings::default(), NOW);
    assert_eq!(marks.len(), MAX_CHART_MARKS);
    assert_eq!(marks[0].0.id, "n005", "oldest five dropped");
    assert!(marks.windows(2).all(|w| w[0].1 <= w[1].1), "oldest first");
}

#[test]
fn signature_moves_with_news_and_with_the_tag_filter() {
    let mut store = store_with(&[(1, vec![item("a", 1_000, &["BTC"], &[])])]);
    let settings = NewsTagSettings::default();
    let base = signature(&store, &settings);
    if let Some(core) = store.core_mut(1) {
        core.news_rev += 1;
    }
    let after_news = signature(&store, &settings);
    assert_ne!(base, after_news);
    let mut settings2 = NewsTagSettings::default();
    settings2.set_hidden("listing", true);
    assert_ne!(after_news, signature(&store, &settings2));
}

#[test]
fn signature_ignores_the_order_cores_are_walked_in() {
    // `store.cores()` iterates a HashMap, so two stores holding the same cores may walk them in
    // different orders; the signature must not move because of that.
    let a = item("a", 1_000, &["BTC"], &[]);
    let mut one = store_with(&[(1, vec![a.clone()]), (2, vec![]), (7, vec![])]);
    let mut two = store_with(&[(7, vec![]), (2, vec![]), (1, vec![a])]);
    for store in [&mut one, &mut two] {
        if let Some(core) = store.core_mut(2) {
            core.news_rev = 5;
        }
    }
    assert_eq!(
        signature(&one, &NewsTagSettings::default()),
        signature(&two, &NewsTagSettings::default())
    );
}

/// Catches drawing the gem at publication instead of delivery: MoonBot marks news where the bot
/// GOT it, and the terminal must agree — a mark at publication claims the news was actionable
/// seconds before any client could have had it.
#[test]
fn a_mark_sits_at_the_moment_the_service_sent_the_news() {
    let mut it = item("a", 5_000, &["BTC"], &[]);
    it.recv_time_ms = Some(5_006);
    it.send_time_ms = Some(11_160);
    let store = store_with(&[(1, vec![it])]);

    let marks = collect(&store, "BTCUSDT", &NewsTagSettings::default(), NOW);
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].1, 11_160, "the gem must sit at the send stamp");
    // The item keeps its publication stamp, which is what the hover card shows beside the mark.
    assert_eq!(marks[0].0.time_ms, 5_000);
}

/// Catches letting the terminal's own receipt into the mark time: it is stamped by this PC's clock
/// and, for news backfilled on connect, is the moment of connect — the mark would land hours from
/// the candle it belongs to.
#[test]
fn the_terminals_own_receipt_never_places_a_mark() {
    let mut it = item("a", 0, &["BTC"], &[]);
    it.recv_terminal_ms = Some(900_000);
    let store = store_with(&[(1, vec![it])]);

    assert!(collect(&store, "BTCUSDT", &NewsTagSettings::default(), NOW).is_empty());
}

/// Catches moving the FEED's clock along with the chart's: the panel orders and counts by when the
/// news existed, and only the gem moved to delivery.
#[test]
fn the_feed_clock_still_reports_publication() {
    let mut it = item("a", 5_000, &["BTC"], &[]);
    it.send_time_ms = Some(11_160);
    assert_eq!(usable_time_ms(&it, NOW), Some(5_000));
    assert_eq!(usable_delivery_time_ms(&it, NOW), Some(11_160));
}
