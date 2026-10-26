use super::super::state::{CoinFilter, CoinLists, CoinsState, SuggestedLists, coin_token};
use super::rows_for;
use moon_core::db::analytics::GroupStat;

/// Mask-covered report rows must be ticked while the literal mask remains editable as a row.
#[test]
fn mask_membership_ticks_report_rows_and_keeps_the_literal() {
    let base = [coin("XALPHA", 3), coin("YBETA", 2)];
    let mut state = CoinsState::default();
    state.work.black.insert("XAL*".into());
    state.work.white.insert("XAL*".into());
    let model = rows_for(&mut state, &base, &base);
    let covered = model
        .rows
        .iter()
        .find(|r| r.name.as_ref() == "XALPHA")
        .unwrap();
    assert!(covered.black && covered.white);
    assert!(model.rows.iter().any(|r| r.name.as_ref() == "XAL*"));
    assert_eq!(model.counts.black, 2);
    assert_eq!(model.counts.white, 2);
}

fn coin(name: &str, n: i64) -> GroupStat {
    GroupStat {
        key: name.to_string(),
        name: name.to_string(),
        n,
        ..Default::default()
    }
}

/// The point of the cache: a repaint that changed none of the model's inputs — which is
/// every hover — must not rebuild it.
#[test]
fn cache_holds_until_an_input_changes() {
    let base = [coin("BTC_RP", 5), coin("ETH", 3)];
    let scoped = [coin("BTC_RP", 5)];
    let mut s = CoinsState::default();

    let first = rows_for(&mut s, &base, &scoped) as *const _;
    let again = rows_for(&mut s, &base, &scoped) as *const _;
    assert_eq!(first, again, "an unchanged repaint reuses the model");

    // Ticking a list is a real change — the model has to come back rebuilt.
    s.toggle(true, coin_token("BTC"));
    let after = rows_for(&mut s, &base, &scoped);
    assert!(
        after.rows.iter().any(|r| r.black),
        "the tick must reach the rebuilt rows"
    );
    assert_eq!(after.counts.black, 1, "counted once per COIN, not per row");
}

/// A coin the scope never traded still gets a row at zero when the "no trades" switch is off,
/// and the trade-count threshold hides it once raised — unless a list names it.
#[test]
fn untraded_coins_still_get_a_row() {
    let base = [coin("BTC_RP", 5), coin("DEAD", 0)];
    // ONE slice for every call: a fresh temporary would change the cache key by pointer
    // and hide a threshold that never reaches the key at all.
    let scoped = [coin("BTC_RP", 5)];
    let mut s = CoinsState {
        hide_zero: false,
        ..Default::default()
    };
    let c = rows_for(&mut s, &base, &scoped);
    let dead = c
        .rows
        .iter()
        .find(|r| r.name.as_ref() == "DEAD")
        .expect("the untraded coin must be listed with the switch off");
    assert_eq!(dead.stat.n, 0);

    // Raise the threshold and the same row goes away — that is the control working.
    s.min_trades = 1;
    let c = rows_for(&mut s, &base, &scoped);
    assert!(
        c.rows.iter().all(|r| r.name.as_ref() != "DEAD"),
        "a threshold of 1 must hide the coin with no trades"
    );
    assert!(
        c.rows.iter().any(|r| r.name.as_ref() == "BTC_RP"),
        "and must keep the one above it"
    );
    assert_eq!(c.counts.hidden, 1);
}

/// A listed coin is never hidden: with "no trades" on and N = 10, the blacklisted coin with
/// no trades stays, and only the unlisted ones count as hidden.
#[test]
fn listed_coins_survive_every_hide_rule() {
    let base = [
        coin("XAAA", 0),
        coin("XBBB", 3),
        coin("XCCC", 0),
        coin("XDDD", 12),
    ];
    let scoped = [coin("XBBB", 3), coin("XDDD", 12)];
    let mut s = CoinsState::default();
    s.adopt(CoinLists::parse("XAAA", ""));
    s.min_trades = 10;
    assert!(s.hide_zero, "on by default");
    let c = rows_for(&mut s, &base, &scoped);
    let names: Vec<&str> = c.rows.iter().map(|r| r.name.as_ref()).collect();
    assert!(
        names.contains(&"XAAA"),
        "the blacklisted coin stays: {names:?}"
    );
    assert!(names.contains(&"XDDD"));
    assert_eq!(c.counts.hidden, 2, "XBBB under N and XCCC with no trades");
    assert_eq!(c.counts.all, 4);
}

