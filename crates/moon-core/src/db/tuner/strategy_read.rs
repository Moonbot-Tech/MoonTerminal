use super::fields::{FIELDS, FieldClass};
use rusqlite::Connection;

/// Open strategies.sqlite READ-ONLY, or `None` when it is absent or will not open.
///
/// A 3 s `busy_timeout` covers the strat_db writer committing on its own thread; without it a
/// write landing under our read is an instant SQLITE_BUSY that a caller would misread as "no
/// such strategy". Shared by every strategy read in this module.
fn open_strategies_ro() -> Option<Connection> {
    let path = crate::config::paths::strategies_db_path();
    if !path.exists() {
        return None;
    }
    let conn =
        Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let _ = conn.busy_timeout(std::time::Duration::from_secs(3));
    crate::db::trace::install_on(&conn);
    Some(conn)
}

/// The current version's `raw_json` for a strategy, scoped to `core` when known.
///
/// `None` — no live head row for this `(strategy_id, core)`. Rows are per-core
/// (`strategyid@core_uid`), so scoping to the exact core reflects the SAME core a write
/// targets, not the newest-checked one. Shared by `strategy_current_values_opt` and
/// `strategy_filters`.
fn load_head_raw_json(conn: &Connection, strategy_id: i64, core: Option<u64>) -> Option<String> {
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
    match core {
        Some(c) => conn
            .query_row(&sql, rusqlite::params![strategy_id, c as i64], |r| r.get(0))
            .ok(),
        None => conn.query_row(&sql, [strategy_id], |r| r.get(0)).ok(),
    }
}

