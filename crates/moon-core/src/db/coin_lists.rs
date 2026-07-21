//! "Which coins sit in the selected strategies' lists, on which core, and when each was last
//! put there."
//!
//! The BLACKLIST is the one with dates — it is the list under review, and the one the panel
//! draws. The whitelist is carried as bare entries only: nothing displays it, but it is
//! editable in the same table, and a save that could not reproduce its value would delete
//! every contract-suffixed entry it holds.
//!
//! The tuner's coin table answers "what did this coin do, and what am I about to do with
//! it"; this answers the other half — what the lists themselves currently hold. The date
//! comes out of the version history in `strategies.sqlite`: a version is written whenever a
//! strategy's content changes, and `changed_json` carries the before/after of every field
//! that moved, so the moment a coin entered a list is recoverable exactly, without replaying
//! whole snapshots.
//!
//! WHAT THE DATE MEANS, precisely: the moment the TERMINAL OBSERVED that edit. It is not
//! "when the user added the coin" and the two can differ — the writer stamps `valid_from`
//! with its own clock, and it flags versions that span server edits it never saw
//! (`ver_gap`).
//!
//! A coin whose addition predates the terminal's first sight of the strategy has no such
//! moment, and gets `before_ms` instead — a BOUND: "it was already listed by then". The two
//! are kept apart HERE so that nothing has to guess which it holds; whether a screen draws
//! them differently is that screen's decision (the coin picker deliberately does not, having
//! been asked for one plain date).
//!
//! The strategy's oldest retained version is therefore usable as a bound but NEVER as a
//! date: `strat_db` tags the first version it ever writes for a strategy `created` even when
//! the strategy is years old and merely new to this database (see `strat_db::write`, the
//! `st.heads.get(&key) => None` arm). Presenting that as "added on" would stamp every list on
//! a fresh install with the install date — measured on a real database as hundreds of
//! "creations" sharing one timestamp, some of them at unix epoch 1. "No later than X" is a
//! fact; "added on X" from the same row is a lie the user cannot see through.

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use super::read_fail::{io_fail, read_fail};
use super::{ReadFail, ReadResult};
use crate::symbol::parse_coin_list;

/// Timestamps at or below this are storage sentinels, not dates.
///
/// The real database carries `valid_from = 1` on rows that arrived through an import path,
/// and "01.01.1970" in a date column is worse than an admitted blank: it reads as data.
/// 2015-01-01, comfortably before anything this application could have observed.
const EPOCH_FLOOR_MS: i64 = 1_420_070_400_000;

/// One coin on one core's list.
///
/// Keyed by core rather than by strategy: several strategies of one core routinely share a
/// list, and repeating a coin once per strategy would turn "BTC is blacklisted here" into a
/// wall of identical rows.
#[derive(Debug, Clone)]
pub struct CoinListRow {
    /// The matching token, as every other coin comparison on the page spells it.
    pub coin: String,
    /// The entries AS WRITTEN that fold to this token, sorted and deduplicated.
    ///
    /// Usually just the coin itself, but a list may hold `BTC, BTC_0626, BTC_0925` — three
    /// entries matching one coin. Anything reproducing the field's VALUE must write all of
    /// them back; anything matching a trade against it wants only `coin`.
    pub entries: Vec<String>,
    pub core_uid: u64,
    pub core_name: String,
    /// unix-ms of the most recent observed edit that put this coin on the list — `None`
    /// when the retained history does not reach back far enough to say.
    pub since_ms: Option<i64>,
    /// Upper bound for a coin with no observed addition: it was already on the list at the
    /// earliest version this read can attest to, so it got there no later than that.
    ///
    /// "Earliest attestable", not literally the strategy's first version: versions carrying a
    /// sentinel timestamp are skipped, which can only make the bound LATER — a weaker claim,
    /// still a true one.
    ///
    /// A weaker claim than `since_ms` and never shown alongside it, but a true one, and far
    /// more use than a blank cell. Not to be confused with the strategy's own `LastEditDate`,
    /// which dates the last edit of ANYTHING in the strategy — for a coin that provably has
    /// not moved since, that would be a confident falsehood.
    pub before_ms: Option<i64>,
}

impl CoinListRow {
    /// The timestamp this row sorts by: the exact date when there is one, otherwise the
    /// bound. A bound means "at least this old", which is exactly where it belongs in a
    /// chronological order.
    pub fn effective_ms(&self) -> Option<i64> {
        self.since_ms.or(self.before_ms)
    }
}

