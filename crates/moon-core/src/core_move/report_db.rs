//! A MoonBot report database rebuilt from this terminal's replica of one core.
//!
//! A core moved to a new server starts with an empty report database unless one is put beside its
//! settings. The replica in `reports.sqlite` holds every field the core ever sent for its rows, so
//! the database can be rebuilt from it — and given the same identity: MoonProto reports a report
//! database's epoch from the file's `PRAGMA application_id`, and the core's persistent row
//! high-water from `PRAGMA user_version`. With both carried over, every terminal that replicates
//! this core resumes where it stopped instead of wiping its replica as a database it never saw.
//!
//! The file is written in the layout the cores' own files show: `MainInfo`, `Channels` and
//! `Orders` keyed by `ID`. Column names keep the replica's lowercase spelling; SQLite matches
//! column names without regard to case.

use std::path::Path;

use anyhow::Context;
use rusqlite::{Connection, OpenFlags};

/// The `MainInfo.dbver` that MoonBot 7.68–7.72 report databases carry. A newer core upgrades an
/// older-numbered file itself, as it does its own after an update.
pub const CORE_DB_VERSION: i64 = 103;

/// Replica columns that are this terminal's own and never reach a core.
const LOCAL_COLUMNS: [&str; 2] = ["core_uid", "core_name"];

/// What a rebuild wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreDbSummary {
    /// Rows written to `Orders`.
    pub rows: u64,
    /// Rows still open (no close date): positions the old core held when the replica last saw it.
    pub open_rows: u64,
    /// The epoch written as `application_id`; `None` when the replica never stored one, in which
    /// case the core picks a new one and terminals download the history again.
    pub epoch: Option<i32>,
    /// The high-water written as `user_version`.
    pub high_water: i64,
}

/// Rebuild core `core_uid`'s report database from the replica at `replica` into a new file `out`.
///
/// Fails when `out` already exists, when the replica holds no row of that core, or when the
/// replica lacks the `id`/`newrecid` columns a core database is keyed by. A failure removes the
/// half-written file.
pub fn build(replica: &Path, core_uid: u64, out: &Path) -> anyhow::Result<CoreDbSummary> {
    anyhow::ensure!(
        !out.exists(),
        "{} already exists: the rebuild never overwrites a file",
        out.display()
    );
    let result = write(replica, core_uid, out);
    if result.is_err() {
        let _ = std::fs::remove_file(out);
    }
    result
}

