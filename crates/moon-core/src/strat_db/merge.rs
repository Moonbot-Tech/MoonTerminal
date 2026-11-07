//! Merges two observers' version chains of one strategy into one history.
//!
//! The station and every terminal record a strategy independently, each from its own feed, so one
//! edit reaches them as two rows. Nothing on those rows identifies the edit across observers:
//! `valid_from` is the observer's local clock, `server_ms` moves without the content changing and
//! `server_ver` repeats across different contents (710 groups on a real archive, zero on 12 767 of
//! its 19 781 rows). What both observers do share is the ORDER in which contents succeeded each
//! other, each having possibly missed a stretch while offline. So the chains are aligned the way a
//! diff aligns two texts, by content and never by clock, and the merge is the shortest history that
//! holds both.
//!
//! The rule was measured on a model over a real archive's chains (`STATION_MCP_ANALYTICS.md` §4.4):
//! no invented versions with one observer offline 30 % of the time and with its clock off by hours,
//! where a rule matching rows by overlapping time invented hundreds.
//!
//! Merging only ever adds. A stored version's `raw_json` is kept, none of this store's versions is
//! lost, and only the columns derived from a row's neighbours move: `valid_from` to the earlier
//! observation, `valid_to`, and the diff when the predecessor changed. The one bound on that is the
//! retention every insert applies (`MAX_VERSIONS_PER_STRATEGY`, newest kept).

use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, ensure};
use rusqlite::{Connection, params};
use serde_json::{Map, Value};

use super::write::{Head, State, diff_maps, hash_of, strip};
use crate::station_api::{StrategyHead, StrategyVersion};
use crate::util::now_unix_ms_i64 as now_ms;

/// A chain longer than this in the middle of an alignment is not aligned at all: the table would
/// outgrow what a one-vCPU station should allocate, and two chains that different share nothing an
/// alignment could use. Two full chains (`MAX_VERSIONS_PER_STRATEGY` squared) stay well under it.
const MAX_ALIGN_CELLS: usize = 1 << 25;
/// How far ahead of this machine's clock an incoming version may be stamped. A day covers any
/// wrong clock a machine still works with.
const MAX_CLOCK_LEAD_MS: i64 = 24 * 60 * 60 * 1000;

/// One version as the alignment sees it: when it was observed and what it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Link {
    /// `valid_from` of the row.
    pub time: i64,
    /// Interned content, equal exactly when the two rows hold the same version.
    pub content: u32,
}

/// One row of the merged chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Planned {
    /// The row's index in the first chain, when it holds it.
    pub first: Option<usize>,
    /// The row's index in the second chain, when it holds it.
    pub second: Option<usize>,
    pub valid_from: i64,
    pub content: u32,
}

