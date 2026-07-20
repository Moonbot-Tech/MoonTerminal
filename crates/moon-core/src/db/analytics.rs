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

use super::read_fail::read_fail;
use super::{ReadFail, ReadResult, SideFilter};

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
    /// Скоуп по КОНКРЕТНОМУ ядру выбранной строки (`core_uid`) — список стратегий
    /// разбит по ядрам, чтобы видеть работу стратегии на отдельном ядре. None = все ядра.
    pub strat_core: Option<u64>,
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
        // Скоуп по конкретному ядру строки (список стратегий разбит по ядрам).
        if let Some(c) = self.strat_core {
            w.push_str(&format!(" AND core_uid = {}", c as i64));
        }
        w
    }
}

/// Базовые колонки, на которые проецируются оба источника отчётов; рыночные
/// поля тюнера (`db::tuner::FIELDS`) дочейниваются АВТОМАТИЧЕСКИ — новое поле
/// тюнера нельзя забыть добавить в проекцию (иначе его SQL молча падал).
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
];

/// Build the unified replica-and-legacy `FROM` source with filters inside each
/// branch so the `closedate` indexes remain usable.
///
/// Missing columns project as NULL. `Ok(None)` means no source has received the
/// required schema; a failed schema probe remains an error because opening a
/// database does not validate its schema b-tree.
pub(super) fn unified_from(conn: &Connection, q: &Query) -> ReadResult<Option<String>> {
    let cols: Vec<&str> = UNIFIED_COLS
        .iter()
        .copied()
        .chain(super::tuner::FIELDS.iter().map(|s| s.col))
        .collect();
    let mut branches = Vec::new();
    for src in super::read_sources_res(conn)? {
        if !src.cols.contains("closedate") || !src.cols.contains("profitbtc") {
            continue; // схема ядра ещё не пришла — агрегировать нечего
        }
        let proj = cols
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
        Ok(None)
    } else {
        Ok(Some(format!("({}) o", branches.join(" UNION ALL "))))
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

/// Ячейка календарной тепловой карты: агрегат суток + вклад по ядрам (для
/// сегментного бара в крупной карточке дня).
#[derive(Clone, Debug, Default)]
pub struct DayCell {
    /// Начало суток (unix-секунды UTC).
    pub start: i64,
    pub profit: f64,
    pub trades: i64,
    /// Прибыльных сделок за день (для W/L и winrate ячейки); убытки = trades−wins.
    pub wins: i64,
}

/// Серия одного ядра для нижнего чарта «Сводки»: профит по вёдрам той же
/// сетки, что `Summary::days` (кумулятив строит UI).
#[derive(Clone, Debug)]
pub struct CoreSeries {
    pub uid: u64,
    pub name: String,
    pub per_bucket: Vec<f64>,
    /// Trades in the SAME bucket — same grid and same length as `per_bucket`, filled in
    /// one pass with it (the Summary popups print profit and count on one line).
    pub per_bucket_trades: Vec<i64>,
    pub total: f64,
    /// The core's trades over the whole period = sum of `per_bucket_trades`.
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
    /// Тип стратегии (SignalType текущей версии); пусто у монет/без БД.
    pub kind: String,
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
    /// Strategy's last edit date — the `LastEditDate` field from the current version's
    /// raw_json (strategies.sqlite). Empty for coins / when the strategy DB is absent.
    pub lastedit: String,
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
        if self.n > 0 {
            self.profit / self.n as f64
        } else {
            0.0
        }
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
    /// The preceding period of equal length, used for KPI comparisons.
    ///
    /// A failed comparison scan is non-fatal and yields `None`, preserving a
    /// readable current period while making its unavailable deltas explicit.
    pub prev: Option<PeriodStats>,
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
    /// Серии по ядрам (сетка `days`, прибыль по убыванию итога) — нижний
    /// чарт «Сводки» «прибыль по ядрам».
    pub core_days: Vec<CoreSeries>,
    /// Самый прибыльный час UTC: (час, профит, сделок).
    pub best_hour: Option<(u32, f64, i64)>,
    /// Profit per strategy type, descending. Filled ONLY for a day or less: on such a
    /// period the per-day series is a single bar, so the chart groups by type instead.
    /// Empty = a longer period, where the chart stays daily.
    pub kinds: Vec<KindStat>,
    /// Ядра, встречающиеся в реплике (для комбобокса фильтра) — БЕЗ учёта
    /// фильтров, чтобы выбор не «схлопывал» список.
    pub cores: Vec<(u64, String)>,
    /// Фактические границы периода (после резолва «Все»).
    pub from: i64,
    pub to: i64,
}

/// One core's share of a strategy type — the popup behind a `KindStat` bar.
#[derive(Clone, Debug)]
pub struct KindCore {
    pub uid: u64,
    pub name: String,
    pub profit: f64,
    pub trades: i64,
}

/// Profit of ONE strategy type (`SignalType`) over the period, with the per-core split
/// behind it. Built only for single-day periods, where a per-day series would be one bar:
/// the chart then groups by type instead of by day, and the popup opens the cores.
///
/// The type comes from the strategies DB; without it every trade lands in one unnamed
/// group (`kind` empty) rather than the chart disappearing.
#[derive(Clone, Debug)]
pub struct KindStat {
    /// `SignalType` of the strategy; empty when unknown (UI shows a dash).
    pub kind: String,
    pub profit: f64,
    pub trades: i64,
    /// Cores that traded this type, most profitable first.
    pub cores: Vec<KindCore>,
}

const WHERE_PERIOD: &str = "closedate >= ?1 AND closedate < ?2 AND closedate > 0";

/// Aggregate the selected period and filters without collapsing read failures.
///
/// Returns `NotReady` when the replica or required core schema is absent, and
/// `Failed` when a required current-period filesystem or SQLite read fails. A
/// failed comparison scan leaves `Summary::prev` absent. A successful empty
/// period has zero current-period counters.
pub fn summary(q: &Query) -> ReadResult<Summary> {
    let conn = super::open_reader()?;
    // ATTACH first: it cannot run inside a transaction.
    let has_strat_names = attach_strategies(&conn);
    // One snapshot for the whole summary. Its counters, series, top trades and
    // group tables are separate statements, and the writer commits between them
    // during catch-up — without this they could disagree about which trades the
    // period contains while being published as one coherent result.
    let snap = super::read_snapshot(&conn)?;
    summary_on(&snap, q, has_strat_names)
}

/// Aggregate a summary on an existing connection for tests and [`summary`].
///
/// `has_strat_names` is supplied so tests do not attach the developer's real
/// strategies database. Missing required source schema returns `NotReady`;
/// schema, query, and row failures return `Failed`.
pub(super) fn summary_on(
    conn: &Connection,
    q: &Query,
    has_strat_names: bool,
) -> ReadResult<Summary> {
    let mut q = q.clone();
    if q.from < 0 {
        q.from = min_closedate(conn)?;
    }
    let Some(src) = unified_from(conn, &q)? else {
        return Err(ReadFail::NotReady);
    };
    let len = (q.to - q.from).max(1);
    // A day or less: a daily series would be a single bar and a cumulative curve a single
    // point, so the grid drops to HOURS — the same series, the same charts, just a scale
    // that has something to show. `core_series` follows this bucket, so the per-core lines
    // and every popup come along for free.
    let one_day = len <= 86_400;
    // Ведро серии: сутки, на многолетних «Все» — неделя (иначе тысячи баров).
    let bucket = if one_day {
        3_600
    } else if len / 86_400 > 400 {
        7 * 86_400
    } else {
        86_400
    };

    let (cur, days, hours) = scan_period(conn, &src, q.from, q.to, bucket)?;
    // The comparison is best-effort because it must not hide a readable current
    // period; see `Summary::prev`.
    let prev = scan_period(conn, &src, q.from - len, q.from, bucket)
        .map(|(st, _, _)| st)
        .ok();

    let best_hour = hours
        .iter()
        .enumerate()
        .filter(|(_, (p, n))| *n > 0 && *p > 0.0)
        .max_by(|a, b| a.1 .0.total_cmp(&b.1 .0))
        .map(|(h, (p, n))| (h as u32, *p, *n));

    let core_days = core_series(conn, &src, &q, &days, bucket)?;
    // Shared latch: the first name-related failure turns enrichment off for the
    // remaining aggregations of THIS summary.
    let mut names = has_strat_names;
    Ok(Summary {
        cur,
        prev,
        bucket_secs: bucket,
        days,
        core_days,
        best: with_name_fallback(&mut names, |n| top_trades(conn, &src, &q, n, true))?,
        worst: with_name_fallback(&mut names, |n| top_trades(conn, &src, &q, n, false))?,
        strategies: with_name_fallback(&mut names, |n| groups(conn, &src, &q, n, true))?,
        coins: with_name_fallback(&mut names, |n| groups(conn, &src, &q, n, false))?,
        best_hour,
        // Only for a day or less — on a longer period the daily chart works and this
        // aggregation's per-row subquery would be paid for nothing.
        kinds: if one_day {
            with_name_fallback(&mut names, |n| kind_stats(conn, &src, &q, n))?
        } else {
            Vec::new()
        },
        cores: super::distinct_cores(conn)?,
        from: q.from,
        to: q.to,
    })
}

/// Profit per strategy type (`SignalType`) with the per-core split behind each one, in ONE
/// pass: the bars and their popups can never disagree about a number.
///
/// Without the strategies DB the type expression is a constant empty string, so every trade
/// folds into one unnamed group instead of the chart vanishing.
fn kind_stats(
    conn: &Connection,
    src: &str,
    q: &Query,
    has_names: bool,
) -> ReadResult<Vec<KindStat>> {
    const CTX: &str = "analytics: kind_stats";
    let kind = if has_names {
        "COALESCE((SELECT json_extract(v.raw_json, '$.SignalType')
                   FROM strat.strategy_versions v
                   WHERE v.core_uid = o.core_uid
                     AND v.strategy_id = o.strategyid
                     AND v.valid_to IS NULL), '')"
    } else {
        "''"
    };
    let sql = format!(
        "SELECT {kind} AS k, o.core_uid, MAX(COALESCE(o.core_name,'')),
                COUNT(*), COALESCE(SUM(o.profitbtc),0)
         FROM {src} GROUP BY k, o.core_uid"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                // NULL core_uid: a source without the column projects it as NULL, and a
                // hard read would abort the whole summary over one unattributable row.
                r.get::<_, Option<i64>>(1)?.unwrap_or(0) as u64,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, f64>(4)?,
            ))
        })
        .map_err(|e| read_fail(CTX, e))?;
    let mut map: std::collections::HashMap<String, KindStat> = std::collections::HashMap::new();
    for row in rows {
        let (kind, uid, name, trades, profit) = row.map_err(|e| read_fail(CTX, e))?;
        let e = map.entry(kind.clone()).or_insert_with(|| KindStat {
            kind,
            profit: 0.0,
            trades: 0,
            cores: Vec::new(),
        });
        e.profit += profit;
        e.trades += trades;
        e.cores.push(KindCore {
            uid,
            name,
            profit,
            trades,
        });
    }
    let mut out: Vec<KindStat> = map.into_values().collect();
    for k in &mut out {
        k.cores.sort_by(|a, b| b.profit.total_cmp(&a.profit));
    }
    out.sort_by(|a, b| b.profit.total_cmp(&a.profit));
    Ok(out)
}

