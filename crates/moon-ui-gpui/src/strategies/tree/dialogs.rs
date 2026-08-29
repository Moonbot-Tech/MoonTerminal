//! Modal operations for the strategy tree: create a strategy or folder, rename, and confirm
//! deletion. MoonUI Root owns the open dialog; this module builds its body and footer and
//! dispatches confirmed operations to `moon-core`.

use super::super::actions::strategy_action_authorized;
use super::super::*;
use super::ops;
use super::ui::TreeOp;
use anyhow::Result;
use moon_core::feed::NewStrategySpec;
use moon_ui::{MoonNotification, MoonWindowExt as _};
use rust_i18n::t;

#[cfg(test)]
mod tests;

/// Return whether a modal still owns the same workspace generation and visible core.
///
/// Args:
///     captured_generation: Auto generation captured when the modal opened, or Classic.
///     current_generation: Current Auto generation immediately before dispatch, or Classic.
///     workspace: Current effective Auto core set, or `None` in Classic.
///     core: Core captured by the modal producer.
///
/// Returns:
///     `true` only while both generation and effective-core authority remain unchanged.
fn tree_op_authorized(
    captured_generation: Option<u64>,
    current_generation: Option<u64>,
    workspace: Option<&[CoreId]>,
    core: CoreId,
) -> bool {
    captured_generation == current_generation && strategy_core_is_visible(workspace, core)
}

/// Build the exact sorted strategy identity and enabled-state snapshot below one folder.
///
/// Args:
///     rows: Current live rows under the folder being considered.
///
/// Returns:
///     Stable `(strategy id, enabled)` identities suitable for confirmation revalidation.
fn folder_targets(rows: &[&StrategyRow]) -> Vec<(u64, bool)> {
    let mut targets = rows
        .iter()
        .map(|row| (row.id, row.checked))
        .collect::<Vec<_>>();
    targets.sort_unstable();
    targets
}

/// Return whether a destructive folder confirmation still describes the complete live folder.
///
/// Args:
///     captured_generation: Auto generation captured before confirmation, or Classic.
///     current_generation: Current Auto generation immediately before dispatch, or Classic.
///     workspace: Current effective Auto core set, or `None` in Classic.
///     core: Core that owns the folder.
///     captured_targets: Exact child identities and enabled states shown for confirmation.
///     current_targets: Fresh child identities and enabled states from the live store.
///
/// Returns:
///     `true` only for the same visible, entirely disabled folder snapshot.
fn folder_delete_authorized(
    captured_generation: Option<u64>,
    current_generation: Option<u64>,
    workspace: Option<&[CoreId]>,
    core: CoreId,
    captured_targets: &[(u64, bool)],
    current_targets: &[(u64, bool)],
) -> bool {
    tree_op_authorized(captured_generation, current_generation, workspace, core)
        && captured_targets == current_targets
        && current_targets.iter().all(|(_, checked)| !checked)
}

fn op_title(op: &TreeOp) -> String {
    match op {
        TreeOp::CreateStrategy { .. } => t!("dialogs.new_strategy").to_string(),
        TreeOp::CreateFolder { .. } => t!("dialogs.new_folder").to_string(),
        TreeOp::RenameFolder { .. } => t!("dialogs.rename_folder").to_string(),
        TreeOp::ConfirmDeleteStrategies { .. } | TreeOp::ConfirmDeleteFolder { .. } => {
            t!("dialogs.delete_q").to_string()
        }
    }
}

fn op_ok_label(op: &TreeOp) -> String {
    match op {
        TreeOp::CreateStrategy { .. } | TreeOp::CreateFolder { .. } => {
            t!("dialogs.create").to_string()
        }
        TreeOp::RenameFolder { .. } => t!("dialogs.rename").to_string(),
        TreeOp::ConfirmDeleteStrategies { .. } | TreeOp::ConfirmDeleteFolder { .. } => {
            t!("dialogs.yes").to_string()
        }
    }
}

fn op_has_close_button(op: &TreeOp) -> bool {
    !matches!(
        op,
        TreeOp::ConfirmDeleteStrategies { .. } | TreeOp::ConfirmDeleteFolder { .. }
    )
}

