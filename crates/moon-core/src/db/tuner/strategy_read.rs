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
    let mut out = std::collections::HashMap::new();
    let conn = open_strategies_ro()?;
    let raw = load_head_raw_json(&conn, strategy_id, core)?;
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str(&raw) else {
        return None;
    };
    for key in keys {
        let Some(v) = map.get(key) else { continue };
        let s = match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => if *b { "YES" } else { "NO" }.to_string(),
            // A list-valued field (a coin list spelled as a JSON array) flattens to the
            // comma form the callers parse. Dropping it here while the SQL column reads it
            // is what makes one screen count coins the other cannot see.
            serde_json::Value::Array(items) => items
                .iter()
                .filter_map(|i| match i {
                    serde_json::Value::String(s) => Some(s.clone()),
                    serde_json::Value::Number(n) => Some(n.to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(","),
            _ => continue,
        };
        out.insert(key.clone(), s);
    }
    Some(out)
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
