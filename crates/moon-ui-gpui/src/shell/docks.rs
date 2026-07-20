//! Док-механика окна группы: отцепление панели в своё окно, возврат закрытой/откреплённой
//! панели на «домашнюю» вкладку, репин по запросу и персист геометрии ОС-окна. Вынесено из
//! `shell.rs`. Методы, дёргаемые из `mod.rs` (`new`), помечены `pub(super)`.

use gpui::*;

use moon_ui::{DockArea, DockPlacement, DockSplitPlacement, PanelInfo, PanelState};

use moon_core::config::GroupLayout;
use moon_core::config::layout::DockSplitSlot;

use crate::{Backend, detached};

use super::Shell;

/// Имена dock-панелей нижней строки в порядке их «домашних» позиций. Возврат
/// откреплённой/закрытой панели вставляет её в TabPanel на индекс, сохраняющий этот
/// порядок (см. [`dock_home_priority`]).
///
/// The array must list ALL bottom-row tabs (`shell/init.rs`), not only those whose order seems to
/// matter: [`strip_names`] identifies the "home" strip by the presence of any name from here, so a
/// missing panel left alone in the strip makes that strip unfindable — restoring a detached panel
/// then falls back to `add_panel(Bottom)` and creates a second bottom zone instead of inserting
/// into the existing one.
///
/// The identification is a heuristic, and completeness cuts both ways: because it matches the FIRST
/// strip holding any of these names, a user who has dragged one of them into a different strip that
/// comes earlier in depth-first order makes that strip win. Restoring by a stable strip identity
/// rather than by name would remove the ambiguity, but that is a MoonUI-side change.
// «CoreStatus» временно убран из строки вкладок (панель отключена до появления метрик в
// moonproto; см. shell/init.rs). Вернуть сюда при повторном включении.
pub(super) const DOCK_TAB_ORDER: [&str; 5] = ["Orders", "Assets", "Report", "Alerts", "Log"];

/// Home index of a bottom-row panel (`Orders < Assets < Report < Alerts < Log`).
///
/// The returning panel is inserted at this position, clamped to the current tab count, so a
/// partially detached set still preserves the configured relative order.
///
/// The order must mirror the push order in `shell/init.rs`, which is what a FRESH layout renders.
/// An existing user is unaffected either way: a saved layout whose `DOCK_VERSION` still matches is
/// restored verbatim, so changing this array reorders nothing already on screen — closing a tab and
/// letting it come back is what re-seats it here.
fn dock_home_priority(name: &str) -> usize {
    DOCK_TAB_ORDER
        .iter()
        .position(|n| *n == name)
        .unwrap_or(DOCK_TAB_ORDER.len())
}

impl Shell {
    pub(super) fn drain_repin_requests(&mut self, cx: &mut Context<Self>) {
        let group = self.group.clone();
        let repins: Vec<String> = self.backend.update(cx, |b, _| {
            let mut mine = Vec::new();
            b.repin_request.retain(|(g, p)| {
                if *g == group {
                    mine.push(p.clone());
                    false
                } else {
                    true
                }
            });
            mine
        });
        if repins.is_empty() {
            return;
        }
        let backend = self.backend.clone();
        let dock = self.dock.clone();
        let handle = self.window_handle;
        cx.defer(move |app| {
            let _ = handle.update(app, move |_, window, app| {
                for panel_name in repins {
                    // Возврат из ОКНА — на запомненное место (в т.ч. в сплит).
                    restore_panel_to_home_tabs(
                        &dock,
                        &backend,
                        &group,
                        &panel_name,
                        true,
                        window,
                        app,
                    );
                    backend.update(app, |b, _| {
                        b.detached
                            .retain(|s| !(s.group == group && s.panel == panel_name));
                        b.detached_dirty = true;
                    });
                }
            });
        });
    }

