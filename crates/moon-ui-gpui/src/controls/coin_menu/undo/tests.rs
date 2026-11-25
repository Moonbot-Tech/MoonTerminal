//! One toast per insertion click and click-time undo authorization.

use std::cell::Cell;

use moon_core::session::SessionManager;

use super::{UndoRecord, UndoStep, revert_plan, take_record, temp_restore_span};

/// Build a recorded successful insertion without a connection.
fn added(_: &SessionManager) -> anyhow::Result<bool> {
    Ok(true)
}

/// Filter already-present pairs and reverse the actual mutations on the same list.
#[test]
fn undo_records_only_inserted_pairs() {
    let steps = [
        UndoStep::inserted(1, "XALPHA", None, Some(added)),
        UndoStep::inserted(
            2,
            "XALPHA",
            None,
            None::<fn(&SessionManager) -> anyhow::Result<bool>>,
        ),
        UndoStep::inserted(1, "XBETA", None, Some(added)),
    ]
    .into_iter()
    .flatten()
    .collect();
    let record = UndoRecord::new(steps, |_| "Added two coins".to_string()).unwrap();
    let actual: Vec<_> = revert_plan(record.steps)
        .map(|step| (step.core, step.coin))
        .collect();
    assert_eq!(
        actual,
        [(1, "XBETA".to_string()), (1, "XALPHA".to_string())]
    );
}

/// A lift has no insertions and must not produce a success toast.
#[test]
fn lift_click_records_nothing() {
    assert!(UndoRecord::new(Vec::new(), |_| panic!("no toast for a lift")).is_none());
}

/// A second button click cannot consume the same undo handler.
#[test]
fn undo_is_one_shot() {
    let steps = UndoStep::inserted(1, "XALPHA", None, Some(added))
        .into_iter()
        .collect();
    let record = Cell::new(UndoRecord::new(steps, |_| "Added XALPHA".to_string()));
    assert!(take_record(&record).is_some());
    assert!(take_record(&record).is_none());
}

/// The deferred undo action must revalidate all the targets before applying any inverse.
#[test]
fn undo_revalidates_workspace_before_write() {
    let source = include_str!("../undo.rs")
        .split_once("pub(super) fn show_undo_toast(")
        .unwrap()
        .1;
    let update = source.find("backend.update(app").unwrap();
    let guard = source
        .find("workspace_action_allows_cores(b, workspace_group.as_deref(), &cores)")
        .unwrap();
    let effect = source.find("apply_revert(b, step)").unwrap();
    assert!(update < guard && guard < effect);
}

/// A newly added ban must be cleared when nothing was published before it.
#[test]
fn temp_undo_without_prior_clears() {
    assert_eq!(temp_restore_span(None, 10_000), None);
}

/// Undo preserves the old deadline, so the time between add and undo does not extend it.
#[test]
fn temp_undo_restores_prior_remaining() {
    assert_eq!(
        temp_restore_span(Some(45_000), 10_000),
        Some(std::time::Duration::from_secs(35))
    );
}

/// A prior ban that already expired must be cleared, including at the exact deadline.
#[test]
fn temp_undo_expired_prior_clears() {
    assert_eq!(temp_restore_span(Some(9_999), 10_000), None);
    assert_eq!(temp_restore_span(Some(10_000), 10_000), None);
}

/// An error sending the add leaves no undo step or success toast.
#[test]
fn temp_failed_send_records_nothing() {
    let steps = UndoStep::temp_sent(1, "XALPHA", Some(45_000), false)
        .into_iter()
        .collect();
    assert!(UndoRecord::new(steps, |_| panic!("failed sends have no toast")).is_none());
}
