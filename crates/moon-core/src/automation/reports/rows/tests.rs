//! What a trade row prints and how rows order.

use super::*;

/// Ids travel as decimal strings yet order as numbers; an empty cell goes last.
#[test]
fn cells_order_numbers_ids_and_empties() {
    use std::cmp::Ordering;
    assert_eq!(compare(&json!("9"), &json!("10")), Ordering::Less);
    assert_eq!(compare(&json!(2.5), &json!(10)), Ordering::Less);
    assert_eq!(compare(&Value::Null, &json!(1)), Ordering::Greater);
    assert_eq!(compare(&json!("ACE"), &json!("BTC")), Ordering::Less);
}

/// A strategy id above 2^63 prints as its unsigned decimal, and USDT rounds to cents.
#[test]
fn a_row_prints_ids_unsigned_and_money_rounded() {
    let mut trade = Trade::sample(1, u64::MAX, "ACE", 5, 1.23456);
    trade.profit_usdt = Some(-0.004);
    let want = Want::default();
    assert_eq!(
        cell(&trade, "strategy_id", &want),
        json!("18446744073709551615")
    );
    // Compared as text: JSON number equality treats -0.0 and 0.0 as equal.
    assert_eq!(cell(&trade, "profit_usdt", &want).to_string(), "0.0");
    assert_eq!(cell(&trade, "unknown_field", &want), Value::Null);
}

/// A trade without a USDT rate goes after the valued ones in BOTH directions: asking for the
/// biggest wins (`profit_usdt` desc) must start with the biggest win, not with unvalued trades,
/// as `compare` promises an empty cell last and `aggregate::order` keeps a missing value last.
#[test]
fn an_empty_cell_sorts_last_in_both_directions() {
    let mut unvalued = Trade::sample(1, 1, "ACE", 1_000, 0.0);
    unvalued.profit_usdt = None;
    let small = Trade::sample(1, 1, "BTC", 2_000, 1.0);
    let big = Trade::sample(1, 1, "ETH", 3_000, 5.0);
    let want = Want::default();
    for (dir, expected) in [
        (Dir::Desc, ["ETH", "BTC", "ACE"]),
        (Dir::Asc, ["BTC", "ETH", "ACE"]),
    ] {
        let mut trades = vec![&unvalued, &small, &big];
        let sort = [super::super::aggregate::Sort {
            by: "profit_usdt".into(),
            dir,
        }];
        order(&mut trades, &sort, &want);
        let coins: Vec<&str> = trades.iter().map(|t| &*t.coin).collect();
        assert_eq!(coins, expected, "{dir:?}");
    }
}