fn op_dialog_body(
    view: Entity<StrategiesView>,
    _window: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    let p = MoonPalette::active(cx);
    let (op, input, backend) = {
        let this = view.read(cx);
        (
            this.op.clone()?,
            this.op_input.clone(),
            this.backend.clone(),
        )
    };

    match op {
        TreeOp::CreateStrategy {
            core, target, kind, ..
        } => {
            let mut kinds: Vec<(u8, String)> = backend
                .read(cx)
                .session
                .store()
                .core(core)
                .and_then(|cd| cd.schema.as_ref())
                .map(|s| {
                    s.kinds
                        .iter()
                        .map(|k| (k.ordinal, k.name.clone()))
                        .collect()
                })
                .unwrap_or_default();
            // Put MoonShot first because it is the most commonly used kind; retain schema order
            // for the rest to reduce navigation through a long menu.
            if let Some(pos) = kinds
                .iter()
                .position(|(_, n)| n.eq_ignore_ascii_case("MoonShot"))
            {
                let k = kinds.remove(pos);
                kinds.insert(0, k);
            }
            let kind_name = kind
                .and_then(|k| kinds.iter().find(|(o, _)| *o == k))
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| t!("strat.pick_kind").to_string());
            let target_label = if target.is_empty() {
                t!("strat.root").to_string()
            } else {
                target
            };
            let mut kind_items = Vec::with_capacity(kinds.len());
            for (ord, name) in kinds {
                let item_view = view.clone();
                kind_items.push(
                    MoonMenuItem::with_key(format!("ck-{ord}"), name)
                        .selected(kind == Some(ord))
                        .on_click(move |_, _, app| {
                            item_view.update(app, |this, c| {
                                if let Some(TreeOp::CreateStrategy { kind, .. }) = &mut this.op {
                                    *kind = Some(ord);
                                    c.notify();
                                }
                            });
                        }),
                );
            }
            let mut body = v_flex()
                .w_full()
                .gap_2()
                .child(
                    div()
                        .text_color(moon(p.text_muted))
                        .child(t!("dialogs.folder_prefix", path = target_label).to_string()),
                )
                .child(
                    MoonDropdown::new("create-kind")
                        .label(kind_name)
                        .trigger_caret(true)
                        .trigger_variant(MoonButtonVariant::Soft)
                        .trigger_size(MoonButtonSize::Action)
                        .trigger_width_scaled(320.0)
                        .menu_width_scaled(320.0)
                        .menu_size(MoonMenuSize::Compact)
                        .menu_max_height_ui(240.0)
                        .items(kind_items),
                );
            if let Some(input) = input {
                body = body.child(MoonInput::new("create-name").state(&input).small());
            }
            Some(body.into_any_element())
        }
        TreeOp::CreateFolder { target, .. } => {
            let target_label = if target.is_empty() {
                t!("strat.root").to_string()
            } else {
                target
            };
            let mut body = v_flex().w_full().gap_2().child(
                div()
                    .text_color(moon(p.text_muted))
                    .child(t!("dialogs.into_prefix", path = target_label).to_string()),
            );
            if let Some(input) = input {
                body = body.child(MoonInput::new("folder-name").state(&input).small());
            }
            Some(body.into_any_element())
        }
        TreeOp::RenameFolder { .. } => {
            let mut body = v_flex().w_full().gap_2();
            if let Some(input) = input {
                body = body.child(MoonInput::new("rename-name").state(&input).small());
            }
            Some(body.into_any_element())
        }
        TreeOp::ConfirmDeleteStrategies { label, .. }
        | TreeOp::ConfirmDeleteFolder { label, .. } => Some(
            div()
                .w_full()
                .text_color(moon(p.text))
                .child(t!("dialogs.delete_confirm", what = label).to_string())
                .into_any_element(),
        ),
    }
}

