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

/// Класс поля — каким Ignore-флагом стратегии выключается его фильтр.
/// `IgnoreFilters` выключает ВСЕ классы; `IgnoreDelta`/`IgnoreVolume` — свои.
/// `DeltaSlot` — дельты, у которых в стратегии НЕТ собственных параметров:
/// они подключаются через слоты Delta2/Delta3 (тип + min/max), т.е. в
/// стратегию сохраняются максимум две; игнорятся как дельты.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldClass {
    Filter,
    Delta,
    DeltaSlot,
    Volume,
}

/// Поля отчёта, доступные фильтрам: (колонка реплики, подпись как в MB/V3,
/// класс). ЕДИНСТВЕННЫЙ источник имён колонок, попадающих в SQL тюнера
/// (вайтлист). Порядок = порядок в сетке, группами по классу.
pub const FIELDS: &[(&str, &str, FieldClass)] = &[
    // Общие фильтры (только IgnoreFilters).
    ("bvsvratio", "bvsv", FieldClass::Filter),
    ("pricebug", "PriceBug", FieldClass::Filter),
    // Слоты Delta2/Delta3 (IgnoreFilters | IgnoreDelta; в стратегию — макс 2).
    ("d1h", "d1h", FieldClass::DeltaSlot),
    ("d15m", "d15m", FieldClass::DeltaSlot),
    ("d5m", "d5m", FieldClass::DeltaSlot),
    ("d1m", "d1m", FieldClass::DeltaSlot),
    ("pump1h", "Pump1H", FieldClass::DeltaSlot),
    ("dump1h", "Dump1H", FieldClass::DeltaSlot),
    // Дельты с собственными параметрами (IgnoreFilters | IgnoreDelta).
    ("d24h", "d24h", FieldClass::Delta),
    ("d3h", "d3h", FieldClass::Delta),
    ("da1m", "da1m", FieldClass::Delta),
    ("d5s", "d5s", FieldClass::Delta),
    ("vd1m", "Vd1m", FieldClass::Delta),
    ("dmark", "dMark", FieldClass::Delta),
    ("btc1hdelta", "dBTC", FieldClass::Delta),
    ("exchange1hdelta", "dMarket", FieldClass::Delta),
    ("btc24hdelta", "d24BTC", FieldClass::Delta),
    ("exchange24hdelta", "dM24", FieldClass::Delta),
    ("btc5mdelta", "dBTC5m", FieldClass::Delta),
    ("dbtc1m", "dBTC1m", FieldClass::Delta),
    // Объёмы (IgnoreFilters | IgnoreVolume).
    ("hvol", "H.Vol", FieldClass::Volume),
    ("hvolf", "H.VolF", FieldClass::Volume),
    ("dvol", "D.Vol", FieldClass::Volume),
];

/// Значения `Delta2_Type`/`Delta3_Type` ↔ поля отчёта. Типы без колонки
/// отчёта (2h/30m/Pump5m) непредставимы в тюнере и пропускаются.
pub const SLOT_TYPES: &[(&str, &str)] = &[
    ("d1h", "1h"),
    ("d15m", "15m"),
    ("d5m", "5m"),
    ("d1m", "1m"),
    ("pump1h", "Pump1h"),
    ("dump1h", "Dump1h"),
];

