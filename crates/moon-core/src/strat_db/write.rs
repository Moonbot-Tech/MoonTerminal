//! Запись полного набора стратегий ядра: head-UPSERT + версии по контент-диффу.
//!
//! Контент = дамп полей БЕЗ игнор-полей (косметика) и `__`-меты. Версия пишется
//! только при смене контента; тогл галки/цвета/звука обновляет только head.
//! Помимо old/new значений (`changed_json`) версия несёт мету: вид изменения,
//! происхождение (наша правка/внешняя), пропуск серверных правок (`ver_gap`),
//! состояние стратегии на момент (`checked_at`).
//!
//! Темпоральность: `valid_from`/`valid_to` (unix-ms) — «сделка → её версия»
//! решается range-join'ом по `buydate` (вход совершён под параметрами версии).

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use rusqlite::{Connection, OptionalExtension};
use serde_json::{Map, Value};

use super::StratDump;
use crate::config::storage::StrategiesStoreCfg;
use crate::util::now_unix_ms_i64 as now_ms;

/// Кэш head-строки: пропускать бездельные UPDATE'ы (полный набор приходит на
/// каждый чих — тогл одной галки не должен трогать 500 строк).
struct Head {
    content_hash: i64,
    head_hash: i64,
    deleted: bool,
    server_ver: i32,
}

pub(super) struct State {
    ignore: HashSet<String>,
    heads: HashMap<(i64, i64), Head>,
}

pub(super) fn init(conn: &Connection, cfg: &StrategiesStoreCfg) -> rusqlite::Result<State> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    let _ = conn.busy_timeout(std::time::Duration::from_secs(3));
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS strategies (
            core_uid     INTEGER NOT NULL,
            strategy_id  INTEGER NOT NULL,
            core_name    TEXT NOT NULL DEFAULT '',
            name         TEXT NOT NULL DEFAULT '',
            kind         TEXT NOT NULL DEFAULT '',
            kind_ordinal INTEGER NOT NULL DEFAULT 0,
            folder_path  TEXT NOT NULL DEFAULT '',
            is_short     INTEGER NOT NULL DEFAULT 0,
            checked      INTEGER NOT NULL DEFAULT 0,
            server_ver   INTEGER NOT NULL DEFAULT 0,
            server_ms    INTEGER NOT NULL DEFAULT 0,
            deleted      INTEGER NOT NULL DEFAULT 0,
            content_hash INTEGER NOT NULL DEFAULT 0,
            head_hash    INTEGER NOT NULL DEFAULT 0,
            updated_ms   INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (core_uid, strategy_id));
        CREATE INDEX IF NOT EXISTS idx_strat_name ON strategies(core_uid, name);
        CREATE TABLE IF NOT EXISTS strategy_versions (
            id           INTEGER PRIMARY KEY,
            core_uid     INTEGER NOT NULL,
            strategy_id  INTEGER NOT NULL,
            valid_from   INTEGER NOT NULL,
            valid_to     INTEGER,
            change_kind  TEXT NOT NULL,
            origin       TEXT,
            n_changed    INTEGER NOT NULL DEFAULT 0,
            ver_gap      INTEGER NOT NULL DEFAULT 0,
            server_ver   INTEGER NOT NULL DEFAULT 0,
            server_ms    INTEGER NOT NULL DEFAULT 0,
            checked_at   INTEGER NOT NULL DEFAULT 0,
            raw_json     TEXT NOT NULL,
            changed_json TEXT,
            UNIQUE (core_uid, strategy_id, valid_from));
        CREATE INDEX IF NOT EXISTS idx_sv_lookup
            ON strategy_versions(core_uid, strategy_id, valid_from DESC);
        CREATE TABLE IF NOT EXISTS app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )?;
    // Кэш head-строк с диска — writer не делает SELECT на каждую стратегию.
    let mut heads = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT core_uid, strategy_id, content_hash, head_hash, deleted, server_ver
             FROM strategies",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                (r.get::<_, i64>(0)?, r.get::<_, i64>(1)?),
                Head {
                    content_hash: r.get(2)?,
                    head_hash: r.get(3)?,
                    deleted: r.get::<_, i64>(4)? != 0,
                    server_ver: r.get(5)?,
                },
            ))
        })?;
        for row in rows {
            let (k, v) = row?;
            heads.insert(k, v);
        }
    }
    Ok(State {
        ignore: cfg.ignore_fields.iter().cloned().collect(),
        heads,
    })
}

