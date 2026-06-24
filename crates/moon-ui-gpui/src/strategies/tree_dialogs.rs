//! Модалки операций над деревом стратегий: создать стратегию/папку, переименовать,
//! подтвердить удаление. Само открытое окно живёт в MoonUI Root; здесь — сборка тела/
//! футера диалога и подтверждённый диспетч в `moon-core`.

use super::tree_ui::TreeOp;
use super::tree_ops;
use super::*;
use anyhow::Result;
use moon_core::feed::NewStrategySpec;
use moon_ui::{MoonNotification, MoonWindowExt as _};
use rust_i18n::t;

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
        TreeOp::CreateStrategy { core, target, kind } => {
            let kinds: Vec<(u8, String)> = backend
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
                        .label(format!("{kind_name} ▾"))
                        .trigger_variant(MoonButtonVariant::Soft)
                        .trigger_size(MoonButtonSize::Action)
                        .trigger_width(320.0)
                        .menu_width(320.0)
                        .menu_size(MoonMenuSize::Compact)
                        .menu_max_height(240.0)
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
        TreeOp::ConfirmDeleteStrategies { label } | TreeOp::ConfirmDeleteFolder { label, .. } => {
            Some(
                div()
                    .w_full()
                    .text_color(moon(p.text))
                    .child(t!("dialogs.delete_confirm", what = label).to_string())
                    .into_any_element(),
            )
        }
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
                        Ok(false) => {}
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
    // ── Открытие модалок ──────────────────────────────────────────────────────

    pub(super) fn open_create_strategy(
        &mut self,
        core: CoreId,
        target: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = self.backend.read(cx).session.store();
        let kind = self.kinds_of(store, core).first().map(|(o, _)| *o);
        self.op_input_init = String::new();
        self.op_input = None; // каждое открытие получает свежий input entity/layout
        self.op = Some(TreeOp::CreateStrategy { core, target, kind });
        self.open_op_dialog(window, cx);
        cx.notify();
    }

    pub(super) fn open_create_folder(
        &mut self,
        core: CoreId,
        target: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.op_input_init = String::new();
        self.op_input = None;
        self.op = Some(TreeOp::CreateFolder { core, target });
        self.open_op_dialog(window, cx);
        cx.notify();
    }

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
        self.op = Some(TreeOp::RenameFolder { core, old_path });
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

    fn confirm_op_dialog(&mut self, cx: &mut Context<Self>) -> Result<bool> {
        let Some(op) = self.op.clone() else {
            return Ok(true);
        };

        match op {
            TreeOp::CreateStrategy { core, target, kind } => {
                let name = self
                    .op_input
                    .as_ref()
                    .map(|i| i.read(cx).value().to_string())
                    .unwrap_or_default();
                if name.trim().is_empty() {
                    return Ok(false);
                }
                if let Some(kind) = kind {
                    self.confirm_create_strategy(core, target, kind, name, cx)?;
                }
            }
            TreeOp::CreateFolder { core, target } => {
                let name = self
                    .op_input
                    .as_ref()
                    .map(|i| i.read(cx).value().to_string())
                    .unwrap_or_default();
                if !name.trim().is_empty() {
                    self.add_ui_folder(core, &target, name.trim());
                }
            }
            TreeOp::RenameFolder { core, old_path } => {
                let name = self
                    .op_input
                    .as_ref()
                    .map(|i| i.read(cx).value().to_string())
                    .unwrap_or_default();
                if !name.trim().is_empty() {
                    self.confirm_rename_folder(core, &old_path, name.trim(), cx)?;
                }
            }
            TreeOp::ConfirmDeleteStrategies { .. } => {
                self.delete_selection(cx)?;
            }
            TreeOp::ConfirmDeleteFolder { core, path, .. } => {
                self.delete_folder(core, &path, cx)?;
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
                    .rounded(px(6.0))
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

    /// Запросить удаление стратегий выделения (с проверкой правила «все выключены»).
    pub(super) fn request_delete_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.backend.read(cx).session.store();
        let rows = self.selection_rows(store);
        if rows.is_empty() {
            return;
        }
        // Правило: удалять можно, только если ВСЕ выбранные выключены.
        if rows.iter().any(|(_, r)| r.checked) {
            return;
        }
        // Выделение может охватывать разные ядра — подтверждение одно, диспетч группирует
        // по ядрам (см. delete_selection, переderives выделение).
        self.op = Some(TreeOp::ConfirmDeleteStrategies {
            label: t!("strat.count_strategies", n = rows.len()).to_string(),
        });
        self.open_op_dialog(window, cx);
        cx.notify();
    }

    /// Запросить удаление папки (правило: все стратегии под ней выключены).
    pub(super) fn request_delete_folder(
        &mut self,
        core: CoreId,
        path: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = self.backend.read(cx).session.store();
        let Some(cd) = store.core(core) else { return };
        let under = tree_ops::rows_under(&cd.strategies, &path);
        if !tree_ops::all_off(&under) {
            return; // есть запущенные — нельзя
        }
        let label = t!(
            "strat.folder_named",
            name = path.last().cloned().unwrap_or_default()
        )
        .to_string();
        self.op = Some(TreeOp::ConfirmDeleteFolder { core, path, label });
        self.open_op_dialog(window, cx);
        cx.notify();
    }

    // ── Подтверждённый диспетч ────────────────────────────────────────────────

    fn confirm_create_strategy(
        &mut self,
        core: CoreId,
        target: String,
        kind_ord: u8,
        name: String,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let spec = {
            let store = self.backend.read(cx).session.store();
            let Some(kind) = store
                .core(core)
                .and_then(|cd| cd.schema.as_ref())
                .and_then(|s| s.kinds.iter().find(|k| k.ordinal == kind_ord).cloned())
            else {
                return Ok(());
            };
            let ns = tree_ops::new_strategy(&kind, &name, &target);
            NewStrategySpec {
                kind_ordinal: ns.kind_ordinal,
                folder_path: ns.folder_path,
                fields: ns.fields,
            }
        };
        self.backend
            .read(cx)
            .session
            .create_strategies(core, vec![spec])?;
        // Новая стратегия выключена — снимаем «только активные» и раскрываем ядро, чтобы её видеть.
        self.filter.only_active = false;
        self.expanded_cores.insert(core);
        // Выберем её, как только ядро пришлёт эхо.
        self.pending_select = Some((core, name));
        Ok(())
    }

    fn confirm_rename_folder(
        &mut self,
        core: CoreId,
        old_path: &[String],
        new_name: &str,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let moves = {
            let store = self.backend.read(cx).session.store();
            let Some(cd) = store.core(core) else {
                return Ok(());
            };
            tree_ops::rename_folder(&cd.strategies, old_path, new_name)
        };
        self.backend.read(cx).session.move_strategies(core, moves)?;
        // UI-папка (пустая) — переименовать локально только после успешной отправки команды.
        self.rename_ui_folder(core, old_path, new_name);
        Ok(())
    }

    /// Удалить выделение (группировка по ядрам; правило уже проверено в request_).
    fn delete_selection(&mut self, cx: &mut Context<Self>) -> Result<()> {
        let rows = {
            let store = self.backend.read(cx).session.store();
            self.selection_rows(store)
        };
        {
            let b = self.backend.read(cx);
            for (core, r) in &rows {
                b.session.delete_strategy(*core, r.id)?;
            }
        }
        self.sel.clear();
        self.selected = None;
        Ok(())
    }

    fn delete_folder(
        &mut self,
        core: CoreId,
        path: &[String],
        cx: &mut Context<Self>,
    ) -> Result<()> {
        self.backend
            .read(cx)
            .session
            .delete_folder(core, tree_ops::join_path(path))?;
        self.remove_ui_folder(core, path);
        Ok(())
    }
}
