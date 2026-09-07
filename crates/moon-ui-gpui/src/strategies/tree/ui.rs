//! Strategy-tree operation types for modals, menus, and DnD payloads, plus shared selection,
//! UI-folder, and toolbar helpers. Modals live in [`super::dialogs`], clipboard and DnD in
//! [`super::dnd`], context menus in [`super::menu`], and pure path and collection logic
//! in [`super::ops`].

use std::cell::Cell;
use std::rc::Rc;

use super::super::*;
use super::ops;
use moon_ui::{MoonButtonIconSlot, MoonNotification};
use rust_i18n::t;

#[cfg(test)]
mod tests;

/// Return whether one full set of footer labels fits beside its fixed Action-button chrome.
///
/// Args:
///     available_width: Rendered width offered by the Strategies tree pane.
///     fixed_width: Buttons, icons, padding, gaps, and divider that never disappear.
///     measured_label_width: Current localized button and staged-label glyph widths.
///
/// Returns:
///     `true` at and above the inclusive full-label boundary.
pub(super) fn footer_labels_fit(
    available_width: f32,
    fixed_width: f32,
    measured_label_width: f32,
) -> bool {
    available_width >= fixed_width + measured_label_width
}

/// Return whether a keystroke is the reorder chord, Ctrl+Shift with an up or down arrow.
///
/// Compared against the whole modifier set rather than testing the two that must be down, and that
/// is load bearing for one of them: `alt-up`/`alt-down` ship as the bindings for "shift the sell
/// order's price" (`moon_core::config::hotkeys`), and one chord that rearranges a list in one
/// window and moves a live order's price in another is a trap regardless of which window has focus
/// today.
///
/// Args:
///     modifiers: Modifier state of the keystroke.
///     key: Key name from the keystroke.
///
/// Returns:
///     `true` for exactly Ctrl+Shift plus an arrow.
fn reorder_chord(modifiers: &Modifiers, key: &str) -> bool {
    *modifiers == Modifiers::control_shift() && matches!(key, "up" | "down")
}

/// Where a paste — or a Create — should land, given the tree's two kinds of selection.
///
/// The folder outranks the strategy, and that is structural rather than a preference: making a
/// strategy the primary selection goes through `selection::focus_strategy` (or
/// `selection::apply_click` for a modified click), both of which clear `selected_folder`, while a
/// folder or core click sets its exact subtree target. So a value here means that subtree was the
/// last thing the user pointed at — which is exactly when "paste it in there" is the answer.
///
/// Both halves of the answer come from one source so the target path and core cannot describe
/// different selections.
///
/// Args:
///     folder: `(core, folder path)` of the clicked folder, when one is selected.
///     from_row: `(core, folder path)` of the primary selected strategy, when it resolves.
///     first_core: First core in canonical order — the last-resort fallback.
///
/// Returns:
///     The `(core, folder path)` to paste or create into; an empty path means the core root.
pub(super) fn resolve_paste_target(
    folder: Option<(CoreId, String)>,
    from_row: Option<(CoreId, String)>,
    first_core: Option<CoreId>,
) -> (CoreId, String) {
    folder
        .or(from_row)
        .unwrap_or_else(|| (first_core.unwrap_or(0), String::new()))
}

/// Return whether a UI-created folder should remain locally owned after a strategy snapshot.
///
/// Args:
///     path: Canonical UI-folder path retained by [`StrategiesView`].
///     rows: Current live strategies for the same core, or `None` while that core is absent.
///
/// Returns:
///     `false` once a live row represents the folder or any of its descendants.
fn keep_ui_folder(path: &str, rows: Option<&[StrategyRow]>) -> bool {
    let prefix = ops::split_path(path);
    rows.is_none_or(|rows| prefix.is_empty() || !ops::has_row_under(rows, &prefix))
}

/// Active mutually exclusive operation modal rendered over the window.
#[derive(Clone)]
pub(crate) enum TreeOp {
    /// Create a strategy in a target folder using the selected kind ordinal.
    CreateStrategy {
        core: CoreId,
        target: String,
        kind: Option<u8>,
        workspace_generation: Option<u64>,
    },
    /// Create a UI-only folder under the target parent.
    CreateFolder {
        core: CoreId,
        target: String,
        workspace_generation: Option<u64>,
    },
    /// Rename ONE strategy through a core-confirmed edit of its `StrategyName` field.
    RenameStrategy {
        core: CoreId,
        id: u64,
        workspace_generation: Option<u64>,
    },
    /// Choose a destination folder for the strategies or folders being moved.
    MoveToFolder {
        core: CoreId,
        sources: ops::MoveSources,
        workspace_generation: Option<u64>,
    },
    /// Rename a folder identified by its core and path segments.
    RenameFolder {
        core: CoreId,
        old_path: Vec<String>,
        workspace_generation: Option<u64>,
    },
    /// Confirm deletion of the exact selected strategies captured when the dialog opened.
    ConfirmDeleteStrategies {
        label: String,
        targets: Vec<Key>,
        workspace_generation: Option<u64>,
    },
    /// Confirm an IRREVERSIBLE purge of deleted strategies and their whole version history.
    ///
    /// The one new confirmation in this window, and it earns it: every other action here either
    /// asks the core, which can be undone, or moves rows about. This one destroys local history
    /// that nothing else keeps a copy of.
    ConfirmForget {
        core: CoreId,
        ids: Vec<u64>,
        label: String,
    },
    /// Confirm folder deletion using its core, path, and display label.
    ConfirmDeleteFolder {
        core: CoreId,
        path: Vec<String>,
        label: String,
        targets: Vec<(u64, bool)>,
        workspace_generation: Option<u64>,
    },
}

