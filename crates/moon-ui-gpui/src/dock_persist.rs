//! Персист раскладки доков («сохранение всего», часть 2: сами доки, не только окна).
//!
//! MoonPalette сериализует `DockArea` в `DockAreaState` (serde) и восстанавливает
//! его через `DockArea::load` + глобальный `PanelRegistry`: по `panel_name` из
//! состояния фабрика заново строит панель. Реестр ОДИН на приложение, а группа —
//! у каждого окна своя, поэтому группу (и прочие параметры реконструкции) каждая
//! панель кладёт в свой `PanelInfo::Panel(json)` через `Panel::dump`, а фабрика
//! читает её оттуда. Карта `группа → DockAreaState` пишется в `docks.json` рядом с
//! exe (как layout.toml для геометрии окон).

use std::collections::HashMap;
use std::rc::Rc;

use gpui::*;
use moon_ui::{DockAreaState, PanelInfo, PanelState, register_panel};

use moon_core::config::paths;

use crate::Backend;
use crate::chart_tabs::ChartTabs;
use crate::panels::{AssetsView, DetectsPanel, LogPanel, OrderPanel, OrdersPanel, ReportPanel};
use moon_core::session::CoreId;

/// Версия схемы раскладки доков. Поднимаем при несовместимом изменении структуры
/// панелей → старый `docks.json` игнорируется (откат к дефолтной раскладке).
pub const DOCK_VERSION: usize = 1;

/// Карта раскладок: группа → состояние её `DockArea`.
pub type DockMap = HashMap<String, DockAreaState>;

/// Загрузить карту раскладок из `docks.json` (нет файла/битый → пусто = дефолт).
pub fn load_all() -> DockMap {
    match std::fs::read_to_string(paths::docks_path()) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            log::warn!("docks.json битый ({e}) → дефолтная раскладка");
            DockMap::new()
        }),
        Err(_) => DockMap::new(),
    }
}

/// Записать карту раскладок в `docks.json` (не фатально: при ошибке только лог).
pub fn save_all(map: &DockMap) {
    match serde_json::to_string_pretty(map) {
        Ok(s) => {
            if let Err(e) = std::fs::write(paths::docks_path(), s) {
                log::warn!("не записал docks.json: {e}");
            }
        }
        Err(e) => log::warn!("не сериализовал docks.json: {e}"),
    }
}

/// Группа панели, зашитая в её `dump()` (см. [`panel_state_with_group`]).
fn group_of(info: &PanelInfo) -> String {
    if let PanelInfo::Panel(v) = info {
        if let Some(g) = v.get("group").and_then(|g| g.as_str()) {
            return g.to_string();
        }
    }
    String::new()
}

/// Хелпер для `Panel::dump` панелей, которым для реконструкции нужна группа:
/// кладёт `{"group": ...}` в `PanelInfo::Panel`, сохраняя `panel_name`.
pub fn panel_state_with_group(panel_name: &str, group: &str) -> PanelState {
    PanelState {
        panel_name: panel_name.to_string(),
        children: Vec::new(),
        info: PanelInfo::panel(serde_json::json!({ "group": group })),
    }
}

/// Фокус-монета группы (первое активное ядро группы + его рынок) — та же логика,
/// что в `main()` при первичном открытии окон; нужна Main-чарту при реконструкции.
/// Зарегистрировать фабрики всех панелей-доков в глобальном `PanelRegistry`.
/// Вызывается один раз на старте (после создания `backend`). `backend`/`epoch`
/// захватываются в замыкания; группа и пр. читаются из `PanelState` при восстановлении.
pub fn register_panels(cx: &mut App, backend: Entity<Backend>, epoch: f64) {
    // Чарт-вкладки: группа из state, тема/фокус — из backend по группе.
    {
        let backend = backend.clone();
        register_panel(cx, "ChartTabs", move |_state, info, window, cx| {
            let group = group_of(info);
            let theme = backend.read(cx).config.theme.clone();
            let backend = backend.clone();
            // Main стартует пустым — монету не открываем автоматически (см. group_window).
            let focus: Option<(CoreId, String)> = None;
            Rc::new(cx.new(|cx| ChartTabs::new(backend, group, focus, epoch, theme, window, cx)))
        });
    }
    // Лента детектов: группа из state.
    {
        let backend = backend.clone();
        register_panel(cx, "Detects", move |_state, info, _window, cx| {
            let group = group_of(info);
            let backend = backend.clone();
            Rc::new(cx.new(|cx| DetectsPanel::new(backend, group, cx)))
        });
    }
    // Таблица ордеров: группа из state.
    {
        let backend = backend.clone();
        register_panel(cx, "Orders", move |_state, info, window, cx| {
            let group = group_of(info);
            let backend = backend.clone();
            // `restored` применяет сохранённое состояние вида (сортировка/тип/фильтр).
            Rc::new(cx.new(|cx| OrdersPanel::restored(backend, group, info, window, cx)))
        });
    }
    // Ордер: без состояния.
    {
        let backend = backend.clone();
        register_panel(cx, "Order", move |_state, info, _window, cx| {
            let group = group_of(info);
            let backend = backend.clone();
            Rc::new(cx.new(|cx| OrderPanel::new(backend, group, cx)))
        });
    }
    // Активы: группа из state; реальные данные ядер группы (таблица + дерево переноса).
    {
        let backend = backend.clone();
        register_panel(cx, "Assets", move |_s, info, window, cx| {
            let group = group_of(info);
            let backend = backend.clone();
            Rc::new(cx.new(|cx| AssetsView::restored_group(backend, group, window, cx)))
        });
    }
    // Лог: группа из state; нужен `window` (поле поиска — InputState).
    {
        let backend = backend.clone();
        register_panel(cx, "Log", move |_s, info, window, cx| {
            let group = group_of(info);
            let backend = backend.clone();
            Rc::new(cx.new(|cx| LogPanel::new(backend, group, window, cx)))
        });
    }
    // Отчёт: группа из state; нужен `window` (поля фильтров — InputState).
    {
        let backend = backend.clone();
        register_panel(cx, "Report", move |_s, info, window, cx| {
            let group = group_of(info);
            let backend = backend.clone();
            Rc::new(cx.new(|cx| ReportPanel::new(backend, group, window, cx)))
        });
    }
}
