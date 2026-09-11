//! Keeping the window in step with the cores: the list, the selection, the anchor's page and the
//! diff across the selection.
//!
//! One function, [`CoreExpertView::sync_from_core`], runs on every backend notification and on
//! every click in the list, and answers everything the frame needs by changing what the window
//! SAYS rather than by closing it. The cheap disqualifications come first, and the anchor's page
//! is taken only while the user has nothing staged on it.

use gpui::*;

use moon_core::feed::CoreConfigState;
use moon_core::session::CoreId;

use crate::Backend;

use super::{CoreExpertView, PageState, cores};

impl CoreExpertView {
    /// Resolve the list, the anchor and what the window can do with it, and take the anchor's
    /// newest page when it may.
    ///
    /// Re-seeding happens only while NOTHING is staged. Past that the user's edits outrank the
    /// core's newer values, and overwriting them mid-edit is the worse failure — the rule the popup
    /// states for the same reason. The staged set survives every transition THIS function makes:
    /// a core losing its page, the anchor moving to another selected core, a replaced process. It
    /// is laid back over whatever page is seeded next. Only a click that replaces the selection
    /// with cores the set was not made for drops it, and that is [`Self::select_core_row`]'s
    /// decision, taken before the sync runs.
    ///
    /// Args:
    ///     cx: Application context used to read the backend.
    ///
    /// Returns:
    ///     Whether anything the window draws changed, which is what gates the repaint.
    pub(super) fn sync_from_core(&mut self, cx: &App) -> bool {
        let b = self.backend.read(cx);
        let mut changed = self.sync_roster(b);
        changed |= self.settle_pending(b);

        // The page belongs to the core it was seeded from. When the anchor is another core — a
        // click, or the anchored core leaving the list — the page goes with the old one, and the
        // staged changes wait for the new one's snapshot.
        let anchor = cores::anchor(&self.selection, &self.roster);
        if anchor != self.anchor {
            self.anchor = anchor;
            self.release_page();
            self.had_page = false;
            changed = true;
        }
        let state = match anchor {
            None => PageState::NoCore,
            Some(core) => {
                let entry = b.session.store().core(core);
                // The verdict on the anchor's last write repaints the banner drawn from it; a
                // give-up moves this revision and nothing else, in any state of the page.
                let edit_rev = entry.map_or(0, |d| d.core_config_edit_rev);
                changed |= self.seen_edit_rev != edit_rev;
                self.seen_edit_rev = edit_rev;
                // The store's own classification, not a second reading of the same facts:
                // `Awaiting` covers both "no page yet" and the drop that happens when a DIFFERENT
                // MoonBot process answers on this connection, and `Stale` covers a page retained
                // across a link that is no longer Ready — a state a hand-rolled
                // `core_config.is_none()` test cannot see at all, and one whose page must not be
                // sent.
                match entry.map(|d| d.core_config_state()) {
                    None | Some(CoreConfigState::Awaiting) if self.had_page => PageState::Replaced,
                    None | Some(CoreConfigState::Awaiting) => PageState::Waiting,
                    Some(CoreConfigState::Stale) => PageState::Stale,
                    Some(CoreConfigState::Live) => match entry.and_then(|d| d.live_core_config()) {
                        Some(latest) => {
                            let rev = entry.map_or(0, |d| d.core_config_rev);
                            changed |= self.take_page(latest, rev, entry);
                            PageState::Ready
                        }
                        // `Live` guarantees a page; this arm cannot run, and reads as the wait
                        // rather than panicking on the frame path if that ever stops holding.
                        None => PageState::Waiting,
                    },
                }
            }
        };
        changed |= self.enter_state(state);
        changed | self.sync_diff(b)
    }

