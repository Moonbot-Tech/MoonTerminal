//! Агрегаты для окна «Аналитика» поверх реплики отчётов (`orders_rep`).
//!
//! Все функции ходят в SQLite и считают по полной выборке периода — вызывать
//! ТОЛЬКО с background executor (UI-поток не должен ждать диск; см. грабли
//! reports-WAL-фриза). Читатель свой (`open_reader`), имена стратегий — через
//! `ATTACH` strategies.sqlite (join `orders_rep.strategyid = strategies.
//! strategy_id`, оба signed i64); без файла стратегий имена = id.
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
}

impl Query {
    /// WHERE периода+фильтров. Плейсхолдеры ?1/?2 = from/to; ядра/сторона/эму —
    /// литералами (целые из конфига, инъекции невозможны).
    fn where_sql(&self) -> String {
        let mut w = String::from(WHERE_PERIOD);
        if !self.cores.is_empty() {
            let list = self
                .cores
                .iter()
                .map(|c| (*c as i64).to_string())
                .collect::<Vec<_>>()
                .join(",");
            w.push_str(&format!(" AND core_uid IN ({list})"));
        }
        match self.side {
            SideFilter::All => {}
            SideFilter::Long => w.push_str(" AND COALESCE(isshort,0) = 0"),
            SideFilter::Short => w.push_str(" AND COALESCE(isshort,0) = 1"),
        }
        match self.emulator {
            None => {}
            Some(false) => w.push_str(" AND COALESCE(emulator,0) = 0"),
            Some(true) => w.push_str(" AND COALESCE(emulator,0) = 1"),
        }
        w
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

/// Агрегат группы (стратегия по имени / монета).
#[derive(Clone, Debug)]
pub struct GroupStat {
    pub name: String,
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
    /// Группы по ИМЕНИ стратегии (одноимённые на разных ядрах сливаются),
    /// прибыль по убыванию.
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

const WHERE_PERIOD: &str =
    "closedate >= ?1 AND closedate < ?2 AND closedate > 0 AND COALESCE(deleted,0) = 0";

/// Сводка за период/фильтры. None — БД/таблицы ещё нет.
pub fn summary(q: &Query) -> Option<Summary> {
    let conn = super::open_reader()?;
    // Таблица могла ещё не дорасти до нужных колонок (свежая реплика).
    let cols = super::rep_columns(&conn);
    for need in ["closedate", "profitbtc"] {
        if !cols.contains(need) {
            return Some(Summary::default());
        }
    }
    let has_strat_names = attach_strategies(&conn);

    let mut q = q.clone();
    if q.from < 0 {
        q.from = conn
            .query_row(
                "SELECT COALESCE(MIN(closedate), 0) FROM orders_rep WHERE closedate > 0",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
            .max(1);
    }
    let len = (q.to - q.from).max(1);
    // Ведро серии: сутки, на многолетних «Все» — неделя (иначе тысячи баров).
    let bucket = if len / 86_400 > 400 { 7 * 86_400 } else { 86_400 };

    let (cur, days, hours) = scan_period(&conn, &q, q.from, q.to, bucket);
    let (prev, _, _) = scan_period(&conn, &q, q.from - len, q.from, bucket);

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
        best: top_trades(&conn, &q, has_strat_names, true),
        worst: top_trades(&conn, &q, has_strat_names, false),
        strategies: groups(&conn, &q, has_strat_names, true),
        coins: groups(&conn, &q, has_strat_names, false),
        best_hour,
        cores: super::distinct_cores(&conn),
        from: q.from,
        to: q.to,
    })
}

/// Детализация стратегии по ИМЕНИ (группа как во вкладке «Стратегии»).
pub fn strategy_detail(q: &Query, name: &str) -> Option<StrategyDetail> {
    let conn = super::open_reader()?;
    let has_names = attach_strategies(&conn);
    let mut q = q.clone();
    if q.from < 0 {
        q.from = 1;
    }
    let name_expr = strategy_name_expr(has_names);
    let wh = q.where_sql();

    // Вклад по монетам этой стратегии.
    let sql = format!(
        "SELECT COALESCE(o.coin,''), COUNT(*), COALESCE(SUM(o.profitbtc),0),
                COALESCE(SUM(o.profitbtc > 0),0),
                COALESCE(SUM(CASE WHEN o.profitbtc > 0 THEN o.profitbtc END),0),
                COALESCE(SUM(CASE WHEN o.profitbtc <= 0 THEN -o.profitbtc END),0),
                COALESCE(MAX(o.profitbtc),0), COALESCE(MIN(o.profitbtc),0)
         FROM orders_rep o WHERE {wh} AND {name_expr} = ?3
         GROUP BY 1 ORDER BY 3 DESC"
    );
    let coins = conn
        .prepare(&sql)
        .ok()?
        .query_map(rusqlite::params![q.from, q.to, name], group_from_row)
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();

    // Последние сделки (новые первыми).
    let sql = format!(
        "SELECT o.closedate, COALESCE(o.coin,''), o.core_name, o.core_name,
                COALESCE(o.profitbtc,0), COALESCE(o.isshort,0)
         FROM orders_rep o WHERE {wh} AND {name_expr} = ?3
         ORDER BY o.closedate DESC LIMIT 10"
    );
    let last = conn
        .prepare(&sql)
        .ok()?
        .query_map(rusqlite::params![q.from, q.to, name], |r| {
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

fn group_from_row(r: &rusqlite::Row) -> rusqlite::Result<GroupStat> {
    let wsum: f64 = r.get(4)?;
    let lsum: f64 = r.get(5)?;
    Ok(GroupStat {
        name: r.get(0)?,
        n: r.get(1)?,
        profit: r.get(2)?,
        wins: r.get(3)?,
        pf: if lsum > 0.0 {
            wsum / lsum
        } else if wsum > 0.0 {
            99.0
        } else {
            0.0
        },
        best: r.get(6)?,
        worst: r.get(7)?,
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
fn scan_period(
    conn: &Connection,
    q: &Query,
    from: i64,
    to: i64,
    bucket: i64,
) -> (PeriodStats, Vec<DayPoint>, [(f64, i64); 24]) {
    let mut st = PeriodStats::default();
    let mut days: Vec<DayPoint> = Vec::new();
    let mut hours = [(0.0f64, 0i64); 24];
    let wh = q.where_sql();
    let sql = format!(
        "SELECT closedate, COALESCE(buydate, closedate), COALESCE(profitbtc, 0)
         FROM orders_rep WHERE {wh} ORDER BY closedate"
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
fn top_trades(conn: &Connection, q: &Query, has_names: bool, best: bool) -> Vec<TopTrade> {
    let order = if best { "DESC" } else { "ASC" };
    let name = strategy_name_expr(has_names);
    let wh = q.where_sql();
    let sql = format!(
        "SELECT o.closedate, COALESCE(o.coin,''), {name}, o.core_name,
                COALESCE(o.profitbtc,0), COALESCE(o.isshort,0)
         FROM orders_rep o WHERE {wh} AND o.profitbtc IS NOT NULL
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

/// Группы периода: по имени стратегии (`by_strategy=true`) или по монете.
/// Прибыль по убыванию (хвост списка = худшие).
fn groups(conn: &Connection, q: &Query, has_names: bool, by_strategy: bool) -> Vec<GroupStat> {
    let key = if by_strategy {
        strategy_name_expr(has_names).to_string()
    } else {
        "COALESCE(o.coin,'')".to_string()
    };
    let wh = q.where_sql();
    let sql = format!(
        "SELECT {key} AS k, COUNT(*), COALESCE(SUM(o.profitbtc),0),
                COALESCE(SUM(o.profitbtc > 0),0),
                COALESCE(SUM(CASE WHEN o.profitbtc > 0 THEN o.profitbtc END),0),
                COALESCE(SUM(CASE WHEN o.profitbtc <= 0 THEN -o.profitbtc END),0),
                COALESCE(MAX(o.profitbtc),0), COALESCE(MIN(o.profitbtc),0)
         FROM orders_rep o WHERE {wh}
         GROUP BY k ORDER BY 3 DESC"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![q.from, q.to], group_from_row)
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}