/// Плотная посуточная серия ячеек за период — для календарных тепловых карт
/// («Год» GitHub-style / крупный «Месяц»). Одно ведро = сутки UTC; агрегат
/// `GROUP BY closedate/86400, core_uid` даёт и суточный итог, и вклад по ядрам
/// (сегментный бар в карточке). Диапазон заполняется ПОЛНОСТЬЮ (дни без сделок —
/// пустые ячейки), чтобы сетка календаря была ровной; в отличие от `summary`
/// НИКОГДА не укрупняет ведро до недели. None — схемы источников ещё нет;
/// Some(пусто) — период без закрытых сделок.
///
/// NOTE: this surface still collapses a read failure into `None`, the pattern
/// the rest of this module moved away from. Converting it is left to the owners
/// of the calendar feature rather than rewritten here; the `.ok()?` calls below
/// only adapt it to the now-fallible helpers.
pub fn calendar_cells(q: &Query) -> Option<Vec<DayCell>> {
    calendar_cells_from(&super::open_reader().ok()?, q)
}

/// Ядро `calendar_cells` над готовым соединением — точка входа для юнит-тестов
/// (сидируем in-memory `orders_rep`, проверяем бакетинг/дыры/разбивку по ядрам).
fn calendar_cells_from(conn: &Connection, q: &Query) -> Option<Vec<DayCell>> {
    let mut q = q.clone();
    let all_history = q.from < 0;
    if all_history {
        q.from = min_closedate(conn).ok()?;
    }
    let Some(src) = unified_from(conn, &q).ok()? else {
        // Схемы источников ещё не пришли — пустой календарь (как `summary`
        // отдаёт Some(default)), а НЕ None: иначе вкладка висла бы на «Загрузка».
        return Some(Vec::new());
    };
    // Бакет по суткам: PnL, число сделок, прибыльных (для W/L и winrate).
    let sql = format!(
        "SELECT (o.closedate / 86400) * 86400 AS d,
                COALESCE(SUM(o.profitbtc), 0), COUNT(*), COALESCE(SUM(o.profitbtc > 0), 0)
         FROM {src} GROUP BY d ORDER BY d"
    );
    let mut stmt = conn.prepare(&sql).ok()?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })
        .ok()?;
    let mut map: std::collections::HashMap<i64, DayCell> = std::collections::HashMap::new();
    let (mut first, mut last) = (i64::MAX, i64::MIN);
    for (d, profit, n, wins) in rows.flatten() {
        first = first.min(d);
        last = last.max(d);
        map.insert(
            d,
            DayCell {
                start: d,
                profit,
                trades: n,
                wins,
            },
        );
    }
    if map.is_empty() {
        return Some(Vec::new()); // период без сделок — пустой календарь
    }
    // Границы плотной сетки: для «Все» — от первого дня с данными (иначе
    // залили бы годы пустот от эпохи); для заданного периода — от его начала.
    // `to` эксклюзивно → последний день = day(to-1); в будущее не заходим.
    let now = crate::util::now_unix_ms_i64() / 1000;
    let today0 = now.div_euclid(86_400) * 86_400;
    let day0 = if all_history {
        first
    } else {
        q.from.div_euclid(86_400) * 86_400
    };
    let last_grid = ((q.to - 1).div_euclid(86_400) * 86_400)
        .min(today0)
        .max(last);
    let day0 = day0.min(last_grid);
    let mut out = Vec::with_capacity((((last_grid - day0) / 86_400) + 1).max(1) as usize);
    let mut t = day0;
    while t <= last_grid {
        out.push(map.remove(&t).unwrap_or(DayCell {
            start: t,
            ..Default::default()
        }));
        t += 86_400;
    }
    Some(out)
}

