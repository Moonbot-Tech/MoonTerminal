//! The hang-watchdog block of the Special page ("Защита от зависаний"), on the core's observer API
//! (#1173): the window's side of it — the edit, the asks, the actions, the controls.
//!
//! The block is NOT part of the page OK sends. Its values belong to `client.watchdogs()`, which is
//! the one writer for them — the safe-share projection no longer carries `sign_orders`, the two
//! position-control flags or the exclusion list — so it keeps an edit of its own and its own Apply,
//! as Moonbot's dialog does for this group.
//!
//! Several selected cores. The observer table is a set of API keys of ONE core, and a row id means
//! nothing to another core, so the block always addresses the ANCHOR alone — the core whose page is
//! drawn — and says so above itself while more than one core is selected. Copying observers onto a
//! fleet would mean copying exchange keys between cores, which is a decision this window does not
//! make on the trader's behalf.
//!
//! Freshness. The edit is built from a snapshot of the CURRENT core session and dies with it: when
//! the store's `watchdog_epoch` moves (link lost, process replaced) the edit is dropped, because the
//! row ids it carries belong to the session that ended. Nothing is replayed after a reconnect — the
//! feed drops an Apply that waited out an outage, and the window asks for a fresh snapshot instead.

use std::collections::HashMap;

use gpui::*;
use moon_ui::{MoonDataTableState, MoonInputEvent, MoonInputState};

use moon_core::feed::{
    ConnStatus, CoreWatchdogState, ServiceAction, ServiceVerdict, WatchdogCmd, WatchdogEditError,
    WatchdogEntryEdit, WatchdogSettingsEdit,
};
use moon_core::session::CoreId;

use crate::Backend;
use crate::shell::parse_num;

use super::pages::{SpecialSection, watchdog_ids as ids};
use super::service::{Awaiting, verdict_applied};
use super::{CoreExpertView, ExpertTab};

/// The window's state for the observer block of its anchor core.
#[derive(Default)]
pub(crate) struct WatchdogBlock {
    /// Core this state belongs to; everything below is dropped when the anchor moves.
    core: Option<CoreId>,
    /// The store's `watchdog_epoch` the edit was built in.
    epoch: u64,
    /// The trader's edit, built from the snapshot on the first change. `None` draws the snapshot.
    pub(super) draft: Option<WatchdogSettingsEdit>,
    /// Picked row, as an index into the rows drawn (the edit's, or the snapshot's).
    pub(super) selected: Option<usize>,
    /// What the last Apply was refused for locally, named beside the controls.
    pub(super) invalid: Option<WatchdogEditError>,
    /// A request this window queued and has not seen answered.
    pub(super) awaiting: Option<Awaiting>,
    /// Whether this window sent anything to this core since the anchor last moved: only then is
    /// the core's verdict on the track worth a banner — an older one describes someone else's
    /// request, or one from an earlier open of this window.
    pub(super) sent_here: bool,
    /// Why the last action could not even be queued (the core's channel is gone).
    pub(super) queue_failed: bool,
    /// The core and epoch a refresh was last asked for; `None` asks on the next sync.
    asked: Option<(CoreId, u64)>,
    /// The store's `watchdog_rev` last drawn — the block's repaint gate.
    seen_rev: u64,
    /// Last synchronized snapshot, so a picked index can be resolved before a push replaces it.
    snapshot: Option<std::sync::Arc<CoreWatchdogState>>,
    /// Advances whenever the values the editors show change under them (a dropped edit, another
    /// row, a new snapshot with no edit), so each editor re-reads its value exactly once.
    generation: u64,
    /// Editors by id, each with the generation it last took a value from.
    inputs: HashMap<&'static str, (u64, Entity<MoonInputState>)>,
    /// Retained state of the observer table.
    pub(super) table: Option<Entity<MoonDataTableState>>,
}

impl WatchdogBlock {
    /// Take a pushed snapshot, keeping the pick on its observer and reseeding its editors.
    /// A draft owns its row order until it is applied or discarded.
    fn take_snapshot(&mut self, snapshot: Option<std::sync::Arc<CoreWatchdogState>>) {
        if self.draft.is_none() {
            let picked = self
                .snapshot
                .as_ref()
                .and_then(|s| s.entries.get(self.selected?))
                .map(|e| e.id.as_str());
            self.selected = picked_row_index(snapshot.as_deref(), picked);
            self.reseed();
        }
        self.snapshot = snapshot;
    }

