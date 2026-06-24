//! Буфер (копировать/вставить) и drag&drop дерева стратегий: в пределах ядра — перенос
//! (`move_strategies`), между ядрами — копирование (`create_strategies`). Чистая логика над
//! путями/наборами — в [`super::tree_ops`].

use super::tree_ops;
use super::tree_ui::{FolderDrag, StratDrag};
use super::*;
use moon_core::feed::NewStrategySpec;

impl StrategiesView {
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