/// Cores where the strategy currently exists (strategies.sqlite heads with `deleted=0`),
/// which are the targets for saving thresholds.
pub fn strategy_cores(strategy_id: i64) -> Vec<u64> {
    let Some(conn) = open_strategies_ro() else {
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

/// Strategy filter card: Ignore flags plus NON-default thresholds for tuner fields.
/// The flags drive both chips (hide ignored classes) and threshold persistence
/// (enable the required classes before writing).
#[derive(Clone, Debug, Default)]
pub struct StratFilters {
    pub found: bool,
    pub ignore_filters: bool,
    pub ignore_delta: bool,
    pub ignore_volume: bool,
    /// Whether the Filters/Base section (leverage, MarkPrice) is ignored.
    pub ignore_base: bool,
    /// BV/SV filter switch (`UseBV_SV_Filter`); `false` means disabled.
    pub use_bvsv: bool,
    /// Whether the Filters/Ping section (PriceBug and pings) is ignored.
    pub ignore_ping: bool,
    pub bounds: std::collections::HashMap<&'static str, (Option<f64>, Option<f64>)>,
    /// Occupied Delta2/Delta3 slots: (number 2|3, report field, min, max).
    /// Slots whose type has no report column (2h/30m/Pump5m) are omitted.
    pub slots: Vec<(u8, &'static str, Option<f64>, Option<f64>)>,
    /// Slots occupied by a type WITHOUT a report column (2h/30m/Pump5m) and configured
    /// thresholds: a live filter invisible to the tuner. Saving must overwrite such a slot
    /// only as a last resort and with a warning.
    pub foreign_slots: Vec<(u8, String)>,
    /// Current strategy comment (`Comment` field); saving appends an analyzer stamp without
    /// erasing the user's description.
    pub comment: String,
}

impl StratFilters {
    /// Whether the current strategy flags ignore the field class.
    pub fn class_ignored(&self, class: FieldClass) -> bool {
        self.ignore_filters
            || match class {
                FieldClass::Filter => false,
                // BV/SV is a Filters/Volume subgroup gated by BOTH IgnoreVolume and its own
                // UseBV_SV_Filter switch.
                FieldClass::BvSv => self.ignore_volume || !self.use_bvsv,
                FieldClass::Ping => self.ignore_ping,
                FieldClass::Base => self.ignore_base,
                FieldClass::Delta | FieldClass::DeltaSlot => self.ignore_delta,
                FieldClass::Volume => self.ignore_volume,
            }
    }

    /// Slot assigned to the field, if any: (number, min, max).
    pub fn slot_of(&self, field: &str) -> Option<(u8, Option<f64>, Option<f64>)> {
        self.slots
            .iter()
            .find(|(_, f, _, _)| *f == field)
            .map(|(n, _, lo, hi)| (*n, *lo, *hi))
    }
}

/// Current strategy parameter values for the "now -> next" write-confirmation dialog.
/// Keys are parameter names from the edit list; a key absent from raw_json is omitted from
/// the result. Values are strings in strategy format (booleans are YES/NO). This accesses
/// strategies.sqlite, so call it from a background executor.
pub fn strategy_current_values(
    strategy_id: i64,
    core: Option<u64>,
    keys: &[String],
) -> std::collections::HashMap<String, String> {
    strategy_current_values_opt(strategy_id, core, keys).unwrap_or_default()
}

/// The same read, but able to say "I could not look".
///
/// `None` — the strategy's row could not be read at all (no database file, the file would not
/// open, no live row for this `(strategy_id, core)`, unparseable `raw_json`). `Some(map)` — the
/// row WAS read, and a key missing from the map means the field is genuinely absent from the
/// strategy, i.e. empty.
///
/// The flattened form above collapses those two into an empty map, which is fine for filling
/// a "now → next" preview but not for anything that must not guess. A caller checking whether
/// an overwrite destroys data has to tell "this strategy lists no coins" from "I have no idea
/// what this strategy lists" — reporting the second as the first says a whole-field overwrite
/// was verified safe when nothing was verified at all.
pub fn strategy_current_values_opt(
    strategy_id: i64,
    core: Option<u64>,
    keys: &[String],
) -> Option<std::collections::HashMap<String, String>> {
    let conn = open_strategies_ro()?;
    let raw = load_head_raw_json(&conn, strategy_id, core)?;
    flatten_values(&raw, keys)
}

/// The values of `keys` as of `at_ms` — from the version whose `[valid_from, valid_to)` holds
/// that moment. A trade older than the first recorded version reads the FIRST version, the
/// closest thing on record to what ran then; `None` as in [`strategy_current_values_opt`].
///
/// The Entry/Exit tuner runs its model on the parameters a trade was actually made under; the
/// head would silently judge yesterday's fill by today's corridor.
///
/// Args:
///     strategy_id: The strategy.
///     core: Its core, when known; rows are per-core.
///     at_ms: The moment, Unix ms — the trade's `buydatems`.
///     keys: Field names to read.
pub fn strategy_values_at(
    strategy_id: i64,
    core: Option<u64>,
    at_ms: i64,
    keys: &[String],
) -> Option<std::collections::HashMap<String, String>> {
    let conn = open_strategies_ro()?;
    let raw = load_raw_json_at(&conn, strategy_id, core, at_ms)?;
    flatten_values(&raw, keys)
}

/// The `raw_json` of the version valid at `at_ms`, else the earliest version on record, scoped
/// to `core` when known. `None` when the strategy has no version at all.
fn load_raw_json_at(
    conn: &Connection,
    strategy_id: i64,
    core: Option<u64>,
    at_ms: i64,
) -> Option<String> {
    // Two spellings per query rather than one with an optional clause: the placeholder
    // numbering shifts with the core clause, and a `?3` bound to nothing is a silent `None`
    // that would send every core-less read to the first version.
    let at = match core {
        Some(c) => conn
            .query_row(
                "SELECT v.raw_json FROM strategy_versions v
                     WHERE v.strategy_id = ?1 AND v.core_uid = ?2
                       AND v.valid_from <= ?3 AND (v.valid_to IS NULL OR v.valid_to > ?3)
                     ORDER BY v.valid_from DESC LIMIT 1",
                rusqlite::params![strategy_id, c as i64, at_ms],
                |r| r.get(0),
            )
            .ok(),
        None => conn
            .query_row(
                "SELECT v.raw_json FROM strategy_versions v
                     WHERE v.strategy_id = ?1
                       AND v.valid_from <= ?2 AND (v.valid_to IS NULL OR v.valid_to > ?2)
                     ORDER BY v.valid_from DESC LIMIT 1",
                rusqlite::params![strategy_id, at_ms],
                |r| r.get(0),
            )
            .ok(),
    };
    if at.is_some() {
        return at;
    }
    match core {
        Some(c) => conn
            .query_row(
                "SELECT v.raw_json FROM strategy_versions v
                     WHERE v.strategy_id = ?1 AND v.core_uid = ?2
                     ORDER BY v.valid_from ASC LIMIT 1",
                rusqlite::params![strategy_id, c as i64],
                |r| r.get(0),
            )
            .ok(),
        None => conn
            .query_row(
                "SELECT v.raw_json FROM strategy_versions v
                     WHERE v.strategy_id = ?1
                     ORDER BY v.valid_from ASC LIMIT 1",
                rusqlite::params![strategy_id],
                |r| r.get(0),
            )
            .ok(),
    }
}

/// The strategy KIND (`SignalType`: `MoonShot`, `Spread`, …) of each `(strategy_id, core_uid)`
/// pair, from its newest version — a strategy never changes kind, and a deleted one still has
/// versions to read it from. Pairs with no version at all are absent from the map.
///
/// Args:
///     pairs: Distinct `(strategy_id, core_uid)` pairs.
///
/// A database that cannot be opened or queried yields an EMPTY map and one warning: the caller
/// then shows every deal as "kind unknown" (no entry model), which is visible, rather than
/// failing the whole read for a file the axis only annotates from.
pub fn strategy_kinds(pairs: &[(i64, u64)]) -> std::collections::HashMap<(i64, u64), String> {
    let mut out = std::collections::HashMap::new();
    if pairs.is_empty() {
        return out;
    }
    let Some(conn) = open_strategies_ro() else {
        log::warn!("[x] tuner: strategies.sqlite unavailable, strategy kinds unresolved");
        return out;
    };
    let mut stmt = match conn.prepare(
        "SELECT json_extract(v.raw_json, '$.SignalType') FROM strategy_versions v
             WHERE v.strategy_id = ?1 AND v.core_uid = ?2
             ORDER BY v.valid_to IS NULL DESC, v.valid_from DESC LIMIT 1",
    ) {
        Ok(stmt) => stmt,
        Err(error) => {
            log::warn!("[x] tuner: strategy kinds query failed to prepare: {error}");
            return out;
        }
    };
    let mut failed = 0usize;
    for &(strategy_id, core_uid) in pairs {
        match stmt.query_row(rusqlite::params![strategy_id, core_uid as i64], |r| {
            r.get::<_, Option<String>>(0)
        }) {
            Ok(Some(kind)) => {
                out.insert((strategy_id, core_uid), kind);
            }
            // No version at all, or a version without the field: genuinely unknown.
            Ok(None) | Err(rusqlite::Error::QueryReturnedNoRows) => {}
            Err(_) => failed += 1,
        }
    }
    if failed > 0 {
        log::warn!(
            "[x] tuner: strategy kinds unresolved for {failed} of {} strategies (query errors)",
            pairs.len()
        );
    }
    out
}

/// `keys` out of one version's `raw_json`, in strategy format — see
/// [`strategy_current_values_opt`] for the rules.
fn flatten_values(raw: &str, keys: &[String]) -> Option<std::collections::HashMap<String, String>> {
    let mut out = std::collections::HashMap::new();
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str(raw) else {
        return None;
    };
    for key in keys {
        let Some(text) = map.get(key).and_then(value_text) else {
            continue;
        };
        out.insert(key.clone(), text);
    }
    Some(out)
}

/// One `raw_json` value in strategy format: a string as is, a number in its shortest form, a
/// boolean as `YES`/`NO`, a list in the comma form; `None` for anything else.
fn value_text(v: &serde_json::Value) -> Option<String> {
    Some(match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => if *b { "YES" } else { "NO" }.to_string(),
        // A list-valued field (a coin list spelled as a JSON array) flattens to the comma form
        // the callers parse. Dropping it here while the SQL column reads it is what makes one
        // screen count coins the other cannot see.
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|i| match i {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(","),
        _ => return None,
    })
}

/// One live strategy as the search's automatic ranges read it
/// (`ticks::params::range::Population`).
#[derive(Clone, Debug, PartialEq)]
pub struct LiveStrategy {
    /// `SignalType` — the spelling a deal's kind has (`strategy_kinds`), not the `kind` column,
    /// which spells some kinds otherwise (`PumpDetection` for `PumpsDetection`).
    pub kind: String,
    /// The asked fields the strategy's dump holds, by LOWERCASE name, in strategy format.
    pub values: std::collections::HashMap<String, String>,
}

/// What [`live_strategies`] last read, and the state of the file it read it from.
struct LiveCache {
    signature: (i64, i64, i64),
    keys: Vec<String>,
    strategies: std::sync::Arc<Vec<LiveStrategy>>,
}

static LIVE_CACHE: std::sync::Mutex<Option<LiveCache>> = std::sync::Mutex::new(None);

/// Every live strategy of every core — the current version of each one not deleted — with the
/// fields `keys` names (matched without regard to case), one per distinct content: a strategy
/// copied onto many cores is one strategy (1 423 live, 1 108 distinct here, 2026-09-25).
///
/// Read once per state of the file (the count of heads, the newest head update and the newest
/// version): the axis loads on every move of the report, the strategies change far more rarely.
/// Measured 2026-09-25 on this machine: 1 423 heads, 4.3 MB of JSON, ~60 ms in Python.
///
/// Returns:
///     The strategies, empty when the file is absent or will not read.
pub fn live_strategies(keys: &[String]) -> std::sync::Arc<Vec<LiveStrategy>> {
    let Some(conn) = open_strategies_ro() else {
        return std::sync::Arc::default();
    };
    let signature = conn
        .query_row(
            "SELECT (SELECT count(*) FROM strategies),
                    (SELECT coalesce(max(updated_ms), 0) FROM strategies),
                    (SELECT coalesce(max(id), 0) FROM strategy_versions)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap_or((-1, -1, -1));
    let mut cache = LIVE_CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(hit) = cache
        .as_ref()
        .filter(|c| signature.0 >= 0 && c.signature == signature && c.keys == keys)
    {
        return std::sync::Arc::clone(&hit.strategies);
    }
    let started = std::time::Instant::now();
    let wanted: std::collections::HashSet<String> =
        keys.iter().map(|k| k.to_ascii_lowercase()).collect();
    let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut out: Vec<LiveStrategy> = Vec::new();
    let read = conn
        .prepare(
            "SELECT s.content_hash, v.raw_json FROM strategies s
               JOIN strategy_versions v
                 ON v.core_uid = s.core_uid AND v.strategy_id = s.strategy_id
              WHERE s.deleted = 0 AND v.valid_to IS NULL",
        )
        .and_then(|mut stmt| {
            let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (hash, raw) = row?;
                if !seen.insert(hash) {
                    continue;
                }
                let Ok(serde_json::Value::Object(map)) = serde_json::from_str(&raw) else {
                    continue;
                };
                let kind = map
                    .get("SignalType")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let values = map
                    .iter()
                    .filter_map(|(key, value)| {
                        let lower = key.to_ascii_lowercase();
                        wanted
                            .contains(&lower)
                            .then(|| value_text(value).map(|text| (lower, text)))
                            .flatten()
                    })
                    .collect();
                out.push(LiveStrategy { kind, values });
            }
            Ok(())
        });
    if let Err(error) = read {
        log::warn!("[x] tuner: live strategies unreadable: {error}");
        return std::sync::Arc::default();
    }
    log::info!(
        target: crate::diagnostics::TICKS_AXIS_TARGET,
        "[x] ticks ranges: read {} distinct live strategies in {} ms",
        out.len(),
        started.elapsed().as_millis()
    );
    let strategies = std::sync::Arc::new(out);
    *cache = Some(LiveCache {
        signature,
        keys: keys.to_vec(),
        strategies: std::sync::Arc::clone(&strategies),
    });
    strategies
}

/// Threshold parameters of the SELECTED strategy for tuner fields.
/// The source is the current strategies.sqlite version (raw_json normalized with schema
/// defaults). `defaults` contains schema defaults (lowercase name -> number): a value EQUAL
/// to its default is hidden because it means "filter not configured," not a deliberate
/// threshold (such as ...100T). `found=false` means the database or row was not found.
pub fn strategy_filters(
    strategy_id: i64,
    core: Option<u64>,
    defaults: &std::collections::HashMap<String, f64>,
) -> StratFilters {
    let mut out = StratFilters::default();
    // Rows are per-core: scope the strategy card (Ignore flags, thresholds) to the SELECTED
    // core so the save diff is computed against the exact core the write targets.
    let Some(conn) = open_strategies_ro() else {
        return out;
    };
    let Some(raw) = load_head_raw_json(&conn, strategy_id, core) else {
        return out;
    };
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
        // A schema-default value means the filter was never configured, so hide it.
        if let Some(d) = defaults.get(&key.to_ascii_lowercase()) {
            if (f - d).abs() <= f64::EPSILON.max(d.abs() * 1e-9) {
                return None;
            }
        }
        Some(f)
    };
    // Ignore flags may be booleans or YES/TRUE/1 strings.
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
    // Delta2/Delta3 slots: map a string type ("15m"/"Pump1h"/...) to a report field.
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
            // 2h/30m/Pump5m have no report column. With configured thresholds this is a
            // live filter, so mark the slot as occupied by a foreign type.
            if lo.is_some() || hi.is_some() {
                out.foreign_slots.push((n, t.to_string()));
            }
            continue;
        };
        out.slots.push((n, field, lo, hi));
    }
    out
}

#[cfg(test)]
mod tests;