    pub(super) fn defer_detach_panel(&mut self, panel_name: String, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let dock = self.dock.clone();
        let group = self.group.clone();
        let handle = self.window_handle;
        cx.defer(move |app| {
            let _ = handle.update(app, move |_, window, app| {
                // Assets отцепляется как обычная панель (per-group окно + удаление вкладки +
                // репин при закрытии). Глобальное окно «все ядра» открывается отдельно —
                // кнопкой «⧉» в тулбаре панели, не даблкликом.
                if !detached::supports_panel(&panel_name) {
                    return;
                }
                let spec = detached::DetachedSpec::with_saved_geom(
                    &backend,
                    app,
                    group.clone(),
                    panel_name.clone(),
                );
                if backend
                    .read(app)
                    .detached
                    .iter()
                    .any(|s| s.group == spec.group && s.panel == spec.panel)
                {
                    return;
                }
                // Запоминаем размещение панели в доке ДО удаления — чтобы возврат встал на то же
                // место. Split-лист (рядом с соседом) → сплит-слот; иначе вкладка → индекс. Хранилища
                // взаимоисключающи (панель либо там, либо там) — второе чистим. None → не трогаем.
                let key = format!("{group}:{panel_name}");
                match captured_slot(&dock, &panel_name, app) {
                    Some(DockSlot::Split {
                        siblings,
                        slot_panels,
                        index,
                        placement,
                        size,
                        sibling_size,
                    }) => {
                        backend.update(app, |b, _| {
                            b.layout.dock_split_slot.insert(
                                key.clone(),
                                DockSplitSlot {
                                    siblings,
                                    slot_panels,
                                    index,
                                    placement,
                                    size,
                                    sibling_size,
                                },
                            );
                            b.layout.dock_tab_index.remove(&key);
                            b.layout_dirty = true;
                        });
                    }
                    Some(DockSlot::Tab { ix, left }) => {
                        log::info!(
                            "[dock] detach {key}: tab ix={ix} left={:?}",
                            left.as_deref().unwrap_or("<leftmost>")
                        );
                        backend.update(app, |b, _| {
                            b.layout.dock_tab_index.insert(key.clone(), ix);
                            // Пустая строка = была крайней слева; иначе имя соседа.
                            b.layout
                                .dock_tab_left
                                .insert(key.clone(), left.unwrap_or_default());
                            b.layout.dock_split_slot.remove(&key);
                            b.layout_dirty = true;
                        });
                    }
                    None => {}
                }
                let owner = window.window_handle();
                if let Err(err) = detached::spawn(app, &backend, &spec, Some(owner)) {
                    log::warn!(
                        "detach panel failed group={} panel={}: {err:#}",
                        group,
                        panel_name
                    );
                    return;
                }
                dock.update(app, |area, cx| {
                    area.remove_panel_by_name(&panel_name, window, cx);
                });
                backend.update(app, |b, _| {
                    b.detached.push(spec);
                    b.detached_dirty = true;
                });
            });
        });
    }

    pub(super) fn defer_restore_closed_panel(
        &mut self,
        panel_name: String,
        cx: &mut Context<Self>,
    ) {
        if !detached::supports_panel(&panel_name) {
            return;
        }
        let backend = self.backend.clone();
        let dock = self.dock.clone();
        let group = self.group.clone();
        let handle = self.window_handle;
        cx.defer(move |app| {
            let _ = handle.update(app, move |_, window, app| {
                // × на ДОКНУТОЙ панели = сброс размещения: убрать из сплита и вернуть ПРОСТО
                // вкладкой в линию (её место). Чистим split-слот, чтобы не восстановился сплитом.
                let key = format!("{group}:{panel_name}");
                backend.update(app, |b, _| {
                    if b.layout.dock_split_slot.remove(&key).is_some() {
                        b.layout_dirty = true;
                    }
                });
                restore_panel_to_home_tabs(
                    &dock,
                    &backend,
                    &group,
                    &panel_name,
                    false,
                    window,
                    app,
                );
            });
        });
    }

