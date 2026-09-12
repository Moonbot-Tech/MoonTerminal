//! Modal operations for the strategy tree: create a strategy or folder, rename, and confirm
//! deletion. MoonUI Root owns the open dialog; this module builds its body and footer and
//! dispatches confirmed operations to `moon-core`.

use super::super::actions::strategy_action_authorized;
use super::super::*;
use super::ops;
use super::ui::{TreeNote, TreeOp};
use anyhow::Result;
use moon_core::feed::NewStrategySpec;
use moon_ui::{MoonListItem, MoonNotification, MoonText, MoonWindowExt as _};
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
        TreeOp::RenameStrategy { .. } => t!("strat.rename_strategy_title").to_string(),
        TreeOp::MoveToFolder { .. } => t!("strat.move_to_title").to_string(),
        TreeOp::ConfirmDeleteStrategies { .. } | TreeOp::ConfirmDeleteFolder { .. } => {
            t!("dialogs.delete_q").to_string()
        }
        TreeOp::ConfirmForget { .. } => t!("strat.forget_title").to_string(),
    }
}

fn op_ok_label(op: &TreeOp) -> String {
    match op {
        TreeOp::CreateStrategy { .. } | TreeOp::CreateFolder { .. } => {
            t!("dialogs.create").to_string()
        }
        TreeOp::RenameFolder { .. } | TreeOp::RenameStrategy { .. } => {
            t!("dialogs.rename").to_string()
        }
        TreeOp::ConfirmDeleteStrategies { .. } | TreeOp::ConfirmDeleteFolder { .. } => {
            t!("dialogs.yes").to_string()
        }
        TreeOp::ConfirmForget { .. } => t!("strat.forget_ok").to_string(),
        // Picked by clicking a destination, so the footer carries Cancel alone.
        TreeOp::MoveToFolder { .. } => t!("dialogs.cancel").to_string(),
    }
}

fn op_has_close_button(op: &TreeOp) -> bool {
    !matches!(
        op,
        TreeOp::ConfirmDeleteStrategies { .. }
            | TreeOp::ConfirmDeleteFolder { .. }
            | TreeOp::ConfirmForget { .. }
    )
}

/// Whether this operation's OK button is the destructive one.
///
/// Asked of the OPERATION rather than inferred from the button's caption: the label test only ever
/// worked because every destructive op happened to say "Yes", and Forget says something else.
fn op_ok_is_danger(op: &TreeOp) -> bool {
    matches!(
        op,
        TreeOp::ConfirmDeleteStrategies { .. }
            | TreeOp::ConfirmDeleteFolder { .. }
            | TreeOp::ConfirmForget { .. }
    )
}

/// Design-reference width of the Strategies tree-operation dialog, in unscaled pixels.
const TREE_OP_DIALOG_W: f32 = 360.0;

/// Unscaled horizontal padding MoonUI Dialog applies on each side (`Edges::all(px(16.))`).
///
/// Subtract this only when converting the outer card (`MoonDialog::w`) into trigger/menu field
/// width. The outer-card clamp uses the client viewport after window-frame insets, not these pads:
/// MoonUI leaves the content pad unscaled while the card grows with the Font slider.
const TREE_OP_DIALOG_PAD: f32 = 16.0;

/// Client width MoonUI Dialog uses as overlay bounds: viewport minus window-frame insets.
///
/// Args:
///     viewport_w: Window viewport width, in rendered pixels.
///     pad_left: MoonUI `window_paddings` left inset, in rendered pixels.
///     pad_right: MoonUI `window_paddings` right inset, in rendered pixels.
///
/// Returns:
///     Viewport width minus both frame insets, floored at 0.
fn tree_op_dialog_client_width(viewport_w: f32, pad_left: f32, pad_right: f32) -> f32 {
    (viewport_w - pad_left - pad_right).max(0.0)
}