/// One coin on one core's WHITELIST — the entries, and nothing else.
///
/// No dates: no screen shows a whitelist date, so the history walk that produces them is not
/// run for it. Everything here exists for one reason — reproducing the field's VALUE when it
/// is saved, which needs the entries as written and nothing more. Kept a separate type rather
/// than a [`CoinListRow`] with `None` dates, because "not read" and "unknown" are different
/// claims and a shared type would let a renderer confuse them.
#[derive(Debug, Clone)]
pub struct CoinListEntries {
    /// The matching token, folded like every other coin comparison on the page.
    pub coin: String,
    /// The entries AS WRITTEN that fold to this token, sorted and deduplicated.
    pub entries: Vec<String>,
    pub core_uid: u64,
}

/// The strategies' lists: the blacklist folded per coin and core with its dates, and the
/// whitelist as bare entries.
///
/// Both sides are carried because both are EDITABLE — the coin table has a tick column for
/// each, and a save that knew only about the blacklist dropped whitelist edits on the floor
/// without a word.
#[derive(Debug, Clone, Default)]
pub struct CoinListRows {
    pub black: Vec<CoinListRow>,
    pub white: Vec<CoinListEntries>,
}

/// Strategy identity inside this module — `(core_uid, strategy_id)`, the DB's own key.
type Key = (i64, i64);

/// One version that edited a list: the value before and after.
struct Change {
    at: i64,
    old: String,
    new: String,
}

/// Coins on the blacklist of the strategies in `targets`, with the core they belong to and
/// when each was last added.
///
/// `targets` is `(strategy_id, core_uid)` exactly as the tuner's selection carries it. An
/// EMPTY slice means an empty selection and yields no rows — deliberately not "then show
/// every strategy": the coin table's tick boxes read the same field through
/// `db::tuner::strategy_current_values` per selected strategy, so an all-strategies fallback
/// here would put a thousand-entry blacklist next to a table with every box unticked and its
/// own "blacklist 0" chip greyed out. One page, one answer.
///
/// Goes to disk: call it on a background executor.
pub fn coin_lists(targets: &[(i64, Option<u64>)]) -> ReadResult<CoinListRows> {
    if targets.is_empty() {
        return Ok(CoinListRows::default());
    }
    let mut conn = open_strategies()?;
    // One snapshot for all three statements. Separate autocommit reads each see a newer
    // state, so a `strat_db` commit landing mid-read could hand us a head listing a coin
    // whose add-version came from the scan before it — an undated row invented by timing.
    let tx = conn
        .transaction()
        .map_err(|e| read_fail("analytics: coin lists (read snapshot)", e))?;
    let scope = scope_sql(targets);
    let changes = read_changes(&tx, &scope)?;
    let first_seen = read_first_seen(&tx, &scope)?;
    let heads = read_heads(&tx, &scope)?;

    // (token, core) → the most recent observed edit that added it on that core. MAX, not
    // "first one wins": with several strategies selected, "when did this list last change
    // for this coin" is the latest such decision, which is what the column claims.
    let mut black: HashMap<(String, u64), Agg> = HashMap::new();
    // The whitelist needs entries only, so it skips the history walk entirely.
    let mut white: HashMap<(String, u64), BTreeSet<String>> = HashMap::new();
    for head in &heads {
        for entry in crate::symbol::split_coin_list(&head.white) {
            white
                .entry((crate::symbol::coin_match_key(entry), head.core_uid as u64))
                .or_default()
                .insert(entry.to_string());
        }
    }
    for head in heads {
        let key = (head.core_uid, head.strategy_id);
        let added = added_when(changes.get(&key).map(Vec::as_slice).unwrap_or(&[]));
        let seen = first_seen.get(&key).copied();
        for entry in crate::symbol::split_coin_list(&head.black) {
            let coin = crate::symbol::coin_match_key(entry);
            let since = added.get(&coin).copied();
            let uid = head.core_uid as u64;
            let slot = black.entry((coin, uid)).or_insert_with(|| Agg {
                core_name: head.core_name.clone(),
                entries: BTreeSet::new(),
                since: None,
                before: None,
            });
            // Kept AS WRITTEN: the field's value is these entries, and one core's strategies
            // may spell the same coin several ways.
            slot.entries.insert(entry.to_string());
            // A known date from one strategy stands for the pair; the row's claim is
            // "this core's list last gained the coin then", and that is still the most
            // recent thing actually known about it.
            slot.since = match (slot.since, since) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
            // The bound comes ONLY from strategies that never observed the addition — for
            // the others the exact date is what we have. MIN, because the earliest strategy
            // still carrying the coin proves the strongest true statement: it has been
            // listed here since at least then.
            if since.is_none() {
                slot.before = match (slot.before, seen) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
            }
        }
    }
    let mut white: Vec<CoinListEntries> = white
        .into_iter()
        .map(|((coin, core_uid), entries)| CoinListEntries {
            coin,
            entries: entries.into_iter().collect(),
            core_uid,
        })
        .collect();
    // Stable order so the saved value does not reshuffle itself between two identical saves.
    white.sort_by(|a, b| {
        a.coin
            .cmp(&b.coin)
            .then_with(|| a.core_uid.cmp(&b.core_uid))
    });
    Ok(CoinListRows {
        black: flatten(black),
        white,
    })
}

