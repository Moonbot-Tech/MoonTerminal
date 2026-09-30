//! Selection and expansion state transitions for the strategy tree.

use super::*;
use moon_core::feed::strategy_path;

/// How long the search text settles before the selection is pruned to it.
///
/// Mirrors Analytics' `MASK_DEBOUNCE` (300 ms), the other strategy-name query typed live.
const SEARCH_PRUNE_DEBOUNCE: Duration = Duration::from_millis(300);

impl StrategiesView {
    /// Apply a strategy click with selection modifiers.
    /// Shift selects the `order` range from the anchor, Ctrl/Cmd toggles one key, and an
    /// unmodified click replaces the selection.
    pub(super) fn apply_click(
        &mut self,
        key: Key,
        order: &[Key],
        shift: bool,
        command: bool,
    ) -> bool {
        let before_selected = self.selected;
        let before_anchor = self.anchor;
        let before_sel = self.sel.clone();
        let before_folder = (self.folder_sel.len(), self.folder_anchor.clone());
        if shift {
            if let Some(a) = self.anchor {
                let ia = order.iter().position(|k| *k == a);
                let ib = order.iter().position(|k| *k == key);
                if let (Some(ia), Some(ib)) = (ia, ib) {
                    let (lo, hi) = if ia <= ib { (ia, ib) } else { (ib, ia) };
                    self.sel = order[lo..=hi].iter().copied().collect();
                } else {
                    self.sel = std::iter::once(key).collect();
                }
            } else {
                self.sel = std::iter::once(key).collect();
                self.anchor = Some(key);
            }
        } else if command {
            if !self.sel.remove(&key) {
                self.sel.insert(key);
            }
            self.anchor = Some(key);
        } else {
            self.sel.clear();
            self.sel.insert(key);
            self.anchor = Some(key);
        }
        // The clicked strategy always becomes the primary schema/section source; keep the section.
        self.selected = Some(key);
        // A strategy click clears folder selection so Ctrl+C copies the strategy selection again.
        self.clear_folder_selection();
        before_selected != self.selected
            || before_anchor != self.anchor
            || before_sel != self.sel
            || before_folder != (self.folder_sel.len(), self.folder_anchor.clone())
    }

    /// Retire the whole folder selection, cursor included.
    ///
    /// One setter rather than two assignments at each site: the set and its anchor have to go
    /// together, and a site that cleared only the set would leave a cursor pointing at a node that
    /// is no longer selected — which is what the keyboard would then move from.
    pub(super) fn clear_folder_selection(&mut self) {
        self.folder_sel.clear();
        self.folder_anchor = None;
    }

    /// Apply a folder or core click with selection modifiers.
    ///
    /// The same three gestures `apply_click` gives strategies — Shift ranges over the drawn node
    /// order, Ctrl toggles one node, a plain click replaces — over the folder set instead.
    ///
    /// It deliberately does NOT touch `selected`/`sel`/`anchor`. A strategy selection retires the
    /// folder selection, but not the reverse: `resolve_paste_target` reads that asymmetry as its
    /// precedence, and `a_selected_folder_outranks_a_stale_strategy` pins it.
    ///
    /// Returns:
    ///     Whether anything about the folder selection actually changed.
    pub(super) fn apply_folder_click(
        &mut self,
        node: (CoreId, String),
        order: &[tree::ops::NavNode],
        shift: bool,
        command: bool,
    ) -> bool {
        let before_sel = self.folder_sel.clone();
        let before_anchor = self.folder_anchor.clone();
        if shift {
            match self.folder_anchor.clone() {
                Some(anchor) => {
                    let range = tree::ops::folder_range(order, &anchor, &node);
                    // An anchor the current frame no longer draws cannot describe a range; fall
                    // back to the single node rather than selecting nothing.
                    match range.is_empty() {
                        true => {
                            self.folder_sel.clear();
                            self.folder_sel.insert(node.clone());
                        }
                        false => self.folder_sel = range.into_iter().collect(),
                    }
                }
                None => {
                    self.folder_sel.clear();
                    self.folder_sel.insert(node.clone());
                }
            }
        } else if command {
            if !self.folder_sel.remove(&node) {
                self.folder_sel.insert(node.clone());
            }
        } else {
            self.folder_sel.clear();
            self.folder_sel.insert(node.clone());
        }
        self.folder_anchor = Some(node);
        before_sel != self.folder_sel || before_anchor != self.folder_anchor
    }

