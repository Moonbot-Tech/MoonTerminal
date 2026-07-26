// Explicit imports on purpose: `use super::*` would pull in the parent's `gpui::*`
// re-export, whose `test` shadows the built-in attribute and makes `#[test]` expand
// recursively ("recursion limit reached").
use super::{aggregate, classify_lower, LineView, RefreshGate};
use crate::panels::log::{LogSource, LogSourceItem, AGG_PER_CORE, VIEW_LIMIT};
use moon_core::applog::LogLine;
use moon_core::session::CoreStore;
use std::collections::HashSet;

/// Builds a core selector entry for aggregate tests without involving UI state.
fn source(id: u64, display: &str) -> LogSourceItem {
    LogSourceItem {
        source: LogSource::Core(id),
        display: display.to_string(),
        file_label: display.to_string(),
    }
}

/// Builds one sortable core log row with a deterministic payload.
fn row(ts: &str, msg: String) -> LogLine {
    LogLine {
        ts: ts.to_string(),
        level: log::Level::Info,
        target: String::new(),
        msg,
    }
}

/// A multi-line message is flattened before its coin range is computed.
#[test]
fn multiline_message_flattens_and_keeps_the_coin_range_valid() {
    let line = LogLine::core(0, "SPK order failed\r\n  at retry 2".to_string());
    let known = HashSet::from(["SPK".to_string()]);
    let lower = line.msg.to_lowercase();
    let view = LineView::from_parts(&line, classify_lower(line.level, &lower), &known);

    assert!(
        !view.flat.contains('\n') && !view.flat.contains('\r'),
        "raw break survived into the log row: {:?}",
        view.flat
    );
    let (range, base) = view
        .coin
        .expect("SPK is a known base and should be detected");
    assert_eq!(&view.flat[range], "SPK");
    assert_eq!(base, "SPK");
}

/// `render.rs:RefreshGate::observe` removing the active guard would rebuild a hidden Log tab on
/// every backend revision instead of catching up once when the user opens it.
#[test]
fn refresh_gate_skips_hidden_revisions_and_catches_up_on_activation() {
    let mut gate = RefreshGate::default();

    assert!(!gate.observe(1), "a never-opened dock tab must stay idle");
    assert!(
        gate.set_active(true, 1),
        "the first activation must load the current tail"
    );
    assert!(
        !gate.observe(1),
        "an unchanged active source must not reload"
    );
    assert!(
        gate.observe(2),
        "an active source must schedule new rows for its next render"
    );
    assert!(
        !gate.observe(2),
        "the same pending revision must not schedule duplicate work"
    );
    assert!(
        gate.take_observed_reload(),
        "the next actual render must consume the scheduled reload"
    );
    gate.record_reload(2);
    assert!(!gate.set_active(false, 2));
    assert!(!gate.observe(3), "a hidden tab must ignore new revisions");
    assert!(
        gate.set_active(true, 3),
        "reactivation must catch up to revisions missed while hidden"
    );
}

/// `render.rs:RefreshGate::request_reload` becoming a no-op would leave a hidden panel at its
/// scrolled position after follow-tail resumes until another log revision happens to arrive.
#[test]
fn requested_reload_survives_deactivation_until_the_next_render() {
    let mut gate = RefreshGate::default();
    assert!(gate.set_active(true, 7));
    assert!(!gate.set_active(false, 7));

    gate.request_reload();

    assert!(
        gate.take_observed_reload(),
        "a hidden follow-tail resume must survive until the panel renders again"
    );
}

/// `render.rs:aggregate` changing the per-core tail iterator from newest-first to oldest-first
/// would show stale rows instead of the newest `VIEW_LIMIT` entries across all cores.
#[test]
fn aggregate_keeps_only_the_globally_newest_rows() {
    let sources = [source(1, "alpha"), source(2, "beta"), source(3, "gamma")];
    let mut store = CoreStore::default();
    for id in 1..=3 {
        store.ensure(id);
    }
    for local_ix in 0..AGG_PER_CORE {
        for (source_ix, id) in (1..=3).enumerate() {
            let global_ix = local_ix * 3 + source_ix;
            store
                .core_mut(id)
                .expect("test core must exist")
                .log
                .push_back(row(&format!("{global_ix:08}"), format!("row-{global_ix}")));
        }
    }

    let lines = aggregate(&store, &sources);

    assert_eq!(lines.len(), VIEW_LIMIT);
    let first_global = AGG_PER_CORE * sources.len() - VIEW_LIMIT;
    for (offset, line) in lines.iter().enumerate() {
        let global_ix = first_global + offset;
        assert_eq!(line.ts, format!("{global_ix:08}"));
        assert_eq!(line.msg, format!("row-{global_ix}"));
        assert_eq!(line.target, sources[global_ix % sources.len()].display);
    }
}

/// `render.rs:aggregate` reversing the heap's source-ordinal tie-break would change the stable
/// equal-timestamp cutoff and relabel which core's oldest rows remain visible.
#[test]
fn aggregate_preserves_source_and_row_order_for_equal_timestamps() {
    let sources = [source(1, "alpha"), source(2, "beta"), source(3, "gamma")];
    let mut store = CoreStore::default();
    for id in 1..=3 {
        store.ensure(id);
        let core = store.core_mut(id).expect("test core must exist");
        for row_ix in 0..AGG_PER_CORE {
            core.log
                .push_back(row("same-time", format!("{id}-{row_ix:04}")));
        }
    }

    let lines = aggregate(&store, &sources);

    assert_eq!(lines.len(), VIEW_LIMIT);
    let dropped_from_first = AGG_PER_CORE * sources.len() - VIEW_LIMIT;
    for (offset, line) in lines[..AGG_PER_CORE - dropped_from_first]
        .iter()
        .enumerate()
    {
        let row_ix = dropped_from_first + offset;
        assert_eq!(line.msg, format!("1-{row_ix:04}"));
        assert_eq!(line.target, "alpha");
    }
    for (source_ix, chunk) in lines[AGG_PER_CORE - dropped_from_first..]
        .chunks_exact(AGG_PER_CORE)
        .enumerate()
    {
        let id = source_ix + 2;
        for (row_ix, line) in chunk.iter().enumerate() {
            assert_eq!(line.msg, format!("{id}-{row_ix:04}"));
            assert_eq!(line.target, sources[id - 1].display);
        }
    }
}
