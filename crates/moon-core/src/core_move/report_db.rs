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
//!
//! `ID` is the core's local row id, and most cores never send it: their schema leaves it out, so
//! the replica has no `id` column at all, or — once another core added one — a column that stays
//! empty for them. Such a core's `ID` is minted from `newRecID`, which is unique per core; the
//! original id was never on the wire and cannot be restored.

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
/// `None`, with no file written, when the replica holds no row of that core: there is nothing to
/// carry, and the moved core starts its own database. Fails when `out` already exists or when the
/// replica lacks the `newrecid` column every row is keyed by. A failure removes the half-written
/// file.
pub fn build(replica: &Path, core_uid: u64, out: &Path) -> anyhow::Result<Option<CoreDbSummary>> {
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
fn write(replica: &Path, core_uid: u64, out: &Path) -> anyhow::Result<Option<CoreDbSummary>> {
    let src = Connection::open_with_flags(
        replica,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open {}", replica.display()))?;
    let Some((columns, has_id)) = replica_columns(&src)? else {
        return Ok(None);
    };
    let uid = i64::try_from(core_uid).context("core uid out of range")?;
    let (rows, max_rec): (i64, Option<i64>) = src.query_row(
        "SELECT COUNT(*), MAX(newrecid) FROM orders_rep WHERE core_uid=?1",
        [uid],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let rows = rows.unsigned_abs();
    if rows == 0 {
        return Ok(None);
    }
    let id_source = id_source(&src, has_id, uid, rows)?;
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
            "INSERT INTO Orders (ID, {names}) SELECT {id_source}, {names} FROM src.orders_rep \
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
    Ok(Some(CoreDbSummary {
        rows,
        open_rows,
        epoch,
        high_water,
    }))
}

/// The replica's core columns with their declared types, in table order, without `id` — the
/// rebuild writes `ID` itself, from [`id_source`] — and whether the replica has an `id` column.
/// `None` when the replica has no `orders_rep` table: the writer never created one, so it holds no
/// row of any core.
fn replica_columns(src: &Connection) -> anyhow::Result<Option<(Vec<(String, String)>, bool)>> {
    let mut stmt = src.prepare("PRAGMA table_info(orders_rep)")?;
    let columns: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?
        .collect::<Result<_, _>>()?;
    if columns.is_empty() {
        return Ok(None);
    }
    let has = |name: &str| columns.iter().any(|(c, _)| c.eq_ignore_ascii_case(name));
    anyhow::ensure!(
        has("newrecid"),
        "the replica lacks the newrecid column its rows are keyed by"
    );
    let has_id = has("id");
    let columns = columns
        .into_iter()
        .filter(|(name, _)| {
            !name.eq_ignore_ascii_case("id")
                && !LOCAL_COLUMNS.iter().any(|l| l.eq_ignore_ascii_case(name))
        })
        .collect();
    Ok(Some((columns, has_id)))
}

/// The replica column `Orders.ID` is filled from for core `uid`, which holds `rows` rows.
///
/// The core's own `id` only when the replica has the column and this core filled it on every row
/// with a distinct integer — anything else would not fit `ID INTEGER PRIMARY KEY`; otherwise
/// `newrecid`, for every row. The two are never mixed: a core that began sending `ID` after an
/// update fills it only on its newer rows, and those ids can equal the `newRecID` of an older row.
fn id_source(src: &Connection, has_id: bool, uid: i64, rows: u64) -> anyhow::Result<&'static str> {
    if !has_id {
        return Ok("newrecid");
    }
    let distinct: i64 = src.query_row(
        "SELECT COUNT(DISTINCT CASE WHEN typeof(id)='integer' THEN id END) \
         FROM orders_rep WHERE core_uid=?1",
        [uid],
        |r| r.get(0),
    )?;
    Ok(if distinct.unsigned_abs() == rows {
        "id"
    } else {
        "newrecid"
    })
}

/// `CREATE TABLE Orders`: `ID` first as the integer primary key, as in the cores' own files, then
/// every replica column with the type the replica declared.
fn orders_ddl(columns: &[(String, String)]) -> String {
    let body = std::iter::once("ID INTEGER PRIMARY KEY".to_owned())
        .chain(
            columns
                .iter()
                .map(|(name, kind)| format!("{} {kind}", quote(name)).trim_end().to_owned()),
        )
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