    pub(super) fn persist_group_geometry(&mut self, window: &Window, cx: &mut Context<Self>) {
        let (bounds, maximized, fullscreen) = match window.window_bounds() {
            WindowBounds::Windowed(bounds) => (Some(bounds), false, false),
            WindowBounds::Maximized(bounds) => (Some(bounds), true, false),
            WindowBounds::Fullscreen(bounds) => (Some(bounds), false, true),
        };
        let Some(bounds) = bounds else {
            return;
        };
        // Монитор окна — по стабильному uuid: на macOS x/y относительны своему экрану и
        // восстановить дисплей по ним нельзя (см. GroupLayout::display_uuid).
        let display_uuid = window
            .display(cx)
            .and_then(|d| d.uuid().ok())
            .map(|u| u.to_string());
        let layout = GroupLayout {
            x: f32::from(bounds.origin.x) as i32,
            y: f32::from(bounds.origin.y) as i32,
            w: f32::from(bounds.size.width) as u32,
            h: f32::from(bounds.size.height) as u32,
            maximized,
            fullscreen,
            collapsed: false,
            tab: 0,
            dock_h: 220.0,
            orders_primary: 0,
            orders_newest_first: true,
            orders_only_current: false,
            orders_kind: 0,
            display_uuid,
        };
        let group = self.group.clone();
        self.backend.update(cx, |backend, _| {
            let changed = backend
                .layout
                .groups
                .get(&group)
                .map(|old| {
                    old.x != layout.x
                        || old.y != layout.y
                        || old.w != layout.w
                        || old.h != layout.h
                        || old.maximized != layout.maximized
                        || old.fullscreen != layout.fullscreen
                        || old.display_uuid != layout.display_uuid
                })
                .unwrap_or(true);
            if changed {
                backend.layout.groups.insert(group, layout);
                backend.layout_dirty = true;
            }
        });
    }
}

/// Имена панелей (слева-направо) ПЕРВОЙ tab-полосы, содержащей любую из [`DOCK_TAB_ORDER`] —
/// «домашней» линии вкладок. Для устойчивого вычисления индекса возврата по левому соседу.
fn strip_names(node: &PanelState) -> Option<Vec<String>> {
    if let PanelInfo::Tabs { .. } = &node.info {
        let names: Vec<String> = node.children.iter().map(|c| c.panel_name.clone()).collect();
        if names.iter().any(|n| DOCK_TAB_ORDER.contains(&n.as_str())) {
            return Some(names);
        }
    }
    node.children.iter().find_map(strip_names)
}

