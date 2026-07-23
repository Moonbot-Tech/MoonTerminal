//! Selection and expansion state transitions for the strategy tree.

use super::*;

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
        self.selected_folder = None;
        before_selected != self.selected || before_anchor != self.anchor || before_sel != self.sel
    }

    pub(super) fn sync_pending_select(&mut self, cx: &App) -> bool {
        let Some((core, name)) = self.pending_select.clone() else {
            return false;
        };
        let key = {
            let store = self.backend.read(cx).session.store();
            store.core(core).and_then(|cd| {
                cd.strategies
                    .iter()
                    .find(|row| row.name == name)
                    .map(|row| (core, row.id))
            })
        };
        let Some(key) = key else {
            return false;
        };
        self.selected = Some(key);
        self.sel.clear();
        self.sel.insert(key);
        self.pending_select = None;
        self.clamp_selected_section(cx);
        true
    }

    /// Drain a `Backend::strategies_goto` request and reveal its strategy.
    /// Disables the active-only filter, resets other filters when they still hide the target,
    /// expands its core and folders, and selects it. Returns the key so render can scroll after
    /// calling `set_items`.
    pub(super) fn drain_goto(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Key> {
        let (core, strat_id) = self.backend.read(cx).strategies_goto?;
        self.backend.update(cx, |b, _| b.strategies_goto = None);
        let row = {
            let store = self.backend.read(cx).session.store();
            store
                .core(core)?
                .strategies
                .iter()
                .find(|r| r.id == strat_id)
                .cloned()?
        };
        self.filter.only_active = false;
        // Reset kind, direction, and search only if they still hide the target. `set_value` does not
        // emit Change, so update the filter explicitly alongside the search input.
        if !self.filter.matches(&row) {
            self.filter.kind = None;
            self.filter.dir = None;
            if !self.filter.search.trim().is_empty() {
                self.filter.search.clear();
                self.search
                    .update(cx, |st, cx| st.set_value(String::new(), window, cx));
            }
        }
        let key: Key = (core, strat_id);
        self.expanded_cores.insert(core);
        self.expand_path(core, tree::ops::path_segments(&row.folder_path));
        self.sel.clear();
        self.sel.insert(key);
        self.anchor = Some(key);
        self.selected = Some(key);
        self.clamp_selected_section(cx);
        Some(key)
    }

    pub(super) fn clamp_selected_section(&mut self, cx: &App) -> bool {
        let store = self.backend.read(cx).session.store();
        let Some(sections) = selected_sections(self, store) else {
            return false;
        };
        if self.selected_section < sections.len() {
            return false;
        }
        self.selected_section = 0;
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

    /// Expand every node when `collapsed` is true; otherwise collapse all nodes.
    pub(super) fn expand_collapse_toggle(
        &mut self,
        cores: &[(CoreId, String)],
        store: &CoreStore,
        collapsed: bool,
    ) {
        if !collapsed {
            self.expanded_cores.clear();
            self.expanded_folders.clear();
            return;
        }
        for (c, _) in cores {
            self.expanded_cores.insert(*c);
            let Some(cd) = store.core(*c) else { continue };
            let paths: Vec<String> = cd
                .strategies
                .iter()
                .map(|r| r.folder_path.clone())
                .collect();
            for path in paths {
                self.expand_path(*c, tree::ops::path_segments(&path));
            }
        }
    }

    // ── Panel 1: strategy tree ───────────────────────────────────────────────
}
