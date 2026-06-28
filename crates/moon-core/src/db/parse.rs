//! Разбор report-SQL ядра в типизированные поля. Поддерживаются ОБЕ формы,
//! которые строит `TDBSaver.BuildCommandSql`:
//!  - `insert into Orders (col, …) values (val, …)` — первая запись строки
//!    (несёт buydate/coin/isshort/buyprice и пр.);
//!  - `update Orders set Key=Val, … where ID=db_id` — последующие изменения
//!    (close: closedate/sellprice/profitbtc/…).
//!
//! Особенности: строки в одинарных кавычках (внутри — запятые/«=»/'' как
//! экранированная кавычка); хвост `where ID=…` у update приклеен к последнему
//! значению (отбрасываем); Comment может содержать не-ASCII (ключевые слова ищем
//! ASCII-регистронезависимо по байтам — ASCII-байт не попадает в UTF-8 мультибайт).

use rusqlite::types::Value;

/// Распарсенные поля отчёта (то, что присутствует в конкретном SQL).
#[derive(Debug, Clone, Default)]
pub struct ParsedReport {
    pub coin: Option<String>,
    pub isshort: Option<bool>,
    pub buyprice: Option<f64>,
    pub sellprice: Option<f64>,
    pub quantity: Option<f64>,
    pub spent_btc: Option<f64>,
    pub gained_btc: Option<f64>,
    pub profit_btc: Option<f64>,
    pub lev: Option<i64>,
    pub status: Option<i64>,
    pub strategyid: Option<i64>,
    pub taskid: Option<i64>,
    pub buydate: Option<i64>,
    pub sellsetdate: Option<i64>,
    pub close_date: Option<i64>,
    pub sell_reason: Option<String>,
    pub comment: Option<String>,
    /// ВСЕ пары (lowercase-имя → типизированное значение) из SQL — основа
    /// универсального passthrough: writer сам заводит недостающие колонки и
    /// пишет их, не требуя кода под каждое новое поле ядра (MarkPriceDelta, …).
    /// Имена провалидированы как SQLite-идентификаторы; NULL опущены.
    pub all: Vec<(String, Value)>,
}

/// Разбирает report-SQL (insert или update) в [`ParsedReport`].
pub fn parse_report_sql(sql: &str) -> ParsedReport {
    let pairs = if is_insert(sql) {
        parse_insert(sql)
    } else {
        parse_update(sql)
    };
    let mut out = ParsedReport::default();
    for (k, v) in &pairs {
        put(&mut out, k, v);
        let key = k.trim().to_ascii_lowercase();
        if valid_ident(&key) {
            if let Some(val) = classify_value(v) {
                out.all.push((key, val));
            }
        }
    }
    out
}

/// Литерал SQL → типизированное значение SQLite. Строка в кавычках → TEXT,
/// целое → INTEGER, дробное → REAL. NULL/неизвестный bareword → None (опускаем).
fn classify_value(v: &str) -> Option<Value> {
    let t = v.trim_start();
    if t.starts_with('\'') {
        return str_val(v).map(Value::Text);
    }
    let tok = t.split_whitespace().next()?; // отсекает хвост `where ID=…`
    if tok.eq_ignore_ascii_case("null") {
        return None;
    }
    if let Ok(i) = tok.parse::<i64>() {
        return Some(Value::Integer(i));
    }
    if let Ok(f) = tok.parse::<f64>() {
        return Some(Value::Real(f));
    }
    None
}

/// Валиден ли lowercase-ключ как SQLite-идентификатор (защита от инъекции в
/// ALTER/UPDATE — имя интерполируется в SQL без кавычек): `[a-z_][a-z0-9_]*`.
fn valid_ident(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && (b[0] == b'_' || b[0].is_ascii_lowercase())
        && b.iter()
            .all(|&c| c == b'_' || c.is_ascii_lowercase() || c.is_ascii_digit())
}

fn is_insert(sql: &str) -> bool {
    find_ci_ascii(sql, "insert").map(|p| p < 8).unwrap_or(false)
}

/// `insert into Orders (c1,c2,…) values (v1,v2,…)` → пары (колонка, значение).
fn parse_insert(sql: &str) -> Vec<(String, String)> {
    let tbl = match find_ci_ascii(sql, "orders") {
        Some(p) => &sql[p..],
        None => return Vec::new(),
    };
    let Some(lp) = tbl.find('(') else {
        return Vec::new();
    };
    let Some((cols_inner, used)) = read_group(&tbl[lp..]) else {
        return Vec::new();
    };
    let rest = &tbl[lp + used..];
    let Some(vp) = find_ci_ascii(rest, "values") else {
        return Vec::new();
    };
    let after_values = &rest[vp..];
    let Some(lp2) = after_values.find('(') else {
        return Vec::new();
    };
    let Some((vals_inner, _)) = read_group(&after_values[lp2..]) else {
        return Vec::new();
    };
    let cols: Vec<String> = cols_inner
        .split(',')
        .map(|c| c.trim().to_string())
        .collect();
    let vals = split_top_level(&vals_inner);
    cols.into_iter().zip(vals).collect()
}

