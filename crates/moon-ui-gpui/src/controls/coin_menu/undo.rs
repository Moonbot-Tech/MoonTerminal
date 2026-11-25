//! One-shot reversal of the blacklist additions made by a single menu click.

use std::cell::Cell;
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

    /// Record a successful temporary ban against the deadline published before the send.
    /// Queued changes that the core has yet to echo are absent from this snapshot.
    pub(super) fn temp_sent(
        core: CoreId,
        symbol: &str,
        prior: Option<i64>,
        sent: bool,
    ) -> Option<Self> {
        Self::inserted(
            core,
            symbol,
            None,
            sent.then(|| {
                let symbol = symbol.to_string();
                move |session: &SessionManager| {
                    let remaining = temp_restore_span(prior, moon_core::util::now_unix_ms_i64());
                    session.set_temp_ban(core, symbol, remaining).map(|()| true)
                }
            }),
        )
    }
}

/// Restore only time remaining at undo; a missing or elapsed deadline clears the ban.
fn temp_restore_span(prior: Option<i64>, now: i64) -> Option<Duration> {
    let remaining = prior?.saturating_sub(now);
    (remaining > 0).then(|| Duration::from_millis(remaining as u64))
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

/// Whether the current workspace authorizes reversing the entire record.
#[derive(Debug, PartialEq, Eq)]
enum UndoDecision {
    Refused,
    Revert,
}

/// Refuse the whole undo when any recorded core has left the workspace.
fn undo_decision(allowed: bool) -> UndoDecision {
    if allowed {
        UndoDecision::Revert
    } else {
        UndoDecision::Refused
    }
}

/// Consume the record once and replay its inverses from newest to oldest.
fn run_undo_once(cell: &Cell<Option<UndoRecord>>, mut revert: impl FnMut(UndoStep)) -> bool {
    let Some(record) = cell.take() else {
        return false;
    };
    for step in record.steps.into_iter().rev() {
        revert(step);
    }
    true
}

/// Show the undo action and report a workspace refusal in the originating window.
pub(super) fn show_undo_toast(
    record: UndoRecord,
    backend: Entity<Backend>,
    workspace_group: Option<String>,
    window: &mut Window,
    app: &mut App,
) {
    let toast = record.toast.clone();
    let cores: Vec<_> = record.steps.iter().map(|step| step.core).collect();
    let record = Cell::new(Some(record));
    let on_undo = move |window: &mut Window, app: &mut App| {
        let refused = backend.update(app, |b, _| {
            let allowed = workspace_action_allows_cores(b, workspace_group.as_deref(), &cores);
            match undo_decision(allowed) {
                UndoDecision::Refused => record.take().is_some(),
                UndoDecision::Revert => {
                    run_undo_once(&record, |step| apply_revert(b, step));
                    false
                }
            }
        });
        if refused {
            window.push_notification(
                MoonNotification::info(t!("coin_menu.undo.refused").to_string()),
                app,
            );
        }
    };
    window.push_notification(
        MoonNotification::success(toast)
            .action(t!("coin_menu.undo.action").to_string(), on_undo)
            .autohide_after(Duration::from_secs(6)),
        app,
    );
}

/// Submit one inverse and keep the same strategy-edit watch as the original action.
fn apply_revert(b: &mut Backend, step: UndoStep) {
    if let Some(sid) = step.strategy
        && !b.session.strategy_has_blacklist(step.core, sid)
    {
        return;
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
