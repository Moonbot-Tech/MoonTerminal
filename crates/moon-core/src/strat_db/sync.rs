//! The strategy version archive's side of the sync between a station and its terminals.
//!
//! A core keeps only a strategy's current state, so its history exists nowhere but in the archives
//! of those who watched it. The station watches all the time, which makes it the archive a new
//! terminal fills from, and a terminal may hold what the station missed while down. Both directions
//! go through one [`super::merge::merge_chain`], on each side's own writer.
//!
//! Reads go through a connection of their own, as every reader of this store does; only the writer
//! writes versions. Everything here is called off any UI or feed thread.

use std::io::{Read as _, Write as _};
use std::sync::Mutex;

use anyhow::{Context, ensure};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rusqlite::{Connection, OptionalExtension, params};

use super::merge::MergeOutcome;
use crate::station_api::{
    StrategyChainPage, StrategyHead, StrategyIndex, StrategyKey, StrategySummary, StrategyVersion,
};

/// JSON of one [`StrategySummary`] with every number at its widest.
const SUMMARY_JSON: usize = 128;
/// Raw version text one page starts from before it is packed. Chains compress well, their
/// neighbouring versions being nearly alike, so a page usually fits its budget at this size;
/// [`chain_page_on`] halves it when one does not.
const PAGE_RAW_BYTES: usize = 8 << 20;
/// Most unpacked bytes one page may claim, so a broken or hostile page cannot exhaust the memory
/// of a small server. Above any honest page: a single version is kilobytes.
const PAGE_UNPACKED_LIMIT: u64 = 64 << 20;
/// Most raw version text a station holds while a chain arrives in pages.
const STAGE_LIMIT: usize = 128 << 20;
/// How long the station's API thread waits for its writer before saying so. The client's next
/// attempt finds the merge done or does it again, either being harmless.
const STATION_MERGE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// The summary of every strategy this store holds versions of, from just after `after` on.
///
/// Args:
///     conn: A connection to the store.
///     after: Where the previous page stopped.
///     budget: Bytes of JSON one page may take.
///
/// Returns:
///     Ascending by core, then strategy; `resume` says where to go on.
pub(super) fn index_on(
    conn: &Connection,
    after: Option<&StrategyKey>,
    budget: usize,
) -> rusqlite::Result<StrategyIndex> {
    let limit = (budget / SUMMARY_JSON).max(1);
    let (after_core, after_strategy) = after.map_or((i64::MIN, i64::MIN), |key| {
        (key.core_uid as i64, key.strategy_id)
    });
    let mut statement = conn.prepare(
        "SELECT core_uid, strategy_id, COUNT(*), MIN(valid_from), MAX(valid_from)
         FROM strategy_versions
         WHERE (core_uid, strategy_id) > (?1, ?2)
         GROUP BY core_uid, strategy_id
         ORDER BY core_uid, strategy_id
         LIMIT ?3",
    )?;
    let strategies = statement
        .query_map(
            params![after_core, after_strategy, limit as i64 + 1],
            |row| {
                Ok(StrategySummary {
                    core_uid: row.get::<_, i64>(0)? as u64,
                    strategy_id: row.get(1)?,
                    versions: row.get(2)?,
                    first_from: row.get(3)?,
                    last_from: row.get(4)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let more = strategies.len() > limit;
    let mut strategies = strategies;
    strategies.truncate(limit);
    let resume = more
        .then(|| strategies.last())
        .flatten()
        .map(|last| StrategyKey {
            core_uid: last.core_uid,
            strategy_id: last.strategy_id,
        });
    Ok(StrategyIndex { strategies, resume })
}

/// The station's answer to `strategies.index`; empty while it has no archive yet.
pub fn index(after: Option<&StrategyKey>, budget: usize) -> anyhow::Result<StrategyIndex> {
    match super::open_reader() {
        Some(conn) => Ok(index_on(&conn, after, budget)?),
        None => Ok(StrategyIndex::default()),
    }
}

/// Every strategy this machine's archive holds versions of.
pub fn local_summaries() -> anyhow::Result<Vec<StrategySummary>> {
    let Some(conn) = super::open_reader() else {
        return Ok(Vec::new());
    };
    let mut all = Vec::new();
    let mut after = None;
    loop {
        let page = index_on(&conn, after.as_ref(), usize::MAX / 2)?;
        all.extend(page.strategies);
        match page.resume {
            Some(resume) => after = Some(resume),
            None => return Ok(all),
        }
    }
}

/// Pack versions for a frame: JSON, gzip, base64.
pub(super) fn pack(versions: &[StrategyVersion]) -> anyhow::Result<String> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&serde_json::to_vec(versions)?)?;
    Ok(STANDARD.encode(encoder.finish()?))
}

/// Unpack what [`pack`] packed, refusing a page that claims more than [`PAGE_UNPACKED_LIMIT`].
pub fn unpack(packed: &str) -> anyhow::Result<Vec<StrategyVersion>> {
    let compressed = STANDARD.decode(packed).context("decode a chain page")?;
    let mut text = Vec::new();
    flate2::read::GzDecoder::new(compressed.as_slice())
        .take(PAGE_UNPACKED_LIMIT + 1)
        .read_to_end(&mut text)
        .context("unpack a chain page")?;
    ensure!(
        text.len() as u64 <= PAGE_UNPACKED_LIMIT,
        "a chain page over {PAGE_UNPACKED_LIMIT} bytes"
    );
    serde_json::from_slice(&text).context("read a chain page")
}

/// The head of one strategy in this store.
fn read_head(
    conn: &Connection,
    core_uid: u64,
    strategy_id: i64,
) -> rusqlite::Result<Option<StrategyHead>> {
    conn.query_row(
        "SELECT name, kind, kind_ordinal, folder_path, is_short, checked, server_ver, server_ms,
                deleted, core_name
         FROM strategies WHERE core_uid=?1 AND strategy_id=?2",
        params![core_uid as i64, strategy_id],
        |row| {
            Ok(StrategyHead {
                name: row.get(0)?,
                kind: row.get(1)?,
                kind_ordinal: row.get::<_, i64>(2)? as u8,
                folder_path: row.get(3)?,
                is_short: row.get::<_, i64>(4)? != 0,
                checked: row.get::<_, i64>(5)? != 0,
                server_ver: row.get(6)?,
                server_ms: row.get(7)?,
                deleted: row.get::<_, i64>(8)? != 0,
                core_name: row.get(9)?,
            })
        },
    )
    .optional()
}

/// One page of a strategy's chain.
///
/// Args:
///     conn: A connection to the store.
///     core_uid: The core, as this store knows it.
///     strategy_id: The strategy.
///     after_from: `valid_from` the previous page ended on; `None` starts the chain and carries
///         the head.
///     budget: Bytes the packed versions may take.
///
/// Returns:
///     The page; `resume` is `None` on the chain's last.
pub(super) fn chain_page_on(
    conn: &Connection,
    core_uid: u64,
    strategy_id: i64,
    after_from: Option<i64>,
    budget: usize,
) -> anyhow::Result<StrategyChainPage> {
    let head = match after_from {
        None => read_head(conn, core_uid, strategy_id)?,
        Some(_) => None,
    };
    let mut statement = conn.prepare(
        "SELECT valid_from, valid_to, change_kind, origin, ver_gap, server_ver, server_ms,
                checked_at, raw_json
         FROM strategy_versions
         WHERE core_uid=?1 AND strategy_id=?2 AND valid_from>?3
         ORDER BY valid_from",
    )?;
    let mut rows = statement.query(params![
        core_uid as i64,
        strategy_id,
        after_from.unwrap_or(i64::MIN)
    ])?;
    let mut versions = Vec::new();
    let mut raw = 0;
    let mut more = false;
    while let Some(row) = rows.next()? {
        if raw >= PAGE_RAW_BYTES {
            more = true;
            break;
        }
        let version = StrategyVersion {
            valid_from: row.get(0)?,
            valid_to: row.get(1)?,
            change_kind: row.get(2)?,
            origin: row.get(3)?,
            ver_gap: row.get::<_, i64>(4)? != 0,
            server_ver: row.get(5)?,
            server_ms: row.get(6)?,
            checked_at: row.get(7)?,
            raw_json: row.get(8)?,
        };
        raw += version.raw_json.len();
        versions.push(version);
    }
    drop(rows);
    let mut packed = pack(&versions)?;
    while packed.len() > budget {
        ensure!(
            versions.len() > 1,
            "one version of strategy {strategy_id} does not fit a frame"
        );
        versions.truncate(versions.len() / 2);
        more = true;
        packed = pack(&versions)?;
    }
    Ok(StrategyChainPage {
        head,
        versions: packed,
        resume: more
            .then(|| versions.last().map(|version| version.valid_from))
            .flatten(),
    })
}

/// One page of a chain of this machine's archive; see [`chain_page_on`].
pub fn chain_page(
    core_uid: u64,
    strategy_id: i64,
    after_from: Option<i64>,
    budget: usize,
) -> anyhow::Result<StrategyChainPage> {
    let conn = super::open_reader().context("the strategy archive is not open")?;
    chain_page_on(&conn, core_uid, strategy_id, after_from, budget)
}

/// Whether a chain whose newest version is `last_from` can only have been forgotten here, so
/// asking for it would bring nothing this machine will accept.
pub fn forgotten_covers(core_uid: u64, strategy_id: i64, last_from: i64) -> bool {
    let Some(conn) = super::open_reader() else {
        return false;
    };
    match super::merge::forgotten_before(&conn, core_uid, strategy_id) {
        Ok(forgotten) => forgotten.is_some_and(|at| last_from < at),
        // A store from before the table: nothing was forgotten in it.
        Err(_) => false,
    }
}

/// A chain arriving at the station in pages.
struct Stage {
    core_uid: u64,
    strategy_id: i64,
    head: StrategyHead,
    versions: Vec<StrategyVersion>,
    raw: usize,
}

/// The chain being received; a new chain replaces an abandoned one. Two terminals sending at once
/// can cost one of them a chain spanning several pages, its next page finding no first: that fails
/// its attempt and the attempt after it sends again, merging being safe to repeat. Almost every
/// chain fits one page, and a page that does is never staged across requests.
static STAGE: Mutex<Option<Stage>> = Mutex::new(None);

/// Take one page of a chain a terminal sends, merging once the last has come.
///
/// The station's name for the core is unknown here and the terminal's travels in the head; the
/// core's next set replaces it with the station's own for a strategy it still lists.
///
/// Args:
///     core_uid: The core, as the station knows it.
///     strategy_id: The strategy.
///     page: The page; the first carries the head, the last has no `resume`.
///
/// Returns:
///     `None` while more pages are due, otherwise what the merge did.
pub fn receive_page(
    core_uid: u64,
    strategy_id: i64,
    page: StrategyChainPage,
) -> anyhow::Result<Option<MergeOutcome>> {
    let versions = unpack(&page.versions)?;
    let mut stage = STAGE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(head) = page.head {
        *stage = Some(Stage {
            core_uid,
            strategy_id,
            head,
            versions: Vec::new(),
            raw: 0,
        });
    }
    let current = stage
        .as_mut()
        .filter(|stage| stage.core_uid == core_uid && stage.strategy_id == strategy_id)
        .context("a chain page without its first")?;
    current.raw += versions.iter().map(|v| v.raw_json.len()).sum::<usize>();
    if current.raw > STAGE_LIMIT {
        *stage = None;
        anyhow::bail!("a chain over {STAGE_LIMIT} bytes");
    }
    current.versions.extend(versions);
    if page.resume.is_some() {
        return Ok(None);
    }
    let Some(done) = stage.take() else {
        anyhow::bail!("a chain page without its first");
    };
    drop(stage);
    let sink = super::sink().context("the strategy archive is not open")?;
    let answer = sink.merge_chain(super::merge::IncomingChain {
        core_uid,
        core_name: done.head.core_name.clone(),
        strategy_id,
        head: Some(done.head),
        versions: done.versions,
        from_station: false,
    })?;
    answer
        .recv_timeout(STATION_MERGE_WAIT)
        .context("the strategy archive did not answer in time")?
        .map(Some)
        .map_err(anyhow::Error::msg)
}

/// `app_meta` key of the marker that this machine's history was saved before its first merge.
const FIRST_MERGE_SNAPSHOT_KEY: &str = "first_merge_snapshot_ms";

/// Save this machine's history once, before anything from a station is merged into it.
///
/// A terminal's file is its only copy of its history, and the merge is the one thing that writes
/// into it from outside its own feed. A store with nothing in it has nothing to save.
///
/// Returns:
///     `Ok` when merging may go on; an error means it must not.
pub fn snapshot_before_first_merge() -> anyhow::Result<()> {
    if !super::backup::source_has_strategy_rows() {
        return Ok(());
    }
    // A marker beside the writer's rows, as `stats` keeps its cache: WAL serialises the two.
    let conn = Connection::open(crate::config::paths::strategies_db_path())
        .context("open the strategy archive")?;
    let _ = conn.busy_timeout(std::time::Duration::from_secs(3));
    let marked: Option<String> = conn
        .query_row(
            "SELECT value FROM app_meta WHERE key=?1",
            [FIRST_MERGE_SNAPSHOT_KEY],
            |row| row.get(0),
        )
        .optional()?;
    if marked.is_some() {
        return Ok(());
    }
    let path = super::backup::snapshot_now()?;
    log::info!(
        "strategies(db): saved before the first merge from the station: {}",
        path.display()
    );
    conn.execute(
        "INSERT OR REPLACE INTO app_meta (key, value) VALUES (?1, ?2)",
        params![
            FIRST_MERGE_SNAPSHOT_KEY,
            crate::util::now_unix_ms_i64().to_string()
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
