//! Writes a core's complete strategy set: head UPSERTs plus versions based on content diffs.
//!
//! Content is the field dump without configured `ignore_fields` or `__` metadata. A version is
//! written only when that content changes. Changes to tracked head fields update only the head row;
//! a change confined to an ignored field outside the head hash, such as `Comment`, produces no
//! write. Version metadata includes the change kind; `changed_json` holds old/new values when a
//! previous dump exists and may be absent on the first `created` version; `origin` is local,
//! external, or NULL for an initial set; `ver_gap` marks a gap in observed server revisions; and
//! `checked_at` stores the exact checked state.
//!
//! Temporal validity uses `valid_from`/`valid_to` (Unix milliseconds): a range join on `buydate`
//! attributes each trade to a locally observed snapshot-validity window. Because `valid_from` is
//! the local observation time, this attribution does not prove which parameters were active when
//! the position opened.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use rusqlite::{Connection, OptionalExtension};
use serde_json::{Map, Value};

use super::StratDump;
use crate::config::storage::StrategiesStoreCfg;
use crate::util::now_unix_ms_i64 as now_ms;

/// Cached head-row hashes used to skip no-op updates when each small change resends the full set.
/// Toggling one checkbox must not update 500 rows.
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
        -- The tuner queries strategies by id without core_uid (strategy_cores/strategy_filters):
        -- the (core_uid, strategy_id) primary-key prefix cannot serve those queries.
        CREATE INDEX IF NOT EXISTS idx_strat_sid ON strategies(strategy_id);
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
        -- idx_sv_lookup duplicated the UNIQUE constraint's implicit index (the same columns;
        -- SQLite can scan it in descending order), adding needless cost to every version insert.
        DROP INDEX IF EXISTS idx_sv_lookup;
        CREATE TABLE IF NOT EXISTS app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )?;
    // Load the head-row cache from disk so the writer does not query every strategy.
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

/// Applies a core's complete strategy set in one transaction.
/// Returns the number of strategies that were logically changed or deleted.
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
            None // First set after a (re)connect: the edit may have occurred while offline.
        } else if d.local_edit {
            Some("local")
        } else {
            Some("external")
        };

        // Restoring unchanged content (deleted, then restored as it was) does not create a new
        // version. Reopen the latest version (`valid_to=NULL`) and revive the head row; by policy,
        // the deletion interval is folded into the reopened version's validity window.
        if let Some(h) = st.heads.get(&key) {
            if h.deleted && h.content_hash == content_hash {
                tx.execute(
                    "UPDATE strategy_versions SET valid_to=NULL
                     WHERE core_uid=?1 AND strategy_id=?2 AND valid_from=(
                        SELECT MAX(valid_from) FROM strategy_versions
                        WHERE core_uid=?1 AND strategy_id=?2)",
                    rusqlite::params![uid, d.strategy_id],
                )?;
                upsert_head(&tx, uid, core_name, d, content_hash, head_hash, now)?;
                st.heads.insert(
                    key,
                    Head {
                        content_hash,
                        head_hash,
                        deleted: false,
                        server_ver: d.server_ver,
                    },
                );
                writes += 1;
                log::info!(
                    "стратегии(db): {core_name} «{}» восстановлена без изменений — версия переоткрыта",
                    d.name
                );
                continue;
            }
        }

        let (need_version, change_kind, ver_gap) = match st.heads.get(&key) {
            None => (true, "created", false),
            Some(h) if h.deleted => (true, "restored", false),
            Some(h) if h.content_hash != content_hash => (
                true,
                "params",
                d.server_ver > h.server_ver.saturating_add(1),
            ),
            Some(h) if h.head_hash != head_hash => (false, "", false),
            Some(_) => continue, // Nothing changed.
        };

        if need_version {
            let raw_json = serde_json::to_string(&Value::Object(d.fields.clone()))?;
            // Diff against the latest version's content, excluding all configured `ignore_fields`
            // and `__` metadata. A `created` version without a previous dump keeps `changed_json`
            // as NULL.
            let prev: Option<(i64, String)> = tx
                .query_row(
                    "SELECT valid_from, raw_json FROM strategy_versions
                     WHERE core_uid=?1 AND strategy_id=?2
                     ORDER BY valid_from DESC LIMIT 1",
                    rusqlite::params![uid, d.strategy_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let (changed_json, n_changed) = match &prev {
                None => (None, 0usize),
                Some((_, prev_raw)) => {
                    let diff = diff_maps(prev_raw, &stripped, &st.ignore);
                    let n = diff.len();
                    if n == 0 && change_kind == "params" {
                        // A hash mismatch with an empty content diff is not a new version.
                        (None, 0)
                    } else {
                        (Some(serde_json::to_string(&Value::Object(diff))?), n)
                    }
                }
            };
            if change_kind == "params" && n_changed == 0 && prev.is_some() {
                // For a false-positive hash change, update the cache and head without a version.
                upsert_head(&tx, uid, core_name, d, content_hash, head_hash, now)?;
                st.heads.insert(
                    key,
                    Head {
                        content_hash,
                        head_hash,
                        deleted: false,
                        server_ver: d.server_ver,
                    },
                );
                writes += 1;
                continue;
            }
            let kind = change_kind;
            // `valid_from` must increase strictly per strategy to satisfy the unique key.
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
            // The live atomic is consulted on version insertion, so a changed limit is applied to
            // this strategy on its next version insertion.
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
        st.heads.insert(
            key,
            Head {
                content_hash,
                head_hash,
                deleted: false,
                server_ver: d.server_ver,
            },
        );
        writes += 1;
    }

    // Strategies absent from a complete set were deleted on the server; soft-delete them while
    // retaining history. Ignore an empty set because it more likely represents incomplete state
    // after connecting than an intentional deletion of every strategy.
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
            log::info!(
                "стратегии(db): {core_name} id={sid} удалена на сервере (история сохранена)"
            );
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

/// Copies a dump without ignored fields or `__` metadata for version-content comparisons.
fn strip(m: &Map<String, Value>, ignore: &HashSet<String>) -> Map<String, Value> {
    m.iter()
        .filter(|(k, _)| !k.starts_with("__") && !ignore.contains(k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Computes a canonical content hash. `serde_json::Map` uses a `BTreeMap`, so serialization is
/// key-sorted and stable across runs.
fn hash_of(stripped: &Map<String, Value>) -> i64 {
    let s = serde_json::to_string(&Value::Object(stripped.clone())).unwrap_or_default();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish() as i64
}

/// Hashes head fields such as name, folder, checkbox state, and kind to skip no-op updates.
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

/// Builds a content diff as `{"Field": {"old": ..., "new": ...}}`; added fields have
/// `old=null`, and removed fields have `new=null`.
fn diff_maps(
    prev_raw: &str,
    new_stripped: &Map<String, Value>,
    ignore: &HashSet<String>,
) -> Map<String, Value> {
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
    out
}

#[cfg(test)]
mod tests;
