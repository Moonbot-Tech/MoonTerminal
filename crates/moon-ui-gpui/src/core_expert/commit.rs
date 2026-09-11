//! What the user does to the page and what OK does with it: staging edits, building the page's
//! controls, switching pages, and the send to every selected core.

use gpui::*;

use moon_core::feed::{CoreChangeSet, CoreConfig};
use moon_core::session::CoreId;

use crate::shell::editors::{self, CoreDraftHost, EditorStore};
use crate::shell::send_core_config_to;

use super::{CoreExpertView, cores, pages, tabs::ExpertTab};

impl CoreExpertView {
    /// Apply one change to the page, if a page is staged.
    ///
    /// A change arriving without one is dropped rather than creating it: a page built from a
    /// control's own value would describe no core.
    ///
    /// The change set measures the edit against its own shadow of the page, so a keystroke or a
    /// slider tick costs a walk over two hundred fields and no clone — and it is that walk that
    /// tells a control that moved a value from one that moved nothing (a dead row's slider, a
    /// stepper pressed at its floor), so the footer's count and the window's "am I still following
    /// the core" answer come from what actually changed rather than from which control fired.
    pub(super) fn edit_draft(
        &mut self,
        apply: impl FnOnce(&mut CoreConfig),
        cx: &mut Context<Self>,
    ) {
        let (Some(draft), Some(base)) = (self.draft.as_mut(), self.base.as_ref()) else {
            return;
        };
        apply(draft);
        self.changes.note_edit(draft, base);
        // The set is authoritative for its fields: laid back over the page so a staged value a
        // mirrored page was showing as a copy comes back the moment the mirror is off — the page
        // shows what OK sends. A no-op for every ordinary staged field, which the page already holds.
        self.changes.overlay(draft);
        cx.notify();
    }

    /// [`Self::edit_draft`] for a change made on a control, which may be drawn MIXED.
    ///
    /// On a mixed control — the selection disagrees on its field — a change that lands on the
    /// value the drawn core already holds is still a decision, "this value, everywhere", and the
    /// fields the change wrote are staged at the page's value. Whether the control IS mixed is
    /// read off the window's own state at the moment of the event, not off a flag captured at the
    /// render that drew it; and what the change WROTE is probed from the change itself, so a
    /// number box whose text did not parse decides nothing.
    ///
    /// Args:
    ///     id: The control the change came from.
    ///     apply: The change; `Fn` rather than `FnOnce` because it is run again to probe it.
    ///     cx: View context used to repaint.
    pub(super) fn edit_control(
        &mut self,
        id: &'static str,
        apply: impl Fn(&mut CoreConfig),
        cx: &mut Context<Self>,
    ) {
        let decided = self.mixed.cached(id);
        let written = decided.then(|| self.mixed.fields_written_by(&apply));
        let (Some(draft), Some(base)) = (self.draft.as_mut(), self.base.as_ref()) else {
            return;
        };
        apply(draft);
        self.changes.note_edit(draft, base);
        for field in written.into_iter().flatten() {
            self.changes.stage(field, draft);
        }
        self.changes.overlay(draft);
        cx.notify();
    }