/// A cross-core cut waiting for the destination to confirm what it received.
///
/// Ids are per core, so a strategy cannot actually move between cores: it is CREATED at the
/// destination and only then retired at the source. This record is what holds the two halves
/// together across the round trip, and it is deliberately process-local - a cut in flight is a
/// gesture, not state worth surviving a window close. If the window closes first the destination
/// keeps its copy and the source keeps its rows, which is the safe direction to fail in.
pub(crate) struct CutFollowUp {
    /// Core the copies were sent to.
    pub(crate) dst: CoreId,
    /// Names the destination must echo back before anything is retired.
    pub(crate) names: Vec<String>,
    /// Core the rows came from.
    pub(crate) src: CoreId,
    /// EVERY row carried across, as it stood when it was copied. The identity travels, not just
    /// the id, so retirement can refuse to delete a row the operator edited in the meantime.
    pub(crate) rows: Vec<ops::CarriedRow>,
    /// Folders carried across whole.
    pub(crate) folders: Vec<Vec<String>>,
    /// Workspace authority captured when the paste was dispatched.
    pub(crate) workspace_generation: Option<u64>,
    /// When the copies went out, for the give-up window.
    pub(crate) sent: std::time::Instant,
}

/// How long a cross-core cut waits for its echo before giving up and keeping the source.
///
/// Giving up NEVER deletes anything: the destination keeps its copy, the source keeps its rows,
/// and the operator is told. A cut that silently became a copy is recoverable; a source deleted
/// against an echo that never came is not.
pub(crate) const CUT_ECHO_WINDOW: std::time::Duration = std::time::Duration::from_secs(45);

/// Context-menu request containing its target and cursor position; MoonUI Root owns the open menu.
pub(super) struct ContextMenu {
    pub(super) core: CoreId,
    pub(super) target: MenuTarget,
    pub(super) pos: Point<Pixels>,
}

/// Cloned into the right-click closure, which is `Fn` and may run more than once.
#[derive(Clone)]
pub(super) enum MenuTarget {
    /// A core root row.
    Core,
    Folder(Vec<String>),
    Strategy(u64),
    /// A core's Deleted heading, offering the bulk Forget.
    DeletedFolder,
    /// Server-deleted strategy from the Deleted folder, offering Restore and Forget.
    DeletedStrategy(u64),
}

/// One thing the window has to SAY about an action it just took, or refused to take.
///
/// A typed value rather than a formatted string, so what an action reported can be asserted
/// without matching prose, and so the wording lives in one place next to the dictionary key. Every
/// action in this window ends in exactly one of these — answering with silence is the defect this
/// whole family exists to remove.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TreeNote {
    /// A delete was refused because the core still runs some of the targets.
    DeleteBlocked { enabled: usize, total: usize },
    /// A core row cannot be deleted from the tree at all.
    CoreNotDeletable,
    /// Several folders are selected and deletion takes one at a time.
    DeleteNeedsOneFolder { selected: usize },
    /// A rename was requested with a selection that is not exactly one strategy.
    RenameNeedsOne { selected: usize },
    /// A core is renamed in Settings, not from this tree.
    CoreRenamedInSettings,
    /// A rename was sent to the core and awaits its confirmation.
    RenameSent { old: String, new: String },
    /// Rows are marked for a move and will travel on the next paste.
    Cut { marked: usize },
    /// A pending cut was abandoned.
    CutCancelled,
    /// A same-core paste moved rows rather than copying them.
    Moved { strategies: usize },
    /// Copies reached another core; the source waits for that core to confirm them.
    CutWaiting,
    /// A cross-core move finished. Rows stay behind when the core still runs them, or when they
    /// were edited after the copy and the destination therefore holds a stale version.
    CutDone {
        moved: usize,
        kept_enabled: usize,
        kept_changed: usize,
    },
    /// Some source rows could not be retired, so they are now duplicated across both cores.
    CutRetireFailed { moved: usize, failed: usize },
    /// The source core disappeared before its rows could be retired.
    CutSourceGone,
    /// The destination confirmed, but the workspace scope moved before the source could be retired.
    CutScopeMoved,
    /// Every selected folder was empty, so there was nothing to put on the clipboard.
    NothingToCopy,
    /// The destination never confirmed, so the source was left exactly as it was.
    CutTimedOut,
    /// A core root cannot be cut - there is nothing above it to move it into.
    CoreNotCut,
    /// A move was dispatched to the core.
    MoveSent { strategies: usize, folder: String },
    /// Move-to-folder works within ONE core; the selection spans several.
    MoveNeedsOneCore,
    /// A purge finished and destroyed this much.
    Forgotten { strategies: usize, versions: usize },
    /// The strategy store is switched off, so there is nothing to purge from.
    ForgetDisabled,
    /// The purge was queued but has not answered yet.
    ///
    /// Deliberately NOT reported as a failure: the writer's queue is serial, so a purge the
    /// window stopped waiting for may still be pending, and telling the operator that an
    /// irreversible action failed when it is about to succeed is the worst answer available.
    ForgetPending,
    /// The purge was refused or errored outright.
    ForgetFailed,
    /// A paste landed.
    Pasted { strategies: usize, cores: usize },
    /// There was nothing on either clipboard to paste.
    NothingToPaste,
}

