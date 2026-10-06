use super::*;

pub fn load_sort(conn: &Connection) -> Option<(String, bool)> {
    let key: String = conn
        .query_row("SELECT value FROM app_meta WHERE key='sort_key'", [], |r| {
            r.get(0)
        })
        .ok()?;
    let desc: String = conn
        .query_row(
            "SELECT value FROM app_meta WHERE key='sort_desc'",
            [],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| "1".into());
    Some((key, desc != "0"))
}

/// Upsert one `app_meta` key/value pair. The single place the shared settings table is written.
/// Plain-private: every caller is this module or a descendant (`rep`).
pub(in crate::db) fn meta_set(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO app_meta(key,value) VALUES(?1,?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![key, value],
    )?;
    Ok(())
}

/// `app_meta` key holding the time-axis generation counter.
pub(crate) const AXIS_GENERATION_KEY: &str = "axis_gen";

/// Advance the time-axis generation counter.
///
/// A single integer, and deliberately the ONLY thing `app_meta` carries about the axis: the
/// segments themselves live in their own normalized table, because packing an append-only list
/// into a scalar store means a hand-rolled parser, a cap policy and malformed-value recovery
/// inside something whose whole contract is "one value". What a counter IS good for is exactly
/// this — a cache anywhere in the process can hold the generation it computed under and compare.
///
/// A missing or unparseable value restarts at 1 rather than failing. The counter's only use is
/// INEQUALITY, so a reset makes every cache recompute once, which is the safe direction; refusing
/// to write would instead leave caches believing they were current.
///
/// Args:
///     conn: Open writer connection, already inside the caller's transaction.
///
/// Returns:
///     Nothing; the stored counter advances by one.
pub(crate) fn bump_axis_generation(conn: &Connection) -> rusqlite::Result<()> {
    let next = meta_get_i64(conn, AXIS_GENERATION_KEY)
        .unwrap_or(0)
        .wrapping_add(1);
    meta_set(conn, AXIS_GENERATION_KEY, &next.to_string())
}

/// Read one `app_meta` value as an integer, returning `None` when lookup or parsing fails.
pub(in crate::db) fn meta_get_i64(conn: &Connection, key: &str) -> Option<i64> {
    conn.query_row("SELECT value FROM app_meta WHERE key=?1", [key], |r| {
        r.get::<_, String>(0)
    })
    .ok()?
    .trim()
    .parse()
    .ok()
}

/// Remove one `app_meta` key. Removing an absent key succeeds.
pub(in crate::db) fn meta_delete(conn: &Connection, key: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM app_meta WHERE key=?1", [key])?;
    Ok(())
}

/// Save the Report sort key and direction as one atomic SQLite statement.
///
/// Args:
///     conn: Open report metadata connection.
///     key: Runtime column name.
///     desc: Whether the selected column sorts descending.
///
/// Returns:
///     Nothing; preference write failures remain non-fatal.
pub fn save_sort(conn: &Connection, key: &str, desc: bool) {
    let _ = conn.execute(
        "INSERT INTO app_meta(key,value) VALUES('sort_key',?1),('sort_desc',?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![key, if desc { "1" } else { "0" }],
    );
}

/// Storage key for the Report comment pane in one host.
///
/// The `:dock` / `:win` split MIRRORS `table_persist::ctx_id` in the UI crate, which keeps the
/// Report's visible columns and widths apart for a docked tab and a detached window. This
/// preference belongs to the same table, so it follows the same rule; the suffix is spelled out
/// here because `moon-core` cannot reach that helper.
fn comment_pane_key(detached: bool) -> &'static str {
    if detached {
        "report_comment_pane:win"
    } else {
        "report_comment_pane:dock"
    }
}

/// Load whether the Report shows the comment pane under its table, for one host.
///
/// Args:
///     conn: Open report metadata connection.
///     detached: Whether the caller is a detached or standalone Report window.
///
/// Returns:
///     The stored preference, or `None` when this host has never changed it. A value written by a
///     newer build, or a hand-edited row, counts as anything other than `"0"` being on — the pane
///     is a display choice, so an unreadable preference must not hide data.
pub fn load_comment_pane(conn: &Connection, detached: bool) -> Option<bool> {
    let value: String = conn
        .query_row(
            "SELECT value FROM app_meta WHERE key=?1",
            [comment_pane_key(detached)],
            |r| r.get(0),
        )
        .ok()?;
    Some(value != "0")
}

/// Store whether the Report shows the comment pane under its table, for one host.
pub fn save_comment_pane(conn: &Connection, detached: bool, shown: bool) {
    let _ = meta_set(
        conn,
        comment_pane_key(detached),
        if shown { "1" } else { "0" },
    );
}

/// The key a save writes. Named rather than indexed, so appending a row cannot redirect the save.
const VISIBLE_KEY_CURRENT: &str = "report_visible_v3";

/// Every `app_meta` visible-column key with the columns introduced by its schema, newest first.
///
/// Saved sets are explicit, so reading a key also restores the columns declared by every newer
/// row encountered before it. Keeping each column on one schema row prevents the restoration
/// rules for older keys from drifting apart.
const VISIBLE_KEYS: &[(&str, &[&str])] = &[
    (VISIBLE_KEY_CURRENT, report_read::COLUMNS_ADDED_SINCE_V2),
    ("report_visible_v2", &[report_read::PROFIT_PERCENT_COLUMN]),
    ("report_visible", &[]),
];

/// Load visible Report columns, restoring any column added since the saved set was written.
///
/// Args:
///     conn: Open report metadata connection.
///
/// Returns:
///     Current or migrated column names, or `None` when no preference exists.
pub fn load_visible(conn: &Connection) -> Option<Vec<String>> {
    // Newest key first; everything passed on the way down was introduced after the key that
    // answers, and so is missing from the set it holds.
    let mut introduced_since: Vec<&str> = Vec::new();
    let stored = VISIBLE_KEYS.iter().find_map(|(key, introduced_by)| {
        let found = conn
            .query_row("SELECT value FROM app_meta WHERE key=?1", [*key], |r| {
                r.get::<_, String>(0)
            })
            .ok();
        if found.is_none() {
            introduced_since.extend_from_slice(introduced_by);
        }
        found
    })?;
    let mut columns: Vec<String> = stored
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    // Collected newest-schema-first; restore them oldest-first so the recovered set reads in the
    // order the columns were actually introduced.
    introduced_since.reverse();
    for column in introduced_since {
        if !columns.iter().any(|saved| saved == column) {
            columns.push(column.to_string());
        }
    }
    Some(columns)
}

/// Save visible Report columns under the current schema key.
///
/// Args:
///     conn: Open report metadata connection.
///     cols: Runtime column names in display order.
///
/// Returns:
///     Nothing; metadata write failures remain non-fatal like the other Report preferences.
pub fn save_visible(conn: &Connection, cols: &[&str]) {
    let _ = meta_set(conn, VISIBLE_KEY_CURRENT, &cols.join(","));
}