/// Почасовые ячейки за период (для режима «День» календаря): `start` = начало
/// ЧАСА UTC, PnL/сделки/wins. Разрежённо (только часы со сделками) — сетку
/// 24×N строит UI. None — БД недоступна; Some(пусто) — период без сделок/схемы.
pub fn calendar_hours(q: &Query) -> Option<Vec<DayCell>> {
    let conn = super::open_reader().ok()?;
    let src = match unified_from(&conn, q) {
        Ok(Some(s)) => s,
        Ok(None) => return Some(Vec::new()), // схема ещё не пришла
        Err(_) => return None,
    };
    let sql = format!(
        "SELECT (o.closedate / 3600) * 3600 AS h,
                COALESCE(SUM(o.profitbtc), 0), COUNT(*), COALESCE(SUM(o.profitbtc > 0), 0)
         FROM {src} GROUP BY h ORDER BY h"
    );
    let mut stmt = conn.prepare(&sql).ok()?;
    let out = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok(DayCell {
                start: r.get(0)?,
                profit: r.get(1)?,
                trades: r.get(2)?,
                wins: r.get(3)?,
            })
        })
        .ok()?
        .flatten()
        .collect();
    Some(out)
}

/// Профиль «час дня» (0..23 UTC): PnL/сделки/прибыльных, агрегированные по
/// всем дням периода. Ячейка нижней тепловой карты «Тюнинг → По времени».
#[derive(Clone, Copy, Debug, Default)]
pub struct HourStat {
    pub profit: f64,
    pub trades: i64,
    pub wins: i64,
}

