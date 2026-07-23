//! Clipboard and drag-and-drop operations for the strategy tree. Dropping within one core moves
//! strategies through `move_strategies`; dropping across cores copies them through
//! `create_strategies`. [`super::ops`] owns the pure path and collection logic.

use super::super::*;
use super::ops;
use super::ui::{FolderDrag, StratDrag};
use moon_core::feed::NewStrategySpec;

impl StrategiesView {
    // ── Clipboard: copy and paste ────────────────────────────────────────────

    pub(super) fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let store = self.backend.read(cx).session.store();
        let rows = self.selection_rows(store);
        if rows.is_empty() {
            return;
        }
        let refs: Vec<&StrategyRow> = rows.iter().map(|(_, r)| r).collect();
        self.set_clipboard(ops::copy_rows(&refs), cx);
        cx.notify();
    }

    pub(super) fn copy_folder(&mut self, core: CoreId, path: Vec<String>, cx: &mut Context<Self>) {
        let clip = {
            let store = self.backend.read(cx).session.store();
            let Some(cd) = store.core(core) else { return };
            ops::copy_folder(&cd.strategies, &path)
        };
        self.set_clipboard(clip, cx);
        cx.notify();
    }

    /// Stores an internal clipboard and a textual copy in the system clipboard.
    ///
    /// A strategy or folder can therefore be pasted into a text editor, shared, and accepted back
    /// by `paste_into`.
    fn set_clipboard(&mut self, clip: Vec<ops::ClipItem>, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(ops::clip_to_text(&clip)));
        self.clipboard = Some(clip);
    }

    pub(super) fn paste_into(&mut self, core: CoreId, target: String, cx: &mut Context<Self>) {
        // Prefer the internal clipboard; when empty, parse the system clipboard's `clip_to_text`
        // format so strategies or folders shared as text can be pasted.
        let clip = self.clipboard.clone().or_else(|| {
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .and_then(|t| ops::clip_from_text(&t))
        });
        let Some(clip) = clip else {
            return;
        };
        let specs = {
            let store = self.backend.read(cx).session.store();
            let existing: std::collections::HashSet<String> = store
                .core(core)
                .map(|cd| cd.strategies.iter().map(|r| r.name.clone()).collect())
                .unwrap_or_default();
            // Remove reserved names already echoed by earlier pastes and treat the remaining names
            // as occupied. Otherwise rapid Ctrl+V operations would read the same store snapshot
            // and create duplicate names.
            self.pending_names
                .retain(|(c, n)| *c != core || !existing.contains(n));
            let mut taken = existing;
            taken.extend(
                self.pending_names
                    .iter()
                    .filter(|(c, _)| *c == core)
                    .map(|(_, n)| n.clone()),
            );
            let plan = ops::paste_plan(&clip, &ops::split_path(&target), &taken);
            specs_from(plan)
        };
        let new_names: Vec<String> = specs
            .iter()
            .filter_map(|s| {
                s.fields
                    .iter()
                    .find(|(n, _)| n == ops::STRATEGY_NAME_FIELD)
                    .map(|(_, v)| v.clone())
            })
            .collect();
        // Select the first pasted name after the core echoes it back.
        let first_name = new_names.first().cloned();
        if let Err(error) = self.backend.read(cx).session.create_strategies(core, specs) {
            log::warn!("paste strategies failed: {error}");
            return;
        }
        self.pending_names
            .extend(new_names.into_iter().map(|n| (core, n)));
        // New and pasted strategies are disabled. Clear the active-only filter and expand the
        // target core so the result remains visible.
        self.filter.only_active = false;
        self.expanded_cores.insert(core);
        self.pending_select = first_name.map(|n| (core, n));
        cx.notify();
    }

    // ── Drag & Drop ───────────────────────────────────────────────────────────

    /// Drops dragged strategies into a target folder; an empty `target` means the core root.
    /// Moves within one core through `move_strategies` and copies across cores.
    pub(super) fn drop_strategies(
        &mut self,
        target_core: CoreId,
        target: Vec<String>,
        drag: &StratDrag,
        cx: &mut Context<Self>,
    ) {
        let ids = drag.ids.clone();
        if ids.is_empty() {
            return;
        }
        if drag.core == target_core {
            let moves = {
                let store = self.backend.read(cx).session.store();
                let rows: Vec<&StrategyRow> = store
                    .core(target_core)
                    .map(|c| {
                        c.strategies
                            .iter()
                            .filter(|r| ids.contains(&r.id))
                            .collect()
                    })
                    .unwrap_or_default();
                ops::move_to(&rows, &target)
            };
            if let Err(error) = self
                .backend
                .read(cx)
                .session
                .move_strategies(target_core, moves)
            {
                log::warn!("move strategies failed: {error}");
                return;
            }
        } else {
            let specs = {
                let store = self.backend.read(cx).session.store();
                let rows: Vec<&StrategyRow> = store
                    .core(drag.core)
                    .map(|c| {
                        c.strategies
                            .iter()
                            .filter(|r| ids.contains(&r.id))
                            .collect()
                    })
                    .unwrap_or_default();
                let clip = ops::copy_rows(&rows);
                let taken: std::collections::HashSet<String> = store
                    .core(target_core)
                    .map(|c| c.strategies.iter().map(|r| r.name.clone()).collect())
                    .unwrap_or_default();
                specs_from(ops::paste_plan(&clip, &target, &taken))
            };
            if let Err(error) = self
                .backend
                .read(cx)
                .session
                .create_strategies(target_core, specs)
            {
                log::warn!("copy strategies failed: {error}");
                return;
            }
            self.filter.only_active = false;
        }
        self.expanded_cores.insert(target_core);
        cx.notify();
    }

    /// Drops a dragged folder under a target parent; an empty `target` means the core root.
    /// Moves the subtree within one core and copies it across cores.
    pub(super) fn drop_folder(
        &mut self,
        target_core: CoreId,
        target: Vec<String>,
        drag: &FolderDrag,
        cx: &mut Context<Self>,
    ) {
        let path = drag.path.clone();
        if drag.core == target_core {
            let moves = {
                let store = self.backend.read(cx).session.store();
                store
                    .core(target_core)
                    .map(|c| ops::move_folder(&c.strategies, &path, &target))
                    .unwrap_or_default()
            };
            if moves.is_empty() {
                return; // Reject self/descendant targets and empty folders.
            }
            if let Err(error) = self
                .backend
                .read(cx)
                .session
                .move_strategies(target_core, moves)
            {
                log::warn!("move strategy folder failed: {error}");
                return;
            }
        } else {
            let specs = {
                let store = self.backend.read(cx).session.store();
                let clip = store
                    .core(drag.core)
                    .map(|c| ops::copy_folder(&c.strategies, &path))
                    .unwrap_or_default();
                let taken: std::collections::HashSet<String> = store
                    .core(target_core)
                    .map(|c| c.strategies.iter().map(|r| r.name.clone()).collect())
                    .unwrap_or_default();
                specs_from(ops::paste_plan(&clip, &target, &taken))
            };
            if let Err(error) = self
                .backend
                .read(cx)
                .session
                .create_strategies(target_core, specs)
            {
                log::warn!("copy strategy folder failed: {error}");
                return;
            }
            self.filter.only_active = false;
        }
        self.expanded_cores.insert(target_core);
        cx.notify();
    }

    /// Returns drag IDs for a strategy row.
    ///
    /// If the row belongs to the selection, include the entire selection from its core; otherwise
    /// include only that row.
    pub(super) fn drag_ids_for(&self, core: CoreId, id: u64) -> Vec<u64> {
        if self.sel.contains(&(core, id)) {
            self.sel
                .iter()
                .filter(|(c, _)| *c == core)
                .map(|(_, i)| *i)
                .collect()
        } else {
            vec![id]
        }
    }
}

/// Converts a paste/create plan into core command specifications.
fn specs_from(plan: Vec<ops::NewStrategy>) -> Vec<NewStrategySpec> {
    plan.into_iter()
        .map(|n| NewStrategySpec {
            kind_ordinal: n.kind_ordinal,
            folder_path: n.folder_path,
            fields: n.fields,
        })
        .collect()
}