/// The longest common subsequence of two chains' contents, as index pairs in order.
///
/// The common head and tail are matched without a table: after the first exchange two chains
/// differ, if at all, in a short stretch, so the table is almost always empty.
///
/// Args:
///     first: The chain ties resolve towards.
///     second: The other chain.
///
/// Returns:
///     Ascending `(first index, second index)` pairs.
fn align(first: &[Link], second: &[Link]) -> Vec<(usize, usize)> {
    let same = |i: usize, j: usize| first[i].content == second[j].content;
    let shorter = first.len().min(second.len());
    let head = (0..shorter).take_while(|&k| same(k, k)).count();
    let tail = (0..shorter - head)
        .take_while(|&k| same(first.len() - 1 - k, second.len() - 1 - k))
        .count();
    let mut pairs: Vec<(usize, usize)> = (0..head).map(|k| (k, k)).collect();
    let (n, m) = (first.len() - head - tail, second.len() - head - tail);
    if n > 0 && m > 0 && (n + 1).saturating_mul(m + 1) <= MAX_ALIGN_CELLS {
        let width = m + 1;
        // u16 holds it: `MAX_ALIGN_CELLS` bounds the shorter side well under 65 536.
        let mut table = vec![0u16; (n + 1) * width];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                table[i * width + j] = if same(head + i, head + j) {
                    table[(i + 1) * width + j + 1] + 1
                } else {
                    table[(i + 1) * width + j].max(table[i * width + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if same(head + i, head + j) {
                pairs.push((head + i, head + j));
                i += 1;
                j += 1;
            } else if table[(i + 1) * width + j] >= table[i * width + j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    pairs.extend((0..tail).map(|k| (first.len() - tail + k, second.len() - tail + k)));
    pairs
}

/// Append one row unless it repeats the content before it, keeping `valid_from` strictly rising.
fn push_row(out: &mut Vec<Planned>, row: Planned) {
    let Some(last) = out.last() else {
        out.push(row);
        return;
    };
    if last.content == row.content {
        return;
    }
    let valid_from = row.valid_from.max(last.valid_from.saturating_add(1));
    out.push(Planned { valid_from, ..row });
}

/// Append the rows only one chain holds, between two aligned rows.
///
/// Each keeps its own time only when that time fits the gap. One outside it comes from a clock the
/// other side disagrees with, and it is the aligned rows that hold the order, so it takes the next
/// free millisecond instead of moving them.
///
/// Args:
///     out: The merged chain so far.
///     rows: The stretch's rows from both chains, in any order.
///     next: `valid_from` of the aligned row that follows, when one does.
fn push_stretch(out: &mut Vec<Planned>, mut rows: Vec<Planned>, next: Option<i64>) {
    // The first chain wins a tie, so both sides of an exchange order the stretch alike.
    rows.sort_by_key(|row| (row.valid_from, row.first.is_none()));
    let count = rows.len() as i64;
    for (index, row) in rows.into_iter().enumerate() {
        let floor = out.last().map(|last| last.valid_from);
        let fits = floor.is_none_or(|floor| floor < row.valid_from)
            && next.is_none_or(|next| row.valid_from < next);
        let valid_from = match (fits, floor, next) {
            (true, ..) => row.valid_from,
            (false, Some(floor), _) => floor.saturating_add(1),
            // Nothing before it and its time is past the aligned row: just before that row.
            (false, None, Some(next)) => next.saturating_sub(count - index as i64),
            (false, None, None) => row.valid_from,
        };
        push_row(out, Planned { valid_from, ..row });
    }
}

/// The merged chain of two chains of one strategy.
///
/// Args:
///     first: The station's chain, whichever side merges, so that both sides resolve an ambiguous
///         alignment alike and arrive at one history.
///     second: The other chain.
///
/// Returns:
///     The rows in order: strictly rising `valid_from`, no two neighbours with one content.
pub(super) fn plan(first: &[Link], second: &[Link]) -> Vec<Planned> {
    let only_first = |i: usize| Planned {
        first: Some(i),
        second: None,
        valid_from: first[i].time,
        content: first[i].content,
    };
    let only_second = |j: usize| Planned {
        first: None,
        second: Some(j),
        valid_from: second[j].time,
        content: second[j].content,
    };
    let mut out = Vec::with_capacity(first.len().max(second.len()));
    let (mut from_first, mut from_second) = (0, 0);
    for (i, j) in align(first, second) {
        // The earlier observation is the closer one to the edit itself.
        let at = first[i].time.min(second[j].time);
        let stretch = (from_first..i)
            .map(only_first)
            .chain((from_second..j).map(only_second))
            .collect();
        push_stretch(&mut out, stretch, Some(at));
        push_row(
            &mut out,
            Planned {
                first: Some(i),
                second: Some(j),
                valid_from: at,
                content: first[i].content,
            },
        );
        (from_first, from_second) = (i + 1, j + 1);
    }
    let stretch = (from_first..first.len())
        .map(only_first)
        .chain((from_second..second.len()).map(only_second))
        .collect();
    push_stretch(&mut out, stretch, None);
    out
}

/// What merging one chain did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MergeOutcome {
    /// Whether this store changed.
    pub changed: bool,
    /// Whether the merged chain holds something the incoming one does not, so its sender should
    /// merge this chain in turn.
    pub other_lacks: bool,
}

/// A chain arriving at the writer.
pub struct IncomingChain {
    pub core_uid: u64,
    /// The core's name here, for a head this store does not have yet.
    pub core_name: String,
    pub strategy_id: i64,
    /// `None` only from a sender that holds no versions of the strategy.
    pub head: Option<StrategyHead>,
    /// Ascending by `valid_from`.
    pub versions: Vec<StrategyVersion>,
    /// Whether the chain is the station's, which is what fixes the alignment's direction.
    pub from_station: bool,
}

/// One row of `strategy_versions`.
#[derive(Clone, Debug)]
struct Row {
    valid_from: i64,
    valid_to: Option<i64>,
    change_kind: String,
    origin: Option<String>,
    n_changed: i64,
    ver_gap: bool,
    server_ver: i32,
    server_ms: i64,
    checked_at: i64,
    raw_json: String,
    changed_json: Option<String>,
}

impl Row {
    /// A row from another observer. Its `local` meant "edited through that observer", which from
    /// here is someone else's edit; its diff is rebuilt against this chain.
    fn incoming(version: &StrategyVersion) -> Self {
        let origin = match version.origin.as_deref() {
            Some("local") => Some("external".to_owned()),
            other => other.map(str::to_owned),
        };
        Self {
            valid_from: version.valid_from,
            valid_to: version.valid_to,
            change_kind: version.change_kind.clone(),
            origin,
            n_changed: 0,
            ver_gap: version.ver_gap,
            server_ver: version.server_ver,
            server_ms: version.server_ms,
            checked_at: version.checked_at,
            raw_json: version.raw_json.clone(),
            changed_json: None,
        }
    }

    /// The row's dump as a map; a row that does not parse counts as empty, never as an error, so
    /// one bad archived row cannot stop a chain from merging.
    fn fields(&self) -> Map<String, Value> {
        match serde_json::from_str::<Value>(&self.raw_json) {
            Ok(Value::Object(fields)) => fields,
            _ => Map::new(),
        }
    }
}

/// Assigns one number per distinct version content, under this store's `ignore_fields`.
///
/// Compared as the canonical text rather than through `hash_of`: `DefaultHasher` promises nothing
/// across builds, and two machines compare here. Canonical means sorted keys, and sorted here: a
/// dependency turns on `serde_json`'s `preserve_order` in some builds and not in others, so a parsed
/// map keeps whatever order the row was written in, and imported rows have their own. Left to the
/// map, one content reads as two, and every first sighting by a new station becomes an edit with an
/// empty diff.
#[derive(Default)]
struct Contents {
    seen: HashMap<String, u32>,
}

impl Contents {
    fn link(&mut self, row: &Row, state: &State) -> Link {
        let text = match serde_json::from_str::<Value>(&row.raw_json) {
            Ok(Value::Object(fields)) => {
                let stripped = strip(&fields, &state.ignore);
                let sorted: BTreeMap<&String, &Value> = stripped.iter().collect();
                serde_json::to_string(&sorted).unwrap_or_else(|_| row.raw_json.clone())
            }
            _ => row.raw_json.clone(),
        };
        let next = self.seen.len() as u32;
        Link {
            time: row.valid_from,
            content: *self.seen.entry(text).or_insert(next),
        }
    }
}

/// The time before which `strategy_id` of `core_uid` was forgotten here, if it was.
///
/// Args:
///     conn: A connection to the store.
///     core_uid: The core.
///     strategy_id: The strategy; a whole core's entry covers every one of its strategies.
///
/// Returns:
///     The later of the strategy's and the core's moment, or `None` when neither was forgotten.
pub(super) fn forgotten_before(
    conn: &Connection,
    core_uid: u64,
    strategy_id: i64,
) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT MAX(at_ms) FROM strategy_forgotten
         WHERE core_uid=?1 AND strategy_id IN (?2, ?3)",
        params![core_uid as i64, strategy_id, super::WHOLE_CORE],
        |row| row.get(0),
    )
}

/// Refuse a chain that cannot be a chain, before it reaches this store.
fn check_incoming(chain: &IncomingChain) -> anyhow::Result<()> {
    ensure!(
        chain.versions.len() <= 2 * super::MAX_VERSIONS_PER_STRATEGY as usize,
        "a chain of {} versions",
        chain.versions.len()
    );
    // A stamp past this is no clock's honest reading, and a chain ending on one would push every
    // later version of the strategy here to a time in the future (`valid_from` only ever rises).
    let latest = now_ms().saturating_add(MAX_CLOCK_LEAD_MS);
    ensure!(
        chain
            .versions
            .iter()
            .all(|version| (1..latest).contains(&version.valid_from)),
        "a version stamped outside any sane time"
    );
    ensure!(
        chain.versions.iter().all(|version| version.valid_from > 0)
            && chain
                .versions
                .windows(2)
                .all(|pair| pair[0].valid_from < pair[1].valid_from),
        "the chain's times do not rise"
    );
    ensure!(
        chain.versions.is_empty() || chain.head.is_some(),
        "a chain without its strategy"
    );
    Ok(())
}

/// Read this store's chain of one strategy.
fn read_rows(conn: &Connection, core_uid: i64, strategy_id: i64) -> rusqlite::Result<Vec<Row>> {
    let mut statement = conn.prepare(
        "SELECT valid_from, valid_to, change_kind, origin, n_changed, ver_gap, server_ver,
                server_ms, checked_at, raw_json, changed_json
         FROM strategy_versions WHERE core_uid=?1 AND strategy_id=?2 ORDER BY valid_from",
    )?;
    let rows = statement
        .query_map(params![core_uid, strategy_id], |row| {
            Ok(Row {
                valid_from: row.get(0)?,
                valid_to: row.get(1)?,
                change_kind: row.get(2)?,
                origin: row.get(3)?,
                n_changed: row.get(4)?,
                ver_gap: row.get::<_, i64>(5)? != 0,
                server_ver: row.get(6)?,
                server_ms: row.get(7)?,
                checked_at: row.get(8)?,
                raw_json: row.get(9)?,
                changed_json: row.get(10)?,
            })
        })?
        .collect();
    rows
}

/// Build the rows to store from the merged plan.
///
/// A stored row that kept its predecessor keeps its stored diff, so archived history that was
/// recorded under other rules is not rewritten by a merge; a row with a new predecessor gets a
/// diff against it.
///
/// Args:
///     planned: The merged chain.
///     own: For each planned row, its index in `local` when this store holds it.
///     local: This store's rows.
///     incoming: For each planned row without `own`, the row that came in.
///     state: Writer state, for `ignore_fields`.
///
/// Returns:
///     The rows, with `valid_to` still to be closed.
fn build_rows(
    planned: &[Planned],
    own: impl Fn(&Planned) -> Option<usize>,
    local: &[Row],
    incoming: impl Fn(&Planned) -> Option<Row>,
    state: &State,
) -> anyhow::Result<Vec<Row>> {
    let mut rows: Vec<Row> = Vec::with_capacity(planned.len());
    let mut previous_own = None;
    for plan_row in planned {
        let own_index = own(plan_row);
        let mut row = match own_index {
            Some(index) => local[index].clone(),
            None => incoming(plan_row).context("a merged row with no source")?,
        };
        row.valid_from = plan_row.valid_from;
        match rows.last() {
            // The history's first row. A stored one keeps what it says: it is first either because
            // it always was, or because retention trimmed its past and it is no creation. A row
            // from elsewhere starts the history here.
            None => {
                if own_index.is_none() {
                    row.change_kind = "created".to_owned();
                    row.changed_json = None;
                    row.n_changed = 0;
                }
            }
            Some(previous) => {
                let kept = matches!(
                    (own_index, previous_own),
                    (Some(index), Some(before)) if index == before + 1
                );
                if !kept {
                    let diff = diff_maps(
                        &previous.raw_json,
                        &strip(&row.fields(), &state.ignore),
                        &state.ignore,
                    );
                    row.n_changed = diff.len() as i64;
                    row.changed_json = Some(serde_json::to_string(&Value::Object(diff))?);
                }
                // A first sighting by one observer is no creation inside a longer history.
                if row.change_kind == "created" {
                    row.change_kind = "params".to_owned();
                }
            }
        }
        previous_own = own_index;
        rows.push(row);
    }
    Ok(rows)
}

/// Close each row at its successor, and the newest as this strategy's state says.
///
/// A stored row whose successor did not change keeps its stored end: that end can be earlier than
/// the successor's start, when the strategy was deleted and restored between them, and that gap is
/// history a merge has no reason to rewrite.
///
/// Args:
///     rows: The merged rows.
///     kept: Whether row `i` keeps its stored end.
///     local_last: `valid_to` of this store's newest row before the merge.
///     locally_deleted: `Some(deleted)` when this store has the head, whose word it keeps.
fn close_ranges(
    rows: &mut [Row],
    kept: impl Fn(usize) -> bool,
    local_last: Option<i64>,
    locally_deleted: Option<bool>,
) {
    let starts: Vec<i64> = rows.iter().map(|row| row.valid_from).collect();
    for (index, (row, next)) in rows.iter_mut().zip(starts.iter().skip(1)).enumerate() {
        if !kept(index) {
            row.valid_to = Some(*next);
        }
    }
    if let Some(last) = rows.last_mut() {
        last.valid_to = match locally_deleted {
            Some(false) => None,
            // Never before the row's own start: a merged newest row can be later than the moment
            // this store saw the strategy go.
            Some(true) => local_last
                .or(last.valid_to)
                .map(|to| to.max(last.valid_from)),
            // A new head: the incoming chain's last row says it, as the sender's head does.
            None => last.valid_to,
        };
    }
}

/// Merge one incoming chain into this store, in one transaction.
///
/// Args:
///     conn: The writer's connection.
///     state: Writer state; its head mirror learns a new head once the merge is durable.
///     chain: What came in.
///
/// Returns:
///     Whether this store changed and whether the sender lacks something.
pub(super) fn merge_chain(
    conn: &Connection,
    state: &mut State,
    chain: IncomingChain,
) -> anyhow::Result<MergeOutcome> {
    check_incoming(&chain)?;
    let core_uid = chain.core_uid as i64;
    let strategy_id = chain.strategy_id;
    // What the user removed from this machine does not come back from another.
    let forgotten = forgotten_before(conn, chain.core_uid, strategy_id)?;
    let incoming: Vec<Row> = chain
        .versions
        .iter()
        .filter(|version| forgotten.is_none_or(|at| version.valid_from >= at))
        .map(Row::incoming)
        .collect();
    let local = read_rows(conn, core_uid, strategy_id)?;
    if incoming.is_empty() && local.is_empty() {
        return Ok(MergeOutcome::default());
    }

    let mut contents = Contents::default();
    let local_links: Vec<Link> = local.iter().map(|r| contents.link(r, state)).collect();
    let incoming_links: Vec<Link> = incoming.iter().map(|r| contents.link(r, state)).collect();
    let (first, second) = if chain.from_station {
        (&incoming_links, &local_links)
    } else {
        (&local_links, &incoming_links)
    };
    let mut planned = plan(first, second);
    // The same bound an ordinary insert applies: newest kept, on both sides alike.
    let bound = super::MAX_VERSIONS_PER_STRATEGY as usize;
    if planned.len() > bound {
        planned.drain(..planned.len() - bound);
    }
    let same_as = |links: &[Link]| {
        links.len() == planned.len()
            && links
                .iter()
                .zip(&planned)
                .all(|(link, row)| link.content == row.content && link.time == row.valid_from)
    };
    let other_lacks = !same_as(&incoming_links);
    if same_as(&local_links) {
        return Ok(MergeOutcome {
            changed: false,
            other_lacks,
        });
    }

    let from_station = chain.from_station;
    let own = |row: &Planned| if from_station { row.second } else { row.first };
    let theirs = |row: &Planned| if from_station { row.first } else { row.second };
    let mut rows = build_rows(
        &planned,
        own,
        &local,
        |row| theirs(row).map(|index| incoming[index].clone()),
        state,
    )?;
    let existing = state.heads.get(&(core_uid, strategy_id));
    close_ranges(
        &mut rows,
        |index| {
            let (this, next) = (own(&planned[index]), planned.get(index + 1));
            match (this, next.map(|row| (own(row), row.valid_from))) {
                (Some(this), Some((Some(following), start))) => {
                    following == this + 1 && local[following].valid_from == start
                }
                _ => false,
            }
        },
        local.last().and_then(|row| row.valid_to),
        existing.map(|head| head.deleted),
    );
    // A strategy this store never saw gets its head; one it has keeps its own, which the next set
    // from the core corrects through the ordinary path if it is behind.
    let new_head = match (existing, &chain.head) {
        (Some(_), _) => None,
        (None, Some(head)) => Some(head),
        (None, None) => anyhow::bail!("a merged strategy with no head"),
    };

    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM strategy_versions WHERE core_uid=?1 AND strategy_id=?2",
        params![core_uid, strategy_id],
    )?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO strategy_versions (core_uid, strategy_id, valid_from, valid_to,
                change_kind, origin, n_changed, ver_gap, server_ver, server_ms,
                checked_at, raw_json, changed_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
        )?;
        for row in &rows {
            insert.execute(params![
                core_uid,
                strategy_id,
                row.valid_from,
                row.valid_to,
                row.change_kind,
                row.origin,
                row.n_changed,
                row.ver_gap as i64,
                row.server_ver,
                row.server_ms,
                row.checked_at,
                row.raw_json,
                row.changed_json,
            ])?;
        }
    }
    // The per-version statistics cache is keyed by `valid_from`, which may just have moved.
    match tx.execute(
        "DELETE FROM version_stats WHERE core_uid=?1 AND strategy_id=?2",
        params![core_uid, strategy_id],
    ) {
        Ok(_) => {}
        Err(rusqlite::Error::SqliteFailure(_, Some(ref message)))
            if message.contains("no such table") => {}
        Err(error) => return Err(error.into()),
    }
    let mut new_mirror = None;
    if let Some(head) = new_head {
        let content_hash = rows
            .last()
            .map(|row| hash_of(&row.fields(), &state.ignore))
            .unwrap_or_default();
        tx.execute(
            "INSERT INTO strategies (core_uid, strategy_id, core_name, name, kind, kind_ordinal,
                folder_path, is_short, checked, server_ver, server_ms, deleted,
                content_hash, head_hash, updated_ms)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,0,?14)",
            params![
                core_uid,
                strategy_id,
                chain.core_name,
                head.name,
                head.kind,
                head.kind_ordinal as i64,
                head.folder_path,
                head.is_short as i64,
                head.checked as i64,
                head.server_ver,
                head.server_ms,
                head.deleted as i64,
                content_hash,
                now_ms(),
            ],
        )?;
        // A head hash of zero matches nothing: the core's next set refreshes the head and, since
        // the content hash matches, adds no version.
        new_mirror = Some(Head {
            content_hash,
            head_hash: 0,
            deleted: head.deleted,
            server_ver: head.server_ver,
        });
    }
    tx.commit()?;
    // After the commit, for the reason `write::forget` gives.
    if let Some(head) = new_mirror {
        state.heads.insert((core_uid, strategy_id), head);
    }
    Ok(MergeOutcome {
        changed: true,
        other_lacks,
    })
}

#[cfg(test)]
mod tests;
