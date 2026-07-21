//! Ленивая статистика по версиям стратегии: профит/сделки за окно действия
//! версии (`orders_rep` по `buydate`, атрибуция входа — параметры версии).
//!
//! Замороженный агрегат в момент закрытия версии НЕЛЬЗЯ (открытые сделки, лаг
//! репликации, RowUpsert задним числом) — поэтому кэш `version_stats` в
//! strategies.sqlite заполняется ЛЕНИВО при чтении и инвалидируется сторожком:
//! `as_of` = MAX(last_update_at) строк стратегии в реплике на момент расчёта.
//! Любая новая/правленая сделка стратегии двигает max → пересчёт только её
//! версий; у затихшей стратегии кэш валиден вечно. Плюс open_left>0 (в окне
//! остались открытые сделки) и открытая версия — всегда пересчёт.
//!
//! Вызывать ТОЛЬКО с background executor: тут SQLite-запросы по реплике.

use rusqlite::{Connection, OptionalExtension};

use crate::config::paths;
use crate::util::now_unix_ms_i64 as now_ms;

/// Версия стратегии + её статистика для UI (список в окне «Стратегии»).
#[derive(Clone, Debug)]
pub struct VersionInfo {
    /// unix-ms начала действия (ключ версии).
    pub valid_from: i64,
    /// unix-ms конца действия; None = текущая.
    pub valid_to: Option<i64>,
    pub change_kind: String,
    pub origin: Option<String>,
    pub n_changed: i64,
    /// Сделки, вошедшие в окно версии (по buydate), и их суммарный профит
    /// (котировка, из profitbtc). Открытые (без closedate) не в профите.
    pub trades: i64,
    pub profit: f64,
    pub open_left: i64,
}

fn open_rw() -> Option<Connection> {
    let path = paths::strategies_db_path();
    if !path.exists() {
        return None;
    }
    let conn = Connection::open(&path).ok()?;
    let _ = conn.busy_timeout(std::time::Duration::from_secs(3));
    Some(conn)
}

/// ATTACH реплики отчётов (read-only не критичен: пишем только в version_stats
/// основного файла). Реплики может не быть — статистика тогда нулевая.
fn attach_reports(conn: &Connection) -> bool {
    let rep = paths::reports_db_path();
    if !rep.exists() {
        return false;
    }
    let sql = format!(
        "ATTACH DATABASE '{}' AS rep",
        rep.to_string_lossy().replace('\'', "''")
    );
    match conn.execute(&sql, []) {
        Ok(_) => true,
        Err(e) => {
            log::warn!("стратегии(stats): ATTACH реплики не удался: {e}");
            false
        }
    }
}