/// What one `(coin, core)` pair accumulates across the strategies that list it.
struct Agg {
    core_name: String,
    entries: BTreeSet<String>,
    since: Option<i64>,
    before: Option<i64>,
}

/// Turn the dedup map into the row list, newest first — the order that answers "what changed
/// lately", which is why the column exists. Coins nothing at all is known about sort last:
/// they are the unknowns, not the oldest ones.
fn flatten(map: HashMap<(String, u64), Agg>) -> Vec<CoinListRow> {
    let mut out: Vec<CoinListRow> = map
        .into_iter()
        .map(|((coin, core_uid), a)| CoinListRow {
            coin,
            entries: a.entries.into_iter().collect(),
            core_uid,
            core_name: a.core_name,
            since_ms: a.since,
            // A bound is only ever the answer when there is no exact date; keeping both
            // would let a renderer show the weaker claim over the stronger one.
            before_ms: a.since.is_none().then_some(a.before).flatten(),
        })
        .collect();
    out.sort_by(|a, b| {
        b.effective_ms()
            .cmp(&a.effective_ms())
            .then_with(|| a.coin.cmp(&b.coin))
            .then_with(|| a.core_name.cmp(&b.core_name))
    });
    out
}

/// The earliest version of each strategy this read will attest to — the upper bound for
/// anything already on a list by then.
///
/// Not necessarily the strategy's true first version: rows at or below [`EPOCH_FLOOR_MS`] are
/// excluded, so a strategy whose genuine first version carries a sentinel timestamp yields a
/// LATER one here. That only weakens the bound, never falsifies it. A strategy with nothing
/// above the floor yields no row at all, and its coins stay undated.
///
/// Touches no JSON at all: it is a `MIN` over an indexed column, unlike the head read.
fn read_first_seen(conn: &Connection, scope: &str) -> ReadResult<HashMap<Key, i64>> {
    const CTX: &str = "analytics: coin lists (first known version)";
    let sql = format!(
        "SELECT v.core_uid, v.strategy_id, MIN(v.valid_from)
           FROM strategy_versions v
           JOIN strategies s
             ON s.core_uid = v.core_uid AND s.strategy_id = v.strategy_id
          WHERE s.deleted = 0 AND v.valid_from > {EPOCH_FLOOR_MS}{scope}
          GROUP BY v.core_uid, v.strategy_id"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map([], |r| Ok(((r.get(0)?, r.get(1)?), r.get(2)?)))
        .map_err(|e| read_fail(CTX, e))?;
    let mut out = HashMap::new();
    for row in rows {
        let (k, v) = row.map_err(|e| read_fail(CTX, e))?;
        out.insert(k, v);
    }
    Ok(out)
}

/// When each coin was last ADDED, walking the observed edits forward.
///
/// A removal drops the coin's date rather than keeping it: a coin taken off a list and put
/// back belongs to the later decision, and one merely removed must not leave a date behind
/// for a row that no longer exists.
fn added_when(changes: &[Change]) -> HashMap<String, i64> {
    let mut when: HashMap<String, i64> = HashMap::new();
    for ch in changes {
        let (old, new) = (parse_coin_list(&ch.old), parse_coin_list(&ch.new));
        for coin in new.difference(&old) {
            when.insert(coin.clone(), ch.at);
        }
        for coin in old.difference(&new) {
            when.remove(coin);
        }
    }
    when
}

/// The current lists of one strategy.
struct Head {
    core_uid: i64,
    strategy_id: i64,
    core_name: String,
    black: String,
    white: String,
}

/// `AND (…)` restricting the read to the selected strategies. Never empty — the caller
/// returns early on an empty selection rather than letting this widen to every strategy.
///
/// The ids are `i64`/`u64` formatted as decimals, so there is nothing here a string could
/// inject; binding them instead would mean rebuilding the SQL per selection size anyway.
fn scope_sql(targets: &[(i64, Option<u64>)]) -> String {
    let terms: Vec<String> = targets
        .iter()
        .map(|(sid, core)| match core {
            Some(c) => format!("(s.strategy_id = {sid} AND s.core_uid = {})", *c as i64),
            None => format!("s.strategy_id = {sid}"),
        })
        .collect();
    format!(" AND ({})", terms.join(" OR "))
}

/// Read-only handle on the strategy database.
///
/// An absent file is `NotReady` (the terminal has simply never synced strategies), while a
/// file that exists and will not open is a real failure — the same distinction the reports
/// replica draws, and for the same reason: "no lists" and "could not read the lists" must
/// not render alike.
fn open_strategies() -> ReadResult<Connection> {
    const CTX: &str = "analytics: coin lists (opening strategies.sqlite)";
    let path = crate::config::paths::strategies_db_path();
    match std::fs::metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ReadFail::NotReady),
        Err(e) => return Err(io_fail(CTX, &e)),
    }
    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| read_fail(CTX, e))?;
    // The strat_db writer commits and checkpoints on its own thread; without this a write
    // landing under our snapshot surfaces to the user as a read failure. Same 3s the
    // strategy store's own reader uses.
    let _ = conn.busy_timeout(Duration::from_secs(3));
    Ok(conn)
}

