use super::*;

#[test]
fn parses_update_close_report() {
    let sql = "update Orders set CloseDate=1780914212, Quantity=58, SellPrice=0.5467, \
               ProfitBTC=0.0866, Lev=10, Comment='Spread: dP 6,2% (it''s fine)', \
               Status=1, SellReason='Auto Price Down' where ID=155860";
    let p = parse_report_sql(sql);
    assert_eq!(p.close_date, Some(1780914212));
    assert_eq!(p.quantity, Some(58.0));
    assert_eq!(p.sellprice, Some(0.5467));
    assert_eq!(p.lev, Some(10));
    assert_eq!(p.status, Some(1));
    assert_eq!(p.sell_reason.as_deref(), Some("Auto Price Down"));
    assert_eq!(p.comment.as_deref(), Some("Spread: dP 6,2% (it's fine)"));
    assert!(p.buydate.is_none()); // update не несёт buydate
}

#[test]
fn parses_insert_report() {
    let sql = "insert into Orders (server_id, id, coin, buydate, closedate, buyprice, \
               sellprice, profitbtc, isshort, lev, comment) values (1, 155861, 'VINEUSDT', \
               1780910000, 1780914212, 0.5, 0.55, 0.12, 1, 10, 'MoonShot, (S65)')";
    let p = parse_report_sql(sql);
    assert_eq!(p.coin.as_deref(), Some("VINEUSDT"));
    assert_eq!(p.buydate, Some(1780910000));
    assert_eq!(p.close_date, Some(1780914212));
    assert_eq!(p.buyprice, Some(0.5));
    assert_eq!(p.isshort, Some(true));
    assert_eq!(p.lev, Some(10));
    assert_eq!(p.comment.as_deref(), Some("MoonShot, (S65)"));
}

/// Универсальный passthrough: НЕзнакомые поля (MarkPriceDelta и новые дельты)
/// сами попадают в `all` с выведенным типом — без правок кода под каждое поле.
#[test]
fn unknown_fields_flow_into_all() {
    let sql = "update Orders set MarkPriceDelta=-1.234, Btc5mDelta=0.5, NewIntField=7, \
               SellReason='x' where ID=1";
    let p = parse_report_sql(sql);
    let get = |k: &str| p.all.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    assert_eq!(get("markpricedelta"), Some(Value::Real(-1.234)));
    assert_eq!(get("btc5mdelta"), Some(Value::Real(0.5)));
    assert_eq!(get("newintfield"), Some(Value::Integer(7)));
    assert_eq!(get("sellreason"), Some(Value::Text("x".to_string())));
    // `where ID=…` хвост не должен стать колонкой.
    assert!(p.all.iter().all(|(n, _)| n != "id" && !n.contains("where")));
}