/// Значение `DeltaN_Type` для поля-слота (None — поле не слот).
pub fn slot_type_for(field: &str) -> Option<&'static str> {
    SLOT_TYPES.iter().find(|(f, _)| *f == field).map(|(_, t)| *t)
}

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
            if !FIELDS.iter().any(|(c, _, _)| *c == b.field) {
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
    if !FIELDS.iter().any(|(c, _, _)| *c == field) {
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

/// Маппинг «поле отчёта → параметры-фильтры стратегии MoonBot» (min, max).
/// Для полей без однозначного параметра (d1h/d15m/… — настраиваемые
/// Delta2/Delta3 окна; PriceBug/da1m/d5s/dMark/H.VolF) маппинга нет.
const STRAT_PARAMS: &[(&str, Option<&str>, Option<&str>)] = &[
    ("bvsvratio", None, Some("BV_SV_Ratio")),
    ("d24h", Some("Delta_24h_Min"), Some("Delta_24h_Max")),
    ("d3h", Some("Delta_3h_Min"), Some("Delta_3h_Max")),
    ("hvol", Some("MinHourlyVolume"), Some("MaxHourlyVolume")),
    ("dvol", Some("MinVolume"), Some("MaxVolume")),
    ("btc1hdelta", Some("Delta_BTC_Min"), Some("Delta_BTC_Max")),
    ("exchange1hdelta", Some("Delta_Market_Min"), Some("Delta_Market_Max")),
    ("btc24hdelta", Some("Delta_BTC_24_Min"), Some("Delta_BTC_24_Max")),
    ("exchange24hdelta", Some("Delta_Market_24_Min"), Some("Delta_Market_24_Max")),
    ("btc5mdelta", None, Some("Delta_BTC_5m_Max")),
    ("dbtc1m", None, Some("Delta_BTC_1m_Max")),
];

/// Параметры стратегии (min, max) для поля отчёта; (None, None) — маппинга нет.
pub fn params_for(field: &str) -> (Option<&'static str>, Option<&'static str>) {
    STRAT_PARAMS
        .iter()
        .find(|(f, _, _)| *f == field)
        .map(|(_, lo, hi)| (*lo, *hi))
        .unwrap_or((None, None))
}

/// Ядра, на которых стратегия сейчас существует (head'ы strategies.sqlite,
/// deleted=0) — адресаты сохранения порогов.
pub fn strategy_cores(strategy_id: i64) -> Vec<u64> {
    let path = crate::config::paths::strategies_db_path();
    if !path.exists() {
        return Vec::new();
    }
    let Ok(conn) =
        Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Vec::new();
    };
    conn.prepare("SELECT core_uid FROM strategies WHERE strategy_id = ?1 AND deleted = 0")
        .ok()
        .and_then(|mut st| {
            st.query_map([strategy_id], |r| r.get::<_, i64>(0))
                .ok()
                .map(|rows| rows.flatten().map(|c| c as u64).collect())
        })
        .unwrap_or_default()
}

/// Фильтровая «карточка» стратегии: Ignore-флаги + НЕдефолтные пороги по
/// полям тюнера. Флаги нужны и чипам (игнорируемый класс не показываем), и
/// сохранению порогов (перед записью включить нужные классы).
#[derive(Clone, Debug, Default)]
pub struct StratFilters {
    pub found: bool,
    pub ignore_filters: bool,
    pub ignore_delta: bool,
    pub ignore_volume: bool,
    pub bounds: std::collections::HashMap<&'static str, (Option<f64>, Option<f64>)>,
    /// Занятые слоты Delta2/Delta3: (номер 2|3, поле отчёта, min, max).
    /// Слоты с типом без колонки отчёта (2h/30m/Pump5m) не попадают.
    pub slots: Vec<(u8, &'static str, Option<f64>, Option<f64>)>,
}

impl StratFilters {
    /// Игнорируется ли класс поля текущими флагами стратегии.
    pub fn class_ignored(&self, class: FieldClass) -> bool {
        self.ignore_filters
            || match class {
                FieldClass::Filter => false,
                FieldClass::Delta | FieldClass::DeltaSlot => self.ignore_delta,
                FieldClass::Volume => self.ignore_volume,
            }
    }

    /// Слот, назначенный полю (если есть): (номер, min, max).
    pub fn slot_of(&self, field: &str) -> Option<(u8, Option<f64>, Option<f64>)> {
        self.slots
            .iter()
            .find(|(_, f, _, _)| *f == field)
            .map(|(n, _, lo, hi)| (*n, *lo, *hi))
    }
}

/// Пороговые параметры ВЫБРАННОЙ стратегии по полям тюнера.
/// Источник — текущая версия в strategies.sqlite (raw_json нормализован со
/// схемными дефолтами). `defaults` — дефолты схемы (lowercase имя → число):
/// значение, РАВНОЕ дефолту, скрывается — это «фильтр не настроен», а не
/// осознанный порог (…100T и т.п.). `found=false` — БД нет / не найдена.
pub fn strategy_filters(
    strategy_id: i64,
    defaults: &std::collections::HashMap<String, f64>,
) -> StratFilters {
    let mut out = StratFilters::default();
    let path = crate::config::paths::strategies_db_path();
    if !path.exists() {
        return out;
    }
    let Ok(conn) = Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return out;
    };
    let raw: Option<String> = conn
        .query_row(
            "SELECT v.raw_json FROM strategies s
             JOIN strategy_versions v
               ON v.core_uid = s.core_uid AND v.strategy_id = s.strategy_id
             WHERE s.strategy_id = ?1 AND s.deleted = 0 AND v.valid_to IS NULL
             ORDER BY s.checked DESC LIMIT 1",
            [strategy_id],
            |r| r.get(0),
        )
        .ok();
    let Some(raw) = raw else { return out };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return out;
    };
    let Some(map) = json.as_object() else { return out };
    let num = |key: Option<&str>| -> Option<f64> {
        let key = key?;
        let v = map.get(key)?;
        let f = match v {
            serde_json::Value::Number(n) => n.as_f64()?,
            serde_json::Value::String(s) => {
                s.trim().trim_end_matches('%').replace(',', ".").parse().ok()?
            }
            _ => return None,
        };
        if !f.is_finite() {
            return None;
        }
        // Равно дефолту схемы = фильтр не настраивали — не показываем.
        if let Some(d) = defaults.get(&key.to_ascii_lowercase()) {
            if (f - d).abs() <= f64::EPSILON.max(d.abs() * 1e-9) {
                return None;
            }
        }
        Some(f)
    };
    // Ignore-флаги: булево либо строка YES/TRUE/1.
    let truthy = |key: &str| -> bool {
        match map.get(key) {
            Some(serde_json::Value::Bool(b)) => *b,
            Some(serde_json::Value::String(s)) => {
                matches!(s.trim().to_ascii_uppercase().as_str(), "YES" | "TRUE" | "1")
            }
            Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
            _ => false,
        }
    };
    out.found = true;
    out.ignore_filters = truthy("IgnoreFilters");
    out.ignore_delta = truthy("IgnoreDelta");
    out.ignore_volume = truthy("IgnoreVolume");
    for (field, pmin, pmax) in STRAT_PARAMS {
        let (lo, hi) = (num(*pmin), num(*pmax));
        if lo.is_some() || hi.is_some() {
            out.bounds.insert(*field, (lo, hi));
        }
    }
    // Слоты Delta2/Delta3: тип строкой («15m»/«Pump1h»/…) → поле отчёта.
    for (n, prefix) in [(2u8, "Delta2"), (3u8, "Delta3")] {
        let Some(serde_json::Value::String(t)) = map.get(&format!("{prefix}_Type")) else {
            continue;
        };
        let t = t.trim();
        let Some((field, _)) = SLOT_TYPES
            .iter()
            .find(|(_, ty)| ty.eq_ignore_ascii_case(t))
        else {
            continue; // 2h/30m/Pump5m — колонки отчёта нет
        };
        let lo = num(Some(&format!("{prefix}_Min")));
        let hi = num(Some(&format!("{prefix}_Max")));
        out.slots.push((n, field, lo, hi));
    }
    out
}