fn op_dialog_footer(
    view: Entity<StrategiesView>,
    p: MoonPalette,
    ok_label: impl Into<SharedString>,
) -> AnyElement {
    let ok_label = ok_label.into();
    let ok_variant = if ok_label == SharedString::from(t!("dialogs.yes").to_string()) {
        MoonButtonVariant::Danger
    } else {
        MoonButtonVariant::Blue
    };
    let cancel_view = view.clone();
    let ok_view = view;
    h_flex()
        .w_full()
        .justify_end()
        .gap_2()
        .child(
            MoonButton::new("modal-cancel")
                .ghost()
                .size(MoonButtonSize::Micro)
                .label(t!("dialogs.cancel").to_string())
                .on_click(move |_, window, cx| {
                    cancel_view.update(cx, |this, cx| this.close_op_dialog(cx));
                    window.close_dialog(cx);
                })
                .render(),
        )
        .child(
            MoonButton::new("modal-ok")
                .size(MoonButtonSize::Micro)
                .variant(ok_variant)
                .label(ok_label)
                .on_click(move |_, window, cx| {
                    match ok_view.update(cx, |this, cx| this.confirm_op_dialog(cx)) {
                        Ok(true) => window.close_dialog(cx),
                        // Keep the dialog open when validation rejects an empty strategy name and
                        // explain the reason instead of failing silently.
                        Ok(false) => {
                            window.push_notification(
                                MoonNotification::warning(t!("dialogs.name_required").to_string()),
                                cx,
                            );
                        }
                        Err(error) => {
                            log::warn!("strategies operation failed: {error}");
                            window
                                .push_notification(MoonNotification::error(error.to_string()), cx);
                        }
                    }
                })
                .render(),
        )
        .text_color(moon(p.text))
        .into_any_element()
}

impl StrategiesView {
    // ── Opening modals ────────────────────────────────────────────────────────

    /// Open a create-strategy modal with the current workspace generation.
    ///
    /// Args:
    ///     core: Core that will own the new strategy.
    ///     target: Canonical destination folder path.
    ///     window: Native owner used to open the modal.
    ///     cx: View context used to capture generation and construct dialog state.
    ///
    /// Returns:
    ///     Nothing; confirmation performs the dispatch-time revalidation.
    pub(super) fn open_create_strategy(
        &mut self,
        core: CoreId,
        target: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = self.backend.read(cx).session.store();
        // Default to MoonShot, the most commonly used kind, rather than the schema's first kind
        // (Telegram). Fall back to the first kind when MoonShot is absent.
        let kinds = self.kinds_of(store, core);
        let kind = kinds
            .iter()
            .find(|(_, n)| n.eq_ignore_ascii_case("MoonShot"))
            .or_else(|| kinds.first())
            .map(|(o, _)| *o);
        self.op_input_init = String::new();
        self.op_input = None; // Give every opening a fresh input entity and layout.
        self.op = Some(TreeOp::CreateStrategy {
            core,
            target,
            kind,
            workspace_generation: self.action_workspace_generation(cx),
        });
        self.open_op_dialog(window, cx);
        cx.notify();
    }

    /// Open a create-folder modal with the current workspace generation.
    ///
    /// Args:
    ///     core: Core that will own the UI folder.
    ///     target: Canonical parent folder path.
    ///     window: Native owner used to open the modal.
    ///     cx: View context used to capture generation and construct dialog state.
    ///
    /// Returns:
    ///     Nothing; confirmation performs the dispatch-time revalidation.
    pub(super) fn open_create_folder(
        &mut self,
        core: CoreId,
        target: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.op_input_init = String::new();
        self.op_input = None;
        self.op = Some(TreeOp::CreateFolder {
            core,
            target,
            workspace_generation: self.action_workspace_generation(cx),
        });
        self.open_op_dialog(window, cx);
        cx.notify();
    }

    /// Open a folder-rename modal with the current workspace generation.
    ///
    /// Args:
    ///     core: Core that owns the folder.
    ///     old_path: Exact canonical path captured by the producer.
    ///     window: Native owner used to open the modal.
    ///     cx: View context used to capture generation and construct dialog state.
    ///
    /// Returns:
    ///     Nothing; confirmation performs the dispatch-time revalidation.
    pub(super) fn open_rename_folder(
        &mut self,
        core: CoreId,
        old_path: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cur = old_path.last().cloned().unwrap_or_default();
        self.op_input_init = cur;
        self.op_input = None;
        self.op = Some(TreeOp::RenameFolder {
            core,
            old_path,
            workspace_generation: self.action_workspace_generation(cx),
        });
        self.open_op_dialog(window, cx);
        cx.notify();
    }

