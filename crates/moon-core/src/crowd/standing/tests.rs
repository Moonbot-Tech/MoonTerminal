use super::*;

fn stat(plus: f64, minus: f64) -> CoinMinute {
    CoinMinute {
        plus,
        minus,
        money: plus + minus,
        trades: 2,
        trades_plus: 1,
        trades_minus: 1,
    }
}

#[test]
fn a_row_carries_both_sides_of_the_minute() {
    let row = Standing::live("BTC", &stat(900.0, 400.0));
    assert_eq!(row.plus, 900.0);
    assert_eq!(row.minus, 400.0);
    assert_eq!(row.money(), 1300.0);
}

#[test]
fn the_loud_come_first() {
    let mut rows = vec![
        Standing::live("BTC", &stat(10.0, 5.0)),
        Standing::live("ETH", &stat(100.0, 5.0)),
    ];
    rows.sort_by(|a, b| louder((a.money(), &a.coin), (b.money(), &b.coin)));
    let order: Vec<&str> = rows.iter().map(|row| row.coin.as_str()).collect();
    assert_eq!(order, ["ETH", "BTC"]);
}

#[test]
fn equal_rows_keep_one_order_forever() {
    // Two coins with the same minute must not trade places between two repaints: the table is
    // rebuilt from a HashMap every tick, and a swap there is a repaint nobody asked for.
    let sort = |rows: &mut [Standing]| {
        rows.sort_by(|a, b| louder((a.money(), &a.coin), (b.money(), &b.coin)));
    };
    let mut rows = vec![
        Standing::live("ZRX", &stat(50.0, 50.0)),
        Standing::live("APE", &stat(50.0, 50.0)),
    ];
    sort(&mut rows);
    assert_eq!(rows[0].coin, "APE");
    let mut again = vec![rows[1].clone(), rows[0].clone()];
    sort(&mut again);
    assert_eq!(again, rows);
}
