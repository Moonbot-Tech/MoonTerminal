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

use super::analytics::{unified_from, Query};
use super::read_fail::read_fail;
use super::{ReadFail, ReadResult};

/// Класс поля — каким Ignore-флагом стратегии выключается его фильтр.
/// `IgnoreFilters` выключает ВСЕ классы; `IgnoreDelta`/`IgnoreVolume` — свои.
/// `DeltaSlot` — дельты, у которых в стратегии НЕТ собственных параметров:
/// они подключаются через слоты Delta2/Delta3 (тип + min/max), т.е. в
/// стратегию сохраняются максимум две; игнорятся как дельты.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldClass {
    Filter,
    /// BV/SV-фильтр: свой включатель `UseBV_SV_Filter` (false = выключен)
    /// поверх общего IgnoreFilters.
    BvSv,
    /// PriceBug: секция Filters/Ping со своим `IgnorePing`.
    Ping,
    /// Секция Filters/Base со своим `IgnoreBase` (плечо, MarkPrice-дельта).
    Base,
    Delta,
    DeltaSlot,
    Volume,
}

/// Описание одного поля тюнера. ЕДИНСТВЕННОЕ место, которое правится при
/// новой колонке отчёта или новом параметре стратегии: одна строка в `FIELDS`
/// — всё остальное (SQL-проекция UNION'а, сетка UI, чипы, автоперебор,
/// сохранение в стратегию) выводится из этой таблицы. Сами колонки в
/// reports.sqlite доращиваются из схемы ядра автоматически (db/rep.rs).
pub struct FieldSpec {
    /// Колонка реплики отчётов (lowercase).
    pub col: &'static str,
    /// Подпись в сетке (как в MB/V3).
    pub label: &'static str,
    /// Класс = каким Ignore-флагом стратегии выключается фильтр.
    pub class: FieldClass,
    /// Параметры-фильтры стратегии (Min, Max); None — в стратегию не пишется.
    pub p_min: Option<&'static str>,
    pub p_max: Option<&'static str>,
    /// Значение `DeltaN_Type` для слот-полей (class == DeltaSlot): порог
    /// пишется через слот Delta2/Delta3, а не собственными параметрами.
    pub slot_type: Option<&'static str>,
}

const fn field(
    col: &'static str,
    label: &'static str,
    class: FieldClass,
    p_min: Option<&'static str>,
    p_max: Option<&'static str>,
    slot_type: Option<&'static str>,
) -> FieldSpec {
    FieldSpec {
        col,
        label,
        class,
        p_min,
        p_max,
        slot_type,
    }
}

impl FieldSpec {
    /// Есть ли способ записать порог в стратегию (параметры или слот).
    /// Немаппленные поля (da1m, d5s) в сетке помечаются и по умолчанию
    /// исключены из автоподбора.
    pub fn mapped(&self) -> bool {
        self.p_min.is_some() || self.p_max.is_some() || self.slot_type.is_some()
    }
}

impl FieldClass {
    /// Родительская секция MoonBot: BV/SV живёт внутри Filters/Volume,
    /// слоты Delta2/Delta3 — внутри Filters/Delta; прочие — сами себе.
    pub fn parent(self) -> FieldClass {
        match self {
            FieldClass::BvSv => FieldClass::Volume,
            FieldClass::DeltaSlot => FieldClass::Delta,
            other => other,
        }
    }
}

/// Поля отчёта, доступные фильтрам. ЕДИНСТВЕННЫЙ источник имён колонок,
/// попадающих в SQL тюнера (вайтлист). Порядок = порядок в сетке и повторяет
/// порядок секций Filters в MoonBot: Base → Ping → Volume (внутри — BV/SV)
/// → Delta (внутри — слоты Delta2/Delta3). Маппинг на параметры сверен
/// 2026-07-17 по union параметров всех видов стратегий. Поля БЕЗ параметров
/// и слот-типа (da1m, d5s) показываются С ПОМЕТКОЙ «нет параметра»: что-если
/// по ним считается, но записать порог в стратегию нечем. Типы слотов
/// 2h/30m/Pump5m без колонки отчёта непредставимы.
pub const FIELDS: &[FieldSpec] = &[
    // Filters/Base (IgnoreFilters | IgnoreBase): плечо и дельта mark-цены (±%).
    field(
        "lev",
        "Lev",
        FieldClass::Base,
        Some("MinLeverage"),
        Some("MaxLeverage"),
        None,
    ),
    field(
        "dmark",
        "dMark",
        FieldClass::Base,
        Some("MarkPriceMin"),
        Some("MarkPriceMax"),
        None,
    ),
    // Filters/Ping (IgnoreFilters | IgnorePing).
    field(
        "pricebug",
        "PriceBug",
        FieldClass::Ping,
        Some("BinancePriceBugMin"),
        Some("BinancePriceBug"),
        None,
    ),
    // Filters/Volume (IgnoreFilters | IgnoreVolume).
    field(
        "hvol",
        "H.Vol",
        FieldClass::Volume,
        Some("MinHourlyVolume"),
        Some("MaxHourlyVolume"),
        None,
    ),
    field(
        "hvolf",
        "H.VolF",
        FieldClass::Volume,
        Some("MinHourlyVolFast"),
        Some("MaxHourlyVolFast"),
        None,
    ),
    field(
        "dvol",
        "D.Vol",
        FieldClass::Volume,
        Some("MinVolume"),
        Some("MaxVolume"),
        None,
    ),
    field(
        "vd1m",
        "Vd1m",
        FieldClass::Volume,
        Some("MinuteVolDeltaMin"),
        Some("MinuteVolDeltaMax"),
        None,
    ),
    // BV/SV — ПОДГРУППА Volume: свой включатель UseBV_SV_Filter поверх
    // IgnoreVolume; параметры фильтра (не детектора BV_SV_Ratio!).
    field(
        "bvsvratio",
        "bvsv",
        FieldClass::BvSv,
        Some("BV_SV_FilterRatio"),
        Some("BV_SV_FilterRatioMax"),
        None,
    ),
    // Filters/Delta (IgnoreFilters | IgnoreDelta).
    field(
        "d24h",
        "d24h",
        FieldClass::Delta,
        Some("Delta_24h_Min"),
        Some("Delta_24h_Max"),
        None,
    ),
    field(
        "d3h",
        "d3h",
        FieldClass::Delta,
        Some("Delta_3h_Min"),
        Some("Delta_3h_Max"),
        None,
    ),
    field("da1m", "da1m", FieldClass::Delta, None, None, None),
    field("d5s", "d5s", FieldClass::Delta, None, None, None),
    field(
        "btc1hdelta",
        "dBTC",
        FieldClass::Delta,
        Some("Delta_BTC_Min"),
        Some("Delta_BTC_Max"),
        None,
    ),
    field(
        "exchange1hdelta",
        "dMarket",
        FieldClass::Delta,
        Some("Delta_Market_Min"),
        Some("Delta_Market_Max"),
        None,
    ),
    field(
        "btc24hdelta",
        "d24BTC",
        FieldClass::Delta,
        Some("Delta_BTC_24_Min"),
        Some("Delta_BTC_24_Max"),
        None,
    ),
    field(
        "exchange24hdelta",
        "dM24",
        FieldClass::Delta,
        Some("Delta_Market_24_Min"),
        Some("Delta_Market_24_Max"),
        None,
    ),
    field(
        "btc5mdelta",
        "dBTC5m",
        FieldClass::Delta,
        Some("Delta_BTC_5m_Min"),
        Some("Delta_BTC_5m_Max"),
        None,
    ),
    field(
        "dbtc1m",
        "dBTC1m",
        FieldClass::Delta,
        Some("Delta_BTC_1m_Min"),
        Some("Delta_BTC_1m_Max"),
        None,
    ),
    // Слоты Delta2/Delta3 — ПОДГРУППА Delta (в стратегию — макс 2).
    field("d1h", "d1h", FieldClass::DeltaSlot, None, None, Some("1h")),
    field(
        "d15m",
        "d15m",
        FieldClass::DeltaSlot,
        None,
        None,
        Some("15m"),
    ),
    field("d5m", "d5m", FieldClass::DeltaSlot, None, None, Some("5m")),
    field("d1m", "d1m", FieldClass::DeltaSlot, None, None, Some("1m")),
    field(
        "pump1h",
        "Pump1H",
        FieldClass::DeltaSlot,
        None,
        None,
        Some("Pump1h"),
    ),
    field(
        "dump1h",
        "Dump1H",
        FieldClass::DeltaSlot,
        None,
        None,
        Some("Dump1h"),
    ),
];

