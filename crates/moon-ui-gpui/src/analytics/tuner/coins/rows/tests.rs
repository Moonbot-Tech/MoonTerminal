use super::super::state::{CoinLists, CoinsState, coin_token};
use super::rows_for;
use moon_core::db::analytics::GroupStat;

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

/// A coin the scope never traded still gets a row, at zero — that is what makes a
/// blacklisted coin visible at all, and what the "trades ≥" threshold must not undo at
/// its default of 0.
#[test]
fn untraded_coins_still_get_a_row() {
    let base = [coin("BTC_RP", 5), coin("DEAD", 0)];
    // ONE slice for every call: a fresh temporary would change the cache key by pointer
    // and hide a threshold that never reaches the key at all.
    let scoped = [coin("BTC_RP", 5)];
    let mut s = CoinsState::default();
    s.adopt(CoinLists::parse("DEAD", ""));
    let c = rows_for(&mut s, &base, &scoped);
    let dead = c
        .rows
        .iter()
        .find(|r| r.name.as_ref() == "DEAD")
        .expect("the blacklisted coin must be listed");
    assert_eq!(dead.stat.n, 0);
    assert!(dead.black && !dead.changed, "saved is not 'changed'");

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
}