impl TreeNote {
    /// The notification to push, with its wording resolved from the active dictionary.
    pub(crate) fn notification(&self) -> MoonNotification {
        match self {
            Self::DeleteBlocked { enabled, total } => MoonNotification::warning(
                t!(
                    "strat.note_delete_blocked",
                    enabled = enabled,
                    total = total
                )
                .to_string(),
            ),
            Self::CoreNotDeletable => {
                MoonNotification::warning(t!("strat.note_core_delete").to_string())
            }
            Self::DeleteNeedsOneFolder { selected } => MoonNotification::warning(
                t!("strat.note_delete_one_folder", n = selected).to_string(),
            ),
            Self::RenameNeedsOne { selected } => {
                MoonNotification::warning(t!("strat.note_rename_one", n = selected).to_string())
            }
            Self::CoreRenamedInSettings => {
                MoonNotification::info(t!("strat.note_core_rename").to_string())
            }
            Self::RenameSent { old, new } => MoonNotification::success(
                t!("strat.note_rename_sent", old = old, new = new).to_string(),
            ),
            Self::Forgotten {
                strategies,
                versions,
            } => MoonNotification::success(
                t!("strat.note_forgotten", n = strategies, versions = versions).to_string(),
            ),
            Self::ForgetDisabled => {
                MoonNotification::warning(t!("strat.note_forget_disabled").to_string())
            }
            Self::ForgetPending => {
                MoonNotification::info(t!("strat.note_forget_pending").to_string())
            }
            Self::ForgetFailed => {
                MoonNotification::error(t!("strat.note_forget_failed").to_string())
            }
            Self::Pasted { strategies, cores } => MoonNotification::success(
                t!("strat.note_pasted", strategies = strategies, cores = cores).to_string(),
            ),
            Self::NothingToPaste => {
                MoonNotification::info(t!("strat.note_nothing_to_paste").to_string())
            }
            Self::Cut { marked } => {
                MoonNotification::info(t!("strat.note_cut", n = marked).to_string())
            }
            Self::CutCancelled => {
                MoonNotification::info(t!("strat.note_cut_cancelled").to_string())
            }
            Self::Moved { strategies } => {
                MoonNotification::success(t!("strat.note_moved", n = strategies).to_string())
            }
            Self::CutWaiting => MoonNotification::info(t!("strat.note_cut_waiting").to_string()),
            Self::CutDone {
                moved,
                kept_enabled,
                kept_changed,
            } => match kept_enabled + kept_changed {
                0 => MoonNotification::success(t!("strat.note_moved", n = moved).to_string()),
                _ => MoonNotification::warning(
                    t!(
                        "strat.note_cut_done",
                        moved = moved,
                        kept = kept_enabled,
                        changed = kept_changed
                    )
                    .to_string(),
                ),
            },
            Self::CutRetireFailed { moved, failed } => MoonNotification::error(
                t!(
                    "strat.note_cut_retire_failed",
                    moved = moved,
                    failed = failed
                )
                .to_string(),
            ),
            Self::CutSourceGone => {
                MoonNotification::error(t!("strat.note_cut_source_gone").to_string())
            }
            Self::CutScopeMoved => {
                MoonNotification::warning(t!("strat.note_cut_scope_moved").to_string())
            }
            Self::NothingToCopy => {
                MoonNotification::info(t!("strat.note_nothing_to_copy").to_string())
            }
            Self::CutTimedOut => {
                MoonNotification::warning(t!("strat.note_cut_timeout").to_string())
            }
            Self::CoreNotCut => {
                MoonNotification::warning(t!("strat.note_core_not_cut").to_string())
            }
            Self::MoveSent { strategies, folder } => MoonNotification::success(
                t!("strat.note_move_sent", n = strategies, folder = folder).to_string(),
            ),
            Self::MoveNeedsOneCore => {
                MoonNotification::warning(t!("strat.note_move_one_core").to_string())
            }
        }
    }

    /// Push this note through a window that is already in hand.
    pub(crate) fn say(self, window: &mut Window, cx: &mut App) {
        window.push_notification(self.notification(), cx);
    }
}

/// Drag-and-drop payload for strategies, containing the source core, IDs, and originating window.
#[derive(Clone)]
pub(super) struct StratDrag {
    pub(super) core: CoreId,
    pub(super) ids: Vec<u64>,
    /// Window that started this StratDrag. Event-time cancellation compares this against the
    /// receiving window and must never pass the receiver as both arguments.
    pub(super) origin_window: WindowId,
}

/// Drag-and-drop payload for a folder, containing its source core and path.
#[derive(Clone)]
pub(super) struct FolderDrag {
    pub(super) core: CoreId,
    pub(super) path: Vec<String>,
}

/// Preview displayed beneath the cursor while dragging.
pub(super) struct DragChip {
    pub(super) label: SharedString,
    /// The tree's local text step, so the floating label matches the row it was dragged from.
    pub(super) step: f32,
    /// Window that started this drag; other windows must not paint the chip.
    pub(super) origin_window: WindowId,
    /// Live folders-and-strategies field written by `strat-tree-scroll` prepaint.
    pub(super) tree_field: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// When true, a failed paint gate also stops the process-global folder or strategy drag.
    pub(super) stop_when_outside: bool,
}

/// Return whether a Strategies tree drag preview may paint at `pointer`.
///
/// GPUI stamps `App::active_drag` onto every window at the cursor. The chip is allowed only in
/// the originating window and only while the cursor is inside the live `strat-tree-scroll`
/// rectangle. Missing bounds hide rather than leak onto the first frame before prepaint.
///
/// Args:
///     origin_window: Window that started the drag.
///     paint_window: Window currently asked to paint the overlay.
///     pointer: Cursor position in that paint window.
///     tree_field: Latest `strat-tree-scroll` bounds, or `None` before the first prepaint.
///
/// Returns:
///     `true` only when both windows match and `pointer` is strictly inside the half-open tree
///     rectangle.
pub(super) fn drag_chip_should_paint(
    origin_window: WindowId,
    paint_window: WindowId,
    pointer: Point<Pixels>,
    tree_field: Option<Bounds<Pixels>>,
) -> bool {
    origin_window == paint_window && tree_field.is_some_and(|bounds| bounds.contains(&pointer))
}

/// Return whether a StratDrag mouse-move must cancel the GPUI drag session.
///
/// An origin-window mismatch cancels independently of bounds. Same-window missing bounds defer
/// only the rectangle decision: a move can land before prepaint writes the live field. Once
/// bounds exist, leaving that rectangle cancels so the gesture cannot continue across versions,
/// sections, params, or the chart.
///
/// Args:
///     origin_window: Window that started the StratDrag.
///     event_window: Window that received this drag-move sample.
///     pointer: Cursor position for the sample.
///     tree_field: Latest `strat-tree-scroll` bounds, or `None` before the first prepaint.
///
/// Returns:
///     `true` when the production path must call `App::stop_active_drag`.
pub(super) fn strat_drag_move_should_stop(
    origin_window: WindowId,
    event_window: WindowId,
    pointer: Point<Pixels>,
    tree_field: Option<Bounds<Pixels>>,
) -> bool {
    if origin_window != event_window {
        return true;
    }
    let Some(bounds) = tree_field else {
        return false;
    };
    !bounds.contains(&pointer)
}