/// Применить полный набор стратегий ядра одной транзакцией.
/// Возвращает число реальных изменений (версии + head-правки + удаления).
pub(super) fn apply_full_set(
    conn: &Connection,
    st: &mut State,
    core_uid: u64,
    core_name: &str,
    initial: bool,
    dumps: &[StratDump],
) -> anyhow::Result<usize> {
    let uid = core_uid as i64;
    let now = now_ms();
    let mut writes = 0usize;
    let tx = conn.unchecked_transaction()?;

    let mut present: HashSet<i64> = HashSet::with_capacity(dumps.len());
    for d in dumps {
        present.insert(d.strategy_id);
        let key = (uid, d.strategy_id);
        let stripped = strip(&d.fields, &st.ignore);
        let content_hash = hash_of(&stripped);
        let head_hash = head_hash_of(d);
        let origin: Option<&str> = if initial {
            None // первый набор после (ре)коннекта: правка могла случиться оффлайн
        } else if d.local_edit {
            Some("local")
        } else {
            Some("external")
        };

        let (need_version, change_kind, ver_gap) = match st.heads.get(&key) {
            None => (true, "created", false),
            Some(h) if h.deleted => (true, "restored", false),
            Some(h) if h.content_hash != content_hash => {
                (true, "params", d.server_ver > h.server_ver.saturating_add(1))
            }
            Some(h) if h.head_hash != head_hash => (false, "", false),
            Some(_) => continue, // ничего не изменилось
        };

        if need_version {
            let raw_json = serde_json::to_string(&Value::Object(d.fields.clone()))?;
            // Дифф со ПОСЛЕДНЕЙ версией (по контенту, без косметики) — created
            // без прошлого дампа остаётся с NULL.
            let prev: Option<(i64, String)> = tx
                .query_row(
                    "SELECT valid_from, raw_json FROM strategy_versions
                     WHERE core_uid=?1 AND strategy_id=?2
                     ORDER BY valid_from DESC LIMIT 1",
                    rusqlite::params![uid, d.strategy_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let (changed_json, n_changed, renamed) = match &prev {
                None => (None, 0usize, false),
                Some((_, prev_raw)) => {
                    let (diff, renamed) = diff_maps(prev_raw, &stripped, &st.ignore);
                    let n = diff.len();
                    if n == 0 && change_kind == "params" {
                        // Хэш дрогнул, а контент тот же (коллизия/порядок) — не версия.
                        (None, 0, false)
                    } else {
                        (Some(serde_json::to_string(&Value::Object(diff))?), n, renamed)
                    }
                }
            };
            if change_kind == "params" && n_changed == 0 && prev.is_some() {
                // Ложная тревога хэша: обновим кэш и head, версию не пишем.
                upsert_head(&tx, uid, core_name, d, content_hash, head_hash, now)?;
                st.heads.insert(key, Head { content_hash, head_hash, deleted: false, server_ver: d.server_ver });
                writes += 1;
                continue;
            }
            let kind = if change_kind == "params" && renamed { "renamed" } else { change_kind };
            // valid_from строго возрастает в пределах стратегии (UNIQUE-ключ).
            let valid_from = prev.as_ref().map(|(f, _)| now.max(f + 1)).unwrap_or(now);
            tx.execute(
                "UPDATE strategy_versions SET valid_to=?3
                 WHERE core_uid=?1 AND strategy_id=?2 AND valid_to IS NULL",
                rusqlite::params![uid, d.strategy_id, valid_from],
            )?;
            tx.execute(
                "INSERT INTO strategy_versions (core_uid, strategy_id, valid_from, valid_to,
                    change_kind, origin, n_changed, ver_gap, server_ver, server_ms,
                    checked_at, raw_json, changed_json)
                 VALUES (?1,?2,?3,NULL,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                rusqlite::params![
                    uid,
                    d.strategy_id,
                    valid_from,
                    kind,
                    origin,
                    n_changed as i64,
                    ver_gap as i64,
                    d.server_ver,
                    d.server_ms,
                    d.checked as i64,
                    raw_json,
                    changed_json,
                ],
            )?;
            // Лимит — живой атомик (правка из «Хранилища» действует сразу).
            let limit = super::version_limit();
            if limit > 0 {
                tx.execute(
                    "DELETE FROM strategy_versions
                     WHERE core_uid=?1 AND strategy_id=?2 AND id NOT IN (
                        SELECT id FROM strategy_versions
                        WHERE core_uid=?1 AND strategy_id=?2
                        ORDER BY valid_from DESC LIMIT ?3)",
                    rusqlite::params![uid, d.strategy_id, limit as i64],
                )?;
            }
            log::info!(
                "стратегии(db): {core_name} «{}» → версия ({kind}, изменено {n_changed})",
                d.name
            );
        }

        upsert_head(&tx, uid, core_name, d, content_hash, head_hash, now)?;
        st.heads.insert(key, Head { content_hash, head_hash, deleted: false, server_ver: d.server_ver });
        writes += 1;
    }

    // Пропавшие из ПОЛНОГО набора — удалены на сервере (soft-delete, история
    // остаётся). Пустой набор не трогаем: это скорее недособранный стейт после
    // коннекта, чем «удалили всё».
    if !dumps.is_empty() {
        let gone: Vec<i64> = st
            .heads
            .iter()
            .filter(|((cu, sid), h)| *cu == uid && !h.deleted && !present.contains(sid))
            .map(|((_, sid), _)| *sid)
            .collect();
        for sid in gone {
            tx.execute(
                "UPDATE strategies SET deleted=1, updated_ms=?3 WHERE core_uid=?1 AND strategy_id=?2",
                rusqlite::params![uid, sid, now],
            )?;
            tx.execute(
                "UPDATE strategy_versions SET valid_to=?3
                 WHERE core_uid=?1 AND strategy_id=?2 AND valid_to IS NULL",
                rusqlite::params![uid, sid, now],
            )?;
            if let Some(h) = st.heads.get_mut(&(uid, sid)) {
                h.deleted = true;
            }
            writes += 1;
            log::info!("стратегии(db): {core_name} id={sid} удалена на сервере (история сохранена)");
        }
    }

    tx.commit()?;
    Ok(writes)
}

fn upsert_head(
    conn: &Connection,
    uid: i64,
    core_name: &str,
    d: &StratDump,
    content_hash: i64,
    head_hash: i64,
    now: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO strategies (core_uid, strategy_id, core_name, name, kind, kind_ordinal,
            folder_path, is_short, checked, server_ver, server_ms, deleted,
            content_hash, head_hash, updated_ms)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,0,?12,?13,?14)
         ON CONFLICT (core_uid, strategy_id) DO UPDATE SET
            core_name=excluded.core_name, name=excluded.name, kind=excluded.kind,
            kind_ordinal=excluded.kind_ordinal, folder_path=excluded.folder_path,
            is_short=excluded.is_short, checked=excluded.checked,
            server_ver=excluded.server_ver, server_ms=excluded.server_ms, deleted=0,
            content_hash=excluded.content_hash, head_hash=excluded.head_hash,
            updated_ms=excluded.updated_ms",
        rusqlite::params![
            uid,
            d.strategy_id,
            core_name,
            d.name,
            d.kind,
            d.kind_ordinal as i64,
            d.folder_path,
            d.is_short as i64,
            d.checked as i64,
            d.server_ver,
            d.server_ms,
            content_hash,
            head_hash,
            now,
        ],
    )?;
    Ok(())
}