/// Значение `DeltaN_Type` для поля-слота (None — поле не слот).
pub fn slot_type_for(field: &str) -> Option<&'static str> {
    FIELDS
        .iter()
        .find(|s| s.col == field)
        .and_then(|s| s.slot_type)
}

/// Диапазон по одному полю; None — граница не задана.
#[derive(Clone, Debug, Default)]
pub struct Bound {
    pub field: String,
    pub from: Option<f64>,
    pub to: Option<f64>,
}

/// WorkingTime: один диапазон времени стратегии. `Day` — окно времени СУТОК
/// (минуты 0..1439, поле пишется как "чч:мм-чч:мм"); `Hour` — окно минут В КАЖДОМ
/// ЧАСЕ (0..59, поле пишется как "N-M"). Оба — единый непрерывный диапазон
/// (`от > до` = через край: конец суток/часа).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TimeWindow {
    Day(u16, u16),
    Hour(u8, u8),
}

/// Время ОТКРЫТИЯ сделки для расписания: WorkingTime/WorkingWeekTime гейтят ВХОД в
/// сделку, поэтому день/минуту берём из `buydate` (когда сделка открылась), а не из
/// `closedate` (закрытие). Fallback на `closedate`, если открытие не записано (0/NULL).
const OPEN_TS: &str = "COALESCE(NULLIF(o.buydate, 0), o.closedate)";

/// Вариант «что-если» = дополнительные условия к базовой выборке. Пустой = «Факт».
/// Ось «По фильтру» задаёт `bounds` (диапазоны полей); ось «По времени» — два
/// НЕЗАВИСИМЫХ поля стратегии, комбинируемых по И: `week_span` (WorkingWeekTime —
/// непрерывный спан по МИНУТЕ НЕДЕЛИ) и `tod` (WorkingTime — одно окно времени).
/// Все условия складываются в один WHERE — структура универсальна.
#[derive(Clone, Debug, Default)]
pub struct Variant {
    pub bounds: Vec<Bound>,
    /// WorkingWeekTime: непрерывный спан по МИНУТЕ НЕДЕЛИ `(от, до)`, минута недели
    /// = `день*1440 + минута_суток` (0..10079, день 0=Пн..6=Вс); включительно,
    /// `от > до` = через воскресенье→понедельник. `None` — без ограничения.
    pub week_span: Option<(u16, u16)>,
    /// WorkingTime: одно окно времени. `None` — без ограничения по времени.
    pub tod: Option<TimeWindow>,
}

impl Variant {
    pub fn is_empty(&self) -> bool {
        self.bounds
            .iter()
            .all(|b| b.from.is_none() && b.to.is_none())
            && self.week_span.is_none()
            && self.tod.is_none()
    }

    /// Хвост WHERE варианта. Поля гейтятся вайтлистом `FIELDS`; NULL считается
    /// нулём (как в остальных фильтрах отчётов); числа — литералами (f64 из
    /// формы, инъекции невозможны). Часы/дни/минуты — целые (тоже без инъекций).
    fn where_sql(&self) -> String {
        let mut w = String::new();
        for b in &self.bounds {
            if !FIELDS.iter().any(|s| s.col == b.field) {
                continue;
            }
            if let Some(v) = b.from.filter(|v| v.is_finite()) {
                w.push_str(&format!(" AND COALESCE(o.\"{}\",0) >= {v}", b.field));
            }
            if let Some(v) = b.to.filter(|v| v.is_finite()) {
                w.push_str(&format!(" AND COALESCE(o.\"{}\",0) <= {v}", b.field));
            }
        }
        if let Some((f, t)) = self.week_span {
            // Минута недели по времени ОТКРЫТИЯ = день*1440 + минута_суток (0..10079,
            // день 0=Пн..6=Вс); непрерывный спан, `от > до` = через воскресенье→пн.
            let wk = format!(
                "((((({OPEN_TS} / 86400) + 4) % 7 + 6) % 7) * 1440 + ({OPEN_TS} % 86400) / 60)"
            );
            let (f, t) = (f.min(10079), t.min(10079));
            if f <= t {
                w.push_str(&format!(" AND ({wk} BETWEEN {f} AND {t})"));
            } else {
                w.push_str(&format!(" AND ({wk} <= {t} OR {wk} >= {f})"));
            }
        }
        if let Some(tw) = self.tod {
            w.push_str(&time_window_where(tw));
        }
        w
    }
}

/// SQL-условие поля `WorkingTime`. `Day` — минута СУТОК `(closedate%86400)/60` в
/// окне 0..1439; `Hour` — минута В ЧАСЕ `((closedate%86400)/60)%60` в окне 0..59.
/// Окно `от <= до` → `BETWEEN`; `от > до` — через край (`<= до` ИЛИ `>= от`, иначе
/// перевёрнутое окно молча давало бы 0 сделок). Литералы целые — инъекции невозможны.
fn time_window_where(tw: TimeWindow) -> String {
    // Минута считается от ВРЕМЕНИ ОТКРЫТИЯ (расписание гейтит вход).
    let (expr, f, t, hi): (String, u16, u16, u16) = match tw {
        TimeWindow::Day(f, t) => (format!("(({OPEN_TS} % 86400) / 60)"), f, t, 1439),
        TimeWindow::Hour(f, t) => (
            format!("((({OPEN_TS} % 86400) / 60) % 60)"),
            f as u16,
            t as u16,
            59,
        ),
    };
    let (f, t) = (f.min(hi), t.min(hi));
    if f <= t {
        format!(" AND ({expr} BETWEEN {f} AND {t})")
    } else {
        format!(" AND ({expr} <= {t} OR {expr} >= {f})")
    }
}

/// Спан по минуте недели `(от, до)` (0..10079) → строка поля `WorkingWeekTime`
/// `день.чч:мм-день.чч:мм` (день 1-based 1=Пн..7=Вс). Край на границе суток пишем
/// коротко: начало дня (мин 0) → просто `день`, конец дня (мин 1439) → просто
/// `день` — т.е. `1-5` = Пн 00:00 → Пт 23:59, `1.23:44-6.22:22` — с временем.
/// Полную неделю вызывающий не пишет (== без ограничения).
pub fn format_week_span((f, t): (u16, u16)) -> String {
    let ends = |m: u16, at_end: bool| {
        let (day, tod) = ((m / 1440) % 7 + 1, m % 1440);
        // Начало дня (0) для «от» и конец дня (1439) для «до» опускаем время.
        if (!at_end && tod == 0) || (at_end && tod == 1439) {
            day.to_string()
        } else {
            format!("{day}.{}", fmt_min(tod))
        }
    };
    format!("{}-{}", ends(f, false), ends(t, true))
}

