//! Purge confirmation wording and safe empty-folder selection. Explicit imports: the parent
//! re-exports `gpui::*`, whose own `test` would shadow the built-in attribute.

use moon_core::feed::StrategyRow;

use super::{deletable_folder_after, purge_core_visible, purge_summary_lines};

/// Extract one method from the `PurgeRun` implementation up to the next documented method.
///
/// Args:
///     src: Full purge module source.
///     anchor: Unique method signature fragment.
///
/// Returns:
///     The method source including its signature and body.
fn purge_run_method<'a>(src: &'a str, anchor: &str) -> &'a str {
    src.split(anchor)
        .nth(1)
        .and_then(|tail| tail.split("\n    ///").next())
        .expect("PurgeRun method must exist")
}

/// Removing the guard before `Ready -> Running` would let a dialog opened on core A dispatch its
/// destructive sequence after Auto ownership switches to core B.
#[test]
fn workspace_switch_before_confirm_refuses_purge_without_dispatch() {
    assert!(purge_core_visible(None, 11));
    assert!(purge_core_visible(Some(&[11]), 11));
    assert!(!purge_core_visible(Some(&[22]), 11));

    let src = include_str!("../purge.rs");
    let confirm = src
        .split("fn confirm_purge(")
        .nth(1)
        .and_then(|tail| tail.split("\n    }").next())
        .expect("purge confirmation must exist");
    let guard = confirm
        .find("if !purge_core_visible(workspace, core_uid)")
        .expect("confirmation must revalidate its captured core");
    let running = confirm
        .find("op.state = PurgeState::Running(PurgeStep::Rows)")
        .expect("confirmation must still start the sequence");
    assert!(guard < running, "scope refusal must precede Running");
    assert!(confirm[guard..running].contains("PurgeState::CountFailed"));
    assert!(!confirm[..running].contains("cx.spawn(async move"));
}

/// `analytics/purge.rs:PurgeRun::send_empty_folder`: removing its `guard_current` call would let
/// Auto switch from core A to core B after confirmation, then queue folder deletion on hidden A.
#[test]
fn workspace_switch_after_confirm_blocks_every_later_operation() {
    assert!(!purge_core_visible(Some(&[22]), 11));

    let src = include_str!("../purge.rs");
    let guarded = [
        "fn open_step(",
        "async fn rec_ids(",
        "fn send(",
        "fn send_empty_folder(",
        "fn inspect_strategies<",
        "async fn await_rows_gone(",
        "async fn await_strategy(",
    ];
    for anchor in guarded {
        assert!(
            purge_run_method(src, anchor).contains("self.guard_current(cx)?;"),
            "{anchor} must revalidate the live workspace before it advances"
        );
    }

    let still_mine = purge_run_method(src, "fn still_mine(");
    assert!(still_mine.contains("purge_core_visible(workspace, op.core_uid)"));
    let guard = purge_run_method(src, "fn guard_current(");
    assert!(guard.contains("Err(PurgeStop::ScopeMoved)"));
    let outcome = src
        .rsplit_once("match outcome {")
        .map(|(_, outcome)| outcome)
        .expect("running purge outcome must be published");
    assert!(
        outcome.contains("Err(PurgeStop::ScopeMoved)")
            && outcome.contains("PurgeState::CountFailed")
            && outcome.contains("analytics.write_failed_stale"),
        "a workspace move must stop visibly instead of abandoning the running dialog"
    );
}

/// Build the only strategy fields the folder decision reads while keeping the fixture explicit.
fn strategy(id: u64, folder_path: &str) -> StrategyRow {
    StrategyRow {
        id,
        name: format!("strategy-{id}"),
        kind: "Demo".to_string(),
        kind_ordinal: 1,
        folder_path: folder_path.to_string(),
        checked: false,
        is_short: false,
        fields: Vec::new(),
    }
}

/// The caveat is the whole reason legacy rows are counted at all: they cannot be deleted, so a
/// confirmation that omits them promises a clean sweep the operation will not deliver.
#[test]
fn the_legacy_caveat_appears_whenever_there_are_legacy_rows() {
    let lines = purge_summary_lines(12, 4, 3);

    assert_eq!(lines.len(), 2, "count line plus the caveat: {lines:?}");
    assert!(
        lines[1].contains('3'),
        "the caveat must state how many rows remain: {lines:?}"
    );
}

/// Adding an unconditional warning would falsely claim that addressable rows must survive.
#[test]
fn no_legacy_rows_means_no_caveat() {
    let lines = purge_summary_lines(12, 4, 0);

    assert_eq!(lines.len(), 1, "nothing to warn about: {lines:?}");
}

/// Printing "0 trades" alongside a period count reads as a broken query rather than as a strategy
/// whose trades are already gone.
#[test]
fn zero_trades_is_stated_not_counted() {
    let zero = purge_summary_lines(0, 0, 0);
    let some = purge_summary_lines(5, 2, 0);

    assert_ne!(
        zero[0], some[0],
        "an empty purge gets its own sentence, not the count sentence with a zero"
    );
}

/// Both numbers are shown because the all-time total normally EXCEEDS the row's period count; a
/// summary carrying only one of them would read as the wrong number rather than as a wider scope.
#[test]
fn the_count_line_states_both_scopes() {
    let lines = purge_summary_lines(120, 37, 0);

    assert!(
        lines[0].contains("120") && lines[0].contains("37"),
        "{lines:?}"
    );
}

/// `analytics/purge.rs:deletable_folder_after`: accepting no captured path or a segmentless root
/// would fabricate a folder command and could target the core root after strategy deletion.
#[test]
fn folder_cleanup_requires_a_captured_non_root_folder() {
    assert_eq!(deletable_folder_after(None, &[]), None);
    assert_eq!(deletable_folder_after(Some(""), &[]), None);
    assert_eq!(deletable_folder_after(Some("/"), &[]), None);
}

/// `analytics/purge.rs:deletable_folder_after`: returning the raw path would ask the core to delete
/// `/a//b/` and leave the actual canonical `a/b` folder behind.
#[test]
fn an_empty_folder_uses_the_canonical_protocol_path() {
    let remaining = vec![strategy(2, "elsewhere")];

    assert_eq!(
        deletable_folder_after(Some("/a//b/"), &remaining),
        Some("a/b".to_string())
    );
}

/// `analytics/purge.rs:deletable_folder_after`: replacing the segment-aware subtree check with raw
/// equality/prefix logic would delete an occupied folder or let unrelated `alpha2` block `alpha`.
#[test]
fn direct_and_descendant_strategies_block_only_their_real_folder() {
    assert_eq!(
        deletable_folder_after(Some("alpha"), &[strategy(2, "alpha")]),
        None
    );
    assert_eq!(
        deletable_folder_after(Some("alpha"), &[strategy(2, "alpha/child")]),
        None
    );
    assert_eq!(
        deletable_folder_after(Some("alpha"), &[strategy(2, "alpha2")]),
        Some("alpha".to_string())
    );
}
