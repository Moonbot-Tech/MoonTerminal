//! Coverage for the pure halves of the ban tab: pairing rows with the bans they were built from,
//! ordering them, and the rule a tab press follows.
//!
//! `banned` itself needs a live `Backend`, so what is tested here is everything it delegates to.

// Named imports, never a glob: the module under test pulls in `gpui::*`, whose own `test` macro
// would then shadow the one this file's attributes mean.
use super::{BanSource, CoinHit, CoinTab, dedup_markets, pair_bans};
use moon_core::market::MarketLabel;
use moon_core::session::CoreId;

/// Build one labelled hit the way `hits_for` would, so a test states only what it is about.
fn hit(core: CoreId, coin: &str) -> CoinHit {
    CoinHit {
        core,
        market: format!("{coin}USDT"),
        server: format!("Core {core}"),
        label: MarketLabel {
            coin: coin.to_string(),
            canonic: String::new(),
            quote: "USDT".to_string(),
            contract: None,
        },
        venue: None,
    }
}

/// Build the listed bans exactly as `banned` collects them, allowed unless a test says otherwise.
fn sources(rows: &[(CoreId, &str, i64)]) -> Vec<BanSource> {
    rows.iter()
        .map(|(core, market, until_ms)| BanSource {
            core: *core,
            market: (*market).to_string(),
            until_ms: *until_ms,
            stale: false,
            allowed: true,
        })
        .collect()
}

/// Rows must carry the deadline of the very core they belong to, and read soonest-first.
///
/// Breakage this pins: zipping hits and bans by POSITION. `hits_for` drops a market whose core is
/// no longer a live session, so a positional join silently shifts every row below the gap onto
/// another core's clock — and offers its lift button for a ban that is not the one shown.
#[test]
fn a_row_keeps_its_own_cores_deadline_and_the_soonest_leads() {
    let hits = vec![hit(1, "BTC"), hit(2, "ETH"), hit(2, "BTC")];
    let src = sources(&[
        (1, "BTCUSDT", 3_000),
        (2, "ETHUSDT", 1_000),
        (2, "BTCUSDT", 2_000),
    ]);

    let rows = pair_bans(hits, src);

    let order: Vec<(CoreId, i64)> = rows.iter().map(|r| (r.hit.core, r.ban.until_ms)).collect();
    assert_eq!(order, vec![(2, 1_000), (2, 2_000), (1, 3_000)]);
}

/// The same coin banned on two cores stays two rows: two clocks, two lifts.
#[test]
fn one_coin_banned_on_two_cores_is_two_rows() {
    let hits = vec![hit(1, "BTC"), hit(2, "BTC")];
    let src = sources(&[(1, "BTCUSDT", 5_000), (2, "BTCUSDT", 5_000)]);

    let rows = pair_bans(hits, src);

    assert_eq!(rows.len(), 2);
    // Equal deadlines keep a stable order rather than reshuffling between repaints.
    assert_eq!(rows[0].hit.core, 1);
}

/// A hit with no listed ban is dropped, never drawn as one about to expire.
#[test]
fn a_hit_without_a_ban_is_not_a_row() {
    let hits = vec![hit(1, "BTC"), hit(1, "ETH")];
    let src = sources(&[(1, "BTCUSDT", 9_000)]);

    let rows = pair_bans(hits, src);

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].hit.market, "BTCUSDT");
}

/// Each core's own facts ride with its own row.
#[test]
fn staleness_and_authority_stay_with_their_core() {
    let hits = vec![hit(1, "BTC"), hit(2, "ETH")];
    let mut src = sources(&[(1, "BTCUSDT", 1_000), (2, "ETHUSDT", 2_000)]);
    src[1].stale = true;
    src[1].allowed = false;

    let rows = pair_bans(hits, src);

    assert!(!rows[0].ban.stale, "the live core reports");
    assert!(rows[0].ban.allowed);
    assert!(rows[1].ban.stale, "the quiet core extrapolates");
    // A row on a core this window may no longer command stays VISIBLE and loses only its press:
    // the ban is real, and hiding it would take away the reason the coin is not trading.
    assert!(!rows[1].ban.allowed);
}

/// Case is not identity here: every other reader of these symbols matches case-insensitively.
#[test]
fn a_core_that_echoes_another_case_still_pairs() {
    let hits = vec![hit(1, "BTC")];
    let src = sources(&[(1, "btcusdt", 7_000)]);

    let rows = pair_bans(hits, src);

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].ban.until_ms, 7_000);
}

/// A core that names one market twice is drawn once.
///
/// Breakage this pins: rendering the core's list verbatim. Both rows would carry the same element
/// id, which GPUI refuses inside one frame — and the list is a hand-edited string on the other side
/// of a wire, so a repeat is a shape we receive rather than one we can rule out.
#[test]
fn a_market_named_twice_is_one_row() {
    let kept = dedup_markets(vec![
        "BTCUSDT".to_string(),
        "ETHUSDT".to_string(),
        "btcusdt".to_string(),
    ]);

    assert_eq!(
        kept,
        vec!["BTCUSDT", "ETHUSDT"],
        "first spelling kept, in order"
    );
}

/// Pressing the highlighted All while a query stands must still act: that press IS how a reader
/// leaves a search.
///
/// Breakage this pins: gating the press on the tab changing. Typing forces the highlight to All, so
/// the one control that looks like the way back would be the one inert control in the strip.
#[test]
fn pressing_the_open_tab_acts_only_while_a_query_stands() {
    assert!(
        CoinTab::All.press_acts(CoinTab::All, false),
        "a query is standing"
    );
    assert!(
        !CoinTab::All.press_acts(CoinTab::All, true),
        "nothing to undo"
    );
    assert!(CoinTab::Banned.press_acts(CoinTab::All, true));
    assert!(CoinTab::Banned.press_acts(CoinTab::All, false));
}

/// The strip's captions and the click handler read the SAME array, in the same order.
#[test]
fn every_tab_has_a_caption_and_a_stable_index() {
    assert_eq!(CoinTab::ALL[0], CoinTab::default());
    let keys: Vec<&str> = CoinTab::ALL.iter().map(|tab| tab.locale_key()).collect();
    assert_eq!(
        keys,
        vec![
            "chart.coin.tab_all",
            "chart.coin.tab_favorites",
            "chart.coin.tab_banned"
        ]
    );
}