/// Результат автоподбора: лучший диапазон поля.
#[derive(Clone, Debug)]
pub struct Suggestion {
    pub from: Option<f64>,
    pub to: Option<f64>,
    /// Профит периода при таком фильтре и сколько сделок остаётся.
    pub profit: f64,
    pub n: i64,
}

/// Автоподбор по ВСЕМ полям одним сканом: для каждого поля лучший диапазон
/// (перебор пар квантильных краёв, максимум профита при ≥`min_n` сделок).
/// Поля без результата (мало данных) в ответ не попадают.
pub fn suggest_all(q: &Query, min_n: i64) -> Option<Vec<(&'static str, Suggestion)>> {
    let conn = super::open_reader()?;
    let mut q = q.clone();
    if q.from < 0 {
        q.from = 1;
    }
    let src = unified_from(&conn, &q)?;
    let cols = FIELDS
        .iter()
        .map(|(c, _, _)| format!("o.\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("SELECT {cols}, COALESCE(o.profitbtc,0) FROM {src}");
    let mut per_field: Vec<Vec<(f64, f64)>> = vec![Vec::new(); FIELDS.len()];
    {
        let mut stmt = conn.prepare(&sql).ok()?;
        let mut rows = stmt.query(rusqlite::params![q.from, q.to]).ok()?;
        while let Ok(Some(r)) = rows.next() {
            let profit: f64 = r.get(FIELDS.len()).unwrap_or(0.0);
            for (fi, vals) in per_field.iter_mut().enumerate() {
                if let Ok(Some(v)) = r.get::<_, Option<f64>>(fi) {
                    if v.is_finite() {
                        vals.push((v, profit));
                    }
                }
            }
        }
    }
    let min_n = min_n.max(1) as usize;
    Some(
        per_field
            .into_iter()
            .enumerate()
            .filter_map(|(fi, mut vals)| {
                best_range(&mut vals, min_n).map(|s| (FIELDS[fi].0, s))
            })
            .collect(),
    )
}

/// Автоподбор порога ОДНОГО поля (кнопка «Подобрать» у выбранной строки).
pub fn suggest_field(q: &Query, field: &str, min_n: i64) -> Option<Suggestion> {
    if !FIELDS.iter().any(|(c, _, _)| *c == field) {
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
    let mut vals: Vec<(f64, f64)> = conn
        .prepare(&sql)
        .ok()?
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((r.get::<_, f64>(0)?, r.get::<_, f64>(1)?))
        })
        .ok()?
        .flatten()
        .filter(|(v, _)| v.is_finite())
        .collect();
    best_range(&mut vals, min_n.max(1) as usize)
}

/// Лучший диапазон одного поля по выборке `(значение, профит)`.
fn best_range(vals: &mut Vec<(f64, f64)>, min_n: usize) -> Option<Suggestion> {
    const EDGES: usize = 16;
    if vals.len() < min_n.max(1) {
        return None;
    }
    vals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let len = vals.len();
    // Префиксные суммы профита + позиции квантильных краёв.
    let mut pre = Vec::with_capacity(len + 1);
    pre.push(0.0f64);
    for (_, p) in vals.iter() {
        pre.push(pre.last().unwrap() + p);
    }
    let pos: Vec<usize> = (0..=EDGES).map(|k| k * len / EDGES).collect();
    let mut best: Option<(f64, usize, usize)> = None;
    for i in 0..EDGES {
        for j in (i + 1)..=EDGES {
            let (a, b) = (pos[i], pos[j]);
            if b - a < min_n {
                continue;
            }
            let profit = pre[b] - pre[a];
            if best.is_none_or(|(bp, _, _)| profit > bp) {
                best = Some((profit, i, j));
            }
        }
    }
    best.map(|(profit, i, j)| {
        let (a, b) = (pos[i], pos[j]);
        Suggestion {
            from: (i > 0).then(|| vals[a].0),
            to: (j < EDGES).then(|| vals[b - 1].0),
            profit,
            n: (b - a) as i64,
        }
    })
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