/// Профили «по часам дня» сразу для нескольких периодов (текущий/неделя/месяц/
/// 90д) — нижняя тепловая карта вкладки «Тюнинг → По времени». Один reader и
/// один снапшот на ВСЕ диапазоны (столбцы одной карты должны видеть одну и ту
/// же выборку). Фильтры (ядра/сторона/эму/стратегия) берутся из `base`; from/to
/// каждого диапазона переопределяют период (`from < 0` → вся история). Возврат
/// выровнен с `ranges`.
pub fn hourly_profiles(base: &Query, ranges: &[(i64, i64)]) -> ReadResult<Vec<[HourStat; 24]>> {
    let conn = super::open_reader()?;
    let snap = super::read_snapshot(&conn)?;
    let conn = &*snap;
    let mut out = Vec::with_capacity(ranges.len());
    for &(from, to) in ranges {
        out.push(hour_profile_one(conn, base, from, to)?);
    }
    Ok(out)
}

/// Один столбец профиля «час дня» за `[from, to)` на готовом снапшоте.
fn hour_profile_one(
    conn: &Connection,
    base: &Query,
    from: i64,
    to: i64,
) -> ReadResult<[HourStat; 24]> {
    const CTX: &str = "analytics: hour_profile";
    let mut q = base.clone();
    q.from = if from < 0 { min_closedate(conn)? } else { from };
    q.to = to;
    let mut prof = [HourStat::default(); 24];
    // Схема источников ещё не пришла — пустой профиль (как `summary`/календарь).
    let Some(src) = unified_from(conn, &q)? else {
        return Ok(prof);
    };
    // Час дня по времени ОТКРЫТИЯ сделки (buydate) — согласованно с расписанием и
    // ползунками тюнера, которые гейтят ВХОД. Fallback на closedate, если открытие
    // не записано (0/NULL). Период по-прежнему по closedate (окно анализа).
    let sql = format!(
        "SELECT ((COALESCE(NULLIF(o.buydate, 0), o.closedate) % 86400) / 3600) AS h,
                COALESCE(SUM(o.profitbtc), 0), COUNT(*), COALESCE(SUM(o.profitbtc > 0), 0)
         FROM {src} GROUP BY h ORDER BY h"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })
        .map_err(|e| read_fail(CTX, e))?;
    for row in rows {
        let (h, profit, trades, wins) = row.map_err(|e| read_fail(CTX, e))?;
        if (0..24).contains(&h) {
            prof[h as usize] = HourStat {
                profit,
                trades,
                wins,
            };
        }
    }
    Ok(prof)
}

/// Run a query that may join the ATTACHed strategies DB, retrying WITHOUT names
/// if it fails while names were in play.
///
/// `strategies.sqlite` is optional enrichment: a successful ATTACH does not
/// prove it is readable, and a failing scalar subquery against it must degrade
/// strategy labels to bare ids — not sink an otherwise healthy reports summary.
/// A failure of the reports DB itself simply fails again on the retry.
fn with_name_fallback<T>(
    names: &mut bool,
    mut run: impl FnMut(bool) -> ReadResult<T>,
) -> ReadResult<T> {
    match run(*names) {
        // Retry on ANY failure while names are in play, permanent included:
        // these statements also read the ATTACHed strategies DB, and the error
        // carries no database provenance, so corruption first met there would
        // otherwise sink a perfectly readable reports summary. The latch below
        // caps the cost at ONE extra scan per summary, not four.
        Err(e) if *names => {
            log::warn!("analytics: strategy names unavailable, retrying with bare ids: {e}");
            // Latch it off: without this, all four aggregations of one summary
            // each pay a failed scan before falling back.
            *names = false;
            run(false)
        }
        other => other,
    }
}

/// Find the earliest `closedate` across both sources for the all-time period.
///
/// A failed probe remains an error; `1` is reserved for a genuinely empty history.
fn min_closedate(conn: &Connection) -> ReadResult<i64> {
    const CTX: &str = "analytics: min_closedate";
    let mut min = i64::MAX;
    for src in super::read_sources_res(conn)? {
        if !src.cols.contains("closedate") {
            continue;
        }
        let sql = format!(
            "SELECT MIN(closedate) FROM {} WHERE closedate > 0",
            src.table
        );
        let got: Option<i64> = conn
            .query_row(&sql, [], |r| r.get::<_, Option<i64>>(0))
            .map_err(|e| read_fail(CTX, e))?;
        if let Some(v) = got {
            min = min.min(v);
        }
    }
    // No rows anywhere is a legitimate empty history, not a failure.
    Ok(if min == i64::MAX { 1 } else { min })
}

/// Load strategy detail by `strategyid`.
///
/// Returns `NotReady` when the replica or required schema is absent and `Failed`
/// when opening the replica, pinning the snapshot, or reading any row fails.
/// A healthy period without matching trades returns empty detail lists.
pub fn strategy_detail(q: &Query, strategy_id: i64) -> ReadResult<StrategyDetail> {
    const CTX: &str = "analytics: strategy_detail";
    let conn = super::open_reader()?;
    // One snapshot: the sections below are separate statements presented as one
    // card, so they must agree about which trades the period contains.
    let snap = super::read_snapshot(&conn)?;
    let conn = &*snap;
    let mut q = q.clone();
    if q.from < 0 {
        q.from = 1;
    }
    let Some(src) = unified_from(conn, &q)? else {
        return Err(ReadFail::NotReady);
    };

    // Вклад по монетам этой стратегии.
    let sql = format!(
        "SELECT COALESCE(o.coin,'') AS k, COALESCE(o.coin,''), '',
                MAX(o.core_name), COUNT(DISTINCT o.core_uid), NULL,
                COUNT(*), COALESCE(SUM(o.profitbtc),0),
                COALESCE(SUM(o.profitbtc > 0),0),
                COALESCE(SUM(CASE WHEN o.profitbtc > 0 THEN o.profitbtc END),0),
                COALESCE(SUM(CASE WHEN o.profitbtc <= 0 THEN -o.profitbtc END),0),
                COALESCE(MAX(o.profitbtc),0), COALESCE(MIN(o.profitbtc),0),
                ''
         FROM {src} WHERE o.strategyid = ?3
         GROUP BY k ORDER BY 8 DESC"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to, strategy_id], group_from_row)
        .map_err(|e| read_fail(CTX, e))?;
    let mut coins = Vec::new();
    for row in rows {
        coins.push(row.map_err(|e| read_fail(CTX, e))?);
    }
    drop(stmt);

    // Последние сделки (новые первыми).
    let sql = format!(
        "SELECT o.closedate, COALESCE(o.coin,''), o.core_name, o.core_name,
                COALESCE(o.profitbtc,0), COALESCE(o.isshort,0)
         FROM {src} WHERE o.strategyid = ?3
         ORDER BY o.closedate DESC LIMIT 10"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
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
        .map_err(|e| read_fail(CTX, e))?;
    let mut last = Vec::new();
    for row in rows {
        last.push(row.map_err(|e| read_fail(CTX, e))?);
    }

    Ok(StrategyDetail { coins, last })
}

/// Decode one aggregate row ordered as key, labels, counts, profit, and extrema.
fn group_from_row(r: &rusqlite::Row) -> rusqlite::Result<GroupStat> {
    let wsum: f64 = r.get(9)?;
    let lsum: f64 = r.get(10)?;
    Ok(GroupStat {
        key: r.get(0)?,
        name: r.get(1)?,
        kind: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        core: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
        cores_n: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
        alive: r.get(5)?,
        n: r.get(6)?,
        profit: r.get(7)?,
        wins: r.get(8)?,
        pf: if lsum > 0.0 {
            wsum / lsum
        } else if wsum > 0.0 {
            99.0
        } else {
            0.0
        },
        best: r.get(11)?,
        worst: r.get(12)?,
        lastedit: r.get::<_, Option<String>>(13)?.unwrap_or_default(),
    })
}

/// Attach the optional strategies database used to enrich strategy names.
fn attach_strategies(conn: &Connection) -> bool {
    let path = crate::config::paths::strategies_db_path();
    if !path.exists() {
        return false;
    }
    let sql = format!(
        "ATTACH DATABASE '{}' AS strat",
        path.to_string_lossy().replace('\'', "''")
    );
    // An absent file is normal and silent; failure to attach an existing file
    // is logged as a real enrichment fault.
    match conn.execute(&sql, []) {
        Ok(_) => true,
        Err(e) => {
            log::warn!("analytics: strategies.sqlite не подключилась: {e}");
            false
        }
    }
}

/// Scan period trades by `closedate` into sequence metrics, buckets, and UTC hours.
///
/// `src` is the filtered source built by [`unified_from`]. Any query or
/// row-conversion failure aborts the complete aggregation.
fn scan_period(
    conn: &Connection,
    src: &str,
    from: i64,
    to: i64,
    bucket: i64,
) -> ReadResult<(PeriodStats, Vec<DayPoint>, [(f64, i64); 24])> {
    const CTX: &str = "analytics: scan_period";
    let mut st = PeriodStats::default();
    let mut days: Vec<DayPoint> = Vec::new();
    let mut hours = [(0.0f64, 0i64); 24];
    let sql = format!(
        "SELECT o.closedate, COALESCE(o.buydate, o.closedate), COALESCE(o.profitbtc, 0)
         FROM {src} ORDER BY o.closedate"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![from, to], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, f64>(2)?,
            ))
        })
        .map_err(|e| read_fail(CTX, e))?;

    let (mut wsum, mut lsum) = (0.0f64, 0.0f64);
    let (mut cum, mut peak) = (0.0f64, 0.0f64);
    let (mut cur_w, mut cur_l) = (0i64, 0i64);
    let mut dur_ms_total = 0i64;
    for row in rows {
        // Every column here moves a number, so no row is skippable: dropping
        // one would silently understate n / profit / pf / max_dd.
        let (close, buy, profit) = row.map_err(|e| read_fail(CTX, e))?;
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
            _ => days.push(DayPoint {
                start,
                profit,
                trades: 1,
            }),
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
                filled.push(DayPoint {
                    start: t,
                    profit: 0.0,
                    trades: 0,
                });
            }
            t += bucket;
        }
        days = filled;
    }
    Ok((st, days, hours))
}

