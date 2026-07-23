use super::*;

fn change(at: i64, old: &str, new: &str) -> Change {
    Change {
        at,
        old: old.to_string(),
        new: new.to_string(),
    }
}

/// An edit dates exactly the coins it ADDED.
#[test]
fn an_edit_dates_only_what_it_added() {
    let when = added_when(&[change(200, "BTC", "BTC, ETH")]);
    assert_eq!(when.get("ETH"), Some(&200));
    assert_eq!(
        when.get("BTC"),
        None,
        "a coin already there is not dated by an edit that did not touch it"
    );
}

/// A coin that was on the list before the terminal ever saw the strategy gets no exact
/// date from the history walk — only the caller's bound, and only as "no later than".
/// `added_when` itself must invent nothing.
#[test]
fn coins_older_than_the_history_stay_undated() {
    assert!(added_when(&[]).is_empty());
    // An edit that only removes leaves the survivors undated too.
    let when = added_when(&[change(200, "BTC, ETH", "BTC")]);
    assert_eq!(when.get("BTC"), None);
}

/// Removing a coin drops its date, and adding it back gives it the LATER one — a list
/// answers for its current contents, not for everything it ever held.
#[test]
fn removal_forgets_and_re_adding_re_dates() {
    let when = added_when(&[change(100, "", "BTC"), change(200, "BTC", "")]);
    assert_eq!(when.get("BTC"), None, "removed — nothing to date");

    let when = added_when(&[
        change(100, "", "BTC"),
        change(200, "BTC", ""),
        change(300, "", "BTC"),
    ]);
    assert_eq!(when.get("BTC"), Some(&300), "the later decision wins");
}

/// The list is matched on the same folded token as everywhere else, so a strategy
/// spelling a coin `mith` still dates the entry every other screen calls `MITH`.
#[test]
fn dates_are_keyed_by_the_shared_coin_token() {
    let when = added_when(&[change(100, "", "mith , btcst_1230")]);
    assert_eq!(when.get("MITH"), Some(&100));
    assert_eq!(when.get("BTCST"), Some(&100));
}

fn agg(since: Option<i64>, before: Option<i64>) -> Agg {
    Agg {
        core_name: "BB1".to_string(),
        entries: BTreeSet::new(),
        since,
        before,
    }
}

/// Newest first, and coins nothing is known about go last rather than passing for the
/// oldest ones. A BOUND sorts at its own timestamp — "no later than X" is a claim that
/// the coin is at least that old, which is exactly where it belongs chronologically.
#[test]
fn rows_are_newest_first_with_unknown_dates_last() {
    let mut map = HashMap::new();
    map.insert(("OLD".into(), 1), agg(Some(100i64), None));
    map.insert(("NEW".into(), 1), agg(Some(300), None));
    map.insert(("NONE".into(), 1), agg(None, None));
    map.insert(("BOUND".into(), 1), agg(None, Some(200)));
    let rows = flatten(map);
    let order: Vec<&str> = rows.iter().map(|r| r.coin.as_str()).collect();
    assert_eq!(order, vec!["NEW", "BOUND", "OLD", "NONE"]);
}

/// An exact date always beats the bound: keeping both would let a renderer show "no
/// later than March" for a coin known to have been added in June.
#[test]
fn an_exact_date_suppresses_the_bound() {
    let mut map = HashMap::new();
    map.insert(("A".into(), 1), agg(Some(300), Some(100)));
    map.insert(("B".into(), 1), agg(None, Some(100)));
    let rows = flatten(map);
    let a = rows.iter().find(|r| r.coin == "A").expect("A");
    assert_eq!((a.since_ms, a.before_ms), (Some(300), None));
    let b = rows.iter().find(|r| r.coin == "B").expect("B");
    assert_eq!((b.since_ms, b.before_ms), (None, Some(100)));
    assert_eq!(b.effective_ms(), Some(100), "the bound orders the row");
}

/// An empty selection yields no rows. The all-strategies fallback it replaced put a
/// thousand-row blacklist beside a coin table showing every box unticked.
#[test]
fn an_empty_selection_reads_nothing() {
    let rows = coin_lists(&[]).expect("an empty selection is not a failure");
    assert!(rows.black.is_empty());
}

/// The scope must pin each strategy to ITS core: the same id on another core is a
/// different strategy, and a list read from it would be someone else's.
#[test]
fn scope_pins_each_strategy_to_its_core() {
    let one = scope_sql(&[(5, Some(7))]);
    assert!(
        one.contains("s.strategy_id = 5") && one.contains("s.core_uid = 7"),
        "{one}"
    );
    let many = scope_sql(&[(5, Some(7)), (9, None)]);
    assert_eq!(many.matches(" OR ").count(), 1, "{many}");
    assert!(many.contains("s.strategy_id = 9"), "{many}");
}