/// Clamp a desired tree-op dialog outer-card width to the MoonUI client viewport.
///
/// `MoonDialog::w` is the outer card, so this must not subtract DialogContent padding. Restoring
/// `client_w - 2 * TREE_OP_DIALOG_PAD` here shrinks a 320 px client to a 288 px card and then 256 px
/// fields, crowding the kind label, caret, and footer.
///
/// Args:
///     desired: Font-scaled design-reference card width, in rendered pixels.
///     client_w: Client viewport width after window-frame insets, in rendered pixels.
///
/// Returns:
///     `desired`, or `client_w` when that is smaller, floored at 0.
fn clamp_tree_op_dialog_width(desired: f32, client_w: f32) -> f32 {
    desired.min(client_w.max(0.0))
}

/// Width available to dropdowns and name fields inside the padded tree-op dialog card.
///
/// Args:
///     dialog_w: Outer card width passed to `MoonDialog::w`.
///
/// Returns:
///     `dialog_w` minus both unscaled DialogContent pads, floored at 0.
fn tree_op_field_width(dialog_w: f32) -> f32 {
    (dialog_w - 2.0 * TREE_OP_DIALOG_PAD).max(0.0)
}

/// Horizontal insets matching MoonUI `window_paddings(window).left/right`.
///
/// MoonUI Dialog subtracts these from `viewport_size` before placing the outer card. `moon_ui`
/// does not re-export `window_paddings`; this uses the same `window_decorations` and
/// `client_inset` inputs. Server decorations (Windows) are zero; client-side frames keep the
/// inset on untiled sides. The Linux 12 px fallback is MoonUI's `SHADOW_SIZE` when the inset
/// has not been set yet.
///
/// Args:
///     window: Window whose decorations and client inset feed the overlay bounds.
///
/// Returns:
///     `(left, right)` frame insets in rendered pixels.
fn tree_op_window_frame_pads(window: &Window) -> (f32, f32) {
    match window.window_decorations() {
        Decorations::Server => (0.0, 0.0),
        Decorations::Client { tiling } => {
            let inset = window
                .client_inset()
                .unwrap_or(if cfg!(target_os = "linux") {
                    px(12.0)
                } else {
                    px(0.0)
                })
                .as_f32();
            (
                if tiling.left { 0.0 } else { inset },
                if tiling.right { 0.0 } else { inset },
            )
        }
    }
}

/// Font-scale the tree-op dialog card and clamp it to the current client viewport.
///
/// Pair the card with `font_w`, not `ui_px`: MoonDropdown's scaled trigger uses the Font slider,
/// while UI scale only grows control height. The outer card is clamped once to MoonUI's client
/// width (`viewport - window_paddings.left/right`); DialogContent's unscaled 16 px pads are
/// subtracted only for trigger/menu field width.
///
/// Args:
///     window: Window whose client viewport bounds the card.
///     cx: Application context used to read the Font-slider scale.
///
/// Returns:
///     Rendered pixel width for `dialog.w(px(...))`.
fn tree_op_dialog_width(window: &Window, cx: &App) -> f32 {
    let (pad_left, pad_right) = tree_op_window_frame_pads(window);
    clamp_tree_op_dialog_width(
        design::font_w(cx, TREE_OP_DIALOG_W),
        tree_op_dialog_client_width(window.viewport_size().width.as_f32(), pad_left, pad_right),
    )
}

/// Wrap a tree-op name `MoonInput` so it fills the padded card without overflowing it.
///
/// MoonInput does not implement Styled; its inner Input always renders `size_full()`, so without
/// a parent that is both `w_full` and `min_w_0` the field can take a min-content size larger than
/// the card.
///
/// Args:
///     id: Element id for the name field (`create-name`, `folder-name`, or `rename-name`).
///     input: Shared input state already constructed for this dialog opening.
///
/// Returns:
///     A full-width, shrinkable slot containing the small name field.
fn tree_op_name_input(
    id: impl Into<SharedString>,
    input: &Entity<MoonInputState>,
) -> impl IntoElement {
    div()
        .w_full()
        .min_w_0()
        .child(MoonInput::new(id).state(input).small())
}