/// Scan core profit AND trade count into the `days` buckets, sorting profitable cores
/// first. Both series share one pass and one grid, so a bucket's profit and its count can
/// never come from different rows.
///
/// Any query or row-conversion failure aborts the complete series.
fn core_series(
    conn: &Connection,
    src: &str,
    q: &Query,
    days: &[DayPoint],
    bucket: i64,
) -> ReadResult<Vec<CoreSeries>> {
    const CTX: &str = "analytics: core_series";
    let Some(t0) = days.first().map(|d| d.start) else {
        return Ok(Vec::new());
    };
    let nb = days.len();
    let sql = format!(
        "SELECT o.core_uid, COALESCE(o.core_name,''), o.closedate,
                COALESCE(o.profitbtc,0)
         FROM {src}"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            // core_uid as Option: `unified_from` projects NULL for a source that has no
            // such column, and a hard i64 read there aborts core_series — which throws the
            // WHOLE summary away, `days` included, over one unattributable row.
            Ok((
                r.get::<_, Option<i64>>(0)?.unwrap_or(0) as u64,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, f64>(3)?,
            ))
        })
        .map_err(|e| read_fail(CTX, e))?;
    let mut map: std::collections::HashMap<u64, (String, Vec<f64>, Vec<i64>)> =
        std::collections::HashMap::new();
    for row in rows {
        // core_uid / closedate / profitbtc all move numbers here.
        let (uid, name, close, profit) = row.map_err(|e| read_fail(CTX, e))?;
        let idx = (((close - t0) / bucket).max(0) as usize).min(nb - 1);
        let e = map
            .entry(uid)
            .or_insert_with(|| (name.clone(), vec![0.0; nb], vec![0i64; nb]));
        if e.0.is_empty() {
            e.0 = name;
        }
        e.1[idx] += profit;
        e.2[idx] += 1;
    }
    let mut out: Vec<CoreSeries> = map
        .into_iter()
        .map(|(uid, (name, per_bucket, per_bucket_trades))| {
            let total = per_bucket.iter().sum();
            let trades = per_bucket_trades.iter().sum();
            CoreSeries {
                uid,
                name,
                per_bucket,
                per_bucket_trades,
                total,
                trades,
            }
        })
        .collect();
    out.sort_by(|a, b| b.total.total_cmp(&a.total));
    Ok(out)
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