fn restore_panel_to_home_tabs(
    dock: &Entity<DockArea>,
    backend: &Entity<Backend>,
    group: &str,
    panel_name: &str,
    prefer_split: bool,
    window: &mut Window,
    app: &mut App,
) {
    let key = format!("{group}:{panel_name}");
    log::info!("[dock] restore {key}: begin (prefer_split={prefer_split})");
    let Some(panel) = detached::build_panel(panel_name, group, backend, window, app) else {
        log::info!("[dock] restore {key}: build_panel returned none");
        return;
    };
    log::info!("[dock] restore {key}: panel built");
    // Приоритет размещения при возврате:
    //   1) split-слот — вернуть РЯДОМ с соседом сплитом (если сосед сейчас в доке) — ТОЛЬКО когда
    //      `prefer_split` (возврат из окна); × на докнутой панели (prefer_split=false) сбрасывает
    //      в линию вкладок;
    //   2) запомненный индекс вкладки — в домашнюю tab-полосу на то же место;
    //   3) каноничный priority.
    // `insert_*` клампят индекс/находят соседа сами; при промахе (1) падаем в (2)/(3) тем же
    // `panel` (в beside/into_home передаём clone, оригинал держим на фолбэк).
    let split = prefer_split
        .then(|| backend.read(app).layout.dock_split_slot.get(&key).cloned())
        .flatten();
    let tab_ix = backend.read(app).layout.dock_tab_index.get(&key).copied();
    // Левый сосед на момент открепления (пустая строка = была крайней слева, None = не сохраняли).
    let tab_left = backend.read(app).layout.dock_tab_left.get(&key).cloned();
    // Живой порядок домашней tab-полосы — ОТДЕЛЬНЫМ dump-апдейтом (как `captured_slot`), НЕ внутри
    // апдейта со вставкой: dump обходит и читает ВСЕ панели дока; делать это в том же апдейте, где
    // мы мутируем дерево (remove/insert), — риск реентранси/дедлока. Отдельный апдейт безопасен.
    let strip: Vec<String> = dock.update(app, |area, cx| {
        let state = area.dump(cx);
        strip_names(&state.center)
            .or_else(|| {
                state
                    .bottom_dock
                    .as_ref()
                    .and_then(|d| strip_names(&d.panel))
            })
            .or_else(|| state.left_dock.as_ref().and_then(|d| strip_names(&d.panel)))
            .or_else(|| {
                state
                    .right_dock
                    .as_ref()
                    .and_then(|d| strip_names(&d.panel))
            })
            .unwrap_or_default()
    });
    log::info!(
        "[dock] restore {key}: strip={strip:?} tab_ix={tab_ix:?} left={:?}",
        tab_left.as_deref()
    );
    dock.update(app, |area, cx| {
        area.remove_panel_by_name(panel_name, window, cx);
        if let Some(slot) = &split {
            let placement = match slot.placement {
                1 => DockSplitPlacement::Right,
                2 => DockSplitPlacement::Top,
                3 => DockSplitPlacement::Bottom,
                _ => DockSplitPlacement::Left,
            };
            // 0.0 = flex → None (без фиксированного размера слота).
            let panel_size = (slot.size > 0.0).then_some(slot.size);
            let sibling_size = (slot.sibling_size > 0.0).then_some(slot.sibling_size);
            let anchors: Vec<&str> = slot.siblings.iter().map(|s| s.as_str()).collect();
            let slot_panels: Vec<&str> = slot.slot_panels.iter().map(|s| s.as_str()).collect();
            if !anchors.is_empty()
                && area.insert_panel_beside_sibling(
                    panel.clone(),
                    &anchors,
                    &slot_panels,
                    slot.index,
                    placement,
                    panel_size,
                    sibling_size,
                    window,
                    cx,
                )
            {
                log::info!("[dock] restore {key}: placed beside sibling (split)");
                return;
            }
        }
        // Индекс возврата — по ЖИВОЙ полосе (`strip`, посчитана выше) относительно левого соседа:
        // место не съезжает, даже если полоса менялась. Соседа нет → фолбэк на сырой `tab_ix` →
        // каноничный priority. Пустой сосед = была крайней слева (ix=0).
        let ix = match tab_left.as_deref() {
            Some("") => 0,
            Some(name) => strip
                .iter()
                .position(|n| n == name)
                .map(|p| p + 1)
                .or(tab_ix)
                .unwrap_or_else(|| dock_home_priority(panel_name)),
            None => tab_ix.unwrap_or_else(|| dock_home_priority(panel_name)),
        };
        log::info!("[dock] restore {key}: -> ix={ix}, inserting");
        if !area.insert_panel_into_home_tabs(panel.clone(), ix, &DOCK_TAB_ORDER, window, cx) {
            area.add_panel(panel, DockPlacement::Bottom, None, window, cx);
        }
        log::info!("[dock] restore {key}: inserted");
    });
    log::info!("[dock] restore {key}: done");
}

/// Запомненное размещение панели в доке (для восстановления при возврате).
enum DockSlot {
    /// Панель была вкладкой в tab-полосе — на позиции `ix`, справа от соседа `left`
    /// (`None` = крайняя слева). Возврат предпочитает соседа (устойчив к сдвигу полосы).
    Tab { ix: usize, left: Option<String> },
    /// Панель была ОТДЕЛЬНЫМ листом в сплите. `siblings` — соседи-якоря (для поиска сплита),
    /// `slot_panels` — панели соседнего слота (обернуть целиком при схлопе), `index` — её позиция
    /// в сплите, `placement` — сторона, `size`/`sibling_size` — пиксельные размеры слотов (0.0 =
    /// flex) для восстановления пропорции.
    Split {
        siblings: Vec<String>,
        slot_panels: Vec<String>,
        index: usize,
        placement: u8,
        size: f32,
        sibling_size: f32,
    },
}

