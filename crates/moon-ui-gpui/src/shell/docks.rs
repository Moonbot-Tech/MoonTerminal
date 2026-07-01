//! Док-механика окна группы: отцепление панели в своё окно, возврат закрытой/откреплённой
//! панели на «домашнюю» вкладку, репин по запросу и персист геометрии ОС-окна. Вынесено из
//! `shell.rs`. Методы, дёргаемые из `mod.rs` (`new`), помечены `pub(super)`.

use gpui::*;

use moon_ui::{DockArea, DockPlacement};

use moon_core::config::GroupLayout;

use crate::{Backend, detached};

use super::Shell;

/// Имена dock-панелей нижней строки в порядке их «домашних» позиций. Возврат
/// откреплённой/закрытой панели вставляет её в TabPanel на индекс, сохраняющий этот
/// порядок (см. [`dock_home_priority`]).
pub(super) const DOCK_TAB_ORDER: [&str; 4] = ["Orders", "Assets", "Log", "Report"];

/// «Домашний» индекс панели в нижней строке (Orders<Assets<Log<Report). Используется как
/// позиция вставки при возврате; форк клампит её к числу вкладок, поэтому при частично
/// откреплённом наборе панель встаёт примерно на своё место (порядок сохраняется).
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
                    restore_panel_to_home_tabs(&dock, &backend, &group, &panel_name, window, app);
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
                let spec = detached::DetachedSpec::new(group.clone(), panel_name.clone());
                if backend
                    .read(app)
                    .detached
                    .iter()
                    .any(|s| s.group == spec.group && s.panel == spec.panel)
                {
                    return;
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
                restore_panel_to_home_tabs(&dock, &backend, &group, &panel_name, window, app);
            });
        });
    }

    pub(super) fn persist_group_geometry(&mut self, window: &Window, cx: &mut Context<Self>) {
        let (bounds, maximized) = match window.window_bounds() {
            WindowBounds::Windowed(bounds) => (Some(bounds), false),
            WindowBounds::Maximized(bounds) => (Some(bounds), true),
            WindowBounds::Fullscreen(bounds) => (Some(bounds), false),
        };
        let Some(bounds) = bounds else {
            return;
        };
        let layout = GroupLayout {
            x: f32::from(bounds.origin.x) as i32,
            y: f32::from(bounds.origin.y) as i32,
            w: f32::from(bounds.size.width) as u32,
            h: f32::from(bounds.size.height) as u32,
            maximized,
            collapsed: false,
            tab: 0,
            dock_h: 220.0,
            orders_primary: 0,
            orders_newest_first: true,
            orders_only_current: false,
            orders_kind: 0,
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
                })
                .unwrap_or(true);
            if changed {
                backend.layout.groups.insert(group, layout);
                backend.layout_dirty = true;
            }
        });
    }
}

fn restore_panel_to_home_tabs(
    dock: &Entity<DockArea>,
    backend: &Entity<Backend>,
    group: &str,
    panel_name: &str,
    window: &mut Window,
    app: &mut App,
) {
    let Some(panel) = detached::build_panel(panel_name, group, backend, window, app) else {
        return;
    };
    let ix = dock_home_priority(panel_name);
    dock.update(app, |area, cx| {
        area.remove_panel_by_name(panel_name, window, cx);
        if !area.insert_panel_into_home_tabs(panel.clone(), ix, &DOCK_TAB_ORDER, window, cx) {
            area.add_panel(panel, DockPlacement::Bottom, None, window, cx);
        }
    });
}