/// The current list values of every strategy in scope.
///
/// `CAST(… AS TEXT)` because `json_extract` returns the value's OWN type: a list stored as a
/// number or a bool comes back INTEGER/REAL and fails the row decode, taking the whole panel
/// down over one odd field. `db::analytics` casts these same two fields for the same reason.
fn read_heads(conn: &Connection, scope: &str) -> ReadResult<Vec<Head>> {
    const CTX: &str = "analytics: coin lists (current values)";
    let sql = format!(
        "SELECT s.core_uid, s.strategy_id, s.core_name,
                COALESCE(CAST(json_extract(v.raw_json, '$.CoinsBlackList') AS TEXT), ''),
                COALESCE(CAST(json_extract(v.raw_json, '$.CoinsWhiteList') AS TEXT), '')
           FROM strategies s
           JOIN strategy_versions v
             ON v.core_uid = s.core_uid AND v.strategy_id = s.strategy_id
          WHERE v.valid_to IS NULL AND s.deleted = 0{scope}"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Head {
                core_uid: r.get(0)?,
                strategy_id: r.get(1)?,
                core_name: r.get(2)?,
                black: r.get(3)?,
                white: r.get(4)?,
            })
        })
        .map_err(|e| read_fail(CTX, e))?;
    let mut out = Vec::new();
    for row in rows {
        // Each row is one strategy's whole list: dropping it would silently shorten a table
        // the user reads as complete.
        out.push(row.map_err(|e| read_fail(CTX, e))?);
    }
    Ok(out)
}