    /// Make `key` the primary selection, replacing whatever was selected.
    ///
    /// Centralizing the assignment preserves the invariant used by `resolve_paste_target`: a
    /// strategy selection always retires the folder selection.
    pub(super) fn focus_strategy(&mut self, key: Key) {
        self.sel.clear();
        self.sel.insert(key);
        self.anchor = Some(key);
        self.selected = Some(key);
        self.clear_folder_selection();
    }

    /// Move the primary selection and the Shift anchor to `key`, leaving the selection sets alone.
    ///
    /// The narrowing counterpart of [`Self::focus_strategy`], which replaces the whole selection:
    /// a prune that drops the primary must keep every other surviving row selected.
    fn repoint_primary(&mut self, key: Option<Key>) {
        self.selected = key;
        self.anchor = key;
    }

    /// Drop every selected row and folder the current filter hides.
    ///
    /// Narrows only, like the delete path in `tree/dialogs.rs`, and never calls
    /// [`Self::focus_strategy`], which would replace the set. A hidden primary moves to the first
    /// surviving row in tree order (or the smallest surviving key when none is on screen).
    /// While a search debounce is pending the search part of the filter is the last SETTLED text,
    /// not the half-typed one: another filter's change must not prune by an unfinished query.
    /// Limitations: folders are pruned by their core's exchange only; a row that stops matching
    /// through its own edit stays selected until the next filter change; pruning is irreversible,
    /// so clearing the filter does not bring rows back.
    ///
    /// Args:
    ///     cx: App context used to read the store and venues.
    ///
    /// Returns:
    ///     Whether the selection changed.
    pub(super) fn prune_selection_to_filter(&mut self, cx: &mut App) -> bool {
        if self.search_prune_debounce.is_none() && self.settled_search != self.filter.search {
            self.settled_search = self.filter.search.clone();
        }
        let query = StrategyQuery::parse(&self.settled_search);
        self.prune_against_settled(query, cx)
    }

    /// Prune against the settled search text, whose parse the caller already holds.
    fn prune_against_settled(&mut self, query: StrategyQuery, cx: &mut App) -> bool {
        self.last_pruned_query = Some(query);
        // The filter is evaluated with the settled text swapped in, then restored.
        let live_search = std::mem::replace(&mut self.filter.search, self.settled_search.clone());
        let hidden = {
            let backend = self.backend.read(cx);
            let store = backend.session.store();
            let venues = backend.session.core_venues();
            let candidates = self.sel.iter().copied().chain(self.selected);
            filter_hidden_keys(candidates, store, &self.filter, venues, &self.deleted)
        };
        self.filter.search = live_search;
        let venues = self.backend.read(cx).session.core_venues();
        let folder_before = self.folder_sel.len();
        self.folder_sel
            .retain(|(core, _)| self.filter.core_matches(venues.get(core)));
        let anchor_hidden = self
            .folder_anchor
            .as_ref()
            .is_some_and(|(core, _)| !self.filter.core_matches(venues.get(core)));
        if hidden.is_empty() && folder_before == self.folder_sel.len() && !anchor_hidden {
            return false;
        }
        if anchor_hidden {
            self.folder_anchor = None;
        }
        self.sel.retain(|k| !hidden.contains(k));
        let primary_moved = self.selected.is_some_and(|k| hidden.contains(&k));
        if primary_moved {
            let next = if self.sel.is_empty() {
                None
            } else {
                self.flat_order
                    .iter()
                    .copied()
                    .find(|k| self.sel.contains(k))
                    .or_else(|| self.sel.iter().copied().min())
            };
            self.repoint_primary(next);
        } else if self.anchor.is_some_and(|k| hidden.contains(&k)) {
            self.anchor = self.selected;
        }
        if primary_moved {
            self.clamp_selected_section(cx);
        }
        true
    }

    /// Settle the current search text and prune for it, unless it parses to the query last
    /// pruned against.
    pub(super) fn prune_for_search(&mut self, cx: &mut Context<Self>) {
        let query = StrategyQuery::parse(&self.filter.search);
        self.settled_search.clone_from(&self.filter.search);
        if self.last_pruned_query.as_ref() == Some(&query) {
            return;
        }
        self.prune_against_settled(query, cx);
        self.persist_session(cx);
        cx.notify();
    }