/// `TimeWindow` → строка поля `WorkingTime`. `Day` → "чч:мм-чч:мм"; `Hour` → "N-M"
/// (минуты в каждом часе — MoonBot различает по отсутствию ":").
pub fn format_working_time(tw: TimeWindow) -> String {
    match tw {
        TimeWindow::Day(f, t) => format!("{}-{}", fmt_min(f), fmt_min(t)),
        TimeWindow::Hour(f, t) => format!("{f}-{t}"),
    }
}

/// Минуты дня → "чч:мм".
fn fmt_min(m: u16) -> String {
    format!("{:02}:{:02}", m / 60, m % 60)
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
        if self.n > 0 {
            self.wins as f64 / self.n as f64 * 100.0
        } else {
            0.0
        }
    }
}

/// Compute KPI values in input order; an empty variant represents the baseline.
///
/// A healthy empty period produces one zero-valued KPI result per variant.
/// Returns `NotReady` when the replica or required schema is absent and `Failed`
/// when opening the replica, pinning the snapshot, or scanning a variant fails.
pub fn variant_stats(q: &Query, variants: &[Variant]) -> ReadResult<Vec<VarStats>> {
    let conn = super::open_reader()?;
    // One snapshot for the whole comparison. Each variant is its own scan, and
    // this table exists precisely to score variants AGAINST the baseline — a row
    // computed over a different set of trades than the baseline it is compared
    // to would still be published as one coherent comparison.
    let snap = super::read_snapshot(&conn)?;
    let mut q = q.clone();
    if q.from < 0 {
        q.from = 1;
    }
    let Some(src) = unified_from(&snap, &q)? else {
        return Err(ReadFail::NotReady);
    };
    variants
        .iter()
        .map(|v| one_variant(&snap, &src, &q, v))
        .collect()
}

/// Scan one variant in `closedate` order, failing if any metric row is unreadable.
fn one_variant(conn: &Connection, src: &str, q: &Query, v: &Variant) -> ReadResult<VarStats> {
    const CTX: &str = "tuner: one_variant";
    let mut st = VarStats::default();
    let wh = v.where_sql();
    let sql = format!(
        "SELECT COALESCE(o.profitbtc,0), COALESCE(o.spentbtc,0)
         FROM {src} WHERE 1=1{wh} ORDER BY o.closedate"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((r.get::<_, f64>(0)?, r.get::<_, f64>(1)?))
        })
        .map_err(|e| read_fail(CTX, e))?;
    let (mut wsum, mut lsum, mut spent) = (0.0f64, 0.0f64, 0.0f64);
    let (mut cum, mut peak) = (0.0f64, 0.0f64);
    for row in rows {
        // profit/spent are the whole point of the variant KPI — never skip.
        let (profit, sp) = row.map_err(|e| read_fail(CTX, e))?;
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
        st.avg_win = if st.wins > 0 {
            wsum / st.wins as f64
        } else {
            0.0
        };
        let losses = st.n - st.wins;
        st.avg_loss = if losses > 0 {
            lsum / losses as f64
        } else {
            0.0
        };
        st.pf = if lsum > 0.0 {
            wsum / lsum
        } else if wsum > 0.0 {
            99.0
        } else {
            0.0
        };
    }
    Ok(st)
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

/// Build at most `want` approximately equal-population buckets for one field.
///
/// NULL field values are excluded. An unknown field or healthy period without
/// values returns an empty vector. `NotReady` means the replica or required
/// schema is absent; `Failed` means opening or scanning it failed.
pub fn histogram(q: &Query, field: &str, want: usize) -> ReadResult<Vec<HistBucket>> {
    const CTX: &str = "tuner: histogram";
    if !FIELDS.iter().any(|s| s.col == field) {
        // Programmer error (unknown field), not a read failure.
        return Ok(Vec::new());
    }
    let conn = super::open_reader()?;
    let mut q = q.clone();
    if q.from < 0 {
        q.from = 1;
    }
    let Some(src) = unified_from(&conn, &q)? else {
        return Err(ReadFail::NotReady);
    };
    let sql = format!(
        "SELECT o.\"{field}\", COALESCE(o.profitbtc,0)
         FROM {src} WHERE o.\"{field}\" IS NOT NULL"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((r.get::<_, f64>(0)?, r.get::<_, f64>(1)?))
        })
        .map_err(|e| read_fail(CTX, e))?;
    let mut pairs: Vec<(f64, f64)> = Vec::new();
    for row in rows {
        let pair = row.map_err(|e| read_fail(CTX, e))?;
        if pair.0.is_finite() {
            pairs.push(pair);
        }
    }
    drop(stmt);
    if pairs.is_empty() {
        return Ok(Vec::new());
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
    Ok(out)
}

/// Параметры стратегии (min, max) для поля отчёта; (None, None) — маппинга нет.
pub fn params_for(field: &str) -> (Option<&'static str>, Option<&'static str>) {
    FIELDS
        .iter()
        .find(|s| s.col == field)
        .map(|s| (s.p_min, s.p_max))
        .unwrap_or((None, None))
}