    fn ensure_op_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.op.is_some() && self.op_input.is_none() {
            let init = self.op_input_init.clone();
            self.op_input = Some(cx.new(|cx| {
                MoonInputState::new(window, cx)
                    .default_value(init)
                    .placeholder(t!("dialogs.name_ph").to_string())
            }));
        }
    }

    fn close_op_dialog(&mut self, cx: &mut Context<Self>) {
        self.op = None;
        self.op_input = None;
        cx.notify();
    }

    /// Confirm the current tree operation against the workspace-visible target at dispatch time.
    ///
    /// Hidden retained Classic selection and folder state never become fallback targets when Auto
    /// moves; an operation whose captured core is no longer visible closes without dispatch.
    ///
    /// Args:
    ///     cx: View context used to resolve inputs, effective selection, and session commands.
    ///
    /// Returns:
    ///     Whether the dialog may close, or a session dispatch error.
    fn confirm_op_dialog(&mut self, cx: &mut Context<Self>) -> Result<bool> {
        let Some(op) = self.op.clone() else {
            return Ok(true);
        };

        match op {
            TreeOp::CreateStrategy {
                core,
                target,
                kind,
                workspace_generation,
            } => {
                let name = self
                    .op_input
                    .as_ref()
                    .map(|i| i.read(cx).value().to_string())
                    .unwrap_or_default();
                if name.trim().is_empty() {
                    return Ok(false);
                }
                if let Some(kind) = kind {
                    self.confirm_create_strategy(
                        core,
                        target,
                        kind,
                        name,
                        workspace_generation,
                        cx,
                    )?;
                }
            }
            TreeOp::CreateFolder {
                core,
                target,
                workspace_generation,
            } => {
                let name = self
                    .op_input
                    .as_ref()
                    .map(|i| i.read(cx).value().to_string())
                    .unwrap_or_default();
                if !name.trim().is_empty()
                    && tree_op_authorized(
                        workspace_generation,
                        self.action_workspace_generation(cx),
                        self.workspace_cores.as_deref(),
                        core,
                    )
                {
                    self.add_ui_folder(core, &target, name.trim());
                    self.persist_session(cx);
                }
            }
            TreeOp::RenameFolder {
                core,
                old_path,
                workspace_generation,
            } => {
                let name = self
                    .op_input
                    .as_ref()
                    .map(|i| i.read(cx).value().to_string())
                    .unwrap_or_default();
                if !name.trim().is_empty() {
                    self.confirm_rename_folder(
                        core,
                        &old_path,
                        name.trim(),
                        workspace_generation,
                        cx,
                    )?;
                }
            }
            TreeOp::ConfirmDeleteStrategies {
                targets,
                workspace_generation,
                ..
            } => {
                self.delete_selection(&targets, workspace_generation, cx)?;
            }
            TreeOp::ConfirmDeleteFolder {
                core,
                path,
                targets,
                workspace_generation,
                ..
            } => {
                self.delete_folder(core, &path, &targets, workspace_generation, cx)?;
            }
        }

        self.close_op_dialog(cx);
        Ok(true)
    }

    fn open_op_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_op_input(window, cx);
        let view = cx.entity();
        window.open_unique_moon_dialog(
            "strategies-tree-op-dialog",
            cx,
            move |dialog, _window, cx| {
                let p = MoonPalette::active(cx);
                let cancel_view = view.clone();
                let close_view = view.clone();
                let content_view = view.clone();
                let footer_view = view.clone();

                let title = view
                    .read(cx)
                    .op
                    .as_ref()
                    .map(op_title)
                    .unwrap_or_else(|| t!("dialogs.operation").to_string());
                let ok_label = view
                    .read(cx)
                    .op
                    .as_ref()
                    .map(op_ok_label)
                    .unwrap_or_else(|| "OK".to_string());
                let close_button = view
                    .read(cx)
                    .op
                    .as_ref()
                    .map(op_has_close_button)
                    .unwrap_or(true);

                dialog
                    .w(px(360.0))
                    .close_button(close_button)
                    .overlay(true)
                    .overlay_closable(true)
                    .bg(moon(p.shell_high))
                    .border_color(moon(p.border))
                    .rounded(design::r_container(cx))
                    .text_color(moon(p.text))
                    .header(
                        div()
                            .w_full()
                            .py_2()
                            .border_b_1()
                            .border_color(moon(p.border))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .on_cancel(move |_, _, cx| {
                        cancel_view.update(cx, |this, cx| this.close_op_dialog(cx));
                        true
                    })
                    .on_close(move |_, _, cx| {
                        close_view.update(cx, |this, cx| this.close_op_dialog(cx));
                    })
                    .content(move |content, window, cx| {
                        let body = op_dialog_body(content_view.clone(), window, cx)
                            .unwrap_or_else(|| div().into_any_element());
                        content.child(body)
                    })
                    .footer(op_dialog_footer(footer_view, p, ok_label))
            },
        );
    }

    /// Requests deletion of the selected strategies after checking that all are disabled.
    pub(super) fn request_delete_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.backend.read(cx).session.store();
        let rows = self.selection_rows(store);
        if rows.is_empty() {
            return;
        }
        // Deletion is allowed only when every selected strategy is disabled.
        if rows.iter().any(|(_, r)| r.checked) {
            return;
        }
        // A selection may span cores. Keep its complete identity in the confirmation so a later
        // workspace transition cannot silently delete only the surviving subset.
        let targets = rows.iter().map(|(core, row)| (*core, row.id)).collect();
        self.op = Some(TreeOp::ConfirmDeleteStrategies {
            label: t!("strat.count_strategies", n = rows.len()).to_string(),
            targets,
            workspace_generation: self.action_workspace_generation(cx),
        });
        self.open_op_dialog(window, cx);
        cx.notify();
    }

    /// Requests folder deletion when every strategy beneath it is disabled.
    pub(super) fn request_delete_folder(
        &mut self,
        core: CoreId,
        path: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = self.backend.read(cx).session.store();
        let Some(cd) = store.core(core) else { return };
        let under = ops::rows_under(&cd.strategies, &path);
        if !ops::all_off(&under) {
            return; // Running strategies prevent deletion.
        }
        let label = t!(
            "strat.folder_named",
            name = path.last().cloned().unwrap_or_default()
        )
        .to_string();
        self.op = Some(TreeOp::ConfirmDeleteFolder {
            core,
            path,
            label,
            targets: folder_targets(&under),
            workspace_generation: self.action_workspace_generation(cx),
        });
        self.open_op_dialog(window, cx);
        cx.notify();
    }

    // ── Confirmed dispatch ────────────────────────────────────────────────────

    /// Create a disabled strategy from schema defaults and select it after the core echo.
    ///
    /// The shared `NewStrategy` conversion keeps dialog creation aligned with paste and drop
    /// dispatch, including placement metadata.
    ///
    /// Args:
    ///     core: Core captured when the modal opened.
    ///     target: Canonical destination folder path.
    ///     kind_ord: Exact schema kind ordinal selected by the user.
    ///     name: Strategy name entered in the modal.
    ///     workspace_generation: Auto generation captured with the modal, or Classic.
    ///     cx: View context used for live authority/schema lookup and dispatch.
    ///
    /// Returns:
    ///     Success for a dispatched create or stale-scope no-op, otherwise a session error.
    fn confirm_create_strategy(
        &mut self,
        core: CoreId,
        target: String,
        kind_ord: u8,
        name: String,
        workspace_generation: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        if !tree_op_authorized(
            workspace_generation,
            self.action_workspace_generation(cx),
            self.workspace_cores.as_deref(),
            core,
        ) {
            return Ok(());
        }
        let spec = {
            let store = self.backend.read(cx).session.store();
            let Some(kind) = store
                .core(core)
                .and_then(|cd| cd.schema.as_ref())
                .and_then(|s| s.kinds.iter().find(|k| k.ordinal == kind_ord).cloned())
            else {
                return Ok(());
            };
            // Through the shared converter, so a field added to the intent cannot reach the
            // core by one dispatch path and not the other.
            NewStrategySpec::from(ops::new_strategy(&kind, &name, &target))
        };
        self.backend
            .read(cx)
            .session
            .create_strategies(core, vec![spec])?;
        // Expand the core so the created row is visible when it echoes back.
        self.expanded_cores.insert(core);
        // Select it after the core echoes it back.
        self.queue_pending_name(core, name, cx);
        self.persist_session(cx);
        Ok(())
    }

    /// Rename a folder only while its captured core remains workspace-visible.
    ///
    /// Args:
    ///     core: Core captured when the rename dialog opened.
    ///     old_path: Existing canonical folder segments.
    ///     new_name: Replacement leaf name entered by the user.
    ///     workspace_generation: Auto generation captured with the modal, or Classic.
    ///     cx: View context used to revalidate scope and dispatch the move.
    ///
    /// Returns:
    ///     Success for a dispatched rename or a stale-scope no-op, otherwise a session error.
    fn confirm_rename_folder(
        &mut self,
        core: CoreId,
        old_path: &[String],
        new_name: &str,
        workspace_generation: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        if !tree_op_authorized(
            workspace_generation,
            self.action_workspace_generation(cx),
            self.workspace_cores.as_deref(),
            core,
        ) {
            return Ok(());
        }
        let moves = {
            let store = self.backend.read(cx).session.store();
            let Some(cd) = store.core(core) else {
                return Ok(());
            };
            ops::rename_folder(&cd.strategies, old_path, new_name)
        };
        self.backend.read(cx).session.move_strategies(core, moves)?;
        // Rename an empty UI-only folder locally only after the move command succeeds.
        self.rename_ui_folder(core, old_path, new_name);
        let mut new_path = old_path.to_vec();
        if let Some(last) = new_path.last_mut() {
            *last = new_name.to_string();
        }
        self.move_row_checks(core, old_path, Some(&new_path));
        self.persist_session(cx);
        Ok(())
    }

    /// Delete one exact confirmed selection only while every target retains workspace authority.
    ///
    /// Args:
    ///     targets: Complete `(core, strategy)` identities captured before confirmation.
    ///     workspace_generation: Auto generation captured with the confirmation, or Classic.
    ///     cx: View context used to revalidate all targets before the first command.
    ///
    /// Returns:
    ///     Success for a complete authority-approved dispatch or a stale-scope no-op.
    fn delete_selection(
        &mut self,
        targets: &[Key],
        workspace_generation: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        if !strategy_action_authorized(
            workspace_generation,
            self.action_workspace_generation(cx),
            self.workspace_cores.as_deref(),
            targets,
        ) {
            return Ok(());
        }
        let rows = {
            let store = self.backend.read(cx).session.store();
            let rows: Option<Vec<(CoreId, StrategyRow)>> = targets
                .iter()
                .map(|(core, id)| row(store, *core, *id).cloned().map(|row| (*core, row)))
                .collect();
            let Some(rows) = rows else {
                return Ok(());
            };
            // Revalidate the destructive precondition atomically too: one restarted strategy
            // rejects the whole confirmation instead of deleting its disabled siblings.
            if rows.iter().any(|(_, row)| row.checked) {
                return Ok(());
            }
            rows
        };
        let deleted: HashSet<Key> = targets.iter().copied().collect();
        {
            let b = self.backend.read(cx);
            for (core, r) in &rows {
                b.session.delete_strategy(*core, r.id)?;
            }
        }
        self.sel.retain(|key| !deleted.contains(key));
        if self.selected.is_some_and(|key| deleted.contains(&key)) {
            self.selected = None;
        }
        self.persist_session(cx);
        Ok(())
    }

    /// Delete a folder only while its captured core remains workspace-visible.
    ///
    /// Args:
    ///     core: Core captured when the delete confirmation opened.
    ///     path: Canonical folder segments captured by the confirmation.
    ///     cx: View context used to revalidate scope and dispatch deletion.
    ///
    /// Returns:
    ///     Success for a dispatched deletion or a stale-scope no-op, otherwise a session error.
    fn delete_folder(
        &mut self,
        core: CoreId,
        path: &[String],
        targets: &[(u64, bool)],
        workspace_generation: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let current_targets = {
            let store = self.backend.read(cx).session.store();
            let Some(cd) = store.core(core) else {
                return Ok(());
            };
            folder_targets(&ops::rows_under(&cd.strategies, path))
        };
        if !folder_delete_authorized(
            workspace_generation,
            self.action_workspace_generation(cx),
            self.workspace_cores.as_deref(),
            core,
            targets,
            &current_targets,
        ) {
            return Ok(());
        }
        self.backend
            .read(cx)
            .session
            .delete_folder(core, ops::join_path(path))?;
        self.remove_ui_folder(core, path);
        self.move_row_checks(core, path, None);
        self.persist_session(cx);
        Ok(())
    }
}