/// Event-time StratDrag cancel decision. Origin comes from the payload, never from the receiving
/// window twice.
///
/// Args:
///     drag: Active StratDrag whose `origin_window` is the gesture's true start window.
///     event_window: Window that received this drag-move sample.
///     pointer: Cursor position for the sample.
///     tree_field: Latest `strat-tree-scroll` bounds, or `None` before the first prepaint.
///
/// Returns:
///     `true` when the production path must call `App::stop_active_drag`.
pub(super) fn strat_drag_event_should_stop(
    drag: &StratDrag,
    event_window: WindowId,
    pointer: Point<Pixels>,
    tree_field: Option<Bounds<Pixels>>,
) -> bool {
    strat_drag_move_should_stop(drag.origin_window, event_window, pointer, tree_field)
}

impl Render for DragChip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let paint_window = window.window_handle().window_id();
        let pointer = window.mouse_position();
        let tree_field = self.tree_field.get();
        if self.stop_when_outside
            && strat_drag_move_should_stop(self.origin_window, paint_window, pointer, tree_field)
        {
            // Overlay prepaint takes `App::active_drag` before this `Render::render` runs and
            // restores it afterward (`moon-gpui` window draw), so a direct `stop_active_drag` is a
            // no-op. Defer until the current effect cycle ends, after that restore.
            window.defer(cx, |window, cx| {
                cx.stop_active_drag(window);
            });
        }
        // Both tree payloads opt into this shared paint gate, so neither preview can render
        // outside its origin tree while the process-global drag is being stopped.
        if self.stop_when_outside
            && !drag_chip_should_paint(self.origin_window, paint_window, pointer, tree_field)
        {
            return div();
        }
        let p = MoonPalette::active(cx);
        div()
            .px_2()
            .py_1()
            .rounded(design::r_button(cx))
            .bg(moon(p.shell_high))
            .border_1()
            .border_color(moon(p.blue))
            .text_color(moon(p.text))
            // Raw GPUI text, so it needs the SCALED value through `text_px`; `moon_text_base`'s
            // result must never reach `.text_size(...)` unscaled.
            .text_size(design::text_px(cx, design::moon_text_base(cx, self.step)))
            .font_family(design::mono())
            .child(self.label.clone())
    }
}

impl StrategiesView {
    // ── Utilities ─────────────────────────────────────────────────────────────

