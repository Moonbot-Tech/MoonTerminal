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
    pub(super) coins: Vec<String>,
    pub(super) strategy: Option<u64>,
    temporary: bool,
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
            coins: vec![coin.to_string()],
            strategy,
            temporary: false,
            revert: Box::new(revert),
        })
    }

    /// Record one inverse for all the tokens added to a core list by a bulk click.
    pub(super) fn inserted_many(
        core: CoreId,
        coins: Vec<String>,
        revert: impl FnOnce(&SessionManager) -> anyhow::Result<bool> + 'static,
    ) -> Self {
        Self {
            core,
            coins,
            strategy: None,
            temporary: false,
            revert: Box::new(revert),
        }
    }

    /// Keep the UI ticker with the inverse, which already owns the destination symbol.
    pub(super) fn temp_sent(
        core: CoreId,
        ticker: &str,
        revert: Option<impl FnOnce(&SessionManager) -> anyhow::Result<bool> + 'static>,
    ) -> Option<Self> {
        Self::inserted(core, ticker, None, revert).map(|mut step| {
            step.temporary = true;
            step
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
        let notice = backend.update(app, |b, _| {
            let allowed = workspace_action_allows_cores(b, workspace_group.as_deref(), &cores);
            if !allowed {
                return record.take().map(|_| t!("coin_menu.undo.refused"));
            }
            let mut changed_ban = false;
            run_undo_once(&record, |step| changed_ban |= apply_revert(b, step));
            changed_ban.then(|| t!("coin_menu.undo.temp_changed"))
        });
        if let Some(notice) = notice {
            window.push_notification(MoonNotification::info(notice.to_string()), app);
        }
    };
    window.push_notification(
        MoonNotification::success(toast)
            .action(t!("coin_menu.undo.action").to_string(), on_undo)
            .autohide_after(Duration::from_secs(6)),
        app,
    );
}

/// Submit one inverse and report whether a newer temporary-ban state refused it.
fn apply_revert(b: &mut Backend, step: UndoStep) -> bool {
    if let Some(sid) = step.strategy
        && !b.session.strategy_has_blacklist(step.core, sid)
    {
        return false;
    }
    match (step.revert)(&b.session) {
        Ok(true) => {
            if let Some(sid) = step.strategy
                && let Some(coin) = step.coins.into_iter().next()
            {
                b.watch_strategy_edit(step.core, sid, coin);
            }
        }
        Ok(false) => return step.temporary,
        Err(err) => log::warn!(
            "coin_menu: undo blacklist on core {} failed: {err:#}",
            step.core
        ),
    }
    false
}

#[cfg(test)]
mod tests;