    /// Schedule the search prune after the text settles, replacing any pending one.
    ///
    /// The same shape as Analytics' strategy-mask debounce, with the same delay.
    pub(super) fn arm_search_prune(&mut self, cx: &mut Context<Self>) {
        self.search_prune_debounce = Some(cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(SEARCH_PRUNE_DEBOUNCE).await;
            cx.update(|cx| {
                // The handle is deliberately NOT cleared here: dropping a task from inside its
                // own body cancels the body, and a spent handle sitting in the field until the
                // next keystroke replaces it costs nothing.
                let _ = this.update(cx, |this, cx| this.prune_for_search(cx));
            });
        }));
    }

    /// Prune the selection to a changed filter, persist, and repaint.
    ///
    /// The one follow-up every row-filter control runs after writing `self.filter`.
    pub(super) fn on_filter_changed(&mut self, cx: &mut Context<Self>) {
        self.prune_selection_to_filter(cx);
        self.persist_session(cx);
        cx.notify();
    }

    /// Resolve a pending create/paste/copy selection after the core echoes the named strategy.
    ///
    /// Expands the core and complete folder chain, then queues the resolved key for render because
    /// only render owns the tree item index required by `MoonTreeState::scroll_to_item`.
    ///
    /// Returns:
    ///     Whether the pending strategy was found and selected.
    pub(super) fn sync_pending_select(&mut self, cx: &mut App) -> bool {
        let Some(request) = self.pending_select.clone() else {
            return false;
        };
        if !request.is_authorized(self.backend.read(cx)) {
            self.pending_select = None;
            return false;
        }
        let core = request.core;
        let RevealTarget::Name(name) = request.target else {
            self.pending_select = None;
            return false;
        };
        let found = {
            let store = self.backend.read(cx).session.store();
            store.core(core).and_then(|cd| {
                cd.strategies
                    .iter()
                    .find(|row| row.name == name)
                    .map(|row| ((core, row.id), row.folder_path.clone()))
            })
        };
        let Some((key, folder_path)) = found else {
            return false;
        };
        self.focus_strategy(key);
        self.expanded_cores.insert(core);
        self.expand_path(core, strategy_path::path_segments(&folder_path));
        // Expansion makes the row eligible for layout; render still needs this key to center the
        // corresponding item because only render owns the tree's item index.
        self.pending_scroll = Some(key);
        self.pending_select = None;
        self.clamp_selected_section(cx);
        self.persist_session(cx);
        true
    }

    /// Queue a local create/paste echo selection with the current singleton workspace authority.
    /// Active-only is disabled through the persisted preference setter before the unchecked echo
    /// can arrive hidden; a process-only exchange selection is cleared when it hides the target
    /// core.
    ///
    /// Args:
    ///     core: Core expected to echo the new strategy.
    ///     name: Exact strategy name to resolve from the future snapshot.
    ///     cx: View context used to persist visibility and capture singleton ownership.
    ///
    /// Returns:
    ///     Nothing; the request remains pending until an authorized matching row arrives.
    pub(super) fn queue_pending_name(
        &mut self,
        core: CoreId,
        name: String,
        cx: &mut Context<Self>,
    ) {
        self.disable_active_only_for_reveal(cx);
        // Cleared HERE, at the queue, for the same reason active-only is: the echo lands on `core`,
        // and nothing between here and `sync_pending_select` re-checks visibility. A create or
        // paste whose target came from a retained selection can name a core this filter hides, and
        // the reveal would then focus and scroll to a row the tree pruned — a request that appears
        // to do nothing at all.
        if !self.core_shown_by_exchange(core, cx) {
            self.filter.exchange = None;
            self.persist_session(cx);
        }
        let workspace_group = self
            .backend
            .read(cx)
            .singleton_workspace()
            .map(|workspace| workspace.group);
        self.pending_select = Some(StrategyRevealRequest::new(
            core,
            RevealTarget::Name(name),
            workspace_group,
        ));
    }

    /// Return whether one core survives the exchange filter, against the live venue map.
    ///
    /// The single seam between the reveal paths and the filter's own predicate, so "hidden by the
    /// exchange" means the same thing here as it does in the tree build.
    ///
    /// Args:
    ///     core: Core a reveal is about to target.
    ///     cx: Context used to read the session's venue identities.
    ///
    /// Returns:
    ///     `true` when no exchange is selected, or when this core belongs to the selected section.
    fn core_shown_by_exchange(&self, core: CoreId, cx: &App) -> bool {
        let venues = self.backend.read(cx).session.core_venues();
        self.filter.core_matches(venues.get(&core))
    }

    /// Clear every filter that could hide a strategy the user explicitly asked to see.
    ///
    /// `set_value` does not emit Change, so the search filter is updated explicitly alongside the
    /// input. Active-only is cleared through the persisted preference setter; kind, direction and
    /// the exchange remain process-only and are cleared here directly.
    ///
    /// Args:
    ///     window: Owning window needed to update the retained search input.
    ///     cx: View context used to persist active-only and notify input state.
    ///
    /// Returns:
    ///     Nothing; the view is ready for the reveal path to expand and select its target.
    fn clear_filters_for_reveal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.disable_active_only_for_reveal(cx);
        self.filter.kind = None;
        self.filter.dir = None;
        self.filter.exchange = None;
        if !self.filter.search.trim().is_empty() {
            self.filter.search.clear();
            self.search
                .update(cx, |st, cx| st.set_value(String::new(), window, cx));
        }
        self.persist_session(cx);
    }

    /// Drain a `Backend::strategies_goto` request and reveal its strategy.
    /// Resets filters when they still hide the target, persists active-only as disabled, expands
    /// its core and folders, and selects it. Returns the key so render can scroll after calling
    /// `set_items`.
    ///
    /// A NAME request whose row has not echoed back yet is handed to `pending_select`, which
    /// finishes the job the moment the core reports it — a just-created strategy has no id here
    /// and would otherwise be dropped for not existing yet.
    ///
    /// An ID request whose row is not on the core is looked up in the Deleted branch: the Report
    /// keeps trades of strategies long gone, and a reveal from one of them is the common case,
    /// not a stale click. Found there, it is selected the way a click in that branch selects —
    /// latest version open — and a version the request names wins over that default. Found
    /// nowhere, the window says so instead of opening on nothing.
    pub(super) fn drain_goto(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Key> {
        let request = match self.backend.read(cx).strategies_goto.clone() {
            Some(request) => {
                self.backend.update(cx, |b, _| b.strategies_goto = None);
                // A fresh request supersedes one still waiting for the Deleted branch.
                self.deferred_goto = None;
                request
            }
            // The Deleted branch has answered since the request was parked; ask again. Still
            // loading: keep waiting, the load's completion repaints this window.
            None if self.deleted_loaded && !self.deleted_inflight => self.deferred_goto.take()?,
            None => return None,
        };
        if !request.is_authorized(self.backend.read(cx)) {
            self.pending_select = None;
            return None;
        }
        let StrategyRevealRequest {
            core,
            target,
            workspace_group,
            version,
        } = request;
        let row = {
            let store = self.backend.read(cx).session.store();
            store.core(core).and_then(|cd| {
                cd.strategies
                    .iter()
                    .find(|r| match &target {
                        RevealTarget::Id(id) => r.id == *id,
                        RevealTarget::Name(name) => r.name == *name,
                    })
                    .cloned()
            })
        };
        let Some(row) = row else {
            let id = match target {
                // Not there yet. Only a NAME request can be waiting on an echo.
                RevealTarget::Name(name) => {
                    // The row is unknown, so `filter.matches` cannot be consulted — clear
                    // everything that could hide it rather than reveal it into a filtered-out
                    // list.
                    self.clear_filters_for_reveal(window, cx);
                    self.pending_select = Some(StrategyRevealRequest::new(
                        core,
                        RevealTarget::Name(name),
                        workspace_group,
                    ));
                    return None;
                }
                RevealTarget::Id(id) => id,
            };
            return self.reveal_deleted(core, id, workspace_group, version, window, cx);
        };
        // The exchange filter has to be asked separately: it selects CORES, and `filter.matches`
        // takes a `StrategyRow`, which carries no venue — so a target on a filtered-out exchange
        // would otherwise pass the guard below and be revealed into a tree that cannot show it.
        // Bound before the `if` because the venue lookup borrows `self.backend`.
        let hidden_by_exchange = !self.core_shown_by_exchange(core, cx);
        // Reset kind, direction, exchange, and search only if they still hide the target.
        if !self.filter.matches(&row) || hidden_by_exchange {
            self.clear_filters_for_reveal(window, cx);
        }
        let key: Key = (core, row.id);
        // A direct request supersedes an unresolved name request; retaining both would let the
        // delayed echo steal the selection after this navigation completes.
        self.pending_select = None;
        self.expanded_cores.insert(core);
        self.expand_path(core, strategy_path::path_segments(&row.folder_path));
        self.focus_strategy(key);
        if let Some(vf) = version {
            self.reveal_version(key, vf, cx);
        }
        self.clamp_selected_section(cx);
        self.persist_session(cx);
        Some(key)
    }

    /// Finish an id reveal whose strategy is not on its core: the Deleted branch, or nowhere.
    ///
    /// Args:
    ///     core: Core the request named.
    ///     id: Strategy id the request named.
    ///     workspace_group: The request's workspace authority, kept for a deferred retry.
    ///     version: Saved version the request asked to open, if any.
    ///     window: The Strategies window, for the missing-target notice.
    ///     cx: View context.
    ///
    /// Returns:
    ///     The selected key when the branch holds the strategy; `None` while the branch is still
    ///     loading (the request is parked) or when nothing holds it (the window says so).
    fn reveal_deleted(
        &mut self,
        core: CoreId,
        id: u64,
        workspace_group: Option<String>,
        version: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Key> {
        if !self.deleted_loaded || self.deleted_inflight {
            // The branch is being read; `ensure_deleted` starts the read on this very render if
            // it has not, and its completion repaints the window, which drains this again.
            self.deferred_goto = Some(
                StrategyRevealRequest::new(core, RevealTarget::Id(id), workspace_group)
                    .with_version(version),
            );
            return None;
        }
        let kept = self
            .deleted
            .get(&core)
            .is_some_and(|rows| rows.iter().any(|h| h.strategy_id as u64 == id));
        if !kept {
            // Pushed directly: render has already drained `pending_notes` this frame, and nothing
            // else would repaint the window to show a note parked there.
            let note = tree::ui::TreeNote::GotoMissing { strategy_id: id };
            window.push_notification(note.notification(), cx);
            return None;
        }
        // The Deleted branch is filtered by the search text only, and it lives under its core.
        self.clear_filters_for_reveal(window, cx);
        self.pending_select = None;
        self.expanded_cores.insert(core);
        self.expanded_deleted.insert(core);
        let key: Key = (core, id);
        self.select_deleted_strategy(key, cx);
        if let Some(vf) = version {
            self.reveal_version(key, vf, cx);
        }
        Some(key)
    }

    pub(super) fn clamp_selected_section(&mut self, cx: &mut App) -> bool {
        let store = self.backend.read(cx).session.store();
        let Some(sections) = selected_sections(self, store) else {
            return false;
        };
        if self.selected_section < sections.len() {
            return false;
        }
        self.selected_section = 0;
        self.persist_session(cx);
        true
    }

    // ── Actions for starting or stopping checked strategies ─────────────────

    /// Insert every cumulative path prefix into `expanded_folders`.
    /// Shared by Expand All and folder creation so a newly created folder is immediately visible.
    pub(super) fn expand_path<'a>(
        &mut self,
        core: CoreId,
        segments: impl Iterator<Item = &'a str>,
    ) {
        let mut acc = String::new();
        for part in segments {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(part);
            self.expanded_folders.insert((core, acc.clone()));
        }
    }

    /// Expand every core and every folder path its live strategies sit in.
    ///
    /// The concrete Auto rail core is kept in `rail_expanded_core`, which neither this nor
    /// [`Self::collapse_all`] touches, so Collapse all cannot hide the sole root of a singleton
    /// workspace. That overlay is not persisted.
    ///
    /// Args:
    ///     cores: Cores the tree currently shows.
    ///     store: Live store supplying each core's folder paths.
    pub(super) fn expand_all(&mut self, cores: &[(CoreId, String)], store: &CoreStore) {
        for (c, _) in cores {
            self.expanded_cores.insert(*c);
            let Some(cd) = store.core(*c) else { continue };
            for r in &cd.strategies {
                self.expand_path(*c, strategy_path::path_segments(&r.folder_path));
            }
        }
    }

    /// Collapse every hand-expanded core and folder.
    ///
    /// `rail_expanded_core` and the Deleted folders are left as they are.
    pub(super) fn collapse_all(&mut self) {
        self.expanded_cores.clear();
        self.expanded_folders.clear();
    }

    // ── Panel 1: strategy tree ───────────────────────────────────────────────
}