/// Every version that edited the blacklist, ascending — the dates themselves.
fn read_changes(conn: &Connection, scope: &str) -> ReadResult<HashMap<Key, Vec<Change>>> {
    const CTX: &str = "analytics: coin lists (edit history)";
    let sql = format!(
        "SELECT v.core_uid, v.strategy_id, v.valid_from,
                CAST(json_extract(v.changed_json, '$.CoinsBlackList.old') AS TEXT),
                CAST(json_extract(v.changed_json, '$.CoinsBlackList.new') AS TEXT)
           FROM strategy_versions v
           JOIN strategies s
             ON s.core_uid = v.core_uid AND s.strategy_id = v.strategy_id
          WHERE s.deleted = 0
            AND v.valid_from > {EPOCH_FLOOR_MS}
            AND json_extract(v.changed_json, '$.CoinsBlackList') IS NOT NULL{scope}
          ORDER BY v.valid_from"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map([], |r| {
            let key: Key = (r.get(0)?, r.get(1)?);
            let at: i64 = r.get(2)?;
            // A NULL side means the field did not exist on that side of the edit — an empty
            // list is the same thing for our purposes.
            let s = |i: usize| -> rusqlite::Result<String> {
                Ok(r.get::<_, Option<String>>(i)?.unwrap_or_default())
            };
            Ok((key, at, s(3)?, s(4)?))
        })
        .map_err(|e| read_fail(CTX, e))?;
    let mut black: HashMap<Key, Vec<Change>> = HashMap::new();
    for row in rows {
        let (key, at, old, new) = row.map_err(|e| read_fail(CTX, e))?;
        if old != new {
            black.entry(key).or_default().push(Change { at, old, new });
        }
    }
    Ok(black)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(at: i64, old: &str, new: &str) -> Change {
        Change {
            at,
            old: old.to_string(),
            new: new.to_string(),
        }
    }

    /// An edit dates exactly the coins it ADDED.
    #[test]
    fn an_edit_dates_only_what_it_added() {
        let when = added_when(&[change(200, "BTC", "BTC, ETH")]);
        assert_eq!(when.get("ETH"), Some(&200));
        assert_eq!(
            when.get("BTC"),
            None,
            "a coin already there is not dated by an edit that did not touch it"
        );
    }

    /// A coin that was on the list before the terminal ever saw the strategy gets no exact
    /// date from the history walk — only the caller's bound, and only as "no later than".
    /// `added_when` itself must invent nothing.
    #[test]
    fn coins_older_than_the_history_stay_undated() {
        assert!(added_when(&[]).is_empty());
        // An edit that only removes leaves the survivors undated too.
        let when = added_when(&[change(200, "BTC, ETH", "BTC")]);
        assert_eq!(when.get("BTC"), None);
    }

    /// Removing a coin drops its date, and adding it back gives it the LATER one — a list
    /// answers for its current contents, not for everything it ever held.
    #[test]
    fn removal_forgets_and_re_adding_re_dates() {
        let when = added_when(&[change(100, "", "BTC"), change(200, "BTC", "")]);
        assert_eq!(when.get("BTC"), None, "removed — nothing to date");

        let when = added_when(&[
            change(100, "", "BTC"),
            change(200, "BTC", ""),
            change(300, "", "BTC"),
        ]);
        assert_eq!(when.get("BTC"), Some(&300), "the later decision wins");
    }

    /// The list is matched on the same folded token as everywhere else, so a strategy
    /// spelling a coin `mith` still dates the entry every other screen calls `MITH`.
    #[test]
    fn dates_are_keyed_by_the_shared_coin_token() {
        let when = added_when(&[change(100, "", "mith , btcst_1230")]);
        assert_eq!(when.get("MITH"), Some(&100));
        assert_eq!(when.get("BTCST"), Some(&100));
    }

    fn agg(since: Option<i64>, before: Option<i64>) -> Agg {
        Agg {
            core_name: "BB1".to_string(),
            entries: BTreeSet::new(),
            since,
            before,
        }
    }

    /// Newest first, and coins nothing is known about go last rather than passing for the
    /// oldest ones. A BOUND sorts at its own timestamp — "no later than X" is a claim that
    /// the coin is at least that old, which is exactly where it belongs chronologically.
    #[test]
    fn rows_are_newest_first_with_unknown_dates_last() {
        let mut map = HashMap::new();
        map.insert(("OLD".into(), 1), agg(Some(100i64), None));
        map.insert(("NEW".into(), 1), agg(Some(300), None));
        map.insert(("NONE".into(), 1), agg(None, None));
        map.insert(("BOUND".into(), 1), agg(None, Some(200)));
        let rows = flatten(map);
        let order: Vec<&str> = rows.iter().map(|r| r.coin.as_str()).collect();
        assert_eq!(order, vec!["NEW", "BOUND", "OLD", "NONE"]);
    }

    /// An exact date always beats the bound: keeping both would let a renderer show "no
    /// later than March" for a coin known to have been added in June.
    #[test]
    fn an_exact_date_suppresses_the_bound() {
        let mut map = HashMap::new();
        map.insert(("A".into(), 1), agg(Some(300), Some(100)));
        map.insert(("B".into(), 1), agg(None, Some(100)));
        let rows = flatten(map);
        let a = rows.iter().find(|r| r.coin == "A").expect("A");
        assert_eq!((a.since_ms, a.before_ms), (Some(300), None));
        let b = rows.iter().find(|r| r.coin == "B").expect("B");
        assert_eq!((b.since_ms, b.before_ms), (None, Some(100)));
        assert_eq!(b.effective_ms(), Some(100), "the bound orders the row");
    }

    /// An empty selection yields no rows. The all-strategies fallback it replaced put a
    /// thousand-row blacklist beside a coin table showing every box unticked.
    #[test]
    fn an_empty_selection_reads_nothing() {
        let rows = coin_lists(&[]).expect("an empty selection is not a failure");
        assert!(rows.black.is_empty());
    }

    /// The scope must pin each strategy to ITS core: the same id on another core is a
    /// different strategy, and a list read from it would be someone else's.
    #[test]
    fn scope_pins_each_strategy_to_its_core() {
        let one = scope_sql(&[(5, Some(7))]);
        assert!(
            one.contains("s.strategy_id = 5") && one.contains("s.core_uid = 7"),
            "{one}"
        );
        let many = scope_sql(&[(5, Some(7)), (9, None)]);
        assert_eq!(many.matches(" OR ").count(), 1, "{many}");
        assert!(many.contains("s.strategy_id = 9"), "{many}");
    }
}
