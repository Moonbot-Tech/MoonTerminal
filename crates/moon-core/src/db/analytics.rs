//! Агрегаты для окна «Аналитика» поверх реплики отчётов (`orders_rep`).
//!
//! Все функции ходят в SQLite и считают по полной выборке периода — вызывать
//! ТОЛЬКО с background executor (UI-поток не должен ждать диск; см. грабли
//! reports-WAL-фриза). Источник — ОБЪЕДИНЕНИЕ typed-реплики и (пока жива)
//! легаси-таблицы `closed_sell_reports`, как в Отчёте: часть ядер ещё пишет
//! легаси, и чтение одной реплики «не видело» их сделок. Читатель свой
//! (`open_reader`), имена стратегий — через `ATTACH` strategies.sqlite (join
//! `strategyid = strategies.strategy_id`, оба signed i64); без файла — id.
//!
//! Периоды — unix-СЕКУНДЫ UTC, `to` эксклюзивно. Учитываются только закрытые
//! сделки (`closedate > 0`) без удалённых (`deleted=0`). Профит — `profitbtc`
//! (котировка пары, у нас USDT).

use rusqlite::Connection;

use super::SideFilter;

/// Фильтры выборки (общие для всех вкладок Аналитики).
#[derive(Clone, Debug, Default)]
pub struct Query {
    /// unix-сек UTC; `from < 0` — вся история. `to` эксклюзивно.
    pub from: i64,
    pub to: i64,
    /// Выбранные ядра (мультивыбор как в Ордерах); пусто = все.
    pub cores: Vec<u64>,
    pub side: SideFilter,
    /// `None` — все, `Some(false)` — реальные, `Some(true)` — эмуляторные
    /// (NULL в колонке считается «реальный», как в Отчёте).
    pub emulator: Option<bool>,
    /// Скоуп по одной стратегии (`strategyid`) — тюнер фильтров в контексте
    /// выбранной строки списка. None = все стратегии.
    pub strategy: Option<i64>,
}

impl Query {
    /// WHERE периода+фильтров для ОДНОГО источника: условия только по колонкам,
    /// которые у него ЕСТЬ (как `build_where` Отчёта — условие по отсутствующей
    /// колонке валило бы весь SELECT). Плейсхолдеры ?1/?2 = from/to;
    /// ядра/сторона/эму — литералами (целые из конфига, инъекции невозможны).
    fn where_sql(&self, cols: &std::collections::HashSet<String>) -> String {
        let has = |n: &str| cols.contains(n);
        let mut w = String::from(WHERE_PERIOD);
        if has("deleted") {
            w.push_str(" AND COALESCE(deleted,0) = 0");
        }
        if !self.cores.is_empty() {
            let list = self
                .cores
                .iter()
                .map(|c| (*c as i64).to_string())
                .collect::<Vec<_>>()
                .join(",");
            w.push_str(&format!(" AND core_uid IN ({list})"));
        }
        if has("isshort") {
            match self.side {
                SideFilter::All => {}
                SideFilter::Long => w.push_str(" AND COALESCE(isshort,0) = 0"),
                SideFilter::Short => w.push_str(" AND COALESCE(isshort,0) = 1"),
            }
        }
        if has("emulator") {
            match self.emulator {
                None => {}
                Some(false) => w.push_str(" AND COALESCE(emulator,0) = 0"),
                Some(true) => w.push_str(" AND COALESCE(emulator,0) = 1"),
            }
        }
        if let Some(sid) = self.strategy {
            if has("strategyid") {
                w.push_str(&format!(" AND COALESCE(strategyid,0) = {sid}"));
            }
        }
        w
    }
}

/// Общий набор колонок, на который проецируются оба источника отчётов.
/// Хвост — рыночные поля входа для тюнера фильтров (`db::tuner`).
const UNIFIED_COLS: &[&str] = &[
    "core_uid",
    "core_name",
    "coin",
    "isshort",
    "buydate",
    "closedate",
    "profitbtc",
    "strategyid",
    "emulator",
    "spentbtc",
    "bvsvratio",
    "pump1h",
    "dump1h",
    "d24h",
    "d3h",
    "d1h",
    "d15m",
    "d5m",
    "d1m",
    "vd1m",
    "hvol",
    "dvol",
    "btc1hdelta",
    "exchange1hdelta",
    "btc24hdelta",
    "btc5mdelta",
    "dbtc1m",
];