    /// The editor built for `id`, once the block has rendered.
    pub(super) fn input(&self, id: &str) -> Option<&Entity<MoonInputState>> {
        self.inputs.get(id).map(|(_, state)| state)
    }

    /// Whether any editor exists, i.e. whether one of them may hold focus.
    pub(super) fn has_inputs(&self) -> bool {
        !self.inputs.is_empty()
    }

    /// The edit the block draws: the trader's, or the snapshot's own.
    pub(super) fn edit_view(
        &self,
        snapshot: Option<&CoreWatchdogState>,
    ) -> Option<WatchdogSettingsEdit> {
        self.draft
            .clone()
            .or_else(|| snapshot.map(CoreWatchdogState::edit))
    }

    /// Drop the edit and make the editors re-read the snapshot.
    fn drop_draft(&mut self) {
        self.draft = None;
        self.invalid = None;
        self.reseed();
    }

    /// The core confirmed deleting row `id`: take it out of the edit, or the next Apply would upsert
    /// it back under an id the core no longer knows. An edit left equal to the snapshot holds no
    /// change any more and is dropped.
    pub(super) fn forget_removed(&mut self, id: &str, snapshot: Option<&CoreWatchdogState>) {
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        draft.entries.retain(|e| e.id != id);
        if snapshot.is_some_and(|s| *draft == s.edit()) {
            self.drop_draft();
        }
        // The rows below the removed one moved up: a pick by index would now name another row,
        // and its editors would stage into it while still showing the old one.
        self.selected = None;
        self.reseed();
    }

    /// Make every editor take its value again on the next build.
    fn reseed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// Forget everything about the previous anchor: its edit, its asks, its banner.
    fn rebind(&mut self, core: Option<CoreId>) {
        let table = self.table.take();
        *self = Self {
            core,
            generation: self.generation.wrapping_add(1),
            inputs: std::mem::take(&mut self.inputs),
            table,
            ..Self::default()
        };
    }
}

/// The text one editor shows for the edit drawn.
///
/// Keys and secrets are never shown: the core does not return them, and the box holds only what the
/// trader types to REPLACE one.
fn input_value(id: &str, edit: Option<&WatchdogSettingsEdit>, row: Option<usize>) -> String {
    let Some(edit) = edit else {
        return String::new();
    };
    let entry = row.and_then(|ix| edit.entries.get(ix));
    match id {
        ids::IP => edit.server_ip.clone(),
        ids::BLACKLIST => edit.blacklist.clone(),
        ids::NAME => entry.map(|e| e.name.clone()).unwrap_or_default(),
        ids::KEY => entry.and_then(|e| e.api_key.clone()).unwrap_or_default(),
        ids::SECRET => entry.and_then(|e| e.api_secret.clone()).unwrap_or_default(),
        ids::PRICE => entry.map(|e| format!("{}", e.price)).unwrap_or_default(),
        ids::TIME => entry.map(|e| e.time.to_string()).unwrap_or_default(),
        _ => String::new(),
    }
}

/// Stage one editor's text into the edit.
///
/// A box that does not parse stages a value [`WatchdogSettingsEdit::validate`] refuses — a NaN
/// price, a negative age — so Apply names the row instead of silently sending the last good number.
/// An emptied key or secret box means "keep the saved one", never "send an empty credential".
pub(super) fn stage_input(
    edit: &mut WatchdogSettingsEdit,
    row: Option<usize>,
    id: &str,
    text: &str,
) {
    let one_line = || text.replace(['\r', '\n'], "");
    match id {
        ids::IP => edit.server_ip = one_line(),
        ids::BLACKLIST => edit.blacklist = one_line(),
        _ => {
            let Some(entry) = row.and_then(|ix| edit.entries.get_mut(ix)) else {
                return;
            };
            match id {
                ids::NAME => entry.name = one_line(),
                ids::KEY => entry.api_key = credential(text),
                ids::SECRET => entry.api_secret = credential(text),
                ids::PRICE => entry.price = parse_num(text).unwrap_or(f64::NAN),
                ids::TIME => {
                    entry.time = parse_num(text)
                        .filter(|v| v.fract() == 0.0 && *v >= 0.0 && *v <= f64::from(i32::MAX))
                        .map_or(-1, |v| v as i32);
                }
                _ => {}
            }
        }
    }
}