/// `update Orders set Key=Val, … where ID=…` → пары (ключ, значение).
fn parse_update(sql: &str) -> Vec<(String, String)> {
    let Some(set_pos) = find_ci_ascii(sql, " set ") else {
        return Vec::new();
    };
    split_top_level(&sql[set_pos + 5..])
        .into_iter()
        .filter_map(|seg| {
            let (k, v) = seg.split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

fn put(out: &mut ParsedReport, key: &str, val: &str) {
    match key.trim().to_ascii_lowercase().as_str() {
        "coin" => out.coin = str_val(val),
        "isshort" => out.isshort = num_i64(val).map(|n| n != 0),
        "buyprice" => out.buyprice = num_f64(val),
        "sellprice" => out.sellprice = num_f64(val),
        "quantity" => out.quantity = num_f64(val),
        "spentbtc" => out.spent_btc = num_f64(val),
        "gainedbtc" => out.gained_btc = num_f64(val),
        "profitbtc" => out.profit_btc = num_f64(val),
        "lev" => out.lev = num_i64(val),
        "status" => out.status = num_i64(val),
        "strategyid" => out.strategyid = num_i64(val),
        "taskid" => out.taskid = num_i64(val),
        "buydate" => out.buydate = num_i64(val),
        "sellsetdate" => out.sellsetdate = num_i64(val),
        "closedate" => out.close_date = num_i64(val),
        "sellreason" => out.sell_reason = str_val(val),
        "comment" => out.comment = str_val(val),
        _ => {}
    }
}

/// Делит «v, v, 'строка с , и =', …» на сегменты по запятым ВНЕ кавычек.
fn split_top_level(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                cur.push('\'');
                if in_str && chars.peek() == Some(&'\'') {
                    cur.push('\'');
                    chars.next();
                } else {
                    in_str = !in_str;
                }
            }
            ',' if !in_str => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// Читает группу `( … )` (s начинается с '('), уважая кавычки/вложенность.
/// Возвращает внутренность и число съеденных байт (включая закрывающую ')').
fn read_group(s: &str) -> Option<(String, usize)> {
    let mut it = s.char_indices();
    if it.next()?.1 != '(' {
        return None;
    }
    let mut depth = 1;
    let mut in_str = false;
    let mut inner = String::new();
    for (i, c) in it {
        match c {
            // '' внутри строки = два переключения = тот же in_str (паритет верный).
            '\'' => {
                in_str = !in_str;
                inner.push(c);
            }
            '(' if !in_str => {
                depth += 1;
                inner.push(c);
            }
            ')' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return Some((inner, i + c.len_utf8()));
                }
                inner.push(c);
            }
            _ => inner.push(c),
        }
    }
    None
}

/// Числовое значение: первый токен (отсекает хвост `where ID=…` и `NULL`).
fn num_f64(v: &str) -> Option<f64> {
    v.split_whitespace().next()?.parse().ok()
}
fn num_i64(v: &str) -> Option<i64> {
    let tok = v.split_whitespace().next()?;
    tok.parse::<i64>()
        .ok()
        .or_else(|| tok.parse::<f64>().ok().map(|f| f as i64))
}

/// Строковое значение в кавычках: до закрывающей кавычки, '' → ', хвост отброшен.
/// Возвращает None для не-строк (числа, NULL).
fn str_val(v: &str) -> Option<String> {
    let v = v.trim_start();
    let mut chars = v.chars();
    if chars.next()? != '\'' {
        return None;
    }
    let mut out = String::new();
    let mut it = chars.peekable();
    while let Some(c) = it.next() {
        if c == '\'' {
            if it.peek() == Some(&'\'') {
                out.push('\'');
                it.next();
            } else {
                break;
            }
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// ASCII-регистронезависимый поиск подстроки, возвращает байтовый индекс.
fn find_ci_ascii(hay: &str, needle: &str) -> Option<usize> {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| {
        n.iter()
            .enumerate()
            .all(|(j, nb)| h[i + j].eq_ignore_ascii_case(nb))
    })
}

#[cfg(test)]
mod tests {
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
}