/// Return the five best or worst period trades, failing if any ranked row is unreadable.
fn top_trades(
    conn: &Connection,
    src: &str,
    q: &Query,
    has_names: bool,
    best: bool,
) -> ReadResult<Vec<TopTrade>> {
    const CTX: &str = "analytics: top_trades";
    let order = if best { "DESC" } else { "ASC" };
    let name = strategy_name_expr(has_names);
    let sql = format!(
        "SELECT o.closedate, COALESCE(o.coin,''), {name}, o.core_name,
                COALESCE(o.profitbtc,0), COALESCE(o.isshort,0)
         FROM {src} WHERE o.profitbtc IS NOT NULL
         ORDER BY o.profitbtc {order} LIMIT 5"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok(TopTrade {
                closedate: r.get(0)?,
                coin: r.get(1)?,
                strategy: r.get(2)?,
                core_name: r.get(3)?,
                profit: r.get(4)?,
                is_short: r.get::<_, i64>(5)? != 0,
            })
        })
        .map_err(|e| read_fail(CTX, e))?;
    let mut out = Vec::new();
    for row in rows {
        // The ranked row carries closedate/profitbtc/isshort, so it is metric-
        // bearing end to end: skipping it would silently drop the extreme trade
        // the user opened this widget to see.
        out.push(row.map_err(|e| read_fail(CTX, e))?);
    }
    Ok(out)
}