/// Ядра, на которых стратегия сейчас существует (head'ы strategies.sqlite,
/// deleted=0) — адресаты сохранения порогов.
pub fn strategy_cores(strategy_id: i64) -> Vec<u64> {
    let path = crate::config::paths::strategies_db_path();
    if !path.exists() {
        return Vec::new();
    }
    let Ok(conn) = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
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
    /// Игнор секции Filters/Base (плечо, MarkPrice).
    pub ignore_base: bool,
    /// Включатель BV/SV-фильтра (`UseBV_SV_Filter`); false = выключен.
    pub use_bvsv: bool,
    /// Игнор секции Filters/Ping (PriceBug и пинги).
    pub ignore_ping: bool,
    pub bounds: std::collections::HashMap<&'static str, (Option<f64>, Option<f64>)>,
    /// Занятые слоты Delta2/Delta3: (номер 2|3, поле отчёта, min, max).
    /// Слоты с типом без колонки отчёта (2h/30m/Pump5m) не попадают.
    pub slots: Vec<(u8, &'static str, Option<f64>, Option<f64>)>,
    /// Слоты, занятые типом БЕЗ колонки отчёта (2h/30m/Pump5m) с настроенными
    /// порогами: живой фильтр, который тюнер не видит — перезаписывать такой
    /// слот сохранение должно в последнюю очередь (и с warn).
    pub foreign_slots: Vec<(u8, String)>,
    /// Текущий комментарий стратегии (поле Comment) — сохранение дописывает
    /// в него штамп анализатора, не затирая описание пользователя.
    pub comment: String,
}

impl StratFilters {
    /// Игнорируется ли класс поля текущими флагами стратегии.
    pub fn class_ignored(&self, class: FieldClass) -> bool {
        self.ignore_filters
            || match class {
                FieldClass::Filter => false,
                // BV/SV — подгруппа Filters/Volume: гейтится И IgnoreVolume,
                // И собственным включателем UseBV_SV_Filter.
                FieldClass::BvSv => self.ignore_volume || !self.use_bvsv,
                FieldClass::Ping => self.ignore_ping,
                FieldClass::Base => self.ignore_base,
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

/// Текущие значения параметров стратегии (для окна подтверждения записи:
/// «сейчас → будет»). Ключи — имена параметров из списка правок; отсутствующий
/// в raw_json ключ в ответ не попадает. Значения строками как в стратегии
/// (булевы — YES/NO). Ходит в strategies.sqlite — вызывать с bg executor.
pub fn strategy_current_values(
    strategy_id: i64,
    core: Option<u64>,
    keys: &[String],
) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let path = crate::config::paths::strategies_db_path();
    if !path.exists() {
        return out;
    }
    let Ok(conn) = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return out;
    };
    // Rows are per-core (`strategyid@core_uid`): scope to the exact core when known, so the
    // "now" preview reflects the SAME core the write targets, not the newest-checked core.
    let core_clause = if core.is_some() {
        " AND s.core_uid = ?2"
    } else {
        ""
    };
    let sql = format!(
        "SELECT v.raw_json FROM strategies s
             JOIN strategy_versions v
               ON v.core_uid = s.core_uid AND v.strategy_id = s.strategy_id
             WHERE s.strategy_id = ?1{core_clause} AND s.deleted = 0 AND v.valid_to IS NULL
             ORDER BY s.checked DESC LIMIT 1"
    );
    let raw: Option<String> = match core {
        Some(c) => conn
            .query_row(&sql, rusqlite::params![strategy_id, c as i64], |r| r.get(0))
            .ok(),
        None => conn.query_row(&sql, [strategy_id], |r| r.get(0)).ok(),
    };
    let Some(raw) = raw else { return out };
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str(&raw) else {
        return out;
    };
    for key in keys {
        let Some(v) = map.get(key) else { continue };
        let s = match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => if *b { "YES" } else { "NO" }.to_string(),
            _ => continue,
        };
        out.insert(key.clone(), s);
    }
    out
}

/// Пороговые параметры ВЫБРАННОЙ стратегии по полям тюнера.
/// Источник — текущая версия в strategies.sqlite (raw_json нормализован со
/// схемными дефолтами). `defaults` — дефолты схемы (lowercase имя → число):
/// значение, РАВНОЕ дефолту, скрывается — это «фильтр не настроен», а не
/// осознанный порог (…100T и т.п.). `found=false` — БД нет / не найдена.
pub fn strategy_filters(
    strategy_id: i64,
    core: Option<u64>,
    defaults: &std::collections::HashMap<String, f64>,
) -> StratFilters {
    let mut out = StratFilters::default();
    let path = crate::config::paths::strategies_db_path();
    if !path.exists() {
        return out;
    }
    let Ok(conn) = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return out;
    };
    // Rows are per-core: scope the strategy card (Ignore flags, thresholds) to the SELECTED
    // core so the save diff is computed against the exact core the write targets.
    let core_clause = if core.is_some() {
        " AND s.core_uid = ?2"
    } else {
        ""
    };
    let sql = format!(
        "SELECT v.raw_json FROM strategies s
             JOIN strategy_versions v
               ON v.core_uid = s.core_uid AND v.strategy_id = s.strategy_id
             WHERE s.strategy_id = ?1{core_clause} AND s.deleted = 0 AND v.valid_to IS NULL
             ORDER BY s.checked DESC LIMIT 1"
    );
    let raw: Option<String> = match core {
        Some(c) => conn
            .query_row(&sql, rusqlite::params![strategy_id, c as i64], |r| r.get(0))
            .ok(),
        None => conn.query_row(&sql, [strategy_id], |r| r.get(0)).ok(),
    };
    let Some(raw) = raw else { return out };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return out;
    };
    let Some(map) = json.as_object() else {
        return out;
    };
    let num = |key: Option<&str>| -> Option<f64> {
        let key = key?;
        let v = map.get(key)?;
        let f = match v {
            serde_json::Value::Number(n) => n.as_f64()?,
            serde_json::Value::String(s) => s
                .trim()
                .trim_end_matches('%')
                .replace(',', ".")
                .parse()
                .ok()?,
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
    out.ignore_base = truthy("IgnoreBase");
    out.comment = map
        .get("Comment")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    out.use_bvsv = truthy("UseBV_SV_Filter");
    out.ignore_ping = truthy("IgnorePing");
    for spec in FIELDS {
        let (lo, hi) = (num(spec.p_min), num(spec.p_max));
        if lo.is_some() || hi.is_some() {
            out.bounds.insert(spec.col, (lo, hi));
        }
    }
    // Слоты Delta2/Delta3: тип строкой («15m»/«Pump1h»/…) → поле отчёта.
    for (n, prefix) in [(2u8, "Delta2"), (3u8, "Delta3")] {
        let Some(serde_json::Value::String(t)) = map.get(&format!("{prefix}_Type")) else {
            continue;
        };
        let t = t.trim();
        let lo = num(Some(&format!("{prefix}_Min")));
        let hi = num(Some(&format!("{prefix}_Max")));
        let Some(field) = FIELDS
            .iter()
            .find(|s| s.slot_type.is_some_and(|ty| ty.eq_ignore_ascii_case(t)))
            .map(|s| s.col)
        else {
            // 2h/30m/Pump5m — колонки отчёта нет; с настроенными порогами это
            // живой фильтр — помечаем слот занятым «чужим» типом.
            if lo.is_some() || hi.is_some() {
                out.foreign_slots.push((n, t.to_string()));
            }
            continue;
        };
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

/// «Умное» округление границы подбора: 3 значащих цифры по разряду числа,
/// НАРУЖУ (`up=false` — вниз для «от», `up=true` — вверх для «до»), чтобы
/// округлённый диапазон не отрезал найденные сделки.
pub fn round_bound(v: f64, up: bool) -> f64 {
    if v == 0.0 || !v.is_finite() {
        return v;
    }
    let mag = v.abs().log10().floor() as i32;
    let step = 10f64.powi(mag - 2);
    let r = if up {
        (v / step).ceil()
    } else {
        (v / step).floor()
    };
    r * step
}

/// Find the best threshold range for one field.
///
/// `edges` controls the quantile search resolution. With `round`, boundaries
/// round outward unless that would stop the range from filtering data.
/// `Ok(None)` means the field is unknown, the sample is too small, or no range
/// beats the baseline.
/// `NotReady` means the replica or required schema is absent; `Failed` means
/// opening or scanning it failed.
pub fn suggest_field(
    q: &Query,
    field: &str,
    min_n: i64,
    edges: usize,
    round: bool,
) -> ReadResult<Option<Suggestion>> {
    const CTX: &str = "tuner: suggest_field";
    if !FIELDS.iter().any(|s| s.col == field) {
        // Programmer error (unknown field), not a read failure.
        return Ok(None);
    }
    let conn = super::open_reader()?;
    let mut q = q.clone();
    if q.from < 0 {
        q.from = 1;
    }
    let Some(src) = unified_from(&conn, &q)? else {
        return Err(ReadFail::NotReady);
    };
    let sql = format!(
        "SELECT o.\"{field}\", COALESCE(o.profitbtc,0)
         FROM {src} WHERE o.\"{field}\" IS NOT NULL"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((r.get::<_, f64>(0)?, r.get::<_, f64>(1)?))
        })
        .map_err(|e| read_fail(CTX, e))?;
    let mut vals: Vec<(f64, f64)> = Vec::new();
    for row in rows {
        let pair = row.map_err(|e| read_fail(CTX, e))?;
        if pair.0.is_finite() {
            vals.push(pair);
        }
    }
    drop(stmt);
    // The outer result reports read status; the inner option reports whether a
    // threshold improves on the baseline.
    Ok(best_range(&mut vals, min_n.max(1) as usize, edges, round))
}

/// Результат автоподбора оси «По времени»: два независимых поля стратегии.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TimeSuggest {
    /// WorkingWeekTime: лучший непрерывный спан по минуте недели (0..10079). `None` — без улучшения.
    pub week_span: Option<(u16, u16)>,
    /// WorkingTime: the best window in the requested format. `None` — no improvement.
    pub tod: Option<TimeWindow>,
}

/// What the "By time" sweep may search, straight from the row checkboxes: a CHECKED row is
/// searched and may be rewritten; an UNCHECKED one is never rewritten.
///
/// `day` and `hour` are two FORMATS of the single `WorkingTime` field, so they are two
/// competing candidates, not two fields: with both checked the sweep tries each and keeps
/// whichever earns more, which is why the result is at most TWO windows (a week span plus
/// one `WorkingTime` window) no matter how many boxes are ticked.
///
/// Hence pinning is per FIELD, not per row. `WorkingWeekTime` is its own field, so an
/// unchecked "Weekly" row holding a value pins the sweep inside it (`fixed_week`). For
/// `WorkingTime` a pin only exists when NEITHER format may be searched (`fixed_tod`): with
/// either box ticked the sweep owns that field and its answer REPLACES whatever is in the
/// pair — a value there is a candidate being replaced, not a constraint, since the two
/// formats cannot both be written to one field.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TimeAxes {
    /// The "Weekly" row is checked → search `WorkingWeekTime`.
    pub week: bool,
    /// The "Day" row is checked → try `WorkingTime` as `hh:mm-hh:mm` (minute of the day).
    pub day: bool,
    /// The "In hour" row is checked → try `WorkingTime` as `N-M` (minute of the hour).
    pub hour: bool,
    /// Week span pinned by an UNCHECKED "Weekly" row (`None` — the row is empty).
    pub fixed_week: Option<(u16, u16)>,
    /// `WorkingTime` window pinned when NEITHER of its two rows is checked.
    pub fixed_tod: Option<TimeWindow>,
}