/// FROM-источник `o`: реплика + легаси одним UNION ALL, у каждой ветки СВОЙ
/// WHERE (фильтры пушатся в ветку — работает индекс closedate), отсутствующие
/// колонки → NULL. None — ни у одного источника ещё нет closedate/profitbtc.
pub(super) fn unified_from(conn: &Connection, q: &Query) -> Option<String> {
    let mut branches = Vec::new();
    for src in super::read_sources(conn) {
        if !src.cols.contains("closedate") || !src.cols.contains("profitbtc") {
            continue; // схема ядра ещё не пришла — агрегировать нечего
        }
        let proj = UNIFIED_COLS
            .iter()
            .map(|c| {
                if src.cols.contains(*c) {
                    format!("\"{c}\"")
                } else {
                    format!("NULL AS \"{c}\"")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        branches.push(format!(
            "SELECT {proj} FROM {} WHERE {}",
            src.table,
            q.where_sql(&src.cols)
        ));
    }
    if branches.is_empty() {
        None
    } else {
        Some(format!("({}) o", branches.join(" UNION ALL ")))
    }
}

/// Итог периода: счётчики + метрики, считанные по последовательности сделок
/// (порядок по `closedate`): profit factor, max drawdown, серии, длительность.
#[derive(Clone, Debug, Default)]
pub struct PeriodStats {
    pub n: i64,
    pub wins: i64,
    pub losses: i64,
    pub profit: f64,
    /// Сумма выигрышей / сумма проигрышей (0 проигрышей и есть выигрыши → 99).
    pub pf: f64,
    pub avg: f64,
    /// Максимальная просадка кумулятивной кривой профита за период.
    pub max_dd: f64,
    pub win_streak: i64,
    pub loss_streak: i64,
    /// Средняя длительность сделки (мин), по `closedate - buydate`.
    pub avg_dur_min: f64,
}

impl PeriodStats {
    pub fn winrate(&self) -> f64 {
        if self.n > 0 {
            self.wins as f64 / self.n as f64 * 100.0
        } else {
            0.0
        }
    }
}

/// Точка дневной (или недельной — см. `Summary::bucket_secs`) серии.
#[derive(Clone, Debug)]
pub struct DayPoint {
    /// Начало ведра (unix-секунды UTC).
    pub start: i64,
    pub profit: f64,
    pub trades: i64,
}

/// Строка топ-сделок (лучшие/худшие за период).
#[derive(Clone, Debug)]
pub struct TopTrade {
    pub closedate: i64,
    pub coin: String,
    pub strategy: String,
    pub core_name: String,
    pub profit: f64,
    pub is_short: bool,
}

/// Агрегат группы (стратегия по id / монета).
#[derive(Clone, Debug)]
pub struct GroupStat {
    /// Ключ группы: для стратегий — `strategyid` текстом, для монет — имя.
    pub key: String,
    /// Отображаемое имя (для стратегий — из strategies.sqlite, иначе id).
    pub name: String,
    /// Имя одного из ядер группы + число разных ядер (колонка «Ядро»).
    pub core: String,
    pub cores_n: i64,
    /// Текущий статус в ядрах (по head'ам strategies.sqlite, максимум по
    /// ядрам группы): None — БД стратегий нет / не стратегия; 0 — удалена
    /// везде; 1 — есть, но выключена; 2 — есть и включена (галка).
    pub alive: Option<i64>,
    pub n: i64,
    pub profit: f64,
    pub wins: i64,
    /// Сумма выигрышей / сумма проигрышей группы (99 = без проигрышей).
    pub pf: f64,
    pub best: f64,
    pub worst: f64,
}

impl GroupStat {
    pub fn winrate(&self) -> f64 {
        if self.n > 0 {
            self.wins as f64 / self.n as f64 * 100.0
        } else {
            0.0
        }
    }

    pub fn avg(&self) -> f64 {
        if self.n > 0 { self.profit / self.n as f64 } else { 0.0 }
    }
}

/// Детализация стратегии (drill-down вкладки «Стратегии»).
#[derive(Clone, Debug, Default)]
pub struct StrategyDetail {
    /// Вклад по монетам, прибыль по убыванию.
    pub coins: Vec<GroupStat>,
    /// Последние сделки (новые первыми).
    pub last: Vec<TopTrade>,
}

/// Данные вкладки «Сводка» одним заходом.
#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub cur: PeriodStats,
    /// Предыдущий период той же длины (сравнение «к пред. периоду»).
    pub prev: PeriodStats,
    /// Размер ведра серии: сутки; для очень длинных периодов — неделя.
    pub bucket_secs: i64,
    pub days: Vec<DayPoint>,
    pub best: Vec<TopTrade>,
    pub worst: Vec<TopTrade>,
    /// Группы по ID стратегии (`strategyid`; синк шлёт одинаковые id на все
    /// ядра, а одноимённые РАЗНЫЕ стратегии не сливаются), прибыль по убыванию.
    pub strategies: Vec<GroupStat>,
    /// Группы по монете, прибыль по убыванию.
    pub coins: Vec<GroupStat>,
    /// Самый прибыльный час UTC: (час, профит, сделок).
    pub best_hour: Option<(u32, f64, i64)>,
    /// Ядра, встречающиеся в реплике (для комбобокса фильтра) — БЕЗ учёта
    /// фильтров, чтобы выбор не «схлопывал» список.
    pub cores: Vec<(u64, String)>,
    /// Фактические границы периода (после резолва «Все»).
    pub from: i64,
    pub to: i64,
}

const WHERE_PERIOD: &str = "closedate >= ?1 AND closedate < ?2 AND closedate > 0";

/// Сводка за период/фильтры. None — БД ещё нет.
pub fn summary(q: &Query) -> Option<Summary> {
    let conn = super::open_reader()?;
    let has_strat_names = attach_strategies(&conn);

    let mut q = q.clone();
    if q.from < 0 {
        q.from = min_closedate(&conn);
    }
    let Some(src) = unified_from(&conn, &q) else {
        // Схемы источников ещё не пришли — пусто, но список ядер отдаём.
        return Some(Summary { cores: super::distinct_cores(&conn), ..Default::default() });
    };
    let len = (q.to - q.from).max(1);
    // Ведро серии: сутки, на многолетних «Все» — неделя (иначе тысячи баров).
    let bucket = if len / 86_400 > 400 { 7 * 86_400 } else { 86_400 };

    let (cur, days, hours) = scan_period(&conn, &src, q.from, q.to, bucket);
    let (prev, _, _) = scan_period(&conn, &src, q.from - len, q.from, bucket);

    let best_hour = hours
        .iter()
        .enumerate()
        .filter(|(_, (p, n))| *n > 0 && *p > 0.0)
        .max_by(|a, b| a.1.0.total_cmp(&b.1.0))
        .map(|(h, (p, n))| (h as u32, *p, *n));

    Some(Summary {
        cur,
        prev,
        bucket_secs: bucket,
        days,
        best: top_trades(&conn, &src, &q, has_strat_names, true),
        worst: top_trades(&conn, &src, &q, has_strat_names, false),
        strategies: groups(&conn, &src, &q, has_strat_names, true),
        coins: groups(&conn, &src, &q, has_strat_names, false),
        best_hour,
        cores: super::distinct_cores(&conn),
        from: q.from,
        to: q.to,
    })
}

/// Самая ранняя closedate по ОБОИМ источникам (резолв периода «Все»).
fn min_closedate(conn: &Connection) -> i64 {
    let mut min = i64::MAX;
    for src in super::read_sources(conn) {
        if !src.cols.contains("closedate") {
            continue;
        }
        let sql =
            format!("SELECT MIN(closedate) FROM {} WHERE closedate > 0", src.table);
        if let Ok(Some(v)) = conn.query_row(&sql, [], |r| r.get::<_, Option<i64>>(0)) {
            min = min.min(v);
        }
    }
    if min == i64::MAX { 1 } else { min }
}

/// Детализация стратегии по ID (`strategyid` — ключ группы вкладки «Стратегии»).
pub fn strategy_detail(q: &Query, strategy_id: i64) -> Option<StrategyDetail> {
    let conn = super::open_reader()?;
    let mut q = q.clone();
    if q.from < 0 {
        q.from = 1;
    }
    let src = unified_from(&conn, &q)?;

    // Вклад по монетам этой стратегии.
    let sql = format!(
        "SELECT COALESCE(o.coin,'') AS k, COALESCE(o.coin,''),
                MAX(o.core_name), COUNT(DISTINCT o.core_uid), NULL,
                COUNT(*), COALESCE(SUM(o.profitbtc),0),
                COALESCE(SUM(o.profitbtc > 0),0),
                COALESCE(SUM(CASE WHEN o.profitbtc > 0 THEN o.profitbtc END),0),
                COALESCE(SUM(CASE WHEN o.profitbtc <= 0 THEN -o.profitbtc END),0),
                COALESCE(MAX(o.profitbtc),0), COALESCE(MIN(o.profitbtc),0)
         FROM {src} WHERE o.strategyid = ?3
         GROUP BY k ORDER BY 7 DESC"
    );
    let coins = conn
        .prepare(&sql)
        .ok()?
        .query_map(rusqlite::params![q.from, q.to, strategy_id], group_from_row)
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();

    // Последние сделки (новые первыми).
    let sql = format!(
        "SELECT o.closedate, COALESCE(o.coin,''), o.core_name, o.core_name,
                COALESCE(o.profitbtc,0), COALESCE(o.isshort,0)
         FROM {src} WHERE o.strategyid = ?3
         ORDER BY o.closedate DESC LIMIT 10"
    );
    let last = conn
        .prepare(&sql)
        .ok()?
        .query_map(rusqlite::params![q.from, q.to, strategy_id], |r| {
            Ok(TopTrade {
                closedate: r.get(0)?,
                coin: r.get(1)?,
                strategy: r.get(2)?, // в детализации поле «стратегия» = ядро
                core_name: r.get(3)?,
                profit: r.get(4)?,
                is_short: r.get::<_, i64>(5)? != 0,
            })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();

    Some(StrategyDetail { coins, last })
}

/// Строка группового запроса: k, name, core, cores_n, alive, n, profit, wins,
/// wsum, lsum, max, min.
fn group_from_row(r: &rusqlite::Row) -> rusqlite::Result<GroupStat> {
    let wsum: f64 = r.get(8)?;
    let lsum: f64 = r.get(9)?;
    Ok(GroupStat {
        key: r.get(0)?,
        name: r.get(1)?,
        core: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        cores_n: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
        alive: r.get(4)?,
        n: r.get(5)?,
        profit: r.get(6)?,
        wins: r.get(7)?,
        pf: if lsum > 0.0 {
            wsum / lsum
        } else if wsum > 0.0 {
            99.0
        } else {
            0.0
        },
        best: r.get(10)?,
        worst: r.get(11)?,
    })
}

/// ATTACH БД стратегий за именами (может отсутствовать — не ошибка).
fn attach_strategies(conn: &Connection) -> bool {
    let path = crate::config::paths::strategies_db_path();
    if !path.exists() {
        return false;
    }
    let sql = format!(
        "ATTACH DATABASE '{}' AS strat",
        path.to_string_lossy().replace('\'', "''")
    );
    conn.execute(&sql, []).is_ok()
}

/// Один проход по сделкам периода (порядок по closedate): метрики
/// последовательности + серия по вёдрам + разбивка по часам UTC.
/// `src` — объединённый FROM-источник (`unified_from`), WHERE уже внутри.
fn scan_period(
    conn: &Connection,
    src: &str,
    from: i64,
    to: i64,
    bucket: i64,
) -> (PeriodStats, Vec<DayPoint>, [(f64, i64); 24]) {
    let mut st = PeriodStats::default();
    let mut days: Vec<DayPoint> = Vec::new();
    let mut hours = [(0.0f64, 0i64); 24];
    let sql = format!(
        "SELECT o.closedate, COALESCE(o.buydate, o.closedate), COALESCE(o.profitbtc, 0)
         FROM {src} ORDER BY o.closedate"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return (st, days, hours);
    };
    let Ok(rows) = stmt.query_map(rusqlite::params![from, to], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, f64>(2)?))
    }) else {
        return (st, days, hours);
    };

    let (mut wsum, mut lsum) = (0.0f64, 0.0f64);
    let (mut cum, mut peak) = (0.0f64, 0.0f64);
    let (mut cur_w, mut cur_l) = (0i64, 0i64);
    let mut dur_ms_total = 0i64;
    for row in rows.flatten() {
        let (close, buy, profit) = row;
        st.n += 1;
        st.profit += profit;
        dur_ms_total += (close - buy).max(0);
        if profit > 0.0 {
            st.wins += 1;
            wsum += profit;
            cur_w += 1;
            cur_l = 0;
            st.win_streak = st.win_streak.max(cur_w);
        } else {
            st.losses += 1;
            lsum -= profit;
            cur_l += 1;
            cur_w = 0;
            st.loss_streak = st.loss_streak.max(cur_l);
        }
        cum += profit;
        peak = peak.max(cum);
        st.max_dd = st.max_dd.max(peak - cum);

        let start = close.div_euclid(bucket) * bucket;
        match days.last_mut() {
            Some(d) if d.start == start => {
                d.profit += profit;
                d.trades += 1;
            }
            _ => days.push(DayPoint { start, profit, trades: 1 }),
        }
        let h = close.rem_euclid(86_400) / 3600;
        let slot = &mut hours[h as usize];
        slot.0 += profit;
        slot.1 += 1;
    }
    if st.n > 0 {
        st.avg = st.profit / st.n as f64;
        st.avg_dur_min = dur_ms_total as f64 / st.n as f64 / 60.0;
        st.pf = if lsum > 0.0 {
            wsum / lsum
        } else if wsum > 0.0 {
            99.0
        } else {
            0.0
        };
    }
    // Дырки серии (дни без сделок) заполняем нулями — бары ровные по времени.
    if !days.is_empty() {
        let mut filled = Vec::with_capacity(days.len());
        let mut t = days[0].start;
        let mut it = days.into_iter().peekable();
        while let Some(d) = it.peek() {
            if d.start == t {
                filled.push(it.next().unwrap());
            } else {
                filled.push(DayPoint { start: t, profit: 0.0, trades: 0 });
            }
            t += bucket;
        }
        days = filled;
    }
    (st, days, hours)
}

/// Имя стратегии в SQL: из ATTACH-нутой БД стратегий либо голый id.
fn strategy_name_expr(has_names: bool) -> &'static str {
    if has_names {
        "COALESCE((SELECT st.name FROM strat.strategies st
                   WHERE st.core_uid = o.core_uid AND st.strategy_id = o.strategyid),
                  CAST(o.strategyid AS TEXT))"
    } else {
        "CAST(o.strategyid AS TEXT)"
    }
}