    /// Create or synchronize every control the drawn pages declare.
    ///
    /// Called at the top of the render, as the popup builds its own: a control exists only once the
    /// row that declares it has been on screen, so a session that never opens this window pays for
    /// none of them.
    pub(super) fn build_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Only the page on screen, and without cloning the projection to read it: the specs own
        // their values, so the borrow ends before the store is touched.
        let tab = self.tab;
        let Some((fields, sliders)) = self.draft.as_ref().map(|draft| {
            (
                pages::field_specs(tab, draft),
                pages::slider_specs(tab, draft),
            )
        }) else {
            return;
        };
        for (id, value, stage) in fields {
            // The box's setter is known only here, so this is where it is probed. Two samples,
            // because the setters parse: a number box takes the first, a time box the second.
            let mixed = self.mixed.is_mixed(id, &|cfg| {
                stage(cfg, "1");
                stage(cfg, "01:00");
            });
            // A mixed box is shown EMPTY, as the strategies window shows one: the page's value is
            // one core's, and printing it would claim the selection agrees. Emptied once — what
            // the trader then types is staged through the editor's own change event. Refilled
            // from the page only if it stops being mixed while still empty (another core
            // deselected): the editor re-reads the page on a re-seed alone, so it would otherwise
            // stay blank for good, and a box the trader is typing into must not have its text
            // rewritten under the caret. The value is cloned for that refill alone.
            let refill = (!mixed && self.emptied.remove(id)).then(|| value.clone());
            let state = editors::input_state(self, id, value, stage, window, cx);
            if mixed {
                if self.emptied.insert(id) {
                    state.update(cx, |s, c| s.sync_value("", c));
                }
            } else if let Some(value) = refill
                && state.read(cx).text().len() == 0
            {
                state.update(cx, |s, c| s.sync_value(value, c));
            }
        }
        for id in pages::scratch_specs(tab) {
            editors::scratch_input_state(self, id, window, cx);
        }
        for (id, bounds, value, stage, mirror) in sliders {
            editors::slider_state(self, id, bounds, value, stage, mirror, window, cx);
            // Probed here for the same reason as the boxes; the slider widget reads the answer
            // back through `mixed::cached`.
            self.mixed.is_mixed(id, &|cfg| stage(cfg, 1.0));
        }
    }

    /// Open one of the Special page's sections, closing the one that was open.
    ///
    /// Moonbot shows one at a time; clicking the open one leaves it open rather than collapsing to
    /// nothing, because a page with every section shut says less than a page with one.
    pub(super) fn set_special_section(
        &mut self,
        section: pages::SpecialSection,
        cx: &mut Context<Self>,
    ) {
        if self.special_section == section {
            return;
        }
        self.special_section = section;
        // The controls of the section being closed stay DECLARED — the specs are per tab — but they
        // stop being drawn, and a disabled field takes focus on a click just as a live one does. So
        // this needs the same blur `set_tab` performs: focus left on a control nothing draws takes
        // the window's dispatch path with it, the one failure `restore_root_focus` cannot repair.
        self.needs_blur |= !self.editors.is_empty();
        cx.notify();
    }

    /// Pick a row in the Telegram page's channel box, or clear the pick.
    pub(super) fn set_selected_channel(&mut self, row: Option<usize>, cx: &mut Context<Self>) {
        if self.selected_channel == row {
            return;
        }
        self.selected_channel = row;
        cx.notify();
    }

    /// Select one of Moonbot's inner Hotkeys tabs.
    pub(super) fn set_hotkeys_sub(&mut self, sub: pages::HotkeysSub, cx: &mut Context<Self>) {
        if self.hotkeys_sub == sub {
            return;
        }
        self.hotkeys_sub = sub;
        cx.notify();
    }

    /// Select a page.
    ///
    /// Every tab opens, including the ones with no wire values behind them: the window reproduces
    /// Moonbot's dialog, and what is blocked there is the CONTROL without a value, not the page.
    pub(super) fn set_tab(&mut self, tab: ExpertTab, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        // The controls belong to the page that declared them, and the page is about to stop being
        // drawn. Dropping them here rather than leaving them in the store is what keeps a window
        // that has visited every tab from holding every tab's controls — and the blur is not
        // optional: a focused editor that stops rendering leaves the window reading as focused
        // over a collapsed dispatch path, which `hotkeys::blur_field` documents as the one focus
        // failure `restore_root_focus` cannot repair. A DISABLED field still takes focus on a
        // click, so this matters most on the pages where nothing is live.
        self.needs_blur |= !self.editors.is_empty();
        self.editors.clear();
        // The boxes went with the controls; a rebuilt one starts from the page again.
        self.emptied.clear();
        self.selected_channel = None;
        cx.notify();
    }

    /// Send the staged changes to every selected core.
    ///
    /// Each target gets its OWN latest snapshot with what an earlier Apply sent it and the staged
    /// fields laid over — never the anchor's page — so a bulk send changes exactly the parameters
    /// the user moved and leaves every other field as that core held it; see
    /// [`super::PendingSend`] for why the earlier send goes on first. The mask names the areas of
    /// the staged fields and nothing more.
    ///
    /// A target without a live page is skipped and logged: the footer counted those cores in
    /// amber BEFORE the click, so the button was pressed knowing it. Two outcomes raise the banner
    /// instead, because they are news the footer could not give: a target the session REFUSES,
    /// and a selection where NO target had a page, since then nothing was written at all.
    ///
    /// Returns:
    ///     The cores the write reached, each with the page it was sent, or `None` when the banner
    ///     was raised instead. The caller checks that there is something to send: an empty change
    ///     set is its decision to make.
    fn send(&mut self, cx: &mut Context<Self>) -> Option<Vec<(CoreId, CoreConfig)>> {
        let sections = self.changes.mask();
        let targets = cores::targets(&self.selection, &self.roster);
        let b = self.backend.read(cx);
        let store = b.session.store();
        let mut reached = Vec::with_capacity(targets.len());
        let mut refused = 0;
        for core in &targets {
            let Some(latest) = store.core(*core).and_then(|d| d.live_core_config()) else {
                log::info!("expert core settings send skipped core {core}: no live page");
                continue;
            };
            let mut page = latest.clone();
            if let Some(earlier) = self.pending.get(core) {
                earlier.sent.overlay(&mut page);
            }
            self.changes.overlay(&mut page);
            if send_core_config_to(b, *core, page.clone(), sections) {
                reached.push((*core, page));
            } else {
                refused += 1;
            }
        }
        if refused > 0 || reached.is_empty() {
            // The total is the whole selection: the banner reads "the rest have no page or
            // refused", so a skipped core counts among the rest. The changes stay staged, so the
            // trader can narrow the selection or retry.
            self.write_refused = Some((reached.len(), targets.len()));
            cx.notify();
            return None;
        }
        // Every earlier refusal was about an earlier selection or an earlier page.
        self.write_refused = None;
        Some(reached)
    }

    /// Send and close, as Moonbot's OK does.
    ///
    /// Nothing staged closes too — Moonbot's OK closes either way — except when the page cannot
    /// be sent at all: closing then would read as a save, and the page keeps saying what it is
    /// waiting for instead. A refusal keeps the window open with its banner.
    pub(super) fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.can_send() {
            return;
        }
        if self.changes.is_empty() || self.send(cx).is_some() {
            window.remove_window();
        }
    }

    /// Send and stay, so the next change can be made over the values just written.
    ///
    /// The page keeps showing what was sent rather than snapping back to the store's copy: the
    /// cores echo the write within a round trip, and the store's revision moves only then. Until
    /// it does, the page is the best account of what the cores hold — so it becomes the new BASE
    /// the next edits are measured against, the change set starts over on it, and what was sent
    /// is remembered per core until its queue has worked through it ([`super::PendingSend`]).
    /// The echoes then re-seed the page exactly as any core-side change would.
    pub(super) fn apply(&mut self, cx: &mut Context<Self>) {
        if !self.state.can_send() || self.changes.is_empty() {
            return;
        }
        let Some(reached) = self.send(cx) else {
            return;
        };
        // Entries the queue has already worked through go first, so an entry found below is one
        // whose send is still queued and whose origin therefore still stands: the queue drains
        // once more after this send too, whether or not it was folded into the earlier packet.
        self.settle_pending(self.backend.read(cx));
        let Some(draft) = self.draft.as_ref() else {
            return;
        };
        let store = self.backend.read(cx).session.store();
        for (core, page) in reached {
            let entry = self.pending.entry(core).or_insert_with(|| {
                // A new entry takes the page once; a repeat only adds the fields sent, the
                // shadow already holding the earlier values and nothing else being read from it.
                let mut sent = CoreChangeSet::default();
                sent.seed(&page);
                super::PendingSend {
                    sent,
                    drained_rev: store.core(core).map_or(0, |d| d.core_config_drained_rev),
                }
            });
            // Staged from the page THIS core was sent, not the anchor's draft: a short gesture
            // is a copy on a page whose mirror is on and a value of its own on one whose mirror
            // is off, and the entry must hold exactly what went on the wire to this core.
            for &field in self.changes.fields() {
                entry.sent.stage(field, &page);
            }
        }
        self.base = Some(draft.clone());
        self.changes.restart(draft);
        cx.notify();
    }

    /// Close the window without sending, as Moonbot's Cancel does.
    ///
    /// The staged changes need no explicit discard: they live in this view, which the window drops.
    pub(super) fn cancel(&mut self, window: &mut Window) {
        window.remove_window();
    }

    /// Turn expert mode off: the gear goes back to the compact popup, so this window closes with
    /// it rather than staying up as the only surface the preference no longer points at.
    ///
    /// The staged changes are DISCARDED, exactly as Cancel discards them — leaving expert mode is
    /// not a confirmation of the values on screen.
    pub(super) fn leave_expert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.backend.update(cx, |b, bcx| {
            b.set_core_settings_expert(false, bcx);
        });
        window.remove_window();
    }
}

impl CoreDraftHost for CoreExpertView {
    fn editors(&mut self) -> &mut EditorStore {
        &mut self.editors
    }

    fn stage_draft(
        &mut self,
        id: &'static str,
        apply: impl Fn(&mut CoreConfig),
        cx: &mut Context<Self>,
    ) {
        self.edit_control(id, apply, cx);
    }

    fn editor_window(&self) -> AnyWindowHandle {
        self.window
    }
}