impl TimeAxes {
    /// Is there anything at all to search (otherwise the whole read is pointless).
    pub fn is_empty(&self) -> bool {
        !self.week && !self.day && !self.hour
    }
}

/// Profit sweep over the schedule fields: a continuous span of days (`WorkingWeekTime`)
/// and one time window (`WorkingTime`). What to search is dictated by `axes` (the row
/// checkboxes); with both `WorkingTime` formats checked the sweep tries each and keeps the
/// better one, and a row that is unchecked but already holds a value pins the sweep inside
/// it. The two fields are independent; each is `None` when the best candidate does not
/// improve on the unrestricted sample. UTC weekday =
/// `(((closedate/86400)+4)%7+6)%7` (0=Mon..6=Sun), minute = `(cd%86400)/60`.
pub fn suggest_time(
    q: &Query,
    min_n: i64,
    edges: usize,
    round: bool,
    axes: TimeAxes,
) -> ReadResult<TimeSuggest> {
    const CTX: &str = "tuner: suggest_time";
    // Nothing is checked → nothing to search; skip the scan instead of reading the whole
    // period only to throw it away.
    if axes.is_empty() {
        return Ok(TimeSuggest::default());
    }
    let conn = super::open_reader()?;
    let mut q = q.clone();
    if q.from < 0 {
        q.from = 1;
    }
    let Some(src) = unified_from(&conn, &q)? else {
        return Err(ReadFail::NotReady);
    };
    let sql = format!(
        "SELECT ((({OPEN_TS} / 86400) + 4) % 7 + 6) % 7 AS wd,
                ({OPEN_TS} % 86400) / 60 AS mn,
                COALESCE(o.profitbtc, 0)
         FROM {src}"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, f64>(2)?,
            ))
        })
        .map_err(|e| read_fail(CTX, e))?;
    let mut trades: Vec<(i64, i64, f64)> = Vec::new();
    for row in rows {
        trades.push(row.map_err(|e| read_fail(CTX, e))?);
    }
    Ok(time_suggest_from_rows(&trades, min_n, edges, round, axes))
}

/// Core of `suggest_time` over ready rows `(weekday, minute, profit)` — the entry point
/// for unit tests (no DB). Only the axes opened by `axes` are searched; rows cut off by
/// the pinned windows (`fixed_*`) are dropped BEFORE the sweep, so the free axis is
/// optimized inside them. The week and time axes are searched INDEPENDENTLY (each the
/// best window of its own projection) but applied with AND. Their intersection can yield
/// LESS profit than the baseline (each drops different winning trades), so we score every
/// combination on the real rows and take the maximum — the baseline is a candidate too,
/// which is why the result is NEVER worse than it.
///
/// NB: with a `fixed_*` window the baseline is the PINNED subset, not the whole sample —
/// the pin is a constraint the sweep may not lift, so "never worse" is measured against
/// what the user fixed. Without pins the baseline is the full sample, i.e. the "Fact"
/// column the UI shows.
fn time_suggest_from_rows(
    rows: &[(i64, i64, f64)],
    min_n: i64,
    edges: usize,
    round: bool,
    axes: TimeAxes,
) -> TimeSuggest {
    if axes.is_empty() {
        return TimeSuggest::default();
    }
    let min_n = min_n.max(1) as usize;
    // Is a trade inside the mask (bounds inclusive; `from > to` = wraps past the edge).
    let span_ok = |v: i64, f: i64, t: i64| {
        if f <= t {
            f <= v && v <= t
        } else {
            v <= t || v >= f
        }
    };
    let in_tod = |mn: i64, tw: TimeWindow| match tw {
        TimeWindow::Day(f, t) => span_ok(mn, f as i64, t as i64),
        TimeWindow::Hour(f, t) => span_ok(mn % 60, f as i64, t as i64),
    };
    // An unchecked row with a value pins the sweep: drop everything outside it, so both the
    // candidates and the baseline below are measured on the subset the user fixed. Nothing
    // pinned → work on the original slice (no copy).
    let pinned: Option<Vec<(i64, i64, f64)>> =
        (axes.fixed_week.is_some() || axes.fixed_tod.is_some()).then(|| {
            rows.iter()
                .copied()
                .filter(|&(wd, mn, _)| {
                    axes.fixed_week
                        .is_none_or(|(f, t)| span_ok(wd * 1440 + mn, f as i64, t as i64))
                        && axes.fixed_tod.is_none_or(|tw| in_tod(mn, tw))
                })
                .collect()
        });
    let rows: &[(i64, i64, f64)] = pinned.as_deref().unwrap_or(rows);
    // Candidates for the opened axes (best window of the projection; None = unrestricted).
    let week = axes
        .week
        .then(|| best_week_span(rows, min_n, edges, round))
        .flatten();
    // The two WorkingTime formats are separate CANDIDATES for one field: each checked row
    // contributes one, and the comparison below keeps whichever earns more. Unchecked → no
    // candidate, so that format can never come out of the sweep.
    let day_w = axes
        .day
        .then(|| {
            let mut vals: Vec<(f64, f64)> =
                rows.iter().map(|&(_w, mn, p)| (mn as f64, p)).collect();
            best_range(&mut vals, min_n, edges, round).map(|s| {
                TimeWindow::Day(
                    s.from.unwrap_or(0.0).clamp(0.0, 1439.0) as u16,
                    s.to.unwrap_or(1439.0).clamp(0.0, 1439.0) as u16,
                )
            })
        })
        .flatten();
    let hour_w = axes
        .hour
        .then(|| {
            let mut vals: Vec<(f64, f64)> = rows
                .iter()
                .map(|&(_w, mn, p)| ((mn % 60) as f64, p))
                .collect();
            best_range(&mut vals, min_n, edges, round).map(|s| {
                TimeWindow::Hour(
                    s.from.unwrap_or(0.0).clamp(0.0, 59.0) as u8,
                    s.to.unwrap_or(59.0).clamp(0.0, 59.0) as u8,
                )
            })
        })
        .flatten();
    let prof = |wk: Option<(u16, u16)>, td: Option<TimeWindow>| -> f64 {
        rows.iter()
            .filter(|&&(wd, mn, _)| {
                wk.is_none_or(|(f, t)| span_ok(wd * 1440 + mn, f as i64, t as i64))
                    && td.is_none_or(|tw| in_tod(mn, tw))
            })
            .map(|r| r.2)
            .sum()
    };
    // The baseline (no window of our own, but already inside any pin) is the starting
    // candidate; every other one has to BEAT it.
    let base: f64 = rows.iter().map(|r| r.2).sum();
    let margin = base.abs().max(1.0) * 1e-9;
    let mut best: (Option<(u16, u16)>, Option<TimeWindow>, f64) = (None, None, base);
    for wk in [None, week] {
        for td in [None, day_w, hour_w] {
            if wk.is_none() && td.is_none() {
                continue;
            }
            let pr = prof(wk, td);
            if pr > best.2 + margin {
                best = (wk, td, pr);
            }
        }
    }
    TimeSuggest {
        week_span: best.0,
        tod: best.1,
    }
}

