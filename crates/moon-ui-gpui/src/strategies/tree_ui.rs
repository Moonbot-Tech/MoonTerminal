//! UI операций над деревом стратегий: тулбар выделения, модалки (создать/переименовать/
//! подтвердить удаление), ПКМ-контекст-меню и диспетч в `moon-core` (группировка по ядрам).
//! Чистая логика над путями/наборами — в [`super::tree_ops`]; команды ядра — в `session`.

use super::tree_ops;
use super::*;
use anyhow::Result;
use moon_core::feed::NewStrategySpec;
use moon_ui::{MoonContextMenuWindowExt as _, MoonNotification, MoonWindowExt as _};
use rust_i18n::t;

/// Активная модалка операции (взаимоисключающая; рисуется оверлеем поверх окна).
#[derive(Clone)]
pub(super) enum TreeOp {
    /// Создать стратегию: целевая папка + выбранный вид (kind ordinal).
    CreateStrategy {
        core: CoreId,
        target: String,
        kind: Option<u8>,
    },
    /// Создать (UI-)папку: целевой родитель.
    CreateFolder { core: CoreId, target: String },
    /// Переименовать папку: ядро + путь папки (сегменты).
    RenameFolder { core: CoreId, old_path: Vec<String> },
    /// Подтверждение удаления стратегий выделения (id переderives при подтверждении).
    ConfirmDeleteStrategies { label: String },
    /// Подтверждение удаления папки: ядро + путь, подпись.
    ConfirmDeleteFolder {
        core: CoreId,
        path: Vec<String>,
        label: String,
    },
}

/// Запрос контекст-меню: цель + позиция курсора. Само открытое меню хранится в MoonUI Root.
pub(super) struct ContextMenu {
    pub(super) core: CoreId,
    pub(super) target: MenuTarget,
    pub(super) pos: Point<Pixels>,
}

pub(super) enum MenuTarget {
    Folder(Vec<String>),
    Strategy(u64),
}

/// Полезная нагрузка drag&drop: перетаскиваемые стратегии (ядро-источник + id).
#[derive(Clone)]
pub(super) struct StratDrag {
    pub(super) core: CoreId,
    pub(super) ids: Vec<u64>,
}

/// Полезная нагрузка drag&drop: перетаскиваемая папка (ядро-источник + путь).
#[derive(Clone)]
pub(super) struct FolderDrag {
    pub(super) core: CoreId,
    pub(super) path: Vec<String>,
}

/// Превью под курсором при перетаскивании.
pub(super) struct DragChip {
    pub(super) label: SharedString,
}

