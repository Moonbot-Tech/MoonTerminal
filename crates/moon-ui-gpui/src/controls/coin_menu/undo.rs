//! One-shot reversal of the blacklist additions made by a single menu click.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{App, Entity, Window};
use moon_core::session::{CoreId, SessionManager};
use moon_ui::{MoonNotification, MoonWindowExt as _};
use rust_i18n::t;

use super::workspace_action_allows_cores;
use crate::Backend;

/// An echo-aware inverse returned by the session for one inserted token.
type Revert = Box<dyn FnOnce(&SessionManager) -> anyhow::Result<bool>>;

/// A successfully submitted insertion with its original target and watch information.
pub(super) struct UndoStep {
    pub(super) core: CoreId,
    pub(super) coin: String,
    pub(super) strategy: Option<u64>,
    revert: Revert,
}

impl UndoStep {
    /// Omit no-ops, including a flag-only enable on a list that already had the token.
    pub(super) fn inserted(
        core: CoreId,
        coin: &str,
        strategy: Option<u64>,
        revert: Option<impl FnOnce(&SessionManager) -> anyhow::Result<bool> + 'static>,
    ) -> Option<Self> {
        revert.map(|revert| Self {
            core,
            coin: coin.to_string(),
            strategy,
            revert: Box::new(revert),
        })
    }
}

/// Undo steps and the localized success caption for one click.
pub(super) struct UndoRecord {
    steps: Vec<UndoStep>,
    toast: String,
}

impl UndoRecord {
    /// A lift or a click that inserted nothing has no toast.
    pub(super) fn new(
        steps: Vec<UndoStep>,
        toast: impl FnOnce(&[UndoStep]) -> String,
    ) -> Option<Self> {
        (!steps.is_empty()).then(|| Self {
            toast: toast(&steps),
            steps,
        })
    }
}

/// Walk writes in reverse so consecutive insertions into one list restore the original bytes.
fn revert_plan(steps: Vec<UndoStep>) -> impl Iterator<Item = UndoStep> {
    steps.into_iter().rev()
}

/// Take the action once, even if the button receives another click while dismissing.
fn take_record(record: &Cell<Option<UndoRecord>>) -> Option<UndoRecord> {
    record.take()
}

/// Show one actionable toast in the window where the click originated.
pub(super) fn show_undo_toast(
    record: UndoRecord,
    backend: Entity<Backend>,
    workspace_group: Option<String>,
    window: &mut Window,
    app: &mut App,
) {
    let toast = record.toast.clone();
    let record = Rc::new(Cell::new(Some(record)));
    window.push_notification(
        MoonNotification::success(toast)
            .action(t!("coin_menu.undo.action").to_string(), move |_, app| {
                let Some(record) = take_record(&record) else {
                    return;
                };
                backend.update(app, |b, _| {
                    let cores: Vec<_> = record.steps.iter().map(|step| step.core).collect();
                    if !workspace_action_allows_cores(b, workspace_group.as_deref(), &cores) {
                        return;
                    }
                    for step in revert_plan(record.steps) {
                        apply_revert(b, step);
                    }
                });
            })
            .autohide_after(Duration::from_secs(6)),
        app,
    );
}

/// Submit one inverse and keep the same strategy-edit watch as the original action.
fn apply_revert(b: &mut Backend, step: UndoStep) {
    if let Some(sid) = step.strategy {
        if !b.session.strategy_has_blacklist(step.core, sid) {
            return;
        }
    }
    match (step.revert)(&b.session) {
        Ok(true) => {
            if let Some(sid) = step.strategy {
                b.watch_strategy_edit(step.core, sid, step.coin);
            }
        }
        Ok(false) => {}
        Err(err) => log::warn!(
            "coin_menu: undo blacklist on core {} failed: {err:#}",
            step.core
        ),
    }
}

#[cfg(test)]
mod tests;
