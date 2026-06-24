//! Типы операций над деревом стратегий (модалки/меню/DnD-нагрузки) и общие утилиты
//! выделения/UI-папок/тулбара. Модалки — в [`super::tree_dialogs`], буфер/DnD —
//! в [`super::tree_dnd`], контекст-меню — в [`super::tree_menu`]. Чистая логика над
//! путями/наборами — в [`super::tree_ops`].

use super::tree_ops;
use super::*;
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

impl StrategiesView {
    // ── Утилиты ───────────────────────────────────────────────────────────────

    /// Виды (ordinal, имя) из схемы ядра — для выбора при создании стратегии.
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

    /// Выбранные строки (мультивыбор) с их ядром — owned-копии (для буфера/проверок).
    pub(super) fn selection_rows(&self, store: &CoreStore) -> Vec<(CoreId, StrategyRow)> {
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

    // ── UI-папки (пустые, до наполнения) ──────────────────────────────────────

    pub(super) fn add_ui_folder(&mut self, core: CoreId, parent: &str, name: &str) {
        let mut parts = tree_ops::split_path(parent);
        parts.push(name.to_string());
        self.ui_folders.insert((core, tree_ops::join_path(&parts)));
        // Раскрыть ядро и родительскую цепочку (все сегменты, кроме новой папки), чтобы она
        // была сразу видна.
        self.expanded_cores.insert(core);
        let ancestors = parts.len().saturating_sub(1);
        self.expand_path(core, parts.iter().take(ancestors).map(String::as_str));
    }

    pub(super) fn remove_ui_folder(&mut self, core: CoreId, path: &[String]) {
        let key = tree_ops::join_path(path);
        self.ui_folders
            .retain(|(c, p)| !(*c == core && (p == &key || p.starts_with(&format!("{key}/")))));
    }

    pub(super) fn rename_ui_folder(&mut self, core: CoreId, old_path: &[String], new_name: &str) {
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
}