    /// Seed the anchor's page when there is none, or re-seed it when the core's page changed
    /// under one with nothing staged — or when the anchor's pending send settled without the
    /// page changing ([`CoreExpertView::page_behind`]); and note the report counters the
    /// AutoStart page prints.
    ///
    /// Returns:
    ///     Whether anything the window draws changed.
    fn take_page(
        &mut self,
        latest: &moon_core::feed::CoreConfig,
        rev: u64,
        entry: Option<&moon_core::session::store::CoreData>,
    ) -> bool {
        let mut changed = false;
        // The AutoStart page prints the core's REPORT counters, which move without the
        // configuration moving. Without them in this gate a Reset the trader just pressed keeps
        // showing the old number until something unrelated repaints the window. Compared as bits
        // so a counter that ever went non-finite cannot repaint the window forever, and only on
        // the page that draws them.
        let profit = entry.and_then(|d| d.profit_state.as_ref()).map(|s| {
            (
                s.total_profit.to_bits(),
                s.total_trades,
                s.hourly_profit.to_bits(),
                s.hourly_trades,
            )
        });
        changed |= self.seen_profit != profit && self.tab == super::ExpertTab::AutoStart;
        self.seen_profit = profit;
        // Both halves are one comparison each: the store's revision moves only when the core's
        // page really changed, and the staged set answers whether the user's edits outrank it
        // without walking the projection.
        if self.draft.is_none()
            || ((self.seen_rev != rev || self.page_behind) && self.changes.is_empty())
        {
            // What Apply sent this core and its queue has not worked through yet goes under the
            // staged changes AND into the base the next edits are measured against — as `apply`
            // itself leaves them: a snapshot that moved for another reason before the echo would
            // otherwise snap the page back to the pre-Apply values it is about to leave, and an
            // edit moving a sent field back to the store's value would read as no change.
            let mut base = latest.clone();
            if let Some(sent) = self.anchor.and_then(|core| self.pending.get(&core)) {
                sent.sent.overlay(&mut base);
            }
            let mut draft = base.clone();
            self.changes.overlay(&mut draft);
            self.changes.seed(&draft);
            self.mixed.seed(&draft);
            self.draft = Some(draft);
            self.base = Some(base);
            self.page_behind = false;
            // The pick into the Telegram channel box is POSITIONAL, and this is a different list:
            // keeping it would highlight one channel and remove another.
            self.selected_channel = None;
            self.seen_rev = rev;
            self.had_page = true;
            self.editors.reseeded();
            // The editors take the page's values back on this; a box emptied for a mixed value
            // is refilled with them, and has to be emptied again if it is still mixed.
            self.emptied.clear();
            // The page under the banner is not the page the refusal was about any more.
            self.write_refused = None;
            changed = true;
        }
        changed
    }

    /// Drop the pending entries of the cores whose write queue has run empty since the send —
    /// or that cannot answer any more.
    ///
    /// The queue's own word (`core_config_drained_rev`), whichever way the edit left it: echoed,
    /// already held by the core and never sent, or given up on — the last is a difference the
    /// page must show again, and the banner names it. A core whose page is no longer live is
    /// dropped too: the session queue survives a reconnect and may still deliver, but the page
    /// this window would build for that core is then a page it cannot send anyway.
    ///
    /// Returns:
    ///     Whether an entry was dropped, which changes what the page marks.
    pub(super) fn settle_pending(&mut self, b: &Backend) -> bool {
        if self.pending.is_empty() {
            return false;
        }
        let store = b.session.store();
        let before = self.pending.len();
        let anchor_held = self
            .anchor
            .is_some_and(|core| self.pending.contains_key(&core));
        self.pending.retain(|core, sent| {
            store.core(*core).is_some_and(|entry| {
                entry.live_core_config().is_some()
                    && entry.core_config_drained_rev == sent.drained_rev
            })
        });
        // The page has been drawn over the anchor's sent values; with those settled it must show
        // the store's page again — which a refused write leaves at a revision that never moved.
        if anchor_held
            && !self
                .anchor
                .is_some_and(|core| self.pending.contains_key(&core))
        {
            self.page_behind = true;
        }
        self.pending.len() != before
    }

    /// Rebuild the list when any of its inputs moved, and prune the selection to it.
    ///
    /// Returns:
    ///     Whether the list changed.
    fn sync_roster(&mut self, b: &Backend) -> bool {
        let key = cores::CoreRoster::key(b);
        if key == self.roster.built_from() {
            return false;
        }
        self.roster = cores::CoreRoster::build(b, key);
        // A core that left the list must leave the selection with it: OK writes to the selection,
        // and a row the user can no longer see is not one they can mean.
        self.selection.retain_visible(self.roster.order());
        true
    }

    /// Recompute where the selected cores disagree, if the selection or any of its pages moved.
    ///
    /// Returns:
    ///     Whether the diff changed.
    fn sync_diff(&mut self, b: &Backend) -> bool {
        let selected = self.roster.selected(&self.selection).map(|row| row.core);
        if cores::diff_key_matches(b, selected, &self.diff_key) {
            return false;
        }
        let targets = cores::targets(&self.selection, &self.roster);
        self.diff_key = cores::diff_key(b, &targets);
        self.diff = moon_core::feed::differing_fields(&cores::live_pages(b, &targets));
        true
    }

