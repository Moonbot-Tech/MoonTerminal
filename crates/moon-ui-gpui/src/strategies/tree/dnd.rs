//! Clipboard and drag-and-drop operations for the strategy tree. Dropping within one core moves
//! strategies through `move_strategies`; dropping across cores copies them through
//! `create_strategies`. [`super::ops`] owns the pure path and collection logic.

use super::super::actions::strategy_action_authorized;
use super::super::*;
use super::ops;
use super::ui::{FolderDrag, StratDrag};
use moon_core::feed::NewStrategySpec;

#[cfg(test)]
mod tests;

impl StrategiesView {
    // ── Clipboard: copy and paste ────────────────────────────────────────────

    /// Copy the selected strategies and retain each row's core-qualified placement anchor.
    pub(super) fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let store = self.backend.read(cx).session.store();
        let rows = self.selection_rows(store);
        if rows.is_empty() {
            return;
        }
        // Each row keeps its own `(core, id)` so a paste back onto that core lands beside it.
        let refs: Vec<(CoreId, &StrategyRow)> = rows.iter().map(|(c, r)| (*c, r)).collect();
        self.set_clipboard(ops::copy_rows(&refs), cx);
        cx.notify();
    }

    /// Copy one folder only while its captured core remains workspace-visible.
    ///
    /// Args:
    ///     core: Source core captured by the tree action.
    ///     path: Canonical source folder segments.
    ///     cx: View context used to read strategies and update clipboards.
    ///
    /// Returns:
    ///     Nothing; a target outside the current scope leaves the retained clipboard untouched.
    pub(super) fn copy_folder(&mut self, core: CoreId, path: Vec<String>, cx: &mut Context<Self>) {
        if !action_cores_visible(self.workspace_cores.as_deref(), [core]) {
            return;
        }
        let clip = {
            let store = self.backend.read(cx).session.store();
            let Some(cd) = store.core(core) else { return };
            ops::copy_folder(&cd.strategies, &path)
        };
        self.set_clipboard(clip, cx);
        cx.notify();
    }

    /// Copy every selected folder and core root, each relative to its own parent.
    ///
    /// So pasting two selected folders recreates BOTH by name. A folder nested inside another
    /// selected one contributes nothing of its own; `ops::copy_folders` owns that rule.
    ///
    /// Args:
    ///     folders: Selected folders and core roots, ordered as the tree draws them.
    ///     cx: View context used to read the visible cores and replace the clipboard.
    ///
    /// Returns:
    ///     Nothing; an empty selection or one outside the active workspace leaves the clipboard unchanged.
    pub(super) fn copy_folders(&mut self, folders: &[(CoreId, String)], cx: &mut Context<Self>) {
        let cores: Vec<CoreId> = folders.iter().map(|(core, _)| *core).collect();
        if !action_cores_visible(self.workspace_cores.as_deref(), cores) {
            return;
        }
        let clip = {
            let store = self.backend.read(cx).session.store();
            let owned: Vec<(CoreId, Vec<StrategyRow>)> = folders
                .iter()
                .map(|(core, _)| *core)
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .filter_map(|core| store.core(core).map(|cd| (core, cd.strategies.clone())))
                .collect();
            let borrowed: Vec<(CoreId, &[StrategyRow])> = owned
                .iter()
                .map(|(core, rows)| (*core, rows.as_slice()))
                .collect();
            let paths: Vec<(CoreId, Vec<String>)> = folders
                .iter()
                .map(|(core, path)| (*core, ops::split_path(path)))
                .collect();
            ops::copy_folders(&borrowed, &paths)
        };
        if clip.is_empty() {
            // Every selected folder is empty. Returning quietly would leave whatever was copied
            // BEFORE still armed, so the next Ctrl+V would paste unrelated strategies the operator
            // never selected — a wrong-data paste dressed as a working one.
            self.clipboard = None;
            self.cut = None;
            self.pending_notes.push(tree::ui::TreeNote::NothingToCopy);
            cx.notify();
            return;
        }
        self.set_clipboard(clip, cx);
        cx.notify();
    }

    /// Mark the selected strategies for a move.
    ///
    /// The clipboard is written exactly as a copy writes it - a cut that is never pasted has to
    /// behave like a copy, and the marks are what turn the next paste into a move.
    ///
    /// Args:
    ///     cx: View context used to read the selected rows and update the clipboard.
    ///
    /// Returns:
    ///     The notice naming the pending cut, or `None` when no strategy is selected.
    pub(super) fn cut_selection(&mut self, cx: &mut Context<Self>) -> Option<tree::ui::TreeNote> {
        let rows = {
            let store = self.backend.read(cx).session.store();
            self.selection_rows(store)
        };
        if rows.is_empty() {
            return None;
        }
        let marks: Vec<(CoreId, u64)> = rows.iter().map(|(core, r)| (*core, r.id)).collect();
        self.copy_selection(cx);
        let marked = marks.len();
        self.cut = Some(ops::CutOrigin {
            rows: marks,
            folders: Vec::new(),
        });
        cx.notify();
        Some(tree::ui::TreeNote::Cut { marked })
    }

    /// Mark the selected folders for a move.
    ///
    /// Args:
    ///     folders: Selected folders and core roots addressed by the tree.
    ///     cx: View context used to copy their contents and store the cut marks.
    ///
    /// Returns:
    ///     The notice naming the pending cut, or `None` when no movable folder is selected.
    pub(super) fn cut_folders(
        &mut self,
        folders: &[(CoreId, String)],
        cx: &mut Context<Self>,
    ) -> Option<tree::ui::TreeNote> {
        // A core root has no parent to move into, so cutting one means nothing. Refused with a
        // reason rather than silently dropped from the set.
        if folders.iter().any(|(_, path)| path.is_empty()) {
            return Some(tree::ui::TreeNote::CoreNotCut);
        }
        self.copy_folders(folders, cx);
        if self.clipboard.is_none() {
            return None;
        }
        let marks: Vec<(CoreId, Vec<String>)> = folders
            .iter()
            .map(|(core, path)| (*core, ops::split_path(path)))
            .collect();
        let marked = marks.len();
        self.cut = Some(ops::CutOrigin {
            rows: Vec::new(),
            folders: marks,
        });
        cx.notify();
        Some(tree::ui::TreeNote::Cut { marked })
    }

    /// Stores an internal clipboard and a textual copy in the system clipboard.
    ///
    /// A strategy or folder can therefore be pasted into a text editor, shared, and accepted back
    /// by `paste_into`.
    fn set_clipboard(&mut self, clip: Vec<ops::ClipItem>, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(ops::clip_to_text(&clip)));
        self.clipboard = Some(clip);
        // A COPY retires any pending cut, here rather than at each caller: the clipboard is what a
        // paste reads, so a cut left standing beside a newer copy would move rows the operator has
        // stopped pointing at.
        self.cut = None;
    }

    /// Paste into a target only while its captured core remains workspace-visible.
    ///
    /// Args:
    ///     core: Destination core resolved from the effective tree selection.
    ///     target: Canonical destination folder path, or empty for the core root.
    ///     cx: View context used to read the clipboard and dispatch creation.
    ///
    /// Returns:
    ///     How many strategies were dispatched to the core; 0 when nothing was pasted, which a
    ///     target outside the current scope and an empty clipboard both produce. The COUNT is what
    ///     lets a fan-out over several cores report one honest figure instead of guessing.
    pub(super) fn paste_into(
        &mut self,
        core: CoreId,
        target: String,
        cx: &mut Context<Self>,
    ) -> usize {
        if !action_cores_visible(self.workspace_cores.as_deref(), [core]) {
            return 0;
        }
        // A pending CUT is a move, not a create, and it takes precedence over the clipboard the
        // cut itself wrote.
        if self.cut.is_some() {
            return self.paste_cut(core, &target, cx);
        }
        // Prefer the internal clipboard; when empty, parse the system clipboard's `clip_to_text`
        // format so strategies or folders shared as text can be pasted.
        let clip = self.clipboard.clone().or_else(|| {
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .and_then(|t| ops::clip_from_text(&t))
        });
        let Some(clip) = clip else {
            return 0;
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
        let landed = specs.len();
        if let Err(error) = self.backend.read(cx).session.create_strategies(core, specs) {
            log::warn!("paste strategies failed: {error}");
            return 0;
        }
        self.pending_names
            .extend(new_names.into_iter().map(|n| (core, n)));
        // New and pasted strategies are disabled. Expand the target core so the result is visible.
        self.expanded_cores.insert(core);
        if let Some(name) = first_name {
            self.queue_pending_name(core, name, cx);
        }
        self.persist_session(cx);
        cx.notify();
        landed
    }

    /// Consume a pending cut into `target`: a move within the core, a create-then-retire across.
    ///
    /// Within one core the rows keep their ids, which is the whole difference between a cut and a
    /// copy - versions and order profit are joined to a strategy through its id, so a "move" that
    /// created new rows would silently detach both.
    ///
    /// Across cores nothing can move, because ids are per core. The copies go out first and the
    /// source is retired only once the destination echoes their names back
    /// (`reconcile_cut_followups`); until then the rows exist in both places, which is the only
    /// failure direction that loses nothing.
    ///
    /// Returns:
    ///     How many strategies were dispatched, for the caller's notice.
    fn paste_cut(&mut self, core: CoreId, target: &str, cx: &mut Context<Self>) -> usize {
        let Some(cut) = self.cut.clone() else {
            return 0;
        };
        let segments = ops::split_path(target);
        let generation = self.action_workspace_generation(cx);

        // Planned inside one store borrow, dispatched outside it.
        let (moves, carries) = {
            let store = self.backend.read(cx).session.store();
            let here = store
                .core(core)
                .map(|cd| ops::cut_move_plan(&cd.strategies, &cut, core, &segments))
                .unwrap_or_default();
            let owned: Vec<(CoreId, Vec<StrategyRow>)> = cut
                .cores()
                .into_iter()
                .filter_map(|c| store.core(c).map(|cd| (c, cd.strategies.clone())))
                .collect();
            let borrowed: Vec<(CoreId, &[StrategyRow])> = owned
                .iter()
                .map(|(c, rows)| (*c, rows.as_slice()))
                .collect();
            (here, ops::cut_carry_plan(&borrowed, &cut, core))
        };

        let mut moved = 0usize;
        for intent in moves {
            moved += intent.moves.len();
            if let Err(error) =
                self.backend
                    .read(cx)
                    .session
                    .move_strategies(core, intent.moves, intent.rebase)
            {
                log::warn!("cut move failed: {error}");
            }
        }

        let mut carried = 0usize;
        for carry in carries {
            if !action_cores_visible(self.workspace_cores.as_deref(), [carry.src, core]) {
                continue;
            }
            let specs = {
                let store = self.backend.read(cx).session.store();
                let existing: std::collections::HashSet<String> = store
                    .core(core)
                    .map(|cd| cd.strategies.iter().map(|r| r.name.clone()).collect())
                    .unwrap_or_default();
                self.pending_names
                    .retain(|(c, n)| *c != core || !existing.contains(n));
                let mut taken = existing;
                taken.extend(
                    self.pending_names
                        .iter()
                        .filter(|(c, _)| *c == core)
                        .map(|(_, n)| n.clone()),
                );
                specs_from(ops::paste_plan(&carry.clip, &segments, &taken))
            };
            let names: Vec<String> = specs
                .iter()
                .filter_map(|spec| {
                    spec.fields
                        .iter()
                        .find(|(n, _)| n == ops::STRATEGY_NAME_FIELD)
                        .map(|(_, v)| v.clone())
                })
                .collect();
            carried += specs.len();
            if let Err(error) = self.backend.read(cx).session.create_strategies(core, specs) {
                log::warn!("cut copy failed: {error}");
                continue;
            }
            self.pending_names
                .extend(names.iter().map(|n| (core, n.clone())));
            // The source is NOT touched here. It waits for these exact names to come back.
            self.cut_followups.push(tree::ui::CutFollowUp {
                dst: core,
                names,
                src: carry.src,
                rows: carry.rows,
                folders: carry.folders,
                workspace_generation: generation,
                sent: std::time::Instant::now(),
            });
        }

        self.cut = None;
        self.expanded_cores.insert(core);
        self.persist_session(cx);
        cx.notify();
        if carried > 0 {
            self.pending_notes.push(tree::ui::TreeNote::CutWaiting);
        }
        moved + carried
    }

    /// Retire the source of every cross-core cut the destination has now confirmed.
    ///
    /// Called from the store observer and from render, so a core that goes quiet still lets the
    /// give-up window expire. Returns whether anything changed.
    pub(in crate::strategies) fn reconcile_cut_followups(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.cut_followups.is_empty() {
            return false;
        }
        let mut settled: Vec<usize> = Vec::new();
        let mut notes: Vec<tree::ui::TreeNote> = Vec::new();
        let mut plans: Vec<(CoreId, ops::CutRetire)> = Vec::new();

        for (at, follow) in self.cut_followups.iter().enumerate() {
            let echoed = {
                let store = self.backend.read(cx).session.store();
                store
                    .core(follow.dst)
                    .is_some_and(|cd| ops::names_echoed(&cd.strategies, &follow.names))
            };
            if !echoed {
                if follow.sent.elapsed() >= tree::ui::CUT_ECHO_WINDOW {
                    // Nothing is deleted. The copies stand and the source is untouched.
                    settled.push(at);
                    notes.push(tree::ui::TreeNote::CutTimedOut);
                }
                continue;
            }
            // The workspace may have moved under us between dispatch and echo.
            let targets: Vec<Key> = follow
                .rows
                .iter()
                .map(|carried| (follow.src, carried.id))
                .collect();
            if !strategy_action_authorized(
                follow.workspace_generation,
                self.action_workspace_generation(cx),
                self.workspace_cores.as_deref(),
                &targets,
            ) {
                // The destination DID confirm; only the workspace authority moved underneath.
                // Reporting a timeout here would tell the operator the opposite of what happened.
                settled.push(at);
                notes.push(tree::ui::TreeNote::CutScopeMoved);
                continue;
            }
            let plan = {
                let store = self.backend.read(cx).session.store();
                store
                    .core(follow.src)
                    .map(|cd| ops::cut_retire_plan(&cd.strategies, &follow.rows, &follow.folders))
            };
            let Some(plan) = plan else {
                // The source core has left the store entirely, so nothing can be retired there.
                // Said out loud: the rows now exist on both cores and only the operator can
                // reconcile that.
                settled.push(at);
                notes.push(tree::ui::TreeNote::CutSourceGone);
                continue;
            };
            settled.push(at);
            plans.push((follow.src, plan));
        }

        if settled.is_empty() {
            return false;
        }
        for (src, plan) in plans {
            let mut moved = 0usize;
            let mut failed = 0usize;
            let mut cleared: Vec<Vec<String>> = Vec::new();
            {
                let session = &self.backend.read(cx).session;
                for id in &plan.delete_rows {
                    match session.delete_strategy(src, *id) {
                        Ok(()) => moved += 1,
                        Err(error) => {
                            failed += 1;
                            log::warn!("cut source delete failed: {error}");
                        }
                    }
                }
                for folder in &plan.empty_folders {
                    // `delete_folder` removes whatever the folder CURRENTLY holds, so it is not
                    // safe here: a row added after the copy was never carried. The rows above are
                    // already going individually, so what is left is the empty shell.
                    match session.delete_empty_folder(src, ops::join_path(folder), Vec::new()) {
                        Ok(()) => cleared.push(folder.clone()),
                        Err(error) => log::warn!("cut source folder cleanup failed: {error}"),
                    }
                }
            }
            // Local bookkeeping follows what actually succeeded, never what was attempted.
            for folder in &cleared {
                self.remove_ui_folder(src, folder);
            }
            notes.push(match failed {
                0 => tree::ui::TreeNote::CutDone {
                    moved,
                    kept_enabled: plan.kept_enabled,
                    kept_changed: plan.kept_changed,
                },
                // Partly retired. Reported as its own outcome rather than as a success with a
                // small number, because the rows that stayed are now duplicated across two cores
                // and only the operator can decide what to do about it.
                failed => tree::ui::TreeNote::CutRetireFailed { moved, failed },
            });
        }
        for at in settled.into_iter().rev() {
            self.cut_followups.remove(at);
        }
        self.pending_notes.extend(notes);
        cx.notify();
        true
    }

    /// Paste the clipboard into the root of EVERY core the tree currently shows.
    ///
    /// One `create_strategies` per core, each planned against that core's OWN taken-name set, so
    /// two cores already holding a "Grid" each end up with their own "(2) Grid" rather than one
    /// name uniquified against the other's list.
    ///
    /// Args:
    ///     window: Window used to report what happened, in one notice for the whole fan-out.
    ///     cx: View context used to resolve the visible cores and dispatch each paste.
    ///
    /// Returns:
    ///     Nothing; the final notice reports whether any visible core accepted a paste.
    pub(super) fn paste_into_all_visible(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cores: Vec<CoreId> = {
            let backend = self.backend.read(cx);
            visible_strategy_cores(self, backend)
                .iter()
                .map(|(core, _)| *core)
                .collect()
        };
        let mut strategies = 0usize;
        let mut reached = 0usize;
        for core in cores {
            let landed = self.paste_into(core, String::new(), cx);
            if landed > 0 {
                strategies += landed;
                reached += 1;
            }
        }
        let note = match strategies {
            0 => tree::ui::TreeNote::NothingToPaste,
            _ => tree::ui::TreeNote::Pasted {
                strategies,
                cores: reached,
            },
        };
        note.say(window, cx);
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
        if !action_cores_visible(self.workspace_cores.as_deref(), [drag.core, target_core]) {
            return;
        }
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
            // No folder tree: dragging strategies OUT of a folder must not delete that folder. The
            // core keeps an emptied folder now, which is the behaviour to preserve — a tree omitting
            // it would take it away as a side effect of moving rows.
            if let Err(error) =
                self.backend
                    .read(cx)
                    .session
                    .move_strategies(target_core, moves, None)
            {
                log::warn!("move strategies failed: {error}");
                return;
            }
        } else {
            let specs = {
                let store = self.backend.read(cx).session.store();
                let rows: Vec<(CoreId, &StrategyRow)> = store
                    .core(drag.core)
                    .map(|c| {
                        c.strategies
                            .iter()
                            .filter(|r| ids.contains(&r.id))
                            .map(|r| (drag.core, r))
                            .collect()
                    })
                    .unwrap_or_default();
                let clip = ops::copy_rows(&rows);
                let taken: std::collections::HashSet<String> = store
                    .core(target_core)
                    .map(|c| c.strategies.iter().map(|r| r.name.clone()).collect())
                    .unwrap_or_default();
                // This branch is the CROSS-core drop; the anchors it carries name the SOURCE
                // core, so the feed's drain discards them and the copies append.
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
        }
        self.expanded_cores.insert(target_core);
        self.persist_session(cx);
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
        if !action_cores_visible(self.workspace_cores.as_deref(), [drag.core, target_core]) {
            return;
        }
        let path = drag.path.clone();
        if drag.core == target_core {
            let mut moved_to = target.clone();
            moved_to.extend(path.last().cloned());
            let moves = {
                let store = self.backend.read(cx).session.store();
                let Some(cd) = store.core(target_core) else {
                    return;
                };
                ops::move_folder(&cd.strategies, &path, &target)
            };
            // Rejects a drop onto the folder itself or into its own subtree, where source and
            // destination are the same place. An EMPTY folder is no longer rejected here: it has no
            // rows to move, and its relocation travels as the subtree intent alone.
            if moves.is_empty() && (moved_to == path || target.starts_with(&path)) {
                return;
            }
            // Same pairing as a rename: without the subtree the folder's old path survives on the
            // core as an empty folder, now that empty folders are something it can hold.
            if let Err(error) = self.backend.read(cx).session.move_strategies(
                target_core,
                moves,
                Some((ops::join_path(&path), ops::join_path(&moved_to))),
            ) {
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
        }
        self.expanded_cores.insert(target_core);
        self.persist_session(cx);
        cx.notify();
    }

    /// Returns the ids of every selected strategy in one core, the payload its selected rows drag.
    ///
    /// Building this shared payload once per core keeps large multi-selections linear in the
    /// number of selected strategies.
    pub(super) fn drag_ids_for_core(&self, core: CoreId) -> std::rc::Rc<[u64]> {
        self.sel
            .iter()
            .filter(|(c, _)| *c == core)
            .map(|(_, i)| *i)
            .collect()
    }
}

/// Validate every source and target core immediately before a clipboard or drag action.
///
/// Args:
///     workspace: Concrete scoped ids from Auto or Classic membership, or `None` when unscoped.
///     cores: Source and target cores used by the pending action.
///
/// Returns:
///     `true` only when every action core remains visible at dispatch time.
fn action_cores_visible(
    workspace: Option<&[CoreId]>,
    cores: impl IntoIterator<Item = CoreId>,
) -> bool {
    cores
        .into_iter()
        .all(|core| strategy_core_is_visible(workspace, core))
}

/// Converts a paste/create plan into core command specifications.
fn specs_from(plan: Vec<ops::NewStrategy>) -> Vec<NewStrategySpec> {
    plan.into_iter().map(NewStrategySpec::from).collect()
}
