//! «Тюнер фильтров» окна «Аналитика» — приём из «Аналитики V3» (Excel-дашборд):
//! «что-если» по порогам рыночных полей отчёта. Считает KPI «Факт vs варианты»
//! (вариант = набор диапазонов от/до по полям входа) и гистограмму распределения
//! профита по КВАНТИЛЬНЫМ вёдрам выбранного поля (фиксированная шкала V3 нашим
//! данным не подходит: значения в процентах и с дикими выбросами, а hvol/dvol —
//! вообще объёмы). Источник — тот же UNION реплики и легаси, что у `analytics`.
//!
//! Все функции ходят в SQLite полными сканами периода — вызывать ТОЛЬКО с
//! background executor.

use rusqlite::Connection;

use super::analytics::{Query, unified_from};

/// Поля отчёта, доступные фильтрам: (колонка реплики, подпись как в MB/V3).
/// ЕДИНСТВЕННЫЙ источник имён колонок, попадающих в SQL тюнера (вайтлист).
pub const FIELDS: &[(&str, &str)] = &[
    ("bvsvratio", "bvsv"),
    ("pump1h", "Pump1H"),
    ("dump1h", "Dump1H"),
    ("d24h", "d24h"),
    ("d3h", "d3h"),
    ("d1h", "d1h"),
    ("d15m", "d15m"),
    ("d5m", "d5m"),
    ("d1m", "d1m"),
    ("vd1m", "Vd1m"),
    ("hvol", "H.Vol"),
    ("dvol", "D.Vol"),
    ("btc1hdelta", "dBTC"),
    ("exchange1hdelta", "dMarket"),
    ("btc24hdelta", "d24BTC"),
    ("btc5mdelta", "dBTC5m"),
    ("dbtc1m", "dBTC1m"),
];

/// Диапазон по одному полю; None — граница не задана.
#[derive(Clone, Debug, Default)]
pub struct Bound {
    pub field: String,
    pub from: Option<f64>,
    pub to: Option<f64>,
}

/// Вариант = набор диапазонов (пустой = «Факт», без доп. условий).
#[derive(Clone, Debug, Default)]
pub struct Variant {
    pub bounds: Vec<Bound>,
}

impl Variant {
    pub fn is_empty(&self) -> bool {
        self.bounds.iter().all(|b| b.from.is_none() && b.to.is_none())
    }

    /// Хвост WHERE варианта. Поля гейтятся вайтлистом `FIELDS`; NULL считается
    /// нулём (как в остальных фильтрах отчётов); числа — литералами (f64 из
    /// формы, инъекции невозможны).
    fn where_sql(&self) -> String {
        let mut w = String::new();
        for b in &self.bounds {
            if !FIELDS.iter().any(|(c, _)| *c == b.field) {
                continue;
            }
            if let Some(v) = b.from.filter(|v| v.is_finite()) {
                w.push_str(&format!(" AND COALESCE(o.\"{}\",0) >= {v}", b.field));
            }
            if let Some(v) = b.to.filter(|v| v.is_finite()) {
                w.push_str(&format!(" AND COALESCE(o.\"{}\",0) <= {v}", b.field));
            }
        }
        w
    }
}

/// KPI одной колонки матрицы «Факт vs v1..vN».
#[derive(Clone, Debug, Default)]
pub struct VarStats {
    pub n: i64,
    pub wins: i64,
    pub profit: f64,
    pub pf: f64,
    pub avg: f64,
    /// Средний выигрыш / средний проигрыш (по модулю).
    pub avg_win: f64,
    pub avg_loss: f64,
    /// Средний размер входа (spentbtc; у нас котировка USDT).
    pub avg_spent: f64,
    pub max_dd: f64,
}

impl VarStats {
    pub fn winrate(&self) -> f64 {
        if self.n > 0 { self.wins as f64 / self.n as f64 * 100.0 } else { 0.0 }
    }
}

/// KPI по каждому варианту (тем же индексом, что вход). Пустой вариант = «Факт».
pub fn variant_stats(q: &Query, variants: &[Variant]) -> Option<Vec<VarStats>> {
    let conn = super::open_reader()?;
    let mut q = q.clone();
    if q.from < 0 {
        q.from = 1;
    }
    let src = unified_from(&conn, &q)?;
    Some(
        variants
            .iter()
            .map(|v| one_variant(&conn, &src, &q, v))
            .collect(),
    )
}