/// Сторожок свежести: MAX(last_update_at) строк стратегии в реплике (0 — нет
/// строк/колонки). Меняется от любого upsert'а сделки стратегии.
fn max_last_update(conn: &Connection, core_uid: i64, sid: i64) -> i64 {
    conn.query_row(
        "SELECT COALESCE(MAX(last_update_at),0) FROM rep.orders_rep
         WHERE core_uid=?1 AND strategyid=?2",
        rusqlite::params![core_uid, sid],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// Версии стратегии (новые первыми) со статистикой; несвежий кэш пересчитывается
/// и дозаполняется на месте. Пустой список — БД/стратегии нет.
pub fn versions_with_stats(core_uid: u64, strategy_id: i64) -> Vec<VersionInfo> {
    let Some(conn) = open_rw() else {
        return Vec::new();
    };
    let uid = core_uid as i64;
    let has_rep = attach_reports(&conn);
    let _ = conn.execute(
        "CREATE TABLE IF NOT EXISTS version_stats (
            core_uid    INTEGER NOT NULL,
            strategy_id INTEGER NOT NULL,
            valid_from  INTEGER NOT NULL,
            trades      INTEGER NOT NULL DEFAULT 0,
            profit      REAL NOT NULL DEFAULT 0,
            open_left   INTEGER NOT NULL DEFAULT 0,
            as_of       INTEGER NOT NULL DEFAULT 0,
            computed_ms INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (core_uid, strategy_id, valid_from))",
        [],
    );

    let mut out: Vec<VersionInfo> = Vec::new();
    {
        let Ok(mut stmt) = conn.prepare(
            "SELECT v.valid_from, v.valid_to, v.change_kind, v.origin, v.n_changed,
                    s.trades, s.profit, s.open_left, s.as_of
             FROM strategy_versions v
             LEFT JOIN version_stats s
               ON s.core_uid=v.core_uid AND s.strategy_id=v.strategy_id
              AND s.valid_from=v.valid_from
             WHERE v.core_uid=?1 AND v.strategy_id=?2
             ORDER BY v.valid_from DESC",
        ) else {
            return Vec::new();
        };
        let rows = stmt.query_map(rusqlite::params![uid, strategy_id], |r| {
            Ok((
                VersionInfo {
                    valid_from: r.get(0)?,
                    valid_to: r.get(1)?,
                    change_kind: r.get(2)?,
                    origin: r.get(3)?,
                    n_changed: r.get(4)?,
                    trades: r.get::<_, Option<i64>>(5)?.unwrap_or(0),
                    profit: r.get::<_, Option<f64>>(6)?.unwrap_or(0.0),
                    open_left: r.get::<_, Option<i64>>(7)?.unwrap_or(0),
                },
                r.get::<_, Option<i64>>(8)?, // as_of кэша (None = кэша нет)
            ))
        });
        let Ok(rows) = rows else { return Vec::new() };

        let max_lu = if has_rep {
            max_last_update(&conn, uid, strategy_id)
        } else {
            0
        };
        for row in rows.flatten() {
            let (mut info, as_of) = row;
            let stale = as_of != Some(max_lu) || info.open_left > 0 || info.valid_to.is_none();
            if stale && has_rep {
                // buydate в реплике — unix-СЕКУНДЫ, границы версии — ms.
                let from_s = info.valid_from / 1000;
                let to_s = info.valid_to.map(|t| t / 1000);
                let agg: Option<(i64, f64, i64)> = conn
                    .query_row(
                        "SELECT COUNT(*),
                                COALESCE(SUM(CASE WHEN closedate>0 THEN profitbtc END),0),
                                COALESCE(SUM(closedate IS NULL OR closedate<=0),0)
                         FROM rep.orders_rep
                         WHERE core_uid=?1 AND strategyid=?2
                           AND buydate>=?3 AND (?4 IS NULL OR buydate<?4)",
                        rusqlite::params![uid, strategy_id, from_s, to_s],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()
                    .unwrap_or(None);
                if let Some((trades, profit, open_left)) = agg {
                    info.trades = trades;
                    info.profit = profit;
                    info.open_left = open_left;
                    let _ = conn.execute(
                        "INSERT INTO version_stats
                            (core_uid, strategy_id, valid_from, trades, profit,
                             open_left, as_of, computed_ms)
                         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
                         ON CONFLICT (core_uid, strategy_id, valid_from) DO UPDATE SET
                            trades=excluded.trades, profit=excluded.profit,
                            open_left=excluded.open_left, as_of=excluded.as_of,
                            computed_ms=excluded.computed_ms",
                        rusqlite::params![
                            uid,
                            strategy_id,
                            info.valid_from,
                            info.trades,
                            info.profit,
                            info.open_left,
                            max_lu,
                            now_ms(),
                        ],
                    );
                }
            }
            out.push(info);
        }
    }
    out
}

/// Содержимое версии для панелей окна «Стратегии»: полный дамп полей + дифф
/// с предыдущей версией.
pub struct VersionView {
    /// Все поля версии (raw_json) в отображаемых строках — формат как у
    /// `fmt_field` живых стратегий (Yes/No, compact-числа).
    pub fields: Vec<(String, String)>,
    /// Изменённые в ЭТОЙ версии поля: имя → старое значение (display-строка;
    /// пустая = поля раньше не было). Пусто у created/без диффа.
    pub changed: Vec<(String, String)>,
}

/// Поля и дифф версии. None — версии нет.
pub fn version_view(core_uid: u64, strategy_id: i64, valid_from: i64) -> Option<VersionView> {
    let conn = open_rw()?;
    let (raw, changed_raw): (String, Option<String>) = conn
        .query_row(
            "SELECT raw_json, changed_json FROM strategy_versions
             WHERE core_uid=?1 AND strategy_id=?2 AND valid_from=?3",
            rusqlite::params![core_uid as i64, strategy_id, valid_from],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .ok()??;
    let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&raw).ok()?;
    let fields = map
        .into_iter()
        .filter(|(k, _)| !k.starts_with("__"))
        .map(|(k, v)| (k, json_display(&v)))
        .collect();
    // Дифф {"Поле":{"old":…,"new":…}} → (имя, старое значение display).
    let mut changed = Vec::new();
    if let Some(cj) = changed_raw {
        if let Ok(serde_json::Value::Object(m)) = serde_json::from_str(&cj) {
            for (name, pair) in m {
                let old = pair
                    .get("old")
                    .filter(|v| !v.is_null())
                    .map(json_display)
                    .unwrap_or_default();
                changed.push((name, old));
            }
        }
    }
    Some(VersionView { fields, changed })
}

/// JSON-значение поля → строка показа (зеркало `feed::strategies::fmt_field`).
fn json_display(v: &serde_json::Value) -> String {
    use serde_json::Value as J;
    match v {
        J::Bool(b) => if *b { "Yes" } else { "No" }.to_string(),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(u) = n.as_u64() {
                u.to_string()
            } else {
                crate::util::fmt::compact(n.as_f64().unwrap_or(0.0), 6)
            }
        }
        J::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Head-строка стратегии из strategies.sqlite (для показа удалённых, которых
/// уже нет в живом сторе ядра).
#[derive(Clone, Debug, PartialEq)]
pub struct HeadRow {
    pub core_uid: u64,
    pub strategy_id: i64,
    pub name: String,
    pub kind: String,
    pub kind_ordinal: u8,
    pub folder_path: String,
    pub is_short: bool,
}

fn head_from_row(r: &rusqlite::Row) -> rusqlite::Result<HeadRow> {
    Ok(HeadRow {
        core_uid: r.get::<_, i64>(0)? as u64,
        strategy_id: r.get(1)?,
        name: r.get(2)?,
        kind: r.get(3)?,
        kind_ordinal: r.get::<_, i64>(4)? as u8,
        folder_path: r.get(5)?,
        is_short: r.get::<_, i64>(6)? != 0,
    })
}

const HEAD_COLS: &str = "core_uid, strategy_id, name, kind, kind_ordinal, folder_path, is_short";

/// Все удалённые на серверах стратегии (head.deleted=1) — папка «Удалённые»
/// в дереве окна Стратегий. История их версий остаётся доступной.
pub fn deleted_heads() -> Vec<HeadRow> {
    let Some(conn) = super::open_reader() else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {HEAD_COLS} FROM strategies WHERE deleted=1 ORDER BY core_uid, name"
    )) else {
        return Vec::new();
    };
    stmt.query_map([], |r| head_from_row(r))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// Поля ПОСЛЕДНЕЙ версии стратегии (display-строки) — материал восстановления
/// удалённой (`RestoreStrategy`). None — версий нет.
pub fn latest_version_fields(core_uid: u64, strategy_id: i64) -> Option<Vec<(String, String)>> {
    let conn = open_rw()?;
    let vf: i64 = conn
        .query_row(
            "SELECT MAX(valid_from) FROM strategy_versions
             WHERE core_uid=?1 AND strategy_id=?2",
            rusqlite::params![core_uid as i64, strategy_id],
            |r| r.get::<_, Option<i64>>(0),
        )
        .ok()
        .flatten()?;
    version_view(core_uid, strategy_id, vf).map(|v| v.fields)
}

/// Head одной стратегии (живой или удалённой) — база синтетической строки,
/// когда стратегии нет в живом сторе.
pub fn head_row(core_uid: u64, strategy_id: i64) -> Option<HeadRow> {
    let conn = super::open_reader()?;
    conn.query_row(
        &format!("SELECT {HEAD_COLS} FROM strategies WHERE core_uid=?1 AND strategy_id=?2"),
        rusqlite::params![core_uid as i64, strategy_id],
        |r| head_from_row(r),
    )
    .optional()
    .ok()
    .flatten()
}

/// «дд.мм» из unix-ms (UTC) — краткая подпись версии в списке.
pub fn short_date(ms: i64) -> String {
    let days = (ms / 1000).div_euclid(86_400);
    let (_, mo, d) = crate::util::time::civil_from_days(days);
    format!("{d:02}.{mo:02}")
}
