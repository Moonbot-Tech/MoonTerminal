//! Strategy-tree operation types for modals, menus, and DnD payloads, plus shared selection,
//! UI-folder, and toolbar helpers. Modals live in [`super::dialogs`], clipboard and DnD in
//! [`super::dnd`], context menus in [`super::menu`], and pure path and collection logic
//! in [`super::ops`].

use super::super::*;
use super::ops;
use moon_ui::MoonButtonIconSlot;
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
    /// Confirm folder deletion using its core, path, and display label.
    ConfirmDeleteFolder {
        core: CoreId,
        path: Vec<String>,
        label: String,
        targets: Vec<(u64, bool)>,
        workspace_generation: Option<u64>,
    },
}

/// Context-menu request containing its target and cursor position; MoonUI Root owns the open menu.
pub(super) struct ContextMenu {
    pub(super) core: CoreId,
    pub(super) target: MenuTarget,
    pub(super) pos: Point<Pixels>,
}

pub(super) enum MenuTarget {
    Folder(Vec<String>),
    Strategy(u64),
    /// Server-deleted strategy from the Deleted folder, offering only Restore.
    DeletedStrategy(u64),
}

/// Drag-and-drop payload for strategies, containing the source core and IDs.
#[derive(Clone)]
pub(super) struct StratDrag {
    pub(super) core: CoreId,
    pub(super) ids: Vec<u64>,
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
}

impl Render for DragChip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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

    pub(super) fn add_ui_folder(&mut self, core: CoreId, parent: &str, name: &str) {
        let mut parts = ops::split_path(parent);
        parts.push(name.to_string());
        self.ui_folders.insert((core, ops::join_path(&parts)));
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
        let old_key = ops::join_path(old_path);
        let mut np = old_path.to_vec();
        *np.last_mut().unwrap() = new_name.to_string();
        let new_key = ops::join_path(&np);
        let affected: Vec<String> = self
            .ui_folders
            .iter()
            .filter(|(c, p)| *c == core && (p == &old_key || p.starts_with(&format!("{old_key}/"))))
            .map(|(_, p)| p.clone())
            .collect();
        for p in affected {
            self.ui_folders.remove(&(core, p.clone()));
            let rebased = p.replacen(&old_key, &new_key, 1);
            self.ui_folders.insert((core, rebased));
        }
    }

    /// Returns empty UI-only folder paths for a core so they can be merged into the tree.
    pub(super) fn ui_folder_paths(&self, core: CoreId) -> Vec<Vec<String>> {
        self.ui_folders
            .iter()
            .filter(|(c, _)| *c == core)
            .map(|(_, p)| ops::split_path(p))
            .collect()
    }

    /// Drop UI-only ownership once live core data represents a folder through a strategy row.
    ///
    /// Empty folders exist only in this view until first use. Retaining their local marker after
    /// a strategy arrives would make the folder reappear as a ghost if another surface later
    /// deletes that strategy and asks the core to remove the now-empty folder.
    ///
    /// Args:
    ///     store: Current per-core live strategy snapshots.
    pub(in crate::strategies) fn reconcile_ui_folders(&mut self, store: &CoreStore) {
        self.ui_folders.retain(|(core, path)| {
            keep_ui_folder(
                path,
                store.core(*core).map(|data| data.strategies.as_slice()),
            )
        });
    }

    // ── Keyboard: Ctrl+C, Ctrl+V, and Delete ──────────────────────────────────

    /// Copy the last clicked visible folder/core root, otherwise the visible strategy selection.
    ///
    /// Args:
    ///     cx: View context used by the existing folder and strategy clipboard writers.
    fn copy_tree_target(&mut self, cx: &mut Context<Self>) {
        if let Some((core, path)) = selected_folder(self) {
            self.copy_folder(core, ops::split_path(&path), cx);
        } else {
            self.copy_selection(cx);
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
        } else if m.control && key == "v" {
            let (core, target) = {
                let b = self.backend.read(cx);
                // Canonical order, so `default_target`'s "first core" means the first one the
                // user actually sees rather than whichever session happens to lead the vec.
                let cores = visible_strategy_cores(self, b);
                self.default_target(b.session.store(), &cores)
            };
            self.paste_into(core, target, cx);
        } else if key == "delete" {
            self.request_delete_selection(window, cx);
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
        cx: &Context<Self>,
    ) -> AnyElement {
        let (has_sel, all_off) = self.selection_summary(store);
        let can_copy = has_sel || selected_folder(self).is_some();
        // Enablement takes the caller's already-resolved list; the click handler below still
        // resolves its own target, because the workspace can move between frame and click.
        let can_paste = self.clipboard.is_some() && has_visible_cores;
        let copy_label = t!("strat.action_copy").to_string();
        let paste_label = t!("strat.action_paste").to_string();
        let delete_label = t!("strat.action_delete").to_string();
        let icon_width = design::glyph_btn_w(cx);

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
            .on_click(cx.listener(|this, _, _, cx| {
                let (core, target) = {
                    let backend = this.backend.read(cx);
                    let cores = visible_strategy_cores(this, backend);
                    this.default_target(backend.session.store(), &cores)
                };
                this.paste_into(core, target, cx);
            }));
        let mut delete = MoonButton::new("sel-delete")
            .danger()
            .size(MoonButtonSize::Action)
            .leading_icon(MoonButtonIconSlot::new("icons/delete.svg"))
            .tooltip(delete_label.clone())
            .disabled(!has_sel || !all_off)
            .on_click(cx.listener(|this, _, window, cx| this.request_delete_selection(window, cx)));
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
            .child(copy.render())
            .child(paste.render())
            .child(delete.render())
            .into_any_element()
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