/// [`build`] without the cleanup.
fn write(replica: &Path, core_uid: u64, out: &Path) -> anyhow::Result<CoreDbSummary> {
    let src = Connection::open_with_flags(
        replica,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open {}", replica.display()))?;
    let columns = replica_columns(&src)?;
    let uid = i64::try_from(core_uid).context("core uid out of range")?;
    let (rows, max_rec): (i64, Option<i64>) = src.query_row(
        "SELECT COUNT(*), MAX(newrecid) FROM orders_rep WHERE core_uid=?1",
        [uid],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let rows = rows.unsigned_abs();
    anyhow::ensure!(
        rows > 0,
        "the replica holds no report row of core {core_uid}"
    );
    let epoch = meta_i64(&src, &format!("rep_epoch_{core_uid}"))?
        .and_then(|e| i32::try_from(e).ok())
        .filter(|e| *e != 0);
    let next = meta_i64(&src, &format!("rep_next_{core_uid}"))?.unwrap_or(0);
    let high_water = max_rec.unwrap_or(0).max(next - 1);
    let high_water_pragma = i32::try_from(high_water).context("row high-water out of range")?;
    drop(src);

    let mut db = Connection::open(out).with_context(|| format!("create {}", out.display()))?;
    db.execute_batch(
        "CREATE TABLE MainInfo (ID INTEGER PRIMARY KEY AUTOINCREMENT,date DATETIME, upd DATETIME,\
         dbver INT, version TEXT, comment TEXT);\
         CREATE TABLE Channels (ID INTEGER PRIMARY KEY AUTOINCREMENT,chanID INT, chanName TEXT, \
         LastDate DATETIME,Comment TEXT);",
    )?;
    db.execute(&orders_ddl(&columns), [])?;
    db.execute_batch(
        "CREATE INDEX IBuyDate ON Orders (BuyDate ASC);\
         CREATE INDEX IRecID ON Orders (newRecID ASC);",
    )?;
    // Attached by plain path: only SELECTs touch it, and a URI would need the path escaped.
    db.execute(
        "ATTACH DATABASE ?1 AS src",
        [replica.to_string_lossy().into_owned()],
    )
    .context("attach the replica")?;
    let names = columns
        .iter()
        .map(|(name, _)| quote(name))
        .collect::<Vec<_>>()
        .join(", ");
    let tx = db.transaction()?;
    tx.execute(
        &format!(
            "INSERT INTO Orders ({names}) SELECT {names} FROM src.orders_rep \
             WHERE core_uid=?1 ORDER BY newrecid"
        ),
        [uid],
    )?;
    // The dates only label the row; `dbver` is the field that matters.
    let stamp = chrono::DateTime::from_timestamp_millis(crate::util::time::now_unix_ms_i64())
        .map(|t| t.format("%d.%m.%Y %H:%M").to_string())
        .unwrap_or_default();
    tx.execute(
        "INSERT INTO MainInfo (date, upd, dbver, version) VALUES (?1, ?1, ?2, ?3)",
        rusqlite::params![stamp, CORE_DB_VERSION, "MoonTerminal core move"],
    )?;
    tx.commit()?;
    db.execute("DETACH DATABASE src", [])?;
    if let Some(epoch) = epoch {
        db.execute_batch(&format!("PRAGMA application_id={epoch}"))?;
    }
    db.execute_batch(&format!("PRAGMA user_version={high_water_pragma}"))?;
    let open_rows: i64 = db.query_row(
        "SELECT COUNT(*) FROM Orders WHERE closedate IS NULL OR closedate<=0",
        [],
        |r| r.get(0),
    )?;
    let open_rows = open_rows.unsigned_abs();
    let written: i64 = db.query_row("SELECT COUNT(*) FROM Orders", [], |r| r.get(0))?;
    let written = written.unsigned_abs();
    anyhow::ensure!(
        written == rows,
        "wrote {written} of {rows} rows of core {core_uid}"
    );
    db.close().map_err(|(_, e)| e)?;
    Ok(CoreDbSummary {
        rows,
        open_rows,
        epoch,
        high_water,
    })
}

/// The replica's core columns with their declared types, in table order.
fn replica_columns(src: &Connection) -> anyhow::Result<Vec<(String, String)>> {
    let mut stmt = src.prepare("PRAGMA table_info(orders_rep)")?;
    let columns: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?
        .collect::<Result<_, _>>()?;
    anyhow::ensure!(!columns.is_empty(), "the replica has no orders_rep table");
    let has = |name: &str| columns.iter().any(|(c, _)| c.eq_ignore_ascii_case(name));
    anyhow::ensure!(
        has("id") && has("newrecid"),
        "the replica lacks the id/newrecid columns a core database is keyed by"
    );
    Ok(columns
        .into_iter()
        .filter(|(name, _)| !LOCAL_COLUMNS.iter().any(|l| l.eq_ignore_ascii_case(name)))
        .collect())
}

/// `CREATE TABLE Orders`: `id` becomes the integer primary key, every other column keeps the type
/// the replica declared.
fn orders_ddl(columns: &[(String, String)]) -> String {
    let body = columns
        .iter()
        .map(|(name, kind)| {
            if name.eq_ignore_ascii_case("id") {
                "ID INTEGER PRIMARY KEY".to_owned()
            } else {
                format!("{} {kind}", quote(name)).trim_end().to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("CREATE TABLE Orders ({body})")
}

/// Quote an identifier for SQLite.
fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// One integer from the replica's `app_meta`, `None` when the key is absent.
fn meta_i64(src: &Connection, key: &str) -> anyhow::Result<Option<i64>> {
    let value: Option<String> =
        match src.query_row("SELECT value FROM app_meta WHERE key=?1", [key], |r| {
            r.get(0)
        }) {
            Ok(v) => Some(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(e.into()),
        };
    Ok(value.and_then(|v| v.trim().parse().ok()))
}

#[cfg(test)]
mod tests;