/// Топ-5 лучших (`best=true`) или худших сделок периода.
fn top_trades(
    conn: &Connection,
    src: &str,
    q: &Query,
    has_names: bool,
    best: bool,
) -> Vec<TopTrade> {
    let order = if best { "DESC" } else { "ASC" };
    let name = strategy_name_expr(has_names);
    let sql = format!(
        "SELECT o.closedate, COALESCE(o.coin,''), {name}, o.core_name,
                COALESCE(o.profitbtc,0), COALESCE(o.isshort,0)
         FROM {src} WHERE o.profitbtc IS NOT NULL
         ORDER BY o.profitbtc {order} LIMIT 5"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![q.from, q.to], |r| {
        Ok(TopTrade {
            closedate: r.get(0)?,
            coin: r.get(1)?,
            strategy: r.get(2)?,
            core_name: r.get(3)?,
            profit: r.get(4)?,
            is_short: r.get::<_, i64>(5)? != 0,
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

/// Группы периода: по ID стратегии (`by_strategy=true`) или по монете.
/// Прибыль по убыванию (хвост списка = худшие).
fn groups(
    conn: &Connection,
    src: &str,
    q: &Query,
    has_names: bool,
    by_strategy: bool,
) -> Vec<GroupStat> {
    // Ключ стратегии — id: переименования не плодят группы, одноимённые
    // разные стратегии не сливаются; имя — только подпись.
    let (key, name, alive) = if by_strategy {
        (
            "CAST(o.strategyid AS TEXT)".to_string(),
            format!("MAX({})", strategy_name_expr(has_names)),
            // Статус «жива сейчас» по head'ам БД стратегий: 2 включена,
            // 1 есть но выключена, 0 удалена; максимум по ядрам группы.
            if has_names {
                "MAX(COALESCE((SELECT CASE WHEN st.deleted <> 0 THEN 0
                                            WHEN COALESCE(st.checked,0) <> 0 THEN 2
                                            ELSE 1 END
                               FROM strat.strategies st
                               WHERE st.core_uid = o.core_uid
                                 AND st.strategy_id = o.strategyid), 0))"
            } else {
                "NULL"
            },
        )
    } else {
        let coin = "COALESCE(o.coin,'')".to_string();
        (coin.clone(), coin, "NULL")
    };
    let sql = format!(
        "SELECT {key} AS k, {name}, MAX(o.core_name), COUNT(DISTINCT o.core_uid),
                {alive},
                COUNT(*), COALESCE(SUM(o.profitbtc),0),
                COALESCE(SUM(o.profitbtc > 0),0),
                COALESCE(SUM(CASE WHEN o.profitbtc > 0 THEN o.profitbtc END),0),
                COALESCE(SUM(CASE WHEN o.profitbtc <= 0 THEN -o.profitbtc END),0),
                COALESCE(MAX(o.profitbtc),0), COALESCE(MIN(o.profitbtc),0)
         FROM {src}
         GROUP BY k ORDER BY 7 DESC"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![q.from, q.to], group_from_row)
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}
