//! Strategy-tree operation types for modals, menus, and DnD payloads, plus shared selection,
//! UI-folder, and toolbar helpers. Modals live in [`super::dialogs`], clipboard and DnD in
//! [`super::dnd`], context menus in [`super::menu`], and pure path and collection logic
//! in [`super::ops`].

use super::super::*;
use super::ops;
use rust_i18n::t;

/// Active mutually exclusive operation modal rendered over the window.
#[derive(Clone)]
pub(crate) enum TreeOp {
    /// Create a strategy in a target folder using the selected kind ordinal.
    CreateStrategy {
        core: CoreId,
        target: String,
        kind: Option<u8>,
    },
    /// Create a UI-only folder under the target parent.
    CreateFolder { core: CoreId, target: String },
    /// Rename a folder identified by its core and path segments.
    RenameFolder { core: CoreId, old_path: Vec<String> },
    /// Confirm deletion of selected strategies; IDs are derived again on confirmation.
    ConfirmDeleteStrategies { label: String },
    /// Confirm folder deletion using its core, path, and display label.
    ConfirmDeleteFolder {
        core: CoreId,
        path: Vec<String>,
        label: String,
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
            .text_size(design::t_body(cx))
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

    /// Returns owned copies of selected rows with their cores for clipboard and validation use.
    pub(super) fn selection_rows(&self, store: &CoreStore) -> Vec<(CoreId, StrategyRow)> {
        selected_keys(self)
            .into_iter()
            .filter_map(|(c, id)| row(store, c, id).map(|r| (c, r.clone())))
            .collect()
    }

    /// Returns the default `(core, path)` target: the primary strategy's folder or the first core's
    /// root.
    pub(super) fn default_target(
        &self,
        store: &CoreStore,
        cores: &crate::core_order::OrderedCores,
    ) -> (CoreId, String) {
        if let Some((core, id)) = self.selected {
            if let Some(r) = row(store, core, id) {
                return (core, r.folder_path.clone());
            }
        }
        (cores.first().map(|(c, _)| *c).unwrap_or(0), String::new())
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

    // ── Keyboard: Ctrl+C, Ctrl+V, and Delete ──────────────────────────────────

    pub(crate) fn handle_tree_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let m = &ev.keystroke.modifiers;
        let key = ev.keystroke.key.as_str();
        if m.control && key == "c" {
            // When no strategies are selected but a folder was clicked, match Moonbot by copying
            // the entire folder and its contents.
            let no_sel = {
                let store = self.backend.read(cx).session.store();
                self.selection_rows(store).is_empty()
            };
            if no_sel {
                if let Some((core, path)) = self.selected_folder.clone() {
                    let path = ops::split_path(&path);
                    self.copy_folder(core, path, cx);
                    return;
                }
            }
            self.copy_selection(cx);
        } else if m.control && key == "v" {
            let (core, target) = {
                let b = self.backend.read(cx);
                // Canonical order, so `default_target`'s "first core" means the first one the
                // user actually sees rather than whichever session happens to lead the vec.
                let cores = crate::core_order::CoreOrder::new(&b.config)
                    .from_sessions(b.session.sessions(), |_| true);
                self.default_target(b.session.store(), &cores)
            };
            self.paste_into(core, target, cx);
        } else if key == "delete" {
            self.request_delete_selection(window, cx);
        }
    }

    // ── Rendering: selection toolbar ──────────────────────────────────────────

    /// Builds selection and clipboard action buttons for the lower action panel.
    pub(super) fn selection_toolbar(&self, store: &CoreStore, cx: &Context<Self>) -> AnyElement {
        let rows = self.selection_rows(store);
        let has_sel = !rows.is_empty();
        let all_off = rows.iter().all(|(_, r)| !r.checked);
        let can_paste = self.clipboard.is_some();
        // Use a fixed-width left group: Copy and Paste each fill half of the first row, while
        // Delete spans the row below through `MoonButton::full_width()`.
        v_flex()
            .w(px(176.0))
            .gap_1()
            .child(
                h_flex()
                    .w_full()
                    .gap_1()
                    .child(
                        div().flex_1().child(
                            MoonButton::new("sel-copy")
                                .outline()
                                .size(MoonButtonSize::Micro)
                                .full_width()
                                .label(t!("strat.action_copy").to_string())
                                .disabled(!has_sel)
                                .on_click(cx.listener(|this, _, _, cx| this.copy_selection(cx)))
                                .render(),
                        ),
                    )
                    .child(
                        div().flex_1().child(
                            MoonButton::new("sel-paste")
                                .outline()
                                .size(MoonButtonSize::Micro)
                                .full_width()
                                .label(t!("strat.action_paste").to_string())
                                .disabled(!can_paste)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    // Paste into the primary strategy's folder or the default root.
                                    let (core, target) = {
                                        let b = this.backend.read(cx);
                                        let cores = crate::core_order::CoreOrder::new(&b.config)
                                            .from_sessions(b.session.sessions(), |_| true);
                                        this.default_target(b.session.store(), &cores)
                                    };
                                    this.paste_into(core, target, cx);
                                }))
                                .render(),
                        ),
                    ),
            )
            .child(
                MoonButton::new("sel-delete")
                    .danger()
                    .size(MoonButtonSize::Micro)
                    .full_width()
                    .label(t!("strat.action_delete").to_string())
                    .disabled(!has_sel || !all_off)
                    .on_click(
                        cx.listener(|this, _, window, cx| {
                            this.request_delete_selection(window, cx)
                        }),
                    )
                    .render(),
            )
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
            .trigger_width_scaled(110.0)
            .menu_width_scaled(180.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
            .into_any_element()
    }
}