/// Build the tree-op dialog body for the current `TreeOp`.
///
/// Create, folder, and rename fields share one width contract: the card is font-scaled and
/// clamped to the MoonUI client viewport, dropdowns use the padded rendered width, and name
/// inputs sit in a `w_full`/`min_w_0` slot. The kind menu is a MoonUI Root popup and is not
/// clipped here.
///
/// Args:
///     view: Strategies view that owns the current operation and input state.
///     window: Window used to measure the client viewport for field widths.
///     cx: Application context for palette, session, and font scale.
///
/// Returns:
///     The body element, or `None` when no operation is open.
fn op_dialog_body(
    view: Entity<StrategiesView>,
    window: &mut Window,
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
            let field_w = tree_op_field_width(tree_op_dialog_width(window, cx));
            let mut body = v_flex()
                .w_full()
                .min_w_0()
                .gap_2()
                .child(
                    div()
                        .text_color(moon(p.text_muted))
                        .child(t!("dialogs.folder_prefix", path = target_label).to_string()),
                )
                .child(
                    div().w_full().min_w_0().child(
                        MoonDropdown::new("create-kind")
                            .label(kind_name)
                            .trigger_caret(true)
                            .trigger_variant(MoonButtonVariant::Soft)
                            .trigger_size(MoonButtonSize::Action)
                            .trigger_width(field_w)
                            .menu_width(field_w)
                            .menu_size(MoonMenuSize::Compact)
                            .menu_max_height_ui(240.0)
                            .items(kind_items),
                    ),
                );
            if let Some(input) = input {
                body = body.child(tree_op_name_input("create-name", &input));
            }
            Some(body.into_any_element())
        }
        TreeOp::CreateFolder { target, .. } => {
            let target_label = if target.is_empty() {
                t!("strat.root").to_string()
            } else {
                target
            };
            let mut body = v_flex().w_full().min_w_0().gap_2().child(
                div()
                    .text_color(moon(p.text_muted))
                    .child(t!("dialogs.into_prefix", path = target_label).to_string()),
            );
            if let Some(input) = input {
                body = body.child(tree_op_name_input("folder-name", &input));
            }
            Some(body.into_any_element())
        }
        TreeOp::RenameFolder { .. } | TreeOp::RenameStrategy { .. } => {
            let mut body = v_flex().w_full().min_w_0().gap_2();
            if let Some(input) = input {
                body = body.child(tree_op_name_input("rename-name", &input));
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
        TreeOp::MoveToFolder { core, sources, .. } => {
            let destinations = {
                let backend = backend.read(cx);
                let store = backend.session.store();
                let rows = store
                    .core(core)
                    .map(|cd| cd.strategies.clone())
                    .unwrap_or_default();
                let reported = store
                    .core(core)
                    .map(|cd| {
                        cd.folders
                            .paths
                            .iter()
                            .map(|path| ops::split_path(path))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let local = view.read(cx).ui_folder_paths(core);
                // A folder cannot move into itself or its own subtree; strategies exclude nothing.
                let exclude = match &sources {
                    ops::MoveSources::Folders(folders) => folders.clone(),
                    ops::MoveSources::Strategies(_) => Vec::new(),
                };
                ops::move_destinations(&rows, &reported, &local, &exclude)
            };
            let what = match &sources {
                ops::MoveSources::Strategies(ids) => {
                    t!("strat.count_strategies", n = ids.len()).to_string()
                }
                ops::MoveSources::Folders(folders) => {
                    t!("strat.count_folders", n = folders.len()).to_string()
                }
            };
            let mut body = v_flex().w_full().min_w_0().gap_2().child(
                MoonText::new(t!("strat.move_to_hint", what = what).to_string())
                    .mono(false)
                    .uppercase(false)
                    .color(p.text_soft)
                    .render(),
            );
            let mut list = v_flex()
                .id("move-to-list")
                .w_full()
                .min_w_0()
                .max_h(design::ui_px(cx, 240.0))
                .overflow_y_scroll();
            for parts in destinations {
                let depth = parts.len();
                let label = match parts.last() {
                    Some(leaf) => leaf.clone(),
                    None => t!("strat.root").to_string(),
                };
                let target = parts.clone();
                let pick_view = view.clone();
                list = list.child(
                    MoonListItem::new(SharedString::from(format!(
                        "mv-{depth}-{}",
                        ops::join_path(&parts)
                    )))
                    .on_click(move |_, window: &mut Window, app: &mut App| {
                        let target = target.clone();
                        pick_view.update(app, |this, cx| this.confirm_move_to_folder(&target, cx));
                        window.close_dialog(app);
                    })
                    .child(
                        div()
                            .w_full()
                            .pl(design::ui_px(cx, 12.0 * depth as f32))
                            .child(
                                MoonText::new(label)
                                    .mono(true)
                                    .uppercase(false)
                                    .color(p.text)
                                    .render(),
                            ),
                    ),
                );
            }
            body = body.child(list);
            Some(body.into_any_element())
        }
        TreeOp::ConfirmForget { label, .. } => Some(
            div()
                .w_full()
                .text_color(moon(p.text))
                .child(t!("strat.forget_confirm", what = label).to_string())
                .into_any_element(),
        ),
    }
}

fn op_dialog_footer(
    view: Entity<StrategiesView>,
    p: MoonPalette,
    ok_label: impl Into<SharedString>,
    danger: bool,
) -> AnyElement {
    let ok_label = ok_label.into();
    let ok_variant = if danger {
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

    /// Open the destination picker for the tree's current target.
    ///
    /// The folder set takes precedence over the strategy selection. A move cannot span cores:
    /// moving within a core rewrites `folder_path`, while a cross-core move must use Cut and Paste
    /// so the destination echo can protect the source from premature deletion.
    ///
    /// Args:
    ///     core: Core addressed by the menu or keyboard action.
    ///     window: Window used to show the picker or a refusal notice.
    ///     cx: View context used to resolve the current selection.
    ///
    /// Returns:
    ///     Nothing; an empty or cross-core selection leaves no operation staged.
    pub(super) fn open_move_for_target(
        &mut self,
        core: CoreId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let folders = selected_folders(self);
        if !folders.is_empty() {
            if folders.iter().any(|(c, _)| *c != core) {
                TreeNote::MoveNeedsOneCore.say(window, cx);
                return;
            }
            let paths: Vec<Vec<String>> = folders
                .iter()
                .filter(|(_, path)| !path.is_empty())
                .map(|(_, path)| ops::split_path(path))
                .collect();
            if paths.is_empty() {
                // Only core roots were selected, and a core root has no parent to move into.
                TreeNote::CoreNotCut.say(window, cx);
                return;
            }
            self.open_move_to_folder(core, ops::MoveSources::Folders(paths), window, cx);
            return;
        }
        let keys = selected_keys(self);
        if keys.is_empty() {
            return;
        }
        if keys.iter().any(|(c, _)| *c != core) {
            TreeNote::MoveNeedsOneCore.say(window, cx);
            return;
        }
        let ids: Vec<u64> = keys.iter().map(|(_, id)| *id).collect();
        self.open_move_to_folder(core, ops::MoveSources::Strategies(ids), window, cx);
    }

    /// Open the destination picker for a move.
    ///
    /// Args:
    ///     core: Core the sources belong to; a move never crosses cores.
    ///     sources: The strategies or folders being moved.
    ///     window: Window used to present the dialog.
    ///     cx: View context used to stage the operation.
    ///
    /// Returns:
    ///     Nothing; confirmation revalidates workspace authority before dispatch.
    pub(super) fn open_move_to_folder(
        &mut self,
        core: CoreId,
        sources: ops::MoveSources,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.op = Some(TreeOp::MoveToFolder {
            core,
            sources,
            workspace_generation: self.action_workspace_generation(cx),
        });
        self.open_op_dialog(window, cx);
        cx.notify();
    }

    /// Dispatch the chosen destination and close the picker.
    ///
    /// Args:
    ///     target: Destination path selected from the picker; an empty path is the core root.
    ///     cx: View context used to revalidate authority and dispatch the planned moves.
    ///
    /// Returns:
    ///     Nothing; an operation whose workspace authority changed is discarded without dispatch.
    pub(super) fn confirm_move_to_folder(&mut self, target: &[String], cx: &mut Context<Self>) {
        let Some(TreeOp::MoveToFolder {
            core,
            sources,
            workspace_generation,
        }) = self.op.clone()
        else {
            return;
        };
        if !tree_op_authorized(
            workspace_generation,
            self.action_workspace_generation(cx),
            self.workspace_cores.as_deref(),
            core,
        ) {
            self.close_op_dialog(cx);
            return;
        }
        let intents = {
            let store = self.backend.read(cx).session.store();
            store
                .core(core)
                .map(|cd| ops::move_to_folder_plan(&cd.strategies, &sources, target))
                .unwrap_or_default()
        };
        let mut moved = 0usize;
        for intent in intents {
            moved += intent.moves.len();
            if let Some((old_key, new_key)) = intent.rebase.clone() {
                self.rebase_ui_folder(core, &old_key, &new_key);
            }
            if let Err(error) =
                self.backend
                    .read(cx)
                    .session
                    .move_strategies(core, intent.moves, intent.rebase)
            {
                log::warn!("move to folder failed: {error}");
            }
        }
        // The destination is opened so the rows are visible where they landed.
        self.expanded_cores.insert(core);
        self.expand_path(core, target.iter().map(String::as_str));
        let folder = match target.last() {
            Some(leaf) => leaf.clone(),
            None => t!("strat.root").to_string(),
        };
        self.pending_notes.push(TreeNote::MoveSent {
            strategies: moved,
            folder,
        });
        self.close_op_dialog(cx);
        self.persist_session(cx);
        cx.notify();
    }

    /// Open a strategy-rename dialog prefilled with the row's current name.
    ///
    /// Args:
    ///     core: Core that owns the strategy.
    ///     id: Strategy to rename.
    ///     window: Window used to present the dialog.
    ///     cx: View context used to read the current name and stage the operation.
    ///
    /// Returns:
    ///     Nothing; a row absent from the current store opens no dialog.
    pub(super) fn open_rename_strategy(
        &mut self,
        core: CoreId,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cur = {
            let store = self.backend.read(cx).session.store();
            row(store, core, id).map(|r| r.name.clone())
        };
        let Some(cur) = cur else { return };
        self.op_input_init = cur;
        self.op_input = None;
        self.op = Some(TreeOp::RenameStrategy {
            core,
            id,
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
                    self.create_folder(core, &target, name.trim(), cx);
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
            TreeOp::RenameStrategy {
                core,
                id,
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
                return self.confirm_rename_strategy(core, id, name, workspace_generation, cx);
            }
            // Confirmed by clicking a destination in the body, not by an OK button.
            TreeOp::MoveToFolder { .. } => {}
            TreeOp::ConfirmForget { core, ids, .. } => {
                // No workspace re-validation here: this touches the LOCAL history database, never
                // the core, so a core going out of scope between the dialog opening and its OK
                // changes nothing about what is being purged.
                self.forget_deleted(core, ids, cx);
            }
        }

        self.close_op_dialog(cx);
        Ok(true)
    }

    /// Open the shared MoonUI Root dialog for the current tree operation.
    ///
    /// The card is font-scaled and clamped to the MoonUI client viewport so kind and name fields
    /// stay inside the padded dialog at supported Font-slider values and in a narrow Strategies
    /// window.
    ///
    /// Args:
    ///     window: Native owner used to present the unique dialog.
    ///     cx: View context used to build dialog state and content.
    ///
    /// Returns:
    ///     Nothing; the dialog stays open until confirm, cancel, or overlay dismiss.
    fn open_op_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_op_input(window, cx);
        let view = cx.entity();
        window.open_unique_moon_dialog(
            "strategies-tree-op-dialog",
            cx,
            move |dialog, window, cx| {
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
                let ok_danger = view.read(cx).op.as_ref().is_some_and(op_ok_is_danger);

                dialog
                    .w(px(tree_op_dialog_width(window, cx)))
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
                    .footer(op_dialog_footer(footer_view, p, ok_label, ok_danger))
            },
        );
    }

    /// Requests deletion of the selected strategies after checking that all are disabled.
    /// Delete whatever the tree currently addresses: the folder set if there is one, else the
    /// strategy selection.
    ///
    /// One entry point so the Delete key, the menu and the toolbar cannot disagree about what
    /// "delete" means once folders and cores can be multi-selected.
    pub(super) fn request_delete_target(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let folders = selected_folders(self);
        if folders.is_empty() {
            self.request_delete_selection(window, cx);
            return;
        }
        // A core root is not something this tree can delete; say so instead of silently acting on
        // the rest of the set, which would look like the core had been deleted too.
        if folders.iter().any(|(_, path)| path.is_empty()) {
            TreeNote::CoreNotDeletable.say(window, cx);
            return;
        }
        // One folder is the ordinary case and keeps the existing confirmation verbatim. Several
        // are refused for now rather than half-handled: each needs its own authorized snapshot,
        // and a partial delete across folders is exactly the outcome that must not be possible.
        let Some((core, path)) = folders.first().cloned() else {
            return;
        };
        if folders.len() > 1 {
            TreeNote::DeleteNeedsOneFolder {
                selected: folders.len(),
            }
            .say(window, cx);
            return;
        }
        self.request_delete_folder(core, ops::split_path(&path), window, cx);
    }

    pub(super) fn request_delete_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.backend.read(cx).session.store();
        let rows = self.selection_rows(store);
        if rows.is_empty() {
            return;
        }
        // Deletion is allowed only when every selected strategy is disabled — and a refusal is
        // SAID. Returning quietly here was the defect: the menu entry was never disabled and the
        // Delete key ran this same path, so pressing either on a running strategy did nothing at
        // all, with nothing on screen to explain why.
        let refs: Vec<&StrategyRow> = rows.iter().map(|(_, r)| r).collect();
        if let Some(block) = ops::delete_block(&refs) {
            TreeNote::DeleteBlocked {
                enabled: block.enabled,
                total: block.total,
            }
            .say(window, cx);
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

    /// Open the ONE confirmation this window adds: an irreversible purge of local history.
    ///
    /// Args:
    ///     core: Core that owns the deleted strategies.
    ///     ids: Strategy ids to forget; an empty list opens nothing.
    ///     label: What the confirmation names — one strategy's name, or a count.
    ///     window: Window used to present the dialog.
    ///     cx: View context used to stage the operation.
    pub(super) fn request_forget(
        &mut self,
        core: CoreId,
        ids: Vec<u64>,
        label: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty() {
            return;
        }
        self.op = Some(TreeOp::ConfirmForget { core, ids, label });
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
        if let Some(block) = ops::delete_block(&under) {
            // Same rule as a strategy selection, and said out loud for the same reason.
            TreeNote::DeleteBlocked {
                enabled: block.enabled,
                total: block.total,
            }
            .say(window, cx);
            return;
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
    /// dispatch, including placement metadata. Warn once about folder names the core will split.
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
        let split_folders = ops::split_folder_names([spec.folder_path.as_str()]);
        self.backend
            .read(cx)
            .session
            .create_strategies(core, vec![spec])?;
        self.note_split_folders(split_folders, cx);
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
    /// Send a strategy rename to the core as an edit of its `StrategyName` field.
    ///
    /// Never a local rename: the name is the core's own, and it is what every other surface keys
    /// on. So this goes through `edit_strategies` and takes the existing Pending/TimedOut phase
    /// like any other field edit.
    ///
    /// A name already used on this core is REFUSED here, inside the dialog: names are global per
    /// core, and the returned error keeps the dialog open with the reason on screen rather than
    /// letting the core reject it silently a round trip later.
    ///
    /// Returns:
    ///     `Ok(false)` to keep the dialog open, `Ok(true)` once the edit is away.
    fn confirm_rename_strategy(
        &mut self,
        core: CoreId,
        id: u64,
        name: String,
        workspace_generation: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Result<bool> {
        if !tree_op_authorized(
            workspace_generation,
            self.action_workspace_generation(cx),
            self.workspace_cores.as_deref(),
            core,
        ) {
            return Ok(true);
        }
        let name = name.trim().to_string();
        let old = {
            let store = self.backend.read(cx).session.store();
            let Some(cd) = store.core(core) else {
                return Ok(true);
            };
            if ops::name_taken(&cd.strategies, id, &name) {
                return Err(anyhow::anyhow!(
                    t!("strat.rename_taken", name = name).to_string()
                ));
            }
            row(store, core, id)
                .map(|r| r.name.clone())
                .unwrap_or_default()
        };
        // Nothing to send, and nothing to report: the operator confirmed the name it already had.
        if old == name {
            return Ok(true);
        }
        self.backend.read(cx).session.edit_strategies(
            core,
            vec![(
                id,
                vec![(ops::STRATEGY_NAME_FIELD.to_string(), name.clone())],
            )],
        )?;
        self.pending_notes
            .push(TreeNote::RenameSent { old, new: name });
        cx.notify();
        Ok(true)
    }

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
        let mut new_path = old_path.to_vec();
        if let Some(leaf) = new_path.last_mut() {
            *leaf = new_name.to_string();
        }
        let moves = {
            let store = self.backend.read(cx).session.store();
            let Some(cd) = store.core(core) else {
                return Ok(());
            };
            ops::rename_folder(&cd.strategies, old_path, new_name)
        };
        // The subtree that moved travels WITH the rows, because rewriting their paths leaves the
        // old folder behind on a core that keeps folders of its own. A folder holding no strategy
        // has no rows at all, and its rename is this same edit with an empty move list.
        self.backend.read(cx).session.move_strategies(
            core,
            moves,
            Some((ops::join_path(old_path), ops::join_path(&new_path))),
        )?;
        // Rename an empty UI-only folder locally only after the move command succeeds.
        self.rename_ui_folder(core, old_path, new_name);
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
        // Two shapes, and which one applies is decided by what the folder HOLDS, not only by what
        // the core can do. Omission from the desired tree removes a folder and nothing else — the
        // core keeps any folder a strategy still occupies, and moonproto re-adds it — so it reaches
        // exactly the folder the legacy command cannot: an empty one on a core that keeps a tree.
        // A folder with strategies in it still goes the legacy way, which deletes the rows with it.
        let by_omission = {
            let store = self.backend.read(cx).session.store();
            store
                .core(core)
                .is_some_and(|cd| cd.folders.editable && !ops::has_row_under(&cd.strategies, path))
        };
        let backend = self.backend.read(cx);
        match by_omission {
            true => backend
                .session
                .remove_core_folder(core, ops::join_path(path))?,
            false => backend.session.delete_folder(core, ops::join_path(path))?,
        }
        self.remove_ui_folder(core, path);
        self.persist_session(cx);
        Ok(())
    }
}