/// Group the period by strategy id or coin, sorted by descending profit.
///
/// Any query or aggregate-row failure aborts the complete grouping.
fn groups(
    conn: &Connection,
    src: &str,
    q: &Query,
    has_names: bool,
    by_strategy: bool,
) -> ReadResult<Vec<GroupStat>> {
    const CTX: &str = "analytics: groups";
    // Ключ стратегии — `id@core_uid`: разбивка ПО ЯДРАМ (видно работу стратегии на
    // каждом ядре отдельно); переименования не плодят группы, одноимённые разные
    // стратегии не сливаются; имя — только подпись.
    let (key, name, kind, alive, lastedit) = if by_strategy {
        (
            "CAST(o.strategyid AS TEXT) || '@' || CAST(o.core_uid AS TEXT)".to_string(),
            format!("MAX({})", strategy_name_expr(has_names)),
            // Тип (SignalType) из текущей версии стратегии (JSON1 доступен —
            // rusqlite bundled).
            if has_names {
                "MAX(COALESCE((SELECT json_extract(v.raw_json, '$.SignalType')
                               FROM strat.strategy_versions v
                               WHERE v.core_uid = o.core_uid
                                 AND v.strategy_id = o.strategyid
                                 AND v.valid_to IS NULL), ''))"
            } else {
                "''"
            },
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
            // Last edit date: the `LastEditDate` field of the strategy's current version.
            if has_names {
                "MAX(COALESCE((SELECT json_extract(v.raw_json, '$.LastEditDate')
                               FROM strat.strategy_versions v
                               WHERE v.core_uid = o.core_uid
                                 AND v.strategy_id = o.strategyid
                                 AND v.valid_to IS NULL), ''))"
            } else {
                "''"
            },
        )
    } else {
        let coin = "COALESCE(o.coin,'')".to_string();
        (coin.clone(), coin, "''", "NULL", "''")
    };
    let sql = format!(
        "SELECT {key} AS k, {name}, {kind}, MAX(o.core_name), COUNT(DISTINCT o.core_uid),
                {alive},
                COUNT(*), COALESCE(SUM(o.profitbtc),0),
                COALESCE(SUM(o.profitbtc > 0),0),
                COALESCE(SUM(CASE WHEN o.profitbtc > 0 THEN o.profitbtc END),0),
                COALESCE(SUM(CASE WHEN o.profitbtc <= 0 THEN -o.profitbtc END),0),
                COALESCE(MAX(o.profitbtc),0), COALESCE(MIN(o.profitbtc),0),
                {lastedit}
         FROM {src}
         GROUP BY k ORDER BY 8 DESC"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], group_from_row)
        .map_err(|e| read_fail(CTX, e))?;
    let mut out = Vec::new();
    for row in rows {
        // Each row carries the group's COUNT/SUM, so dropping one would
        // understate the table the user reads as complete.
        out.push(row.map_err(|e| read_fail(CTX, e))?);
    }
    Ok(out)
}

/// Regression tests for the read-failure contract: a damaged replica must
/// surface as an error, never as an empty period.
#[cfg(test)]
mod read_failure_tests {
    use super::super::test_support::{
        build_replica, corrupt_leaf_page, remove_db, spread_rows, temp_db,
    };
    use super::*;

    /// Build the minimal real-trade query used by analytics regression tests.
    fn q(from: i64, to: i64) -> Query {
        Query {
            from,
            to,
            cores: Vec::new(),
            side: SideFilter::All,
            emulator: Some(false),
            strategy: None,
            strat_core: None,
        }
    }

    /// A single-day period switches the series grid to HOURS and reports the profit split
    /// by strategy type — and that split must reconcile with the period total, or the two
    /// charts on screen contradict each other.
    #[test]
    fn single_day_uses_hourly_grid_and_kinds_reconcile() {
        let path = temp_db("oneday");
        let day = 1_780_000_000i64 / 86_400 * 86_400;
        // Four trades spread across three different hours of ONE day.
        let conn = build_replica(
            &path,
            &[
                (day + 3_600, 10.0, "BTCUSDT"),
                (day + 3_700, -4.0, "BTCUSDT"),
                (day + 7_200, 6.0, "ETHUSDT"),
                (day + 18_000, -2.0, "ETHUSDT"),
            ],
        );

        let s = summary_on(&conn, &q(day, day + 86_400), false).expect("healthy DB reads");
        assert_eq!(s.bucket_secs, 3_600, "a day or less → hourly grid");
        // The grid steps by exactly an hour with no holes (empty hours are zero-filled —
        // a chart's X axis has to be continuous), and three of them hold the trades.
        assert!(
            s.days.windows(2).all(|w| w[1].start - w[0].start == 3_600),
            "grid step must be one hour: {:?}",
            s.days
        );
        assert_eq!(
            s.days.iter().filter(|d| d.trades > 0).count(),
            3,
            "three hours with trades: {:?}",
            s.days
        );

        // Without the strategies DB every trade folds into ONE unnamed type — the chart
        // still has something to draw instead of vanishing.
        assert_eq!(s.kinds.len(), 1, "no strategies DB → a single group");
        assert_eq!(s.kinds[0].kind, "");

        // The split must add up to the period total, both in money and in count.
        let ksum: f64 = s.kinds.iter().map(|k| k.profit).sum();
        let kn: i64 = s.kinds.iter().map(|k| k.trades).sum();
        assert!(
            (ksum - s.cur.profit).abs() < 1e-9,
            "Σ of types {ksum} != period total {}",
            s.cur.profit
        );
        assert_eq!(kn, s.cur.n, "Σ of type trades != the period's n");

        // And inside a type, the per-core rows the popup lists must add up to its own bar.
        let csum: f64 = s.kinds[0].cores.iter().map(|c| c.profit).sum();
        assert!(
            (csum - s.kinds[0].profit).abs() < 1e-9,
            "Σ of cores {csum} != the type's profit {}",
            s.kinds[0].profit
        );

        // A longer period keeps the daily grid and computes no type split at all.
        let wide = summary_on(&conn, &q(day - 86_400, day + 86_400), false).expect("reads");
        assert_eq!(wide.bucket_secs, 86_400, "two days → daily grid");
        assert!(wide.kinds.is_empty(), "no type split on a long period");

        drop(conn);
        remove_db(&path);
    }