impl Render for DragChip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        div()
            .px_2()
            .py_1()
            .rounded(px(4.0))
            .bg(moon(p.shell_high))
            .border_1()
            .border_color(moon(p.blue))
            .text_color(moon(p.text))
            .text_size(design::t_body(cx))
            .font_family(design::mono())
            .child(self.label.clone())
    }
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
    // ── Утилиты ───────────────────────────────────────────────────────────────

    /// Виды (ordinal, имя) из схемы ядра — для выбора при создании стратегии.
    fn kinds_of(&self, store: &CoreStore, core: CoreId) -> Vec<(u8, String)> {
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

    /// Выбранные строки (мультивыбор) с их ядром — owned-копии (для буфера/проверок).
    fn selection_rows(&self, store: &CoreStore) -> Vec<(CoreId, StrategyRow)> {
        selected_keys(self)
            .into_iter()
            .filter_map(|(c, id)| row(store, c, id).map(|r| (c, r.clone())))
            .collect()
    }

    /// Целевая папка по умолчанию (ядро, путь) — папка первичной стратегии или корень
    /// первого ядра.
    pub(super) fn default_target(
        &self,
        store: &CoreStore,
        cores: &[(CoreId, String)],
    ) -> (CoreId, String) {
        if let Some((core, id)) = self.selected {
            if let Some(r) = row(store, core, id) {
                return (core, r.folder_path.clone());
            }
        }
        (cores.first().map(|(c, _)| *c).unwrap_or(0), String::new())
    }

    // ── Открытие модалок/меню ────────────────────────────────────────────────

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

    // ── Буфер (копировать/вставить) ──────────────────────────────────────────

    pub(super) fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let store = self.backend.read(cx).session.store();
        let rows = self.selection_rows(store);
        if rows.is_empty() {
            return;
        }
        let refs: Vec<&StrategyRow> = rows.iter().map(|(_, r)| r).collect();
        self.clipboard = Some(tree_ops::copy_rows(&refs));
        cx.notify();
    }

    pub(super) fn copy_folder(&mut self, core: CoreId, path: Vec<String>, cx: &mut Context<Self>) {
        let store = self.backend.read(cx).session.store();
        let Some(cd) = store.core(core) else { return };
        self.clipboard = Some(tree_ops::copy_folder(&cd.strategies, &path));
        cx.notify();
    }

    pub(super) fn paste_into(&mut self, core: CoreId, target: String, cx: &mut Context<Self>) {
        let Some(clip) = self.clipboard.clone() else {
            return;
        };
        let specs = {
            let store = self.backend.read(cx).session.store();
            let taken: std::collections::HashSet<String> = store
                .core(core)
                .map(|cd| cd.strategies.iter().map(|r| r.name.clone()).collect())
                .unwrap_or_default();
            let plan = tree_ops::paste_plan(&clip, &tree_ops::split_path(&target), &taken);
            specs_from(plan)
        };
        // Имя первой вставленной — выберем её, как только ядро пришлёт эхо.
        let first_name = specs.first().and_then(|s| {
            s.fields
                .iter()
                .find(|(n, _)| n == tree_ops::STRATEGY_NAME_FIELD)
                .map(|(_, v)| v.clone())
        });
        if let Err(error) = self.backend.read(cx).session.create_strategies(core, specs) {
            log::warn!("paste strategies failed: {error}");
            return;
        }
        // Новые/вставленные стратегии всегда выключены — снимаем «только активные», иначе их
        // не видно. Раскрываем целевое ядро, чтобы результат был на виду.
        self.filter.only_active = false;
        self.expanded_cores.insert(core);
        self.pending_select = first_name.map(|n| (core, n));
        cx.notify();
    }

    // ── Drag & Drop ───────────────────────────────────────────────────────────

    /// Сбросить перетаскиваемые СТРАТЕГИИ в целевую папку (`target` пуст = корень ядра).
    /// В пределах ядра — перенос (`move_strategies`); между ядрами — копирование.
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
                tree_ops::move_to(&rows, &target)
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
                let clip = tree_ops::copy_rows(&rows);
                let taken: std::collections::HashSet<String> = store
                    .core(target_core)
                    .map(|c| c.strategies.iter().map(|r| r.name.clone()).collect())
                    .unwrap_or_default();
                specs_from(tree_ops::paste_plan(&clip, &target, &taken))
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

    /// Сбросить перетаскиваемую ПАПКУ в целевую папку-родитель (`target` пуст = корень).
    /// В пределах ядра — перенос поддерева; между ядрами — копирование.
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
                    .map(|c| tree_ops::move_folder(&c.strategies, &path, &target))
                    .unwrap_or_default()
            };
            if moves.is_empty() {
                return; // в себя/потомка или пустая папка
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
                    .map(|c| tree_ops::copy_folder(&c.strategies, &path))
                    .unwrap_or_default();
                let taken: std::collections::HashSet<String> = store
                    .core(target_core)
                    .map(|c| c.strategies.iter().map(|r| r.name.clone()).collect())
                    .unwrap_or_default();
                specs_from(tree_ops::paste_plan(&clip, &target, &taken))
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

    /// Список id для перетаскивания стратегии: весь мультивыбор этого ядра, если строка в
    /// выборе; иначе только она.
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

    // ── UI-папки (пустые, до наполнения) ──────────────────────────────────────

    fn add_ui_folder(&mut self, core: CoreId, parent: &str, name: &str) {
        let mut parts = tree_ops::split_path(parent);
        parts.push(name.to_string());
        self.ui_folders.insert((core, tree_ops::join_path(&parts)));
        // Раскрыть ядро и родительскую цепочку (все сегменты, кроме новой папки), чтобы она
        // была сразу видна.
        self.expanded_cores.insert(core);
        let ancestors = parts.len().saturating_sub(1);
        self.expand_path(core, parts.iter().take(ancestors).map(String::as_str));
    }

    fn remove_ui_folder(&mut self, core: CoreId, path: &[String]) {
        let key = tree_ops::join_path(path);
        self.ui_folders
            .retain(|(c, p)| !(*c == core && (p == &key || p.starts_with(&format!("{key}/")))));
    }

    fn rename_ui_folder(&mut self, core: CoreId, old_path: &[String], new_name: &str) {
        if old_path.is_empty() {
            return;
        }
        let old_key = tree_ops::join_path(old_path);
        let mut np = old_path.to_vec();
        *np.last_mut().unwrap() = new_name.to_string();
        let new_key = tree_ops::join_path(&np);
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

    /// Пустые UI-папки данного ядра (сегменты пути) — для подмешивания в дерево.
    pub(super) fn ui_folder_paths(&self, core: CoreId) -> Vec<Vec<String>> {
        self.ui_folders
            .iter()
            .filter(|(c, _)| *c == core)
            .map(|(_, p)| tree_ops::split_path(p))
            .collect()
    }

    // ── Клавиатура (Ctrl+C / Ctrl+V / Delete) ────────────────────────────────

    pub(super) fn handle_tree_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let m = &ev.keystroke.modifiers;
        let key = ev.keystroke.key.as_str();
        if m.control && key == "c" {
            self.copy_selection(cx);
        } else if m.control && key == "v" {
            let (core, target) = {
                let store = self.backend.read(cx).session.store();
                let cores: Vec<(CoreId, String)> = self
                    .backend
                    .read(cx)
                    .session
                    .sessions()
                    .iter()
                    .map(|s| (s.id, s.name.clone()))
                    .collect();
                self.default_target(store, &cores)
            };
            self.paste_into(core, target, cx);
        } else if key == "delete" {
            self.request_delete_selection(window, cx);
        }
    }

    // ── Контекст-меню (ПКМ) ───────────────────────────────────────────────────

    pub(super) fn open_menu(
        &mut self,
        menu: ContextMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.op = None;
        self.op_input = None;
        let pos = menu.pos;
        let items = self.context_menu_items(&menu, cx);
        window.open_moon_context_menu(cx, "strategies-context-menu", pos, items, 190.0);
        cx.notify();
    }

    // ── Рендер: тулбар выделения ──────────────────────────────────────────────

    /// Кнопки операций над выделением/буфером (в нижней панели действий).
    pub(super) fn selection_toolbar(&self, store: &CoreStore, cx: &Context<Self>) -> AnyElement {
        let rows = self.selection_rows(store);
        let has_sel = !rows.is_empty();
        let all_off = rows.iter().all(|(_, r)| !r.checked);
        let can_paste = self.clipboard.is_some();
        // Левая группа фикс. ширины: ряд [копировать][вставить] (каждая тянется на свою
        // половину), под ними [удалить] во всю ширину — через `MoonButton::full_width()`.
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
                                    // вставка в папку первичной стратегии (или корень).
                                    let (core, target) = {
                                        let store = this.backend.read(cx).session.store();
                                        let cores: Vec<(CoreId, String)> = this
                                            .backend
                                            .read(cx)
                                            .session
                                            .sessions()
                                            .iter()
                                            .map(|s| (s.id, s.name.clone()))
                                            .collect();
                                        this.default_target(store, &cores)
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

    /// Кнопка «＋ Создать» (дропдаун: стратегия/папка) для шапки дерева.
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
            .label(format!("＋ {} ▾", t!("strat.menu_create")))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(110.0)
            .menu_width(180.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
            .into_any_element()
    }

    // ── Контекст-меню ───────────────────────────────────────────────────────

    fn context_menu_items(&self, menu: &ContextMenu, cx: &Context<Self>) -> Vec<MoonMenuItem> {
        let core = menu.core;
        let can_paste = self.clipboard.is_some();
        let view = cx.entity();

        let mut items: Vec<MoonMenuItem> = Vec::new();
        match &menu.target {
            MenuTarget::Folder(path) => {
                let pp = path.clone();
                items.push(
                    MoonMenuItem::with_key("rename-folder", t!("strat.menu_rename").to_string())
                        .on_click({
                            let view = view.clone();
                            move |_, window, app| {
                                window.close_context_menu(app);
                                view.update(app, |this, cx| {
                                    this.open_rename_folder(core, pp.clone(), window, cx);
                                });
                            }
                        }),
                );
                let pp = path.clone();
                items.push(
                    MoonMenuItem::with_key("copy-folder", t!("strat.menu_copy").to_string())
                        .on_click({
                            let view = view.clone();
                            move |_, window, app| {
                                window.close_context_menu(app);
                                view.update(app, |this, cx| {
                                    this.copy_folder(core, pp.clone(), cx);
                                    cx.notify();
                                });
                            }
                        }),
                );
                if can_paste {
                    let t = tree_ops::join_path(path);
                    items.push(
                        MoonMenuItem::with_key(
                            "paste-here",
                            t!("strat.menu_paste_here").to_string(),
                        )
                        .on_click({
                            let view = view.clone();
                            move |_, window, app| {
                                window.close_context_menu(app);
                                view.update(app, |this, cx| {
                                    this.paste_into(core, t.clone(), cx);
                                    cx.notify();
                                });
                            }
                        }),
                    );
                }
                let t = tree_ops::join_path(path);
                items.push(
                    MoonMenuItem::with_key(
                        "new-strategy-here",
                        t!("strat.menu_new_strategy_here").to_string(),
                    )
                    .on_click({
                        let view = view.clone();
                        move |_, window, app| {
                            window.close_context_menu(app);
                            view.update(app, |this, cx| {
                                this.open_create_strategy(core, t.clone(), window, cx);
                            });
                        }
                    }),
                );
                let t = tree_ops::join_path(path);
                items.push(
                    MoonMenuItem::with_key(
                        "new-folder-here",
                        t!("strat.menu_new_folder_here").to_string(),
                    )
                    .on_click({
                        let view = view.clone();
                        move |_, window, app| {
                            window.close_context_menu(app);
                            view.update(app, |this, cx| {
                                this.open_create_folder(core, t.clone(), window, cx);
                            });
                        }
                    }),
                );
                let pp = path.clone();
                items.push(
                    MoonMenuItem::with_key(
                        "delete-folder",
                        t!("strat.menu_delete_folder").to_string(),
                    )
                    .tone(MoonTone::Danger)
                    .on_click({
                        let view = view.clone();
                        move |_, window, app| {
                            window.close_context_menu(app);
                            view.update(app, |this, cx| {
                                this.request_delete_folder(core, pp.clone(), window, cx);
                            });
                        }
                    }),
                );
            }
            MenuTarget::Strategy(_id) => {
                items.push(
                    MoonMenuItem::with_key("copy-strategy", t!("strat.menu_copy").to_string())
                        .on_click({
                            let view = view.clone();
                            move |_, window, app| {
                                window.close_context_menu(app);
                                view.update(app, |this, cx| {
                                    this.copy_selection(cx);
                                    cx.notify();
                                });
                            }
                        }),
                );
                items.push(
                    MoonMenuItem::with_key(
                        "delete-strategy",
                        t!("strat.menu_delete_strategy").to_string(),
                    )
                    .tone(MoonTone::Danger)
                    .on_click({
                        let view = view.clone();
                        move |_, window, app| {
                            window.close_context_menu(app);
                            view.update(app, |this, cx| {
                                this.request_delete_selection(window, cx);
                            });
                        }
                    }),
                );
            }
        }

        items
    }
}

/// Преобразовать план вставки/создания в команды ядра.
fn specs_from(plan: Vec<tree_ops::NewStrategy>) -> Vec<NewStrategySpec> {
    plan.into_iter()
        .map(|n| NewStrategySpec {
            kind_ordinal: n.kind_ordinal,
            folder_path: n.folder_path,
            fields: n.fields,
        })
        .collect()
}