/// Размещение панели `panel_name` в доке (обходя дамп). Split-лист (одиночная панель прямо в
/// сплите) → [`DockSlot::Split`] с соседом и стороной; вкладка → [`DockSlot::Tab`] с индексом;
/// `None`, если не нашли. Обход по всем зонам, первое совпадение.
fn captured_slot(dock: &Entity<DockArea>, panel_name: &str, app: &mut App) -> Option<DockSlot> {
    /// Первое имя панели-листа в поддереве (для якоря-соседа).
    fn first_panel_name(node: &PanelState) -> Option<String> {
        if matches!(node.info, PanelInfo::Panel(_)) && !node.panel_name.is_empty() {
            return Some(node.panel_name.clone());
        }
        node.children.iter().find_map(first_panel_name)
    }
    /// Все имена панелей-листьев в поддереве (панели соседнего слота — обернуть целиком).
    fn collect_panel_names(node: &PanelState, out: &mut Vec<String>) {
        if matches!(node.info, PanelInfo::Panel(_)) && !node.panel_name.is_empty() {
            out.push(node.panel_name.clone());
        }
        for c in &node.children {
            collect_panel_names(c, out);
        }
    }
    fn walk(node: &PanelState, target: &str) -> Option<DockSlot> {
        match &node.info {
            PanelInfo::Tabs { .. } => {
                if let Some(ix) = node.children.iter().position(|c| c.panel_name == target) {
                    // Левый сосед (для устойчивого возврата): панель на ix-1, пустое имя пропускаем.
                    let left = ix.checked_sub(1).and_then(|j| {
                        node.children
                            .get(j)
                            .map(|c| c.panel_name.clone())
                            .filter(|n| !n.is_empty())
                    });
                    return Some(DockSlot::Tab { ix, left });
                }
            }
            PanelInfo::Stack { axis, sizes } => {
                // Панель = отдельный лист прямо в сплите (не вложена в Tabs) → сплит-слот.
                for (i, child) in node.children.iter().enumerate() {
                    let is_leaf_target =
                        child.panel_name == target && matches!(child.info, PanelInfo::Panel(_));
                    if is_leaf_target {
                        let horizontal = *axis == 0;
                        // Сосед — соседний слот; сторона панели относительно него (для схлопа 2→1).
                        let (sib_ix, target_after) =
                            if i > 0 { (i - 1, true) } else { (i + 1, false) };
                        let placement = match (horizontal, target_after) {
                            (true, true) => 1,   // справа от соседа
                            (true, false) => 0,  // слева
                            (false, true) => 3,  // снизу
                            (false, false) => 2, // сверху
                        };
                        // Все соседи по сплиту — якоря для поиска сплита при возврате.
                        let siblings: Vec<String> = node
                            .children
                            .iter()
                            .enumerate()
                            .filter(|(j, _)| *j != i)
                            .filter_map(|(_, c)| first_panel_name(c))
                            .collect();
                        if siblings.is_empty() {
                            return None;
                        }
                        // Панели СОСЕДНЕГО слота целиком (слот мог быть вложенным сплитом).
                        let mut slot_panels = Vec::new();
                        if let Some(sib) = node.children.get(sib_ix) {
                            collect_panel_names(sib, &mut slot_panels);
                        }
                        // Пиксельные размеры слотов (0.0 = flex) — вернуть прежнюю пропорцию.
                        let size = sizes.get(i).copied().unwrap_or(0.0);
                        let sibling_size = sizes.get(sib_ix).copied().unwrap_or(0.0);
                        return Some(DockSlot::Split {
                            siblings,
                            slot_panels,
                            index: i,
                            placement,
                            size,
                            sibling_size,
                        });
                    }
                }
            }
            _ => {}
        }
        node.children.iter().find_map(|c| walk(c, target))
    }
    let state = dock.update(app, |area, cx| area.dump(cx));
    walk(&state.center, panel_name)
        .or_else(|| {
            state
                .bottom_dock
                .as_ref()
                .and_then(|d| walk(&d.panel, panel_name))
        })
        .or_else(|| {
            state
                .left_dock
                .as_ref()
                .and_then(|d| walk(&d.panel, panel_name))
        })
        .or_else(|| {
            state
                .right_dock
                .as_ref()
                .and_then(|d| walk(&d.panel, panel_name))
        })
}

#[cfg(test)]
mod tests {
    // NOT `use super::*`: the glob would pull in the `gpui::test` macro, and `#[test]` would
    // expand into itself (recursion limit).
    use super::DOCK_TAB_ORDER;
    use crate::panel_meta::tab_label;

    #[test]
    fn every_home_tab_has_a_localized_label() {
        // Pins the link between TWO files rather than the map's own behaviour: adding a panel
        // touches nine places (see CLAUDE.md), and "added the name to DOCK_TAB_ORDER, forgot the
        // label" reddens here. It does NOT claim every panel calls this helper, and it does not
        // check translation completeness in locales/.
        for name in DOCK_TAB_ORDER {
            assert!(
                tab_label(name).is_some(),
                "{name} is a home dock tab but has no entry in panel_meta::tab_label"
            );
        }
    }
}