    /// A healthy replica retains exact summary metrics and empty-period semantics.
    #[test]
    fn healthy_summary_exact_values() {
        let path = temp_db("healthy");
        // Four same-day trades: +10, -4, +6, -2 => profit 10 and two wins.
        let day = 1_780_000_000i64 / 86_400 * 86_400 + 3_600;
        let conn = build_replica(
            &path,
            &[
                (day, 10.0, "BTCUSDT"),
                (day + 60, -4.0, "BTCUSDT"),
                (day + 120, 6.0, "ETHUSDT"),
                (day + 180, -2.0, "ETHUSDT"),
            ],
        );

        let s = summary_on(&conn, &q(day - 86_400, day + 86_400), false)
            .expect("здоровая БД должна читаться");
        assert_eq!(s.cur.n, 4);
        assert_eq!(s.cur.wins, 2);
        assert_eq!(s.cur.losses, 2);
        assert!(
            (s.cur.profit - 10.0).abs() < 1e-9,
            "profit={}",
            s.cur.profit
        );
        assert!((s.cur.winrate() - 50.0).abs() < 1e-9);
        // Profit factor is total wins divided by total losses: 16 / 6.
        assert!((s.cur.pf - 16.0 / 6.0).abs() < 1e-9, "pf={}", s.cur.pf);
        // Cumulative profit 10 -> 6 -> 12 -> 10 has a maximum drawdown of 4.
        assert!((s.cur.max_dd - 4.0).abs() < 1e-9, "max_dd={}", s.cur.max_dd);
        assert!((s.cur.avg - 2.5).abs() < 1e-9);
        assert_eq!(s.coins.len(), 2, "две монеты");
        assert_eq!(s.cores, vec![(1u64, "CORE-A".to_string())]);
        assert_eq!(s.best.len(), 4);

        // A genuinely empty period succeeds with zero counters.
        let empty = summary_on(&conn, &q(day - 10 * 86_400, day - 9 * 86_400), false)
            .expect("пустой период — успешное чтение");
        assert_eq!(empty.cur.n, 0);

        drop(conn);
        remove_db(&path);
    }

    /// Index-page corruption surfaces as an error rather than an empty period.
    #[test]
    fn corrupt_replica_surfaces_error_not_empty() {
        let path = temp_db("corrupt");
        // Enough rows keep the target index leaf away from the header page so
        // corruption surfaces during the period scan rather than file opening.
        let day = 1_780_000_000i64 / 86_400 * 86_400;
        let conn = build_replica(&path, &spread_rows(day, 2000));

        // Prove the fixture is healthy before introducing damage.
        let before = summary_on(&conn, &q(day - 86_400, day + 10 * 86_400), false)
            .expect("до порчи БД читается");
        assert_eq!(before.cur.n, 2000);

        // The scan must use the index whose leaf page is about to be damaged.
        let plan: String = conn
            .query_row(
                "EXPLAIN QUERY PLAN SELECT closedate FROM orders_rep
                 WHERE closedate >= 1 AND closedate < 2 AND closedate > 0",
                [],
                |r| r.get(3),
            )
            .unwrap();
        assert!(
            plan.contains("idx_rep_closedate"),
            "план без индекса: {plan}"
        );

        corrupt_leaf_page(conn, &path, "idx_rep_closedate");

        // The intact header allows opening; the period read reaches the damage.
        let conn = Connection::open(&path).expect("битая БД всё ещё открывается");
        let wide = q(day - 86_400, day + 10 * 86_400);

        // Pin the period scan itself so another query cannot mask skipped rows
        // by failing later in the summary pipeline.
        let src = unified_from(&conn, &wide)
            .expect("схема читается")
            .expect("источник есть");
        assert!(
            scan_period(&conn, &src, wide.from, wide.to, 86_400).is_err(),
            "скан периода обязан вернуть ошибку, а не усечённую статистику"
        );

        let res = summary_on(&conn, &wide, false);

        assert!(
            !matches!(res, Ok(_)),
            "ошибка чтения не должна превращаться в успешный — в том числе \
             пустой или частичный — период: это и есть чинимый баг"
        );
        match res {
            Err(ReadFail::Failed { kind, .. }) => assert_eq!(
                kind,
                super::super::FailKind::Corrupt,
                "порча должна классифицироваться как Corrupt"
            ),
            Err(ReadFail::NotReady) => {
                panic!("порча не должна выглядеть как «реплика не готова»")
            }
            Ok(_) => unreachable!("уже проверено выше"),
        }

        remove_db(&path);
    }
}

#[cfg(test)]
mod tests {
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
        let empty =
            hour_profile_one(&c, &Query::default(), D0 - 5 * 86_400, D0 - 4 * 86_400).unwrap();
        assert!(empty.iter().all(|h| h.trades == 0));
    }
}