/// Лучшее НЕПРЕРЫВНОЕ окно по минуте недели (0..10079 = день*1440 + минута_суток)
/// по профиту, через квантильный `best_range`. `None` — если лучшее окно не
/// улучшает полную неделю. Линейный поиск (`от <= до`) — окно ЧЕРЕЗ воскресенье
/// автоподбором не предлагается (ручной ползунок это может).
fn best_week_span(
    rows: &[(i64, i64, f64)],
    min_n: usize,
    edges: usize,
    round: bool,
) -> Option<(u16, u16)> {
    let mut vals: Vec<(f64, f64)> = rows
        .iter()
        .filter(|(wd, ..)| (0..7).contains(wd))
        .map(|&(wd, mn, p)| ((wd * 1440 + mn) as f64, p))
        .collect();
    best_range(&mut vals, min_n, edges, round).map(|s| {
        (
            s.from.unwrap_or(0.0).clamp(0.0, 10079.0) as u16,
            s.to.unwrap_or(10079.0).clamp(0.0, 10079.0) as u16,
        )
    })
}

/// Профили среднего профита для раскраски трёх ползунков «По времени»:
/// `week[168]` (день×час, 0=Пн·00ч), `day[1440]` (минута суток), `hour[60]`
/// (минута в часе). Значение ячейки = средний профит сделки в ней (0.0 если пусто).
#[derive(Clone, Copy, Debug)]
pub struct SliderProfiles {
    pub week: [f32; 168],
    pub day: [f32; 1440],
    pub hour: [f32; 60],
}

/// Посчитать `SliderProfiles` по тому же скоупу, что и автоподбор (`tuner_query`).
pub fn slider_profiles(q: &Query) -> ReadResult<SliderProfiles> {
    const CTX: &str = "tuner: slider_profiles";
    let conn = super::open_reader()?;
    let mut q = q.clone();
    if q.from < 0 {
        q.from = 1;
    }
    let Some(src) = unified_from(&conn, &q)? else {
        return Err(ReadFail::NotReady);
    };
    let sql = format!(
        "SELECT ((({OPEN_TS} / 86400) + 4) % 7 + 6) % 7 AS wd,
                ({OPEN_TS} % 86400) / 60 AS mn,
                COALESCE(o.profitbtc, 0)
         FROM {src}"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, f64>(2)?,
            ))
        })
        .map_err(|e| read_fail(CTX, e))?;
    let mut trades: Vec<(i64, i64, f64)> = Vec::new();
    for row in rows {
        trades.push(row.map_err(|e| read_fail(CTX, e))?);
    }
    Ok(slider_profiles_from_rows(&trades))
}

/// Ядро `slider_profiles` над строками `(день, минута_суток, профит)` — для тестов.
fn slider_profiles_from_rows(rows: &[(i64, i64, f64)]) -> SliderProfiles {
    let (mut ws, mut wc) = ([0f64; 168], [0u32; 168]);
    let (mut ds, mut dc) = (vec![0f64; 1440], vec![0u32; 1440]);
    let (mut hs, mut hc) = ([0f64; 60], [0u32; 60]);
    for &(wd, mn, p) in rows {
        if !(0..7).contains(&wd) || !(0..1440).contains(&mn) {
            continue;
        }
        let (mi, h, moh) = (mn as usize, (mn / 60) as usize, (mn % 60) as usize);
        let wk = wd as usize * 24 + h;
        ws[wk] += p;
        wc[wk] += 1;
        ds[mi] += p; // минута суток 0..1439
        dc[mi] += 1;
        hs[moh] += p;
        hc[moh] += 1;
    }
    let avg = |s: f64, c: u32| if c > 0 { (s / c as f64) as f32 } else { 0.0 };
    SliderProfiles {
        week: std::array::from_fn(|i| avg(ws[i], wc[i])),
        day: std::array::from_fn(|i| avg(ds[i], dc[i])),
        hour: std::array::from_fn(|i| avg(hs[i], hc[i])),
    }
}