/// Один скан варианта (порядок по closedate — для max drawdown).
fn one_variant(conn: &Connection, src: &str, q: &Query, v: &Variant) -> VarStats {
    let mut st = VarStats::default();
    let wh = v.where_sql();
    let sql = format!(
        "SELECT COALESCE(o.profitbtc,0), COALESCE(o.spentbtc,0)
         FROM {src} WHERE 1=1{wh} ORDER BY o.closedate"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return st;
    };
    let Ok(rows) = stmt.query_map(rusqlite::params![q.from, q.to], |r| {
        Ok((r.get::<_, f64>(0)?, r.get::<_, f64>(1)?))
    }) else {
        return st;
    };
    let (mut wsum, mut lsum, mut spent) = (0.0f64, 0.0f64, 0.0f64);
    let (mut cum, mut peak) = (0.0f64, 0.0f64);
    for (profit, sp) in rows.flatten() {
        st.n += 1;
        st.profit += profit;
        spent += sp;
        if profit > 0.0 {
            st.wins += 1;
            wsum += profit;
        } else {
            lsum -= profit;
        }
        cum += profit;
        peak = peak.max(cum);
        st.max_dd = st.max_dd.max(peak - cum);
    }
    if st.n > 0 {
        st.avg = st.profit / st.n as f64;
        st.avg_spent = spent / st.n as f64;
        st.avg_win = if st.wins > 0 { wsum / st.wins as f64 } else { 0.0 };
        let losses = st.n - st.wins;
        st.avg_loss = if losses > 0 { lsum / losses as f64 } else { 0.0 };
        st.pf = if lsum > 0.0 {
            wsum / lsum
        } else if wsum > 0.0 {
            99.0
        } else {
            0.0
        };
    }
    st
}

/// Ведро гистограммы: `[lo, hi)` (последнее включает hi).
#[derive(Clone, Debug)]
pub struct HistBucket {
    pub lo: f64,
    pub hi: f64,
    pub n: i64,
    pub wins: i64,
    /// Сумма выигрышей / сумма проигрышей (по модулю) ведра.
    pub wsum: f64,
    pub lsum: f64,
}

/// Гистограмма распределения сделок/профита по значению поля на входе.
/// Вёдра квантильные (≈равнонаполненные, ≤`want`); NULL-поля пропускаются.
pub fn histogram(q: &Query, field: &str, want: usize) -> Option<Vec<HistBucket>> {
    if !FIELDS.iter().any(|(c, _)| *c == field) {
        return None;
    }
    let conn = super::open_reader()?;
    let mut q = q.clone();
    if q.from < 0 {
        q.from = 1;
    }
    let src = unified_from(&conn, &q)?;
    let sql = format!(
        "SELECT o.\"{field}\", COALESCE(o.profitbtc,0)
         FROM {src} WHERE o.\"{field}\" IS NOT NULL"
    );
    let mut pairs: Vec<(f64, f64)> = conn
        .prepare(&sql)
        .ok()?
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((r.get::<_, f64>(0)?, r.get::<_, f64>(1)?))
        })
        .ok()?
        .flatten()
        .filter(|(v, _)| v.is_finite())
        .collect();
    if pairs.is_empty() {
        return Some(Vec::new());
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));

    // Квантильные края: want равнонаполненных вёдер, совпавшие края схлопываем
    // (поля с массой одинаковых значений/нулей).
    let want = want.clamp(2, 64).min(pairs.len().max(2));
    let mut edges: Vec<f64> = Vec::with_capacity(want + 1);
    for i in 0..=want {
        let idx = (i * (pairs.len() - 1)) / want;
        let e = pairs[idx].0;
        if edges.last().is_none_or(|l| *l < e) {
            edges.push(e);
        }
    }
    if edges.len() < 2 {
        // Все значения одинаковые — одно ведро.
        edges = vec![pairs[0].0, pairs[0].0];
    }

    let nb = edges.len() - 1;
    let mut out: Vec<HistBucket> = (0..nb)
        .map(|i| HistBucket {
            lo: edges[i],
            hi: edges[i + 1],
            n: 0,
            wins: 0,
            wsum: 0.0,
            lsum: 0.0,
        })
        .collect();
    let mut bi = 0usize;
    for (v, profit) in pairs {
        while bi + 1 < nb && v >= out[bi].hi {
            bi += 1;
        }
        let b = &mut out[bi];
        b.n += 1;
        if profit > 0.0 {
            b.wins += 1;
            b.wsum += profit;
        } else {
            b.lsum -= profit;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_where_whitelists_fields() {
        let v = Variant {
            bounds: vec![
                Bound { field: "d1h".into(), from: Some(1.5), to: Some(10.0) },
                Bound { field: "evil\"; DROP TABLE x;--".into(), from: Some(1.0), to: None },
                Bound { field: "hvol".into(), from: None, to: None },
            ],
        };
        let w = v.where_sql();
        assert!(w.contains("COALESCE(o.\"d1h\",0) >= 1.5"));
        assert!(w.contains("<= 10"));
        assert!(!w.contains("DROP"));
        assert!(!w.contains("hvol"), "пустые границы не добавляют условий");
    }

    #[test]
    fn empty_variant_is_fact() {
        assert!(Variant::default().is_empty());
        let v = Variant {
            bounds: vec![Bound { field: "d1h".into(), from: Some(0.0), to: None }],
        };
        assert!(!v.is_empty());
    }
}