/// A typed credential, or `None` for an empty box — which keeps the saved one on the core.
fn credential(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The row a new observer starts as: nothing watched until the trader ticks it, so an Apply with a
/// half-filled row arms nothing by surprise.
pub(super) fn new_row() -> WatchdogEntryEdit {
    WatchdogEntryEdit::default()
}

/// Whether the drawn row at `row` may be removed with a request, rather than only from the edit.
fn saved_row_id(edit: Option<&WatchdogSettingsEdit>, row: Option<usize>) -> Option<String> {
    let entry = edit?.entries.get(row?)?;
    (!entry.id.is_empty()).then(|| entry.id.clone())
}

/// Resolve a saved observer's identity in a replacement snapshot, clearing a vanished pick.
fn picked_row_index(snapshot: Option<&CoreWatchdogState>, picked: Option<&str>) -> Option<usize> {
    let id = picked?;
    snapshot?.entries.iter().position(|e| e.id == id)
}

/// Whether the edit's verdict means the core now holds it, so the edit is rebuilt from the state the
/// core returned — "applied but not saved" included: moonproto says to rebuild from that state
/// before retrying, so new rows keep the ids the core gave them.
pub(super) fn verdict_takes_edit(action: ServiceAction, verdict: &ServiceVerdict) -> bool {
    action == ServiceAction::WatchdogApply && verdict_applied(verdict)
}

impl CoreExpertView {
    /// Keep the block in step with the store: rebind on a new anchor, drop an edit whose session
    /// ended, settle what this window waited on, and ask for a snapshot when one is due.
    ///
    /// Returns:
    ///     Whether anything the block draws changed.
    pub(super) fn sync_watchdog(&mut self, b: &Backend) -> bool {
        let anchor = self.anchor;
        let mut changed = false;
        if self.watchdog.core != anchor {
            self.watchdog.rebind(anchor);
            changed = true;
        }
        let Some(core) = anchor else {
            return changed;
        };
        let Some(entry) = b.session.store().core(core) else {
            return changed;
        };
        if entry.watchdog_rev != self.watchdog.seen_rev {
            self.watchdog.seen_rev = entry.watchdog_rev;
            changed = true;
            self.watchdog.take_snapshot(entry.watchdog.clone());
        }
        if entry.watchdog_epoch != self.watchdog.epoch {
            self.watchdog.epoch = entry.watchdog_epoch;
            if self.watchdog.draft.is_some() {
                log::info!(
                    "expert window dropped an unsent observer edit for core {core}: its session ended"
                );
                self.watchdog.drop_draft();
            }
            self.watchdog.selected = None;
            changed = true;
        }
        // A link that is down cannot answer what was asked of it: ask again once it is back, even
        // when no snapshot ever arrived — the epoch moves only when one existed, so it cannot be the
        // trigger, and a refresh lost with the link would otherwise never be repeated.
        if entry.status != ConnStatus::Ready {
            self.watchdog.asked = None;
        }
        if let Some(awaiting) = self.watchdog.awaiting.clone()
            && entry.service.watchdog.settled_since(awaiting.sent_before)
        {
            self.watchdog.awaiting = None;
            if let Some((ServiceAction::WatchdogRemove, verdict)) = &entry.service.watchdog.outcome
                && verdict_applied(verdict)
                && let Some(id) = awaiting.removed.as_deref()
            {
                self.watchdog.forget_removed(id, entry.watchdog.as_deref());
            }
            if let Some((action, verdict)) = &entry.service.watchdog.outcome
                && verdict_takes_edit(*action, verdict)
            {
                // Keep the pick on the same row by id: a new row now has one.
                let picked = saved_row_id(self.watchdog.draft.as_ref(), self.watchdog.selected);
                self.watchdog.drop_draft();
                self.watchdog.selected =
                    picked_row_index(entry.watchdog.as_deref(), picked.as_deref());
            }
            changed = true;
        }
        self.ask_watchdog(b, core);
        changed
    }

    /// Ask the anchor for its observer snapshot when the block is on screen.
    ///
    /// Once per open of the block, per anchor and per stretch of a Ready link (the sync forgets the
    /// ask whenever the link is down), and once more per epoch while the store holds no snapshot
    /// at all. A core too old to answer is asked once per connection, not every frame.
    fn ask_watchdog(&mut self, b: &Backend, core: CoreId) {
        let open = self.tab == ExpertTab::Special
            && self.special_section == SpecialSection::Watchdog
            && self.seeded() == Some(core);
        if !open {
            return;
        }
        let Some(entry) = b.session.store().core(core) else {
            return;
        };
        if entry.status != ConnStatus::Ready {
            return;
        }
        let key = (core, entry.watchdog_epoch);
        let due = match self.watchdog.asked {
            None => true,
            Some(asked) => asked != key && entry.watchdog.is_none(),
        };
        if !due {
            return;
        }
        self.watchdog.asked = Some(key);
        if let Err(error) = b.session.watchdog_cmd(core, WatchdogCmd::Refresh) {
            log::warn!("expert window could not ask core {core} for its observers: {error}");
        }
    }

    /// The block was opened (its section, or its tab): ask afresh, whatever was asked before.
    pub(super) fn watchdog_opened(&mut self, cx: &App) {
        self.watchdog.asked = None;
        let b = self.backend.read(cx);
        if let Some(core) = self.anchor {
            self.ask_watchdog(b, core);
        }
    }

    /// The anchor's snapshot and whether it may be edited right now.
    fn watchdog_live(&self, cx: &App) -> Option<(CoreId, std::sync::Arc<CoreWatchdogState>)> {
        let core = self.seeded()?;
        let entry = self.backend.read(cx).session.store().core(core)?;
        let live = entry.status == ConnStatus::Ready && entry.watchdog_fresh;
        live.then(|| entry.watchdog.clone())
            .flatten()
            .map(|s| (core, s))
    }

    /// Change the edit, building it from the live snapshot on the first change.
    ///
    /// Refused while the snapshot is stale or absent: an edit built from it would carry row ids
    /// the core may no longer know. Refused too while a request is in flight: an applied verdict
    /// rebuilds the edit from the core's state, which would silently drop whatever was typed since.
    pub(super) fn watchdog_edit(
        &mut self,
        apply: impl FnOnce(&mut WatchdogSettingsEdit, Option<usize>),
        cx: &mut Context<Self>,
    ) {
        if self.watchdog.awaiting.is_some() {
            return;
        }
        let Some((_, snapshot)) = self.watchdog_live(cx) else {
            return;
        };
        let selected = self.watchdog.selected;
        let draft = self.watchdog.draft.get_or_insert_with(|| snapshot.edit());
        apply(draft, selected);
        self.watchdog.invalid = None;
        cx.notify();
    }

    /// Pick a row of the table, or clear the pick.
    pub(super) fn watchdog_select(&mut self, row: Option<usize>, cx: &mut Context<Self>) {
        if self.watchdog.selected == row {
            return;
        }
        self.watchdog.selected = row;
        self.watchdog.reseed();
        // The row editors move to another row; one of them may hold focus.
        self.needs_blur |= self.watchdog.has_inputs();
        cx.notify();
    }

    /// Append a new observer row to the edit and pick it.
    pub(super) fn watchdog_add(&mut self, cx: &mut Context<Self>) {
        let mut added = None;
        self.watchdog_edit(
            |draft, _| {
                draft.entries.push(new_row());
                added = Some(draft.entries.len() - 1);
            },
            cx,
        );
        if added.is_some() {
            self.watchdog_select(added, cx);
        }
    }

    /// Remove the picked row: from the edit alone when the core never saw it, otherwise by asking
    /// the core to delete it — the API's only way to delete, an Apply never removes a row.
    pub(super) fn watchdog_remove(&mut self, cx: &mut Context<Self>) {
        let Some((core, snapshot)) = self.watchdog_live(cx) else {
            return;
        };
        let Some(row) = self.watchdog.selected else {
            return;
        };
        let view = self.watchdog.edit_view(Some(&snapshot));
        match saved_row_id(view.as_ref(), Some(row)) {
            // A saved row leaves the edit when the core confirms — see `Awaiting::removed`.
            Some(id) => {
                if !self.send_watchdog(core, WatchdogCmd::Remove(id), cx) {
                    return;
                }
            }
            // A row the core never saw is the edit's alone.
            None => {
                if let Some(draft) = self.watchdog.draft.as_mut()
                    && row < draft.entries.len()
                {
                    draft.entries.remove(row);
                }
                if self
                    .watchdog
                    .draft
                    .as_ref()
                    .is_some_and(|d| *d == snapshot.edit())
                {
                    // Removing the only change leaves nothing to apply.
                    self.watchdog.drop_draft();
                }
            }
        }
        self.watchdog_select(None, cx);
        cx.notify();
    }

    /// Ask the controller to cancel the tracked buys of the observed accounts.
    pub(super) fn watchdog_cancel_buys(&mut self, cx: &mut Context<Self>) {
        let Some((core, _)) = self.watchdog_live(cx) else {
            return;
        };
        self.send_watchdog(core, WatchdogCmd::CancelBuys, cx);
        cx.notify();
    }

    /// Send the edit, after checking what the core would refuse.
    pub(super) fn watchdog_apply(&mut self, cx: &mut Context<Self>) {
        let Some((core, _)) = self.watchdog_live(cx) else {
            return;
        };
        let Some(draft) = self.watchdog.draft.clone() else {
            return;
        };
        if let Err(error) = draft.validate() {
            if let Some(index) = invalid_row(&error) {
                self.watchdog_select(Some(index), cx);
            }
            self.watchdog.invalid = Some(error);
            cx.notify();
            return;
        }
        self.send_watchdog(core, WatchdogCmd::Apply(draft), cx);
        cx.notify();
    }

    /// Drop the edit and show the core's snapshot again.
    pub(super) fn watchdog_discard(&mut self, cx: &mut Context<Self>) {
        if self.watchdog.draft.is_none() {
            return;
        }
        self.watchdog.drop_draft();
        self.watchdog.selected = None;
        self.needs_blur |= self.watchdog.has_inputs();
        cx.notify();
    }

    /// Queue one tracked action and start waiting on its verdict.
    ///
    /// Returns:
    ///     Whether the core's channel took it.
    fn send_watchdog(&mut self, core: CoreId, cmd: WatchdogCmd, cx: &App) -> bool {
        let Some(action) = cmd.tracked_action() else {
            return false;
        };
        let removed = match &cmd {
            WatchdogCmd::Remove(id) => Some(id.clone()),
            _ => None,
        };
        let b = self.backend.read(cx);
        let sent_before = b
            .session
            .store()
            .core(core)
            .map_or(0, |d| d.service.watchdog.sent);
        self.watchdog.sent_here = true;
        match b.session.watchdog_cmd(core, cmd) {
            Ok(()) => {
                self.watchdog.queue_failed = false;
                self.watchdog.awaiting = Some(Awaiting {
                    action,
                    sent_before,
                    removed,
                });
                true
            }
            Err(error) => {
                log::warn!(
                    "expert window could not queue an observer action for core {core}: {error}"
                );
                self.watchdog.queue_failed = true;
                false
            }
        }
    }

    /// Create or synchronize the block's editors. Called at the top of the render while the block
    /// is on screen, the way [`Self::build_editors`] builds the page's.
    pub(super) fn build_watchdog_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.tab == ExpertTab::Special
            && self.special_section == SpecialSection::Watchdog
            && self.draft.is_some();
        if !open {
            return;
        }
        if self.watchdog.table.is_none() {
            self.watchdog.table = Some(cx.new(|_| MoonDataTableState::new()));
        }
        let snapshot = self
            .anchor
            .and_then(|core| self.backend.read(cx).session.store().core(core))
            .and_then(|d| d.watchdog.clone());
        let edit = self.watchdog.edit_view(snapshot.as_deref());
        let row = self.watchdog.selected;
        let generation = self.watchdog.generation;
        for id in ids::ALL {
            let value = input_value(id, edit.as_ref(), row);
            if let Some((seen, state)) = self.watchdog.inputs.get_mut(id) {
                if *seen != generation {
                    *seen = generation;
                    if state.read(cx).value() != value {
                        state.update(cx, |s, c| s.sync_value(value, c));
                    }
                }
                continue;
            }
            let masked = matches!(id, ids::KEY | ids::SECRET);
            let state = cx.new(|c| {
                MoonInputState::new(window, c)
                    .masked(masked)
                    .default_value(value)
            });
            cx.subscribe(
                &state,
                move |this: &mut Self, state, ev: &MoonInputEvent, cx| {
                    if matches!(ev, MoonInputEvent::Change) {
                        let text = state.read(cx).value().to_string();
                        this.watchdog_edit(|draft, row| stage_input(draft, row, id, &text), cx);
                    }
                },
            )
            .detach();
            self.watchdog.inputs.insert(id, (generation, state));
        }
    }

    /// Drop the block's editors, e.g. when its tab or section stops being drawn.
    pub(super) fn release_watchdog_inputs(&mut self) {
        self.needs_blur |= self.watchdog.has_inputs();
        self.watchdog.inputs.clear();
        self.watchdog.reseed();
    }
}

/// The row a local refusal names, so Apply can pick it.
pub(super) fn invalid_row(error: &WatchdogEditError) -> Option<usize> {
    match error {
        WatchdogEditError::ServerIp => None,
        WatchdogEditError::ApiKey { index }
        | WatchdogEditError::ApiSecret { index }
        | WatchdogEditError::NewRowCredentials { index }
        | WatchdogEditError::Price { index }
        | WatchdogEditError::Time { index } => Some(*index),
    }
}

#[cfg(test)]
mod tests;
