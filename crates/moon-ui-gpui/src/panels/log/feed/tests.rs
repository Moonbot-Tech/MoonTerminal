// Explicit imports on purpose: `use super::*` would pull in the parent's `gpui::*` re-export,
// whose `test` shadows the built-in attribute and makes `#[test]` expand recursively
// ("recursion limit reached").
use super::LiveCursors;
use crate::panels::log::{LogSource, LogSourceItem};
use moon_core::applog::LogLine;
use moon_core::session::{CoreId, CoreStore};
use std::collections::HashSet;

/// Builds a core selector entry, as `LogPanel::sources` would.
fn source(id: CoreId, display: &str) -> LogSourceItem {
    LogSourceItem {
        source: LogSource::Core(id),
        display: display.to_string(),
        file_label: display.to_string(),
    }
}

/// Pushes `msgs` into a core's ring the way `CoreData::apply` does, advancing both counters.
fn push(store: &mut CoreStore, id: CoreId, msgs: &[(&str, &str)]) {
    let core = store.core_mut(id).expect("test core must exist");
    for (ts, msg) in msgs {
        core.log.push_back(LogLine {
            ts: (*ts).to_string(),
            level: log::Level::Info,
            target: String::new(),
            msg: (*msg).to_string(),
        });
        core.log_seq += 1;
    }
}

fn store_with(ids: &[CoreId]) -> CoreStore {
    let mut store = CoreStore::default();
    for id in ids {
        store.ensure(*id);
    }
    store
}

fn msgs(lines: &[LogLine]) -> Vec<&str> {
    lines.iter().map(|l| l.msg.as_str()).collect()
}

/// After a snapshot, a pull must return the lines that arrived afterwards and nothing else.
///
/// Dropping the `seek_to_end` call would replay the whole ring the snapshot already showed; losing
/// the cursor update inside `pull` would replay every line on every backend revision — the exact
/// full-rebuild cost this module exists to remove.
#[test]
fn a_pull_returns_only_what_arrived_after_the_snapshot() {
    let mut store = store_with(&[1]);
    push(&mut store, 1, &[("01", "old-a"), ("02", "old-b")]);
    let sources = [source(1, "alpha")];
    let mut cursors = LiveCursors::default();

    cursors.seek_cores(&store, &sources, None);
    assert!(
        cursors.pull_cores(&store, &sources, None).is_empty(),
        "everything present at the snapshot must count as seen"
    );

    push(&mut store, 1, &[("03", "new-a")]);
    assert_eq!(msgs(&cursors.pull_cores(&store, &sources, None)), ["new-a"]);
    assert!(
        cursors.pull_cores(&store, &sources, None).is_empty(),
        "a line must be handed over exactly once"
    );
}

/// A reader that has never seen a core takes its whole ring, so opening the panel shows history.
#[test]
fn a_fresh_cursor_reads_the_whole_ring() {
    let mut store = store_with(&[1]);
    push(&mut store, 1, &[("01", "a"), ("02", "b")]);
    let sources = [source(1, "alpha")];

    let mut cursors = LiveCursors::default();
    assert_eq!(
        msgs(&cursors.pull_cores(&store, &sources, None)),
        ["a", "b"]
    );
}

/// More lines than the ring holds: the evicted ones are gone, and what remains arrives once.
///
/// Returning nothing would be worse than the gap — the panel would sit frozen while the core kept
/// logging — and returning the ring twice would duplicate rows the reader already has.
#[test]
fn an_overflowed_ring_yields_its_survivors_without_duplicates() {
    let mut store = store_with(&[1]);
    let sources = [source(1, "alpha")];
    let mut cursors = LiveCursors::default();
    push(&mut store, 1, &[("01", "seen")]);
    cursors.seek_cores(&store, &sources, None);

    // Six lines arrive, but the ring only kept the last three.
    let core = store.core_mut(1).expect("test core must exist");
    core.log.clear();
    core.log_seq += 6;
    push(&mut store, 1, &[("07", "x"), ("08", "y"), ("09", "z")]);

    assert_eq!(
        msgs(&cursors.pull_cores(&store, &sources, None)),
        ["x", "y", "z"],
        "the survivors come back even though the gap is unrecoverable"
    );
    assert!(cursors.pull_cores(&store, &sources, None).is_empty());
}

/// A core removed and re-added reuses its id, so its counter restarts BELOW the stored cursor.
///
/// Trusting the stale cursor would leave the panel permanently empty for that core: every
/// subtraction would saturate to zero.
#[test]
fn a_restarted_core_is_read_from_the_beginning() {
    let mut store = store_with(&[1]);
    let sources = [source(1, "alpha")];
    let mut cursors = LiveCursors::default();
    push(&mut store, 1, &[("01", "a"), ("02", "b"), ("03", "c")]);
    cursors.seek_cores(&store, &sources, None);

    let core = store.core_mut(1).expect("test core must exist");
    core.log.clear();
    core.log_seq = 0;
    push(&mut store, 1, &[("01", "restarted")]);

    assert_eq!(
        msgs(&cursors.pull_cores(&store, &sources, None)),
        ["restarted"]
    );
}

