use super::*;
use rusqlite::Connection;

/// In-memory `orders_rep` (closedate+core_uid+profitbtc) — `unified_from`
/// строит ветку по колонкам, которые ЕСТЬ.
fn seed(rows: &[(i64, i64, f64)]) -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(
        "CREATE TABLE orders_rep(closedate INTEGER, core_uid INTEGER, profitbtc REAL);",
    )
    .unwrap();
    for (d, uid, p) in rows {
        c.execute(
            "INSERT INTO orders_rep(closedate, core_uid, profitbtc) VALUES (?1, ?2, ?3)",
            rusqlite::params![d, uid, p],
        )
        .unwrap();
    }
    c
}

// 2021-01-01 00:00:00 UTC.
const D0: i64 = 1_609_459_200;

#[test]
fn buckets_by_utc_day_fills_gaps_and_counts_wins() {
    // День0: две прибыльные (+10,+5); день1: пусто; день2: одна убыточная (−3).
    let c = seed(&[
        (D0 + 3_600, 1, 10.0),
        (D0 + 7_200, 2, 5.0),
        (D0 + 2 * 86_400 + 100, 1, -3.0),
    ]);
    let q = Query {
        from: D0,
        to: D0 + 3 * 86_400,
        ..Default::default()
    };
    let days = calendar_cells_from(&c, &q).unwrap();
    // Плотный диапазон: ровно 3 дня, включая пустой день1.
    assert_eq!(
        days.iter().map(|d| d.start).collect::<Vec<_>>(),
        vec![D0, D0 + 86_400, D0 + 2 * 86_400]
    );
    assert_eq!((days[0].trades, days[0].wins, days[0].profit), (2, 2, 15.0));
    assert_eq!((days[1].trades, days[1].profit), (0, 0.0)); // дыра заполнена
    assert_eq!((days[2].trades, days[2].wins, days[2].profit), (1, 0, -3.0));
}

#[test]
fn empty_period_is_some_empty_not_none() {
    let c = seed(&[]);
    let q = Query {
        from: D0,
        to: D0 + 86_400,
        ..Default::default()
    };
    // Схема есть, сделок нет → пустой календарь, а НЕ None и не бесконечный fill.
    assert_eq!(calendar_cells_from(&c, &q).unwrap().len(), 0);
}

#[test]
fn respects_period_bounds_excluding_to() {
    // Сделка в день3 вне [from, to) не попадает; хвостовой пустой день2 есть.
    let c = seed(&[(D0 + 100, 1, 7.0), (D0 + 3 * 86_400 + 100, 1, 99.0)]);
    let q = Query {
        from: D0,
        to: D0 + 3 * 86_400,
        ..Default::default()
    };
    let days = calendar_cells_from(&c, &q).unwrap();
    assert_eq!(days.len(), 3);
    assert_eq!(days[0].trades, 1);
    assert!(days.iter().all(|d| (d.profit - 99.0).abs() > 1e-9)); // день3 исключён
}

#[test]
fn hour_profile_buckets_by_hour_of_day_across_days() {
    // Час 1: +10 и −4 (день0) и +3 (день1) → агрегат по ЧАСУ ДНЯ обоих суток.
    // Час 22: +7 (день0). Остальные часы пусты.
    let c = seed(&[
        (D0 + 3_600 + 60, 1, 10.0),
        (D0 + 3_600 + 120, 1, -4.0),
        (D0 + 22 * 3_600, 1, 7.0),
        (D0 + 86_400 + 3_600, 1, 3.0),
    ]);
    let prof = hour_profile_one(&c, &Query::default(), D0, D0 + 3 * 86_400).unwrap();
    // Час 1 объединяет оба дня: profit 10−4+3=9, 3 сделки, 2 прибыльных.
    assert_eq!((prof[1].trades, prof[1].wins), (3, 2));
    assert!(
        (prof[1].profit - 9.0).abs() < 1e-9,
        "profit={}",
        prof[1].profit
    );
    // Час 22: одна сделка +7.
    assert_eq!((prof[22].trades, prof[22].wins), (1, 1));
    assert!((prof[22].profit - 7.0).abs() < 1e-9);
    // Час без сделок — нули (плотный массив 24).
    assert_eq!((prof[0].trades, prof[0].profit), (0, 0.0));
    // Сделка вне [from, to) в профиль не попадает: свежий период пуст.
    let empty = hour_profile_one(&c, &Query::default(), D0 - 5 * 86_400, D0 - 4 * 86_400).unwrap();
    assert!(empty.iter().all(|h| h.trades == 0));
}