/// Лучший диапазон одного поля по выборке `(значение, профит)`.
fn best_range(
    vals: &mut Vec<(f64, f64)>,
    min_n: usize,
    edges: usize,
    round: bool,
) -> Option<Suggestion> {
    if vals.len() < min_n.max(1) {
        return None;
    }
    vals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let len = vals.len();
    // Корзин не больше, чем точек данных (лишние — избыточны). Верхний потолок 512
    // поднят со 128 ради оси времени: при малом числе сделок на день это даёт
    // нарезку «по каждой сделке» = максимальная точность окна. На РЕЗУЛЬТАТ фильтра
    // `min(len)` не влияет: при edges ≥ len позиции `k*len/edges` покрывают ровно
    // {0..len} — тот же набор различимых границ, что при edges=len, лишь без
    // дублирующих итераций (у фильтра данных обычно тысячи → ветка и не срабатывает).
    let edges = edges.clamp(4, 512).min(len.max(4));
    // Префиксные суммы профита + позиции квантильных краёв.
    let mut pre = Vec::with_capacity(len + 1);
    pre.push(0.0f64);
    for (_, p) in vals.iter() {
        pre.push(pre.last().unwrap() + p);
    }
    let pos: Vec<usize> = (0..=edges).map(|k| k * len / edges).collect();
    let total = pre[len];
    let mut best: Option<(f64, usize, usize)> = None;
    for i in 0..edges {
        for j in (i + 1)..=edges {
            let (a, b) = (pos[i], pos[j]);
            if b - a < min_n {
                continue;
            }
            // Полное покрытие данных (min..max) — фильтр-пустышка, не кандидат.
            if a == 0 && b == len {
                continue;
            }
            let profit = pre[b] - pre[a];
            if best.is_none_or(|(bp, _, _)| profit > bp) {
                best = Some((profit, i, j));
            }
        }
    }
    // Диапазон предлагаем только если он РЕАЛЬНО улучшает профит против
    // «без фильтра» — больше, чем на float-шум суммирования (иначе диапазон,
    // отрезающий только нулевые сделки, «выигрывает» на ~1e-12).
    let margin = total.abs().max(1.0) * 1e-9;
    best.filter(|(profit, _, _)| *profit > total + margin)
        .map(|(profit, i, j)| {
            let (a, b) = (pos[i], pos[j]);
            // Always return both bounds; distribution edges use the observed
            // minimum or maximum rather than an open interval.
            let (mut from, mut to) = (vals[a].0, vals[b - 1].0);
            if round {
                let (rf, rt) = (round_bound(from, false), round_bound(to, true));
                // Outward rounding can move both distribution-edge bounds past
                // the observed range and make the pair filter nothing. Preserve
                // raw bounds in that case.
                if rf > vals[0].0 || rt < vals[len - 1].0 {
                    (from, to) = (rf, rt);
                }
            }
            Suggestion {
                from: Some(from),
                to: Some(to),
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
                Bound {
                    field: "d1h".into(),
                    from: Some(1.5),
                    to: Some(10.0),
                },
                Bound {
                    field: "evil\"; DROP TABLE x;--".into(),
                    from: Some(1.0),
                    to: None,
                },
                Bound {
                    field: "hvol".into(),
                    from: None,
                    to: None,
                },
            ],
            ..Default::default()
        };
        let w = v.where_sql();
        assert!(w.contains("COALESCE(o.\"d1h\",0) >= 1.5"));
        assert!(w.contains("<= 10"));
        assert!(!w.contains("DROP"));
        assert!(!w.contains("hvol"), "пустые границы не добавляют условий");
    }

    #[test]
    fn variant_week_span_predicate() {
        // Пн 00:00 → Сб 23:59 (мин недели 0..8639): непрерывный → BETWEEN, исключает Вс.
        let v = Variant {
            week_span: Some((0, 8639)),
            ..Default::default()
        };
        let w = v.where_sql();
        // По времени ОТКРЫТИЯ (buydate), не closedate.
        assert!(
            w.contains("BETWEEN 0 AND 8639") && w.contains("buydate"),
            "w={w}"
        );
        assert!(!v.is_empty(), "week_span-вариант не равен «Факту»");

        // Через воскресенье→понедельник (от > до): Сб 12:00 (8640-720=7920) → Пн 12:00 (720).
        let v = Variant {
            week_span: Some((7920, 720)),
            ..Default::default()
        };
        let w = v.where_sql();
        assert!(
            w.contains("<= 720 OR"),
            "через воскресенье — до Пн 12:00: {w}"
        );
        assert!(
            w.contains(">= 7920"),
            "через воскресенье — от Сб 12:00: {w}"
        );
        assert!(!w.contains("BETWEEN"), "перевёрнутое окно не BETWEEN: {w}");
    }

    #[test]
    fn variant_time_window_predicate() {
        // WorkingTime «Day» 09:00–21:00 → минута суток BETWEEN.
        let v = Variant {
            tod: Some(TimeWindow::Day(9 * 60, 21 * 60)),
            ..Default::default()
        };
        let w = v.where_sql();
        assert!(
            w.contains("BETWEEN 540 AND 1260") && w.contains("buydate"),
            "w={w}"
        );
        assert!(!v.is_empty());

        // «Day» через полночь (22:00–06:00) → «<= 360 OR >= 1320».
        let v = Variant {
            tod: Some(TimeWindow::Day(22 * 60, 6 * 60)),
            ..Default::default()
        };
        let w = v.where_sql();
        assert!(w.contains("<= 360 OR"), "через полночь до 06:00: {w}");
        assert!(w.contains(">= 1320"), "через полночь от 22:00: {w}");

        // WorkingTime «Hour» 1–50 → минута В ЧАСЕ (mod 60) BETWEEN.
        let v = Variant {
            tod: Some(TimeWindow::Hour(1, 50)),
            ..Default::default()
        };
        let w = v.where_sql();
        assert!(w.contains("% 60) BETWEEN 1 AND 50"), "w={w}");

        // week_span И tod складываются в один WHERE (обе оси).
        let v = Variant {
            week_span: Some((0, 8639)),
            tod: Some(TimeWindow::Day(1, 1430)),
            ..Default::default()
        };
        let w = v.where_sql();
        assert!(w.contains("BETWEEN 0 AND 8639"));
        assert!(w.contains("BETWEEN 1 AND 1430"));
    }

    #[test]
    fn working_time_format() {
        // Минута недели: Пн 00:00 (0) → Сб 23:59 (8639) — края суток пишем коротко → «1-6».
        assert_eq!(format_week_span((0, 8639)), "1-6");
        // С временем: Пн 23:44 (1424) → Сб 22:22 (5*1440+1342=8542) → «1.23:44-6.22:22».
        assert_eq!(format_week_span((1424, 8542)), "1.23:44-6.22:22");
        // WorkingTime Day → «чч:мм-чч:мм»; Hour → «N-M».
        assert_eq!(
            format_working_time(TimeWindow::Day(1, 23 * 60 + 50)),
            "00:01-23:50"
        );
        assert_eq!(format_working_time(TimeWindow::Hour(1, 50)), "1-50");
    }

    /// Opened axes with nothing pinned — the plain "search these" case.
    fn axes_of(week: bool, day: bool, hour: bool) -> TimeAxes {
        TimeAxes {
            week,
            day,
            hour,
            ..Default::default()
        }
    }

    /// Minute 0..14 of EVERY hour loses. Separable by the minute-of-hour projection;
    /// over the minute of the DAY the losses are scattered across the whole range.
    fn rows_bad_first_minutes() -> Vec<(i64, i64, f64)> {
        (0..24)
            .flat_map(|h| (0..60).map(move |m| (0i64, h * 60 + m, if m < 15 { -1.0 } else { 1.0 })))
            .collect()
    }

    #[test]
    fn suggest_time_week_span_and_mode() {
        // Sun (wd6) loses, Mon..Sat win → the best week window cuts Sunday off.
        let mut rows: Vec<(i64, i64, f64)> = Vec::new();
        for wd in 0..6 {
            rows.extend((0..30).map(|i| (wd, i * 40, 1.0)));
        }
        rows.extend((0..30).map(|i| (6i64, i * 40, -1.0))); // Sunday in the red
        let s = time_suggest_from_rows(&rows, 1, 64, false, axes_of(true, false, false));
        let (_, to) = s.week_span.expect("a week window must be found");
        assert!(
            to < 6 * 1440,
            "the window must end before Sunday (minute of week < 8640), got {to}"
        );

        // Every box ticked — the "just sweep everything" case: the two WorkingTime formats
        // compete and the sweep keeps the one that actually separates the loss (Hour).
        let rows = rows_bad_first_minutes();
        let s = time_suggest_from_rows(&rows, 1, 32, false, axes_of(true, true, true));
        match s.tod {
            Some(TimeWindow::Hour(f, _t)) => {
                assert!(f > 0, "the Hour window must cut minutes 0..14, from={f}")
            }
            other => panic!("with both formats offered the better (Hour) must win, got {other:?}"),
        }
    }

    #[test]
    fn suggest_time_respects_axes() {
        let rows = rows_bad_first_minutes();

        // No row checked → no sweep at all (and no reason to read the DB).
        let none = time_suggest_from_rows(&rows, 1, 32, false, axes_of(false, false, false));
        assert_eq!(none, TimeSuggest::default(), "no checkbox, no sweep");

        // The gate that matters: on the SAME data "In hour" checked yields an Hour window,
        // while with only "Day" checked that window may not appear — an unchecked format is
        // never produced, however profitable it would be.
        let hour = time_suggest_from_rows(&rows, 1, 32, false, axes_of(false, false, true));
        assert!(
            matches!(hour.tod, Some(TimeWindow::Hour(..))),
            "the checked Hour format must be searched, got {:?}",
            hour.tod
        );
        let day = time_suggest_from_rows(&rows, 1, 32, false, axes_of(false, true, false));
        assert!(
            !matches!(day.tod, Some(TimeWindow::Hour(..))),
            "an unchecked Hour format may not come out of the sweep, got {:?}",
            day.tod
        );

        // "Weekly" unchecked → its field stays untouched even when cutting it clearly pays.
        let mut wk_rows: Vec<(i64, i64, f64)> = Vec::new();
        for wd in 0..6 {
            wk_rows.extend((0..30).map(|i| (wd, i * 40, 1.0)));
        }
        wk_rows.extend((0..30).map(|i| (6i64, i * 40, -1.0)));
        let s = time_suggest_from_rows(&wk_rows, 1, 64, false, axes_of(false, true, true));
        assert_eq!(s.week_span, None, "an unchecked week is not searched");
    }

    #[test]
    fn suggest_time_pins_unchecked_row() {
        // Every day is profitable overall, but inside HOUR 0 Sunday alone loses. This is
        // the reachable UI state: "Weekly" checked while neither WorkingTime row is, so
        // the WorkingTime window already in the grid pins the sweep.
        let rows: Vec<(i64, i64, f64)> = (0..7i64)
            .flat_map(|wd| {
                (0..24i64).map(move |h| {
                    let p = match (h, wd) {
                        (0, 6) => -5.0, // Sunday, hour 0 — the only loss
                        (0, _) => 1.0,
                        (_, 6) => 3.0, // Sunday is the best day everywhere else
                        _ => 1.0,
                    };
                    (wd, h * 60, p)
                })
            })
            .collect();

        // Unpinned: every day is in the black, so no week window improves on the whole week.
        let free = time_suggest_from_rows(&rows, 1, 64, false, axes_of(true, false, false));
        assert_eq!(
            free.week_span, None,
            "over the full sample no week window beats the baseline"
        );

        // Pinned to hour 0 — the sweep must work INSIDE it, where Sunday is the loss.
        let pinned = time_suggest_from_rows(
            &rows,
            1,
            64,
            false,
            TimeAxes {
                week: true,
                day: false,
                hour: false,
                fixed_week: None,
                fixed_tod: Some(TimeWindow::Day(0, 59)),
            },
        );
        let (_, to) = pinned
            .week_span
            .expect("inside the pinned hour, cutting Sunday pays");
        assert!(to < 6 * 1440, "the window must end before Sunday, got {to}");
    }

    #[test]
    fn suggest_time_never_worse_than_base() {
        // Оси недели и времени по отдельности «улучшают», но их пересечение могло бы
        // выкинуть разные плюсовые сделки → профит НИЖЕ базового. Проверяем, что
        // подбор этого не допускает (база — кандидат).
        let mut rows: Vec<(i64, i64, f64)> = Vec::new();
        for wd in 0..6i64 {
            for mn in (0..1440).step_by(20) {
                rows.push((wd, mn, if mn / 60 == 3 { -2.0 } else { 1.0 })); // час 3 в минусе
            }
        }
        for mn in (0..1440).step_by(20) {
            rows.push((6, mn, -1.0)); // Вс в минусе
        }
        let base: f64 = rows.iter().map(|r| r.2).sum();
        let s = time_suggest_from_rows(&rows, 1, 64, false, axes_of(true, true, true));
        let span_ok = |v: i64, f: i64, t: i64| {
            if f <= t {
                f <= v && v <= t
            } else {
                v <= t || v >= f
            }
        };
        let got: f64 = rows
            .iter()
            .filter(|&&(wd, mn, _)| {
                s.week_span
                    .map_or(true, |(f, t)| span_ok(wd * 1440 + mn, f as i64, t as i64))
                    && s.tod.map_or(true, |tw| match tw {
                        TimeWindow::Day(f, t) => span_ok(mn, f as i64, t as i64),
                        TimeWindow::Hour(f, t) => span_ok(mn % 60, f as i64, t as i64),
                    })
            })
            .map(|r| r.2)
            .sum();
        assert!(
            got >= base,
            "подбор не должен быть хуже базы: got={got} base={base}"
        );
    }

    #[test]
    fn slider_profiles_bucketize() {
        // Пн (wd0) 00:00 +2; Вт (wd1) 05:30 -4 → раскладка по трём осям.
        let rows = vec![(0i64, 0i64, 2.0), (1i64, 5 * 60 + 30, -4.0)];
        let p = slider_profiles_from_rows(&rows);
        assert_eq!(p.week[0], 2.0, "неделя: Пн·00ч (wd0*24+0)");
        assert_eq!(p.week[24 + 5], -4.0, "неделя: Вт·05ч (wd1*24+5)");
        assert_eq!(p.day[0], 2.0, "сутки: минута 0");
        assert_eq!(p.day[5 * 60 + 30], -4.0, "сутки: минута 330 (05:30)");
        assert_eq!(p.hour[0], 2.0);
        assert_eq!(p.hour[30], -4.0);
    }

    #[test]
    fn best_range_skips_noop_full_range() {
        // Все сделки прибыльные: никакой поддиапазон не лучше «без фильтра» —
        // пару min/max-пустышку предлагать нельзя.
        let mut vals: Vec<(f64, f64)> = (0..100).map(|i| (i as f64, 1.0)).collect();
        assert!(best_range(&mut vals, 1, 16, false).is_none());
        // Нижняя треть в минусе: диапазон предлагается и он НЕ весь размах.
        let mut vals: Vec<(f64, f64)> = (0..99)
            .map(|i| (i as f64, if i < 33 { -1.0 } else { 1.0 }))
            .collect();
        let s = best_range(&mut vals, 1, 16, false).expect("фильтр должен найтись");
        assert!(
            s.from.unwrap() > 0.0,
            "нижняя граница должна отрезать минус"
        );
        assert_eq!(s.to.unwrap(), 98.0, "верхняя пара — фактический max данных");
    }

    #[test]
    fn round_falls_back_when_pair_stops_cutting() {
        // Диапазон режет только два хвостовых минуса у самых краёв данных:
        // округление наружу увело бы ОБЕ границы за min/max (пустышка) —
        // должны остаться сырые режущие значения.
        let mut vals: Vec<(f64, f64)> = (0..100)
            .map(|i| (1000.0 + i as f64, if i < 2 { -5.0 } else { 1.0 }))
            .collect();
        let s = best_range(&mut vals, 1, 50, true).expect("фильтр должен найтись");
        let from = s.from.unwrap();
        assert!(from > 1000.0, "граница обязана резать данные, got {from}");
    }

    #[test]
    fn empty_variant_is_fact() {
        assert!(Variant::default().is_empty());
        let v = Variant {
            bounds: vec![Bound {
                field: "d1h".into(),
                from: Some(0.0),
                to: None,
            }],
            ..Default::default()
        };
        assert!(!v.is_empty());
    }
}