/// Копия дампа без игнор-полей и `__`-меты — «контент» для сравнения версий.
fn strip(m: &Map<String, Value>, ignore: &HashSet<String>) -> Map<String, Value> {
    m.iter()
        .filter(|(k, _)| !k.starts_with("__") && !ignore.contains(k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Канонический хэш контента: `serde_json::Map` — BTreeMap, сериализация
/// отсортирована по ключам и стабильна между запусками.
fn hash_of(stripped: &Map<String, Value>) -> i64 {
    let s = serde_json::to_string(&Value::Object(stripped.clone())).unwrap_or_default();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish() as i64
}

/// Хэш head-полей (имя/папка/галка/вид/…) — пропуск бездельных UPDATE'ов.
fn head_hash_of(d: &StratDump) -> i64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    d.name.hash(&mut h);
    d.kind_ordinal.hash(&mut h);
    d.folder_path.hash(&mut h);
    d.is_short.hash(&mut h);
    d.checked.hash(&mut h);
    d.server_ver.hash(&mut h);
    d.server_ms.hash(&mut h);
    h.finish() as i64
}

/// Дифф контента: `{"Поле": {"old": …, "new": …}}` (добавленное — old=null,
/// удалённое — new=null). Второй элемент — «изменилось ТОЛЬКО StrategyName».
fn diff_maps(
    prev_raw: &str,
    new_stripped: &Map<String, Value>,
    ignore: &HashSet<String>,
) -> (Map<String, Value>, bool) {
    let prev_stripped = match serde_json::from_str::<Value>(prev_raw) {
        Ok(Value::Object(m)) => strip(&m, ignore),
        _ => Map::new(),
    };
    let mut out = Map::new();
    for (k, new_v) in new_stripped {
        match prev_stripped.get(k) {
            Some(old_v) if old_v == new_v => {}
            old => {
                let mut pair = Map::new();
                pair.insert("old".into(), old.cloned().unwrap_or(Value::Null));
                pair.insert("new".into(), new_v.clone());
                out.insert(k.clone(), Value::Object(pair));
            }
        }
    }
    for (k, old_v) in &prev_stripped {
        if !new_stripped.contains_key(k) {
            let mut pair = Map::new();
            pair.insert("old".into(), old_v.clone());
            pair.insert("new".into(), Value::Null);
            out.insert(k.clone(), Value::Object(pair));
        }
    }
    let renamed = out.len() == 1 && out.contains_key("StrategyName");
    (out, renamed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> StrategiesStoreCfg {
        StrategiesStoreCfg::default()
    }

    fn dump(id: i64, name: &str, tp: i64, comment: &str) -> StratDump {
        let mut fields = Map::new();
        fields.insert("StrategyName".into(), Value::from(name));
        fields.insert("TakeProfit".into(), Value::from(tp));
        fields.insert("Comment".into(), Value::from(comment)); // косметика (игнор)
        StratDump {
            strategy_id: id,
            name: name.into(),
            kind: "Drops".into(),
            kind_ordinal: 2,
            folder_path: "f".into(),
            is_short: false,
            checked: false,
            server_ver: 1,
            server_ms: 1000,
            fields,
            local_edit: false,
        }
    }

    fn versions(conn: &Connection, id: i64) -> Vec<(String, i64, Option<i64>)> {
        let mut stmt = conn
            .prepare(
                "SELECT change_kind, n_changed, valid_to FROM strategy_versions
                 WHERE strategy_id=?1 ORDER BY valid_from",
            )
            .unwrap();
        stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn setup() -> (Connection, State) {
        let conn = Connection::open_in_memory().unwrap();
        let st = init(&conn, &cfg()).unwrap();
        (conn, st)
    }

    #[test]
    fn create_then_cosmetic_then_param() {
        let (conn, mut st) = setup();
        // Создание → версия created.
        apply_full_set(&conn, &mut st, 7, "core", true, &[dump(1, "A", 5, "x")]).unwrap();
        assert_eq!(versions(&conn, 1).len(), 1);
        assert_eq!(versions(&conn, 1)[0].0, "created");
        // Косметика (Comment) — версия НЕ создаётся.
        apply_full_set(&conn, &mut st, 7, "core", false, &[dump(1, "A", 5, "y")]).unwrap();
        assert_eq!(versions(&conn, 1).len(), 1);
        // Реальная правка TakeProfit → версия params, прошлая закрыта valid_to.
        apply_full_set(&conn, &mut st, 7, "core", false, &[dump(1, "A", 7, "y")]).unwrap();
        let v = versions(&conn, 1);
        assert_eq!(v.len(), 2);
        assert_eq!(v[1].0, "params");
        assert_eq!(v[1].1, 1, "изменено одно поле");
        assert!(v[0].2.is_some(), "первая версия закрыта");
        assert!(v[1].2.is_none(), "текущая открыта");
    }

    #[test]
    fn rename_only_marks_renamed() {
        let (conn, mut st) = setup();
        apply_full_set(&conn, &mut st, 7, "core", true, &[dump(1, "A", 5, "x")]).unwrap();
        apply_full_set(&conn, &mut st, 7, "core", false, &[dump(1, "B", 5, "x")]).unwrap();
        assert_eq!(versions(&conn, 1)[1].0, "renamed");
    }

    #[test]
    fn missing_marks_deleted_and_reappear_restores() {
        let (conn, mut st) = setup();
        apply_full_set(
            &conn, &mut st, 7, "core", true,
            &[dump(1, "A", 5, "x"), dump(2, "B", 3, "x")],
        )
        .unwrap();
        // Стратегия 2 пропала из полного набора → deleted, версия закрыта.
        apply_full_set(&conn, &mut st, 7, "core", false, &[dump(1, "A", 5, "x")]).unwrap();
        let del: i64 = conn
            .query_row("SELECT deleted FROM strategies WHERE strategy_id=2", [], |r| r.get(0))
            .unwrap();
        assert_eq!(del, 1);
        assert!(versions(&conn, 2)[0].2.is_some(), "версия удалённой закрыта");
        // Вернулась → restored.
        apply_full_set(
            &conn, &mut st, 7, "core", false,
            &[dump(1, "A", 5, "x"), dump(2, "B", 3, "x")],
        )
        .unwrap();
        let v = versions(&conn, 2);
        assert_eq!(v.last().unwrap().0, "restored");
    }

    #[test]
    fn empty_set_does_not_mass_delete() {
        let (conn, mut st) = setup();
        apply_full_set(&conn, &mut st, 7, "core", true, &[dump(1, "A", 5, "x")]).unwrap();
        apply_full_set(&conn, &mut st, 7, "core", false, &[]).unwrap();
        let del: i64 = conn
            .query_row("SELECT deleted FROM strategies WHERE strategy_id=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(del, 0, "пустой набор не считается «удалили всё»");
    }

    #[test]
    fn state_cache_survives_reload() {
        let (conn, mut st) = setup();
        apply_full_set(&conn, &mut st, 7, "core", true, &[dump(1, "A", 5, "x")]).unwrap();
        // «Рестарт writer'а»: state перечитывается с диска, дедуп не ломается.
        let mut st2 = init(&conn, &cfg()).unwrap();
        apply_full_set(&conn, &mut st2, 7, "core", false, &[dump(1, "A", 5, "x")]).unwrap();
        assert_eq!(versions(&conn, 1).len(), 1, "эхо после рестарта не плодит версию");
    }
}