/// Every member contributes its unseen rows, each labelled with the core that wrote it.
///
/// Rows come out in canonical SOURCE order — one core's lines, then the next — because putting them
/// in time order is the buffer's invariant, established once in `RowBuffer::ingest`. A stable sort
/// there turns this order into the tie-break for equal timestamps, which is why it is asserted.
#[test]
fn an_aggregate_pull_labels_each_member_in_canonical_order() {
    let mut store = store_with(&[1, 2]);
    let sources = [source(1, "alpha"), source(2, "beta")];
    push(&mut store, 1, &[("01", "a1"), ("03", "a3")]);
    push(&mut store, 2, &[("02", "b2"), ("04", "b4")]);

    let mut cursors = LiveCursors::default();
    let lines = cursors.pull_cores(&store, &sources, None);

    assert_eq!(msgs(&lines), ["a1", "a3", "b2", "b4"]);
    assert_eq!(
        lines.iter().map(|l| l.target.as_str()).collect::<Vec<_>>(),
        ["alpha", "alpha", "beta", "beta"]
    );
}

/// A selected exchange reads only its members: a foreign core's rows must not appear under it.
#[test]
fn membership_bounds_the_rows() {
    let mut store = store_with(&[1, 2]);
    let sources = [source(1, "alpha"), source(2, "beta")];
    push(&mut store, 1, &[("01", "mine")]);
    push(&mut store, 2, &[("02", "foreign")]);
    let allowed = HashSet::from([1]);

    let mut cursors = LiveCursors::default();
    assert_eq!(
        msgs(&cursors.pull_cores(&store, &sources, Some(&allowed))),
        ["mine"]
    );
}

/// Clearing must forget positions, or a source switch would show the next source only its tail.
#[test]
fn clearing_replays_the_source() {
    let mut store = store_with(&[1]);
    let sources = [source(1, "alpha")];
    push(&mut store, 1, &[("01", "a")]);
    let mut cursors = LiveCursors::default();
    cursors.seek_cores(&store, &sources, None);

    cursors.clear();

    assert_eq!(msgs(&cursors.pull_cores(&store, &sources, None)), ["a"]);
}

/// Rows come out in ring order; ordering them by time is the buffer's invariant, covered by
/// `buffer::tests::an_unsorted_batch_is_ordered_on_the_way_in`.
#[test]
fn a_pull_preserves_ring_order() {
    let mut store = store_with(&[1]);
    push(
        &mut store,
        1,
        &[("03", "late"), ("01", "early"), ("02", "mid")],
    );
    let sources = [source(1, "alpha")];

    let mut cursors = LiveCursors::default();
    assert_eq!(
        msgs(&cursors.pull_cores(&store, &sources, None)),
        ["late", "early", "mid"]
    );
}

/// A core that leaves the membership KEEPS its cursor.
///
/// Dropping it looks like tidy housekeeping, and it is what makes rows duplicate: the core's rows
/// stay in the panel's buffer, so returning at cursor zero replays its whole ring on top of them.
#[test]
fn a_core_leaving_and_returning_does_not_replay_its_ring() {
    let mut store = store_with(&[1, 2]);
    let both = [source(1, "alpha"), source(2, "beta")];
    push(&mut store, 1, &[("01", "a1")]);
    push(&mut store, 2, &[("02", "b1")]);
    let mut cursors = LiveCursors::default();
    assert_eq!(cursors.pull_cores(&store, &both, None).len(), 2);

    // Beta drops out of scope for a while, then comes back with one new line.
    let only_alpha = HashSet::from([1]);
    assert!(
        cursors
            .pull_cores(&store, &both, Some(&only_alpha))
            .is_empty()
    );
    push(&mut store, 2, &[("03", "b2")]);

    assert_eq!(
        msgs(&cursors.pull_cores(&store, &both, None)),
        ["b2"],
        "the returning core must resume, not replay"
    );
}

/// A core read for the first time is capped at the same per-core budget the snapshot path uses.
#[test]
fn a_first_read_respects_the_per_core_budget() {
    let mut store = store_with(&[1]);
    let lines: Vec<(String, String)> = (0..crate::panels::log::AGG_PER_CORE + 500)
        .map(|i| (format!("{i:08}"), format!("row-{i}")))
        .collect();
    let refs: Vec<(&str, &str)> = lines
        .iter()
        .map(|(ts, msg)| (ts.as_str(), msg.as_str()))
        .collect();
    push(&mut store, 1, &refs);
    let sources = [source(1, "alpha")];

    let pulled = LiveCursors::default().pull_cores(&store, &sources, None);

    assert_eq!(pulled.len(), crate::panels::log::AGG_PER_CORE);
    assert_eq!(
        pulled.first().map(|l| l.msg.as_str()),
        Some("row-500"),
        "the budget keeps the NEWEST rows"
    );
}