/// A token on a list that neither report read names still gets a row (0 trades, empty
/// numbers) and stays visible with "no trades" on.
#[test]
fn a_listed_token_absent_from_the_report_gets_a_row() {
    let base = [coin("XAAA", 4)];
    let scoped = [coin("XAAA", 4)];
    let mut s = CoinsState::default();
    s.adopt(CoinLists::parse("XGONE", "XWHITE"));
    let c = rows_for(&mut s, &base, &scoped);
    let gone = c
        .rows
        .iter()
        .find(|r| r.name.as_ref() == "XGONE")
        .expect("the listed token must get a row");
    assert!(gone.black && gone.stat.n == 0);
    assert!(
        c.rows
            .iter()
            .any(|r| r.name.as_ref() == "XWHITE" && r.white)
    );
    assert_eq!(c.counts.black, 1);
}

/// The Suggested view keeps every contract of a suggested token and tags it for its list.
#[test]
fn the_suggested_view_shows_every_contract_of_a_pick() {
    let base = [
        coin("XAAA_RP", 5),
        coin("XAAA", 2),
        coin("XBBB", 7),
        coin("XCCC", 3),
    ];
    let scoped = base.clone();
    let mut s = CoinsState {
        suggestion: Some(SuggestedLists {
            black: vec!["XAAA".to_string()],
            white: vec!["XCCC".to_string()],
        }),
        ..Default::default()
    };
    s.pick_rev += 1;
    s.filter = CoinFilter::Suggested;
    let c = rows_for(&mut s, &base, &scoped);
    let mut names: Vec<&str> = c.rows.iter().map(|r| r.name.as_ref()).collect();
    names.sort();
    assert_eq!(names, ["XAAA", "XAAA_RP", "XCCC"]);
    assert!(
        c.rows
            .iter()
            .all(|r| r.auto == Some(r.name.as_ref() != "XCCC"))
    );
    assert_eq!(c.counts.suggested, 2, "two tokens, three rows");
}

/// A suggested coin under the trade threshold stays visible in Suggested and is not
/// counted as hidden: the pick is about exactly that coin.
#[test]
fn a_suggested_coin_is_never_hidden() {
    let base = [coin("XAAA", 2), coin("XBBB", 1)];
    let scoped = base.clone();
    let mut s = CoinsState {
        min_trades: 5,
        suggestion: Some(SuggestedLists {
            black: vec!["XAAA".to_string()],
            white: Vec::new(),
        }),
        ..Default::default()
    };
    s.pick_rev += 1;
    s.filter = CoinFilter::Suggested;
    let c = rows_for(&mut s, &base, &scoped);
    let names: Vec<&str> = c.rows.iter().map(|r| r.name.as_ref()).collect();
    assert_eq!(names, ["XAAA"]);
    assert_eq!(c.counts.hidden, 1, "only the unsuggested XBBB is hidden");
}

/// Toggling "no trades" changes what the table shows, so it must change the cached model.
#[test]
fn toggling_no_trades_rebuilds_the_rows() {
    let base = [coin("XAAA", 0), coin("XBBB", 3)];
    let scoped = [coin("XBBB", 3)];
    let mut s = CoinsState::default();
    assert_eq!(rows_for(&mut s, &base, &scoped).rows.len(), 1);
    s.hide_zero = false;
    assert_eq!(rows_for(&mut s, &base, &scoped).rows.len(), 2);
}

/// Restoring a 300-row cut or truncating before recording `total` must fail these assertions;
/// otherwise the coin surface would disagree with the shared 500-row Tuning limit and its label.
#[test]
fn coin_result_window_keeps_five_hundred_of_the_complete_total() {
    let base: Vec<GroupStat> = (0..505)
        .map(|index| coin(&format!("COIN{index:03}"), 1))
        .collect();
    let mut state = CoinsState::default();

    let model = rows_for(&mut state, &base, &base);

    assert_eq!(model.rows.len(), 500);
    assert_eq!(model.total, 505);
}

/// The blacklist chip counts the listed coins the strategy traded and names the rest: a list
/// shared across cores lists coins this strategy never touched.
#[test]
fn the_blacklist_chip_counts_coins_this_strategy_traded() {
    let base = [coin("XAAA", 4), coin("XBBB", 3), coin("XCCC", 2)];
    let scoped = [coin("XAAA", 4), coin("XBBB", 0)];
    let mut s = CoinsState::default();
    s.adopt(CoinLists::parse("XAAA, XBBB, XCCC, XGONE", ""));
    let c = rows_for(&mut s, &base, &scoped);
    assert_eq!(c.counts.black, 4, "every listed coin still gets a row");
    assert_eq!(c.counts.black_traded, 1);
    assert_eq!(c.black_idle, ["XBBB", "XCCC", "XGONE"]);
}