    /// Move to a state, releasing a page that state may no longer use.
    ///
    /// Every blocked state releases the page, so the window recovers on its own the moment the
    /// obstacle clears: the replacement MoonBot sending its configuration then reaches
    /// [`PageState::Ready`] instead of being read as a core whose page is already gone. The
    /// staged changes are NOT dropped: they are laid over the next page seeded, so a link that
    /// blinks costs the trader nothing they typed.
    ///
    /// Args:
    ///     state: State the sync resolved.
    ///
    /// Returns:
    ///     Whether anything the window draws changed.
    fn enter_state(&mut self, state: PageState) -> bool {
        if !state.can_send() {
            self.release_page();
        }
        if state == self.state {
            return false;
        }
        self.write_refused = None;
        self.state = state;
        true
    }

    /// Drop the page and the controls built for it.
    ///
    /// The controls go with the page: one retained past it would seed the next core's row with
    /// the previous core's text on its first frame. Focus may be sitting in one of them — but only
    /// if one existed, so a store that never built anything does not cost the window its focus.
    /// Idempotent, so every blocked state may call it without checking.
    fn release_page(&mut self) {
        self.draft = None;
        self.base = None;
        self.selected_channel = None;
        self.needs_blur |= !self.editors.is_empty();
        self.editors.clear();
        self.emptied.clear();
        self.write_refused = None;
    }

    /// Apply one click in the core list to the selection, and follow it.
    ///
    /// Staged changes go with the cores they were made for: a click that keeps one of them
    /// selected keeps the changes (and lays them over the page of whichever core is drawn), a
    /// click that replaces the selection with other cores drops them — see
    /// [`cores::keeps_changes`].
    ///
    /// Args:
    ///     core: The clicked core.
    ///     modifiers: Native modifier snapshot from the click.
    ///     cx: View context used to repaint.
    pub(super) fn select_core_row(
        &mut self,
        core: CoreId,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        let before = cores::targets(&self.selection, &self.roster);
        self.selection.press(
            Some(core),
            self.roster.order(),
            modifiers.shift,
            modifiers.secondary(),
        );
        let after = cores::targets(&self.selection, &self.roster);
        if !self.changes.is_empty() && !cores::keeps_changes(&before, &after) {
            log::info!(
                "expert core settings dropped {} unsaved parameter(s): the selection moved to other cores",
                self.changes.len()
            );
            self.changes.clear();
        }
        // The banner described a send to the previous selection.
        self.write_refused = None;
        self.sync_from_core(cx);
        cx.notify();
    }

    /// Select the one core a group's gear stands for, replacing the selection.
    ///
    /// In Auto Overview the gear stands for no core at all (`Backend::scoped_trade_core`), so
    /// nothing is selected and the list is left for the trader to pick from.
    ///
    /// Args:
    ///     group: Group whose active trading core is selected.
    ///     cx: Application context used to read the backend.
    pub(super) fn select_group_core(&mut self, group: &str, cx: &App) {
        let b = self.backend.read(cx);
        // The list has to exist before a core can be checked against it.
        self.sync_roster(b);
        let core = b
            .scoped_trade_core(group)
            .filter(|core| self.roster.contains(*core));
        match core {
            Some(core) => self.selection.select_only(Some(core)),
            None => self.selection.clear(),
        }
        self.sync_from_core(cx);
    }

    /// Point an already-open window at another group's core.
    ///
    /// The window is an application-wide singleton, so the gear of a SECOND group window has to
    /// reach the one that exists. With nothing staged, that gear's core is selected as it would be
    /// on a fresh open. With changes staged, the press is the "bring the window forward" gesture:
    /// reselecting would throw the edits away, while the core the gear stands for is one click away
    /// in the list.
    ///
    /// Args:
    ///     group: Group whose gear was pressed.
    ///     cx: View context used to repaint.
    pub(super) fn rebind(&mut self, group: &str, cx: &mut Context<Self>) {
        if self.changes.is_empty() {
            self.select_group_core(group, cx);
        } else {
            log::info!(
                "expert core settings kept its staged changes: another group's gear only brought it forward"
            );
        }
        cx.notify();
    }
}