    /// Returns `(ordinal, name)` kinds from the core schema for strategy creation.
    pub(super) fn kinds_of(&self, store: &CoreStore, core: CoreId) -> Vec<(u8, String)> {
        store
            .core(core)
            .and_then(|cd| cd.schema.as_ref())
            .map(|s| {
                s.kinds
                    .iter()
                    .map(|k| (k.ordinal, k.name.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns whether anything is selected and whether every selected row is disabled.
    ///
    /// The rendering path computes these booleans directly so it does not clone the selected rows
    /// and their fields on every frame.
    pub(super) fn selection_summary(&self, store: &CoreStore) -> (bool, bool) {
        let mut any = false;
        let mut all_off = true;
        for (c, id) in selected_keys(self) {
            if let Some(r) = row(store, c, id) {
                any = true;
                all_off &= !r.checked;
            }
        }
        (any, all_off)
    }

    /// Returns owned copies of selected rows with their cores for clipboard and validation use.
    pub(super) fn selection_rows(&self, store: &CoreStore) -> Vec<(CoreId, StrategyRow)> {
        selected_keys(self)
            .into_iter()
            .filter_map(|(c, id)| row(store, c, id).map(|r| (c, r.clone())))
            .collect()
    }

    /// Returns the default `(core, path)` target for Paste and Create.
    ///
    /// Resolves the primary strategy into its folder and hands both selections to
    /// [`resolve_paste_target`], which owns the precedence.
    pub(super) fn default_target(
        &self,
        store: &CoreStore,
        cores: &crate::core_order::OrderedCores,
    ) -> (CoreId, String) {
        let from_row = selected_key(self)
            .and_then(|(core, id)| row(store, core, id).map(|r| (core, r.folder_path.clone())));
        resolve_paste_target(
            selected_folder(self),
            from_row,
            cores
                .iter()
                .find(|(core, _)| strategy_core_is_visible(self.workspace_cores.as_deref(), *core))
                .map(|(core, _)| *core),
        )
    }

    // ── UI-only folders, empty until populated ────────────────────────────────

    /// Create an empty folder: asked of the core, and marked locally either way.
    ///
    /// The local mark is not a fallback but a latency answer — a core that accepts the folder still
    /// has to echo its tree back, and an operator who just typed a name should not watch nothing
    /// happen for a round trip. `reconcile_ui_folders` drops the mark once the core reports the
    /// folder as its own; on a core that keeps no folder tree the mark is all there ever is.
    ///
    /// Only the PATH goes to the feed. The wire form is the complete desired tree — the core
    /// deletes every folder the list omits — and a tree assembled here would be assembled from a
    /// snapshot that may already be stale, turning a create into a silent delete of whatever
    /// arrived meanwhile. The feed owns the list; see `CoreCmd::AddFolder`.
    ///
    /// Args:
    ///     core: Core to create the folder on.
    ///     parent: Folder path to create it under; empty for the core root.
    ///     name: Leaf name the operator typed, already trimmed.
    ///     cx: View context used to send the command.
    ///
    /// Returns:
    ///     Nothing; a core that cannot hold the folder keeps the local mark and nothing else.
    pub(super) fn create_folder(
        &mut self,
        core: CoreId,
        parent: &str,
        name: &str,
        cx: &mut Context<Self>,
    ) {
        let mut parts = ops::split_path(parent);
        parts.push(name.to_string());
        let key = ops::join_path(&parts);

        if let Err(error) = self
            .backend
            .read(cx)
            .session
            .add_core_folder(core, key.clone())
        {
            log::warn!("create folder failed: {error}");
        }

        self.ui_folders.insert((core, key));
        // Expand the core and parent chain, excluding the new folder itself, so it is immediately
        // visible.
        self.expanded_cores.insert(core);
        let ancestors = parts.len().saturating_sub(1);
        self.expand_path(core, parts.iter().take(ancestors).map(String::as_str));
    }

    pub(super) fn remove_ui_folder(&mut self, core: CoreId, path: &[String]) {
        let key = ops::join_path(path);
        self.ui_folders
            .retain(|(c, p)| !(*c == core && (p == &key || p.starts_with(&format!("{key}/")))));
    }

    pub(super) fn rename_ui_folder(&mut self, core: CoreId, old_path: &[String], new_name: &str) {
        if old_path.is_empty() {
            return;
        }
        let mut np = old_path.to_vec();
        *np.last_mut().unwrap() = new_name.to_string();
        self.rebase_ui_folder(core, &ops::join_path(old_path), &ops::join_path(&np));
    }

    /// Move every UI-only folder at or below `old_key` to sit under `new_key` instead.
    ///
    /// A rename is one case of this and a move to another parent is the other; both rewrite the
    /// same prefix, so they share the walk rather than keeping two copies of it. Empty folders
    /// live only here until their first strategy arrives, so a move that skipped them would leave
    /// the folder behind at its old place.
    ///
    /// Args:
    ///     core: Core whose local folder markers are rebased.
    ///     old_key: Canonical prefix being replaced.
    ///     new_key: Canonical prefix that replaces it.
    ///
    /// Returns:
    ///     Nothing; an empty or unchanged source prefix has no local folders to rebase.
    pub(super) fn rebase_ui_folder(&mut self, core: CoreId, old_key: &str, new_key: &str) {
        if old_key.is_empty() || old_key == new_key {
            return;
        }
        let affected: Vec<String> = self
            .ui_folders
            .iter()
            .filter(|(c, p)| *c == core && (p == old_key || p.starts_with(&format!("{old_key}/"))))
            .map(|(_, p)| p.clone())
            .collect();
        for p in affected {
            self.ui_folders.remove(&(core, p.clone()));
            let rebased = p.replacen(old_key, new_key, 1);
            self.ui_folders.insert((core, rebased));
        }
    }

    /// Returns empty UI-only folder paths for a core, in the order the tree appends them.
    ///
    /// SORTED here rather than by the caller: these come out of a `HashSet`, whose iteration order
    /// is not stable, and the tree appends them in the order it receives them — so unsorted, two
    /// frames rendering identical data would put the same folders in different places. Ordered
    /// case-insensitively first and by the spelling itself second, because on the folded key alone
    /// two siblings differing only in case compare equal and the tie goes back to the set.
    pub(super) fn ui_folder_paths(&self, core: CoreId) -> Vec<Vec<String>> {
        let mut paths: Vec<Vec<String>> = self
            .ui_folders
            .iter()
            .filter(|(c, _)| *c == core)
            .map(|(_, p)| ops::split_path(p))
            .collect();
        paths.sort_by_cached_key(|parts| {
            let joined = parts.join("/");
            (joined.to_lowercase(), joined)
        });
        paths
    }

    /// Drop UI-only ownership once the core itself represents the folder.
    ///
    /// Two ways that happens: a strategy arrives in it, or — on a core that synchronizes folders —
    /// the core reports the folder in its own tree. Retaining the local marker past either would
    /// make the folder reappear as a ghost after another surface deleted it, which is precisely
    /// what the mark cannot be allowed to do once the core owns the answer.
    ///
    /// Args:
    ///     store: Current per-core live strategy and folder snapshots.
    pub(in crate::strategies) fn reconcile_ui_folders(&mut self, store: &CoreStore) {
        self.ui_folders.retain(|(core, path)| {
            let Some(data) = store.core(*core) else {
                // The core is gone from the store entirely; nothing can contradict the mark.
                return true;
            };
            let confirmed = data.folders.supported
                && data
                    .folders
                    .paths
                    .iter()
                    .any(|seen| seen.to_lowercase() == path.to_lowercase());
            !confirmed && keep_ui_folder(path, Some(data.strategies.as_slice()))
        });
    }

    // ── Keyboard: Ctrl+C, Ctrl+V, Ctrl+Shift+Up/Down, and Delete ──────────────

    /// Copy the last clicked visible folder/core root, otherwise the visible strategy selection.
    ///
    /// Args:
    ///     cx: View context used by the existing folder and strategy clipboard writers.
    ///
    /// Returns:
    ///     Nothing; folders take precedence, otherwise the current strategy selection is copied.
    pub(super) fn copy_tree_target(&mut self, cx: &mut Context<Self>) {
        let folders = selected_folders(self);
        if folders.is_empty() {
            self.copy_selection(cx);
        } else {
            self.copy_folders(&folders, cx);
        }
    }

    /// Where a Ctrl+V should land: every selected folder or core root, else the single default.
    ///
    /// A pending CUT collapses this to ONE destination. A cut is consumed by the paste that
    /// spends it, so fanning it over several targets would move the rows into the first and copy
    /// them into the rest - an order-dependent mixture that is neither a move nor a copy.
    ///
    /// Args:
    ///     cx: View context used to resolve the visible default core when no folder target applies.
    ///
    /// Returns:
    ///     Selected folder targets, or one visible default target for an ordinary paste or a cut.
    pub(super) fn paste_targets(&self, cx: &Context<Self>) -> Vec<(CoreId, String)> {
        let folders = selected_folders(self);
        if self.cut.is_some() || folders.is_empty() {
            let backend = self.backend.read(cx);
            let cores = visible_strategy_cores(self, backend);
            return vec![self.default_target(backend.session.store(), &cores)];
        }
        folders
    }

    /// Paste into every target the tree currently addresses, and report once.
    ///
    /// Args:
    ///     window: Window used to show the aggregate paste or move result.
    ///     cx: View context used to resolve targets and dispatch each paste.
    ///
    /// Returns:
    ///     Nothing; cross-core cuts defer their final outcome until the destination echo arrives.
    pub(super) fn paste_to_targets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let moving = self.cut.is_some();
        let targets = self.paste_targets(cx);
        let mut strategies = 0usize;
        let mut reached = 0usize;
        for (core, path) in targets {
            let landed = self.paste_into(core, path, cx);
            if landed > 0 {
                strategies += landed;
                reached += 1;
            }
        }
        let note = match (strategies, moving) {
            (0, _) => tree::ui::TreeNote::NothingToPaste,
            // A cross-core cut reports through its own follow-up, so it is not double-announced.
            (n, true) => tree::ui::TreeNote::Moved { strategies: n },
            (n, false) => tree::ui::TreeNote::Pasted {
                strategies: n,
                cores: reached,
            },
        };
        note.say(window, cx);
    }

    /// Mark whatever the tree addresses for a move: the folder set if there is one, else the
    /// strategy selection.
    ///
    /// Same precedence as Copy, so the two gestures cannot disagree about what they act on.
    ///
    /// Args:
    ///     window: Window used to show the pending-cut notice.
    ///     cx: View context used to copy and mark the current target.
    ///
    /// Returns:
    ///     Nothing; an empty target produces no cut and no notice.
    pub(super) fn cut_tree_target(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let folders = selected_folders(self);
        let note = if folders.is_empty() {
            self.cut_selection(cx)
        } else {
            self.cut_folders(&folders, cx)
        };
        if let Some(note) = note {
            note.say(window, cx);
        }
    }

    /// Dispatch tree keyboard actions only through the current workspace-visible selection.
    ///
    /// Copy, Paste, and Delete resolve effective strategies, folders, and fallback cores at the
    /// keystroke, while retained hidden Classic selection and clipboard state remain untouched.
    ///
    /// Args:
    ///     ev: Keyboard event received by the strategy tree.
    ///     window: Strategies window used for destructive confirmation dialogs.
    ///     cx: View context used to resolve current scope and dispatch the selected action.
    ///
    /// Returns:
    ///     Nothing; unsupported keys and unavailable effective targets are no-ops.
    pub(crate) fn handle_tree_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.focus.is_focused(window) {
            return;
        }
        let m = &ev.keystroke.modifiers;
        let key = ev.keystroke.key.as_str();
        if m.control && key == "c" {
            self.copy_tree_target(cx);
        } else if m.control && key == "x" {
            self.cut_tree_target(window, cx);
        } else if m.control && key == "v" {
            // Every selected folder or core root, or the single default when nothing is selected -
            // `paste_targets` owns that, and collapses a pending cut to one destination.
            self.paste_to_targets(window, cx);
        } else if reorder_chord(m, key) {
            // A HELD arrow is refused. OS auto-repeat fires this handler tens of times a second,
            // and each pass queues a whole-list reorder that the feed turns into a full snapshot to
            // the core; the repo's own keyboard path (`hotkeys::pre_dispatch`) refuses repeats for
            // the same reason. One press, one move.
            if !ev.is_held {
                let step = match key == "up" {
                    true => ops::MoveStep::Up,
                    false => ops::MoveStep::Down,
                };
                self.move_selection(step, cx);
            }
        } else if m.control && key == "a" {
            // Every VISIBLE strategy: `flat_order` is what the tree drew this frame, so a filter
            // narrows the reach of Select All exactly as it narrows the tree.
            self.sel = self.flat_order.iter().copied().collect();
            self.selected = self.flat_order.first().copied();
            self.anchor = self.selected;
            self.clear_folder_selection();
            self.clamp_selected_section(cx);
            self.persist_session(cx);
            cx.notify();
        } else if key == "f2" {
            self.rename_cursor(window, cx);
        } else if key == "escape" {
            if self.cut.take().is_some() {
                TreeNote::CutCancelled.say(window, cx);
            }
            self.sel.clear();
            self.selected = None;
            self.anchor = None;
            self.clear_folder_selection();
            self.persist_session(cx);
            cx.notify();
        } else if matches!(key, "up" | "down") {
            // Auto-repeat is allowed here, unlike the reorder chord above: navigation is local to
            // the window, so a held arrow costs nothing but a repaint, while each reorder press
            // sends the core its whole list.
            let step = match key == "up" {
                true => ops::NavStep::Up,
                false => ops::NavStep::Down,
            };
            self.move_cursor(step, m.shift, cx);
        } else if matches!(key, "left" | "right") {
            self.expand_cursor(key == "right", cx);
        } else if key == "delete" {
            self.request_delete_target(window, cx);
        }
    }

    /// The row the keyboard is currently on.
    ///
    /// The folder cursor OUTRANKS the strategy selection, the precedence `resolve_paste_target`
    /// already applies: a folder selection does not clear the strategy one, so without a fixed
    /// order the two could each claim the cursor.
    ///
    /// Returns:
    ///     The selected visible node, or `None` when neither retained selection is drawn.
    fn nav_cursor(&self) -> Option<ops::NavNode> {
        if let Some((core, path)) = selected_folder(self) {
            return Some(match path.is_empty() {
                true => ops::NavNode::Core(core),
                false => ops::NavNode::Folder(core, path),
            });
        }
        let (core, id) = selected_key(self)?;
        let live = ops::NavNode::Strategy(core, id);
        // One key identifies a live row and a deleted one alike, so which node it is has to come
        // from what the tree actually drew.
        if self.nav_order.contains(&live) {
            return Some(live);
        }
        let deleted = ops::NavNode::DeletedStrategy(core, id);
        self.nav_order.contains(&deleted).then_some(deleted)
    }

    /// Move the selection one visible row, optionally extending a strategy range.
    ///
    /// Args:
    ///     step: Direction through the currently drawn navigation order.
    ///     shift: Whether a strategy row extends the existing strategy range.
    ///     cx: View context used to update dependent selection and persisted state.
    ///
    /// Returns:
    ///     Nothing; stepping beyond the visible order leaves selection unchanged.
    fn move_cursor(&mut self, step: ops::NavStep, shift: bool, cx: &mut Context<Self>) {
        let order = self.nav_order.clone();
        let Some(next) = ops::nav_step(&order, self.nav_cursor().as_ref(), step) else {
            return;
        };
        match next {
            ops::NavNode::Strategy(core, id) => {
                let key = (core, id);
                match shift {
                    // Ranges extend over `flat_order`, the same slice a Shift-CLICK ranges over,
                    // so mouse and keyboard cannot disagree about what a range contains.
                    true => {
                        let flat = self.flat_order.clone();
                        self.apply_click(key, &flat, true, false);
                    }
                    false => self.focus_strategy(key),
                }
                self.clamp_selected_section(cx);
                self.pending_scroll = Some(key);
            }
            ops::NavNode::Core(core) => {
                self.apply_folder_click((core, String::new()), &order, shift, false);
            }
            ops::NavNode::Folder(core, path) => {
                self.apply_folder_click((core, path), &order, shift, false);
            }
            ops::NavNode::DeletedStrategy(core, id) => {
                self.select_deleted_strategy((core, id), cx);
            }
            // `nav_step` never lands on one.
            ops::NavNode::DeletedFolder(_) => return,
        }
        self.persist_session(cx);
        cx.notify();
    }

    /// Right opens the row under the cursor; Left closes it, or steps out to its parent.
    ///
    /// Args:
    ///     open: `true` for Right and `false` for Left.
    ///     cx: View context used to persist expansion or the stepped-out cursor.
    ///
    /// Returns:
    ///     Nothing; an absent cursor or an unopened row at its root is unchanged.
    fn expand_cursor(&mut self, open: bool, cx: &mut Context<Self>) {
        let Some(cursor) = self.nav_cursor() else {
            return;
        };
        let (core, path) = match &cursor {
            ops::NavNode::Core(core) => (*core, String::new()),
            ops::NavNode::Folder(core, path) => (*core, path.clone()),
            // A strategy has nothing to open: Right does nothing, Left steps out.
            _ => {
                if !open {
                    self.step_out(&cursor, cx);
                }
                return;
            }
        };
        let is_core = path.is_empty();
        let was_open = match is_core {
            true => self.expanded_cores.contains(&core) || self.rail_expanded_core == Some(core),
            false => self.expanded_folders.contains(&(core, path.clone())),
        };
        if open == was_open {
            // Already in the requested state, so Right stays put and Left steps out.
            if !open {
                self.step_out(&cursor, cx);
            }
            return;
        }
        match (is_core, open) {
            (true, _) => self.toggle_core_expanded(core),
            (false, true) => {
                self.expanded_folders.insert((core, path));
            }
            (false, false) => {
                self.expanded_folders.remove(&(core, path));
            }
        }
        self.persist_session(cx);
        cx.notify();
    }

    /// Move the cursor to whatever contains the row it is on.
    ///
    /// Args:
    ///     cursor: Current navigation node whose parent is requested.
    ///     cx: View context used to look up a strategy's live folder path and persist selection.
    ///
    /// Returns:
    ///     Nothing; nodes without a navigable parent leave selection unchanged.
    fn step_out(&mut self, cursor: &ops::NavNode, cx: &mut Context<Self>) {
        let folder_path = match cursor {
            ops::NavNode::Strategy(core, id) => {
                let store = self.backend.read(cx).session.store();
                row(store, *core, *id).map(|r| r.folder_path.clone())
            }
            _ => None,
        };
        let Some(parent) = ops::nav_parent(cursor, folder_path.as_deref()) else {
            return;
        };
        let key = match parent {
            ops::NavNode::Core(core) => (core, String::new()),
            ops::NavNode::Folder(core, path) => (core, path),
            _ => return,
        };
        let order = self.nav_order.clone();
        self.apply_folder_click(key, &order, false, false);
        self.persist_session(cx);
        cx.notify();
    }

    /// F2: rename whatever the cursor is on.
    ///
    /// Args:
    ///     window: Window used to show the rename dialog or an explanatory refusal.
    ///     cx: View context used to resolve the current target.
    ///
    /// Returns:
    ///     Nothing; a core root is directed to Settings and a non-single strategy selection is refused.
    fn rename_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((core, path)) = selected_folder(self) {
            match path.is_empty() {
                // A core's name belongs to its connection settings, not to this tree.
                true => TreeNote::CoreRenamedInSettings.say(window, cx),
                false => self.open_rename_folder(core, ops::split_path(&path), window, cx),
            }
            return;
        }
        let keys = selected_keys(self);
        match keys.len() {
            1 => {
                let (core, id) = keys[0];
                self.open_rename_strategy(core, id, window, cx);
            }
            n => TreeNote::RenameNeedsOne { selected: n }.say(window, cx),
        }
    }

    // ── Rendering: selection toolbar ──────────────────────────────────────────

    /// Build the horizontal Copy, Paste, and Delete group for the responsive footer.
    ///
    /// Args:
    ///     store: Current strategy snapshot used for action enablement.
    ///     show_labels: Shared density decision for every footer button.
    ///     has_visible_cores: Whether the caller's canonical core list has anywhere to paste into.
    ///     cx: View context used to create callbacks and scaled button widths.
    ///
    /// Returns:
    ///     One Action-size group whose callbacks reuse the canonical clipboard/delete paths.
    pub(super) fn selection_toolbar(
        &self,
        store: &CoreStore,
        show_labels: bool,
        has_visible_cores: bool,
        moves: (bool, bool),
        cx: &Context<Self>,
    ) -> AnyElement {
        let (has_sel, all_off) = self.selection_summary(store);
        let folders = selected_folders(self);
        let can_copy = has_sel || !folders.is_empty();
        // Delete resolves the SAME target the key and the menus do: the folder set outranks the
        // strategy selection. Reading only `has_sel` here both hid the button for a folder-only
        // selection AND, with a stale strategy selection beside a newer folder one, enabled it to
        // delete the strategies instead of the folder in focus.
        let can_delete = match folders.is_empty() {
            true => has_sel && all_off,
            // A core root is refused with a reason rather than greyed out, so the button stays
            // live and the refusal explains itself.
            false => true,
        };
        // Enablement takes the caller's already-resolved list; the click handler below still
        // resolves its own target, because the workspace can move between frame and click.
        let can_paste = self.clipboard.is_some() && has_visible_cores;
        let copy_label = t!("strat.action_copy").to_string();
        let paste_label = t!("strat.action_paste").to_string();
        let delete_label = t!("strat.action_delete").to_string();
        let icon_width = design::glyph_btn_w(cx);

        let move_up = self.move_button(ops::MoveStep::Up, moves.0, icon_width, cx);
        let move_down = self.move_button(ops::MoveStep::Down, moves.1, icon_width, cx);

        let mut copy = MoonButton::new("sel-copy")
            .outline()
            .size(MoonButtonSize::Action)
            .leading_icon(MoonButtonIconSlot::new("icons/copy.svg"))
            .tooltip(copy_label.clone())
            .disabled(!can_copy)
            .on_click(cx.listener(|this, _, _, cx| this.copy_tree_target(cx)));
        let mut paste = MoonButton::new("sel-paste")
            .outline()
            .size(MoonButtonSize::Action)
            .leading_icon(MoonButtonIconSlot::new("icons/inbox.svg"))
            .tooltip(paste_label.clone())
            .disabled(!can_paste)
            .on_click(cx.listener(|this, _, window, cx| this.paste_to_targets(window, cx)));
        let mut delete = MoonButton::new("sel-delete")
            .danger()
            .size(MoonButtonSize::Action)
            .leading_icon(MoonButtonIconSlot::new("icons/delete.svg"))
            .tooltip(delete_label.clone())
            .disabled(!can_delete)
            .on_click(cx.listener(|this, _, window, cx| this.request_delete_target(window, cx)));
        if show_labels {
            copy = copy.padding_x(7.0).label(copy_label);
            paste = paste.padding_x(7.0).label(paste_label);
            delete = delete.padding_x(7.0).label(delete_label);
        } else {
            copy = copy.width(icon_width);
            paste = paste.width(icon_width);
            delete = delete.width(icon_width);
        }

        let group_gap = if show_labels {
            design::CHROME_GAP
        } else {
            design::CHROME_GAP / 2.0
        };
        h_flex()
            .flex_none()
            .items_center()
            .gap(design::ui_px(cx, group_gap))
            .child(move_up.render())
            .child(move_down.render())
            .child(copy.render())
            .child(paste.render())
            .child(delete.render())
            .into_any_element()
    }

    /// Build one of the footer's two move buttons.
    ///
    /// Icon-only in BOTH densities, unlike its neighbours: the group already carries three labelled
    /// buttons at the labelled density, and two more would push it past the footer's width at the
    /// pane sizes this window is normally used at. An arrow needs the word less than "Copy" does,
    /// and the tooltip names both the action and the chord.
    ///
    /// Args:
    ///     step: Direction this button moves the selection.
    ///     enabled: Whether the cached plan says it would rearrange anything.
    ///     icon_width: Shared icon-density button width.
    ///     cx: View context used to build the click listener.
    ///
    /// Returns:
    ///     The configured button, ready to render.
    fn move_button(
        &self,
        step: ops::MoveStep,
        enabled: bool,
        icon_width: f32,
        cx: &Context<Self>,
    ) -> MoonButton {
        let (id, icon, label, chord) = match step {
            ops::MoveStep::Up => (
                "sel-move-up",
                "icons/arrow-up.svg",
                t!("strat.action_move_up"),
                t!("strat.move_up_chord"),
            ),
            ops::MoveStep::Down => (
                "sel-move-down",
                "icons/arrow-down.svg",
                t!("strat.action_move_down"),
                t!("strat.move_down_chord"),
            ),
        };
        MoonButton::new(id)
            .outline()
            .size(MoonButtonSize::Action)
            .width(icon_width)
            .leading_icon(MoonButtonIconSlot::new(icon))
            .tooltip(format!("{label} · {chord}"))
            .disabled(!enabled)
            .on_click(cx.listener(move |this, _, _, cx| this.move_selection(step, cx)))
    }

    /// Builds the Create dropdown for a strategy or folder in the tree header.
    pub(super) fn create_dropdown(
        &self,
        core: CoreId,
        target: String,
        cx: &Context<Self>,
    ) -> AnyElement {
        let view = cx.entity();
        let t1 = target.clone();
        let items = vec![
            MoonMenuItem::with_key("new-strat", t!("strat.menu_new_strategy").to_string())
                .on_click({
                    let view = view.clone();
                    move |_, window, app| {
                        let (core, t) = (core, t1.clone());
                        view.update(app, |this, c| this.open_create_strategy(core, t, window, c));
                    }
                }),
            MoonMenuItem::with_key("new-folder", t!("strat.menu_new_folder").to_string()).on_click(
                {
                    let view = view.clone();
                    let t2 = target.clone();
                    move |_, window, app| {
                        let (core, t) = (core, t2.clone());
                        view.update(app, |this, c| this.open_create_folder(core, t, window, c));
                    }
                },
            ),
        ];
        MoonDropdown::new("strat-create")
            .label(format!("＋ {}", t!("strat.menu_create")))
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .fit_trigger_width(96.0, 110.0)
            .menu_width_scaled(180.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
            .into_any_element()
    }
}
