//! Откреплённые dock-панели в отдельных ОС-окнах (порт egui `app/detached.rs` +
//! `WindowLayout.detached`). Панель «уходит» из дока (`TabPanel::remove_panel`) в своё
//! окно; факт открепления и геометрия окна персистятся в `detached.json` и
//! восстанавливаются на старте (панель сразу открывается отцепленной). Закрытие окна
//! открепления → репин: панель возвращается в док окна-владельца (через
//! `Backend.repin_request`, который дренит `Shell`).
//!
//! Контент окна — СВЕЖИЙ экземпляр панели (данные тянет из общего `Backend`, поэтому
//! живой). Обёртка [`DetachedWindow`] рендерит его, следит за геометрией окна и просит
//! репин по закрытию. Чарт-вкладки персистятся отдельно (нужна сериализация панелей).

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonPalette, MoonWindowFrame, PanelView, Root, h_flex, v_flex,
};
use serde::{Deserialize, Serialize};

use rust_i18n::t;

use crate::Backend;
use crate::panels::{AssetsView, LogPanel, OrdersPanel, ReportPanel, StubPanel};
use moon_core::config::paths;

/// Одно откреплённое окно: какая панель (`panel_name`), из какой группы, геометрия окна.
#[derive(Clone, Serialize, Deserialize)]
pub struct DetachedSpec {
    pub group: String,
    /// `panel_name` панели: Orders / Assets / Log / Report.
    pub panel: String,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl DetachedSpec {
    /// Спека с дефолтной геометрией (каскад) — для первого открепления.
    pub fn new(group: String, panel: String) -> Self {
        Self {
            group,
            panel,
            x: 200,
            y: 160,
            w: 1100,
            h: 520,
        }
    }
}

/// Загрузить список откреплённых из `detached.json` (нет/битый → пусто).
pub fn load_all() -> Vec<DetachedSpec> {
    match std::fs::read_to_string(paths::detached_path()) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            log::warn!("detached.json битый ({e}) → без откреплённых");
            Vec::new()
        }),
        Err(_) => Vec::new(),
    }
}

/// Записать список откреплённых в `detached.json` (не фатально).
pub fn save_all(list: &[DetachedSpec]) {
    match serde_json::to_string_pretty(list) {
        Ok(s) => {
            if let Err(e) = std::fs::write(paths::detached_path(), s) {
                log::warn!("не записал detached.json: {e}");
            }
        }
        Err(e) => log::warn!("не сериализовал detached.json: {e}"),
    }
}

/// Заголовок (локализованный) и панель по `panel_name` — единый источник для окна/репина.
fn panel_title(name: &str) -> String {
    match name {
        "Orders" => t!("dock.tab.orders").to_string(),
        "Assets" => t!("dock.tab.assets").to_string(),
        "Log" => t!("dock.tab.log").to_string(),
        "Report" => t!("dock.tab.report").to_string(),
        _ => t!("dock.tab.generic").to_string(),
    }
}

/// True for panels that can be moved into a detached OS window.
pub fn supports_panel(name: &str) -> bool {
    matches!(name, "Orders" | "Assets" | "Log" | "Report")
}

/// Свежий экземпляр dock-панели по `panel_name` как `Rc<dyn PanelView>` — для репина
/// (вернуть в док) и как контент окна открепления.
pub fn build_panel(
    name: &str,
    group: &str,
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut App,
) -> Option<Rc<dyn PanelView>> {
    let panel: Rc<dyn PanelView> =
        match name {
            "Orders" => Rc::new(
                cx.new(|cx| OrdersPanel::new(backend.clone(), group.to_string(), window, cx)),
            ),
            "Log" => {
                Rc::new(cx.new(|cx| LogPanel::new(backend.clone(), group.to_string(), window, cx)))
            }
            "Report" => Rc::new(
                cx.new(|cx| ReportPanel::new(backend.clone(), group.to_string(), window, cx)),
            ),
            "Assets" => Rc::new(cx.new(|cx| {
                AssetsView::restored_group(backend.clone(), group.to_string(), window, cx)
            })),
            _ => return None,
        };
    Some(panel)
}

/// Обёртка-вид окна открепления: рендерит панель, следит за геометрией окна
/// (пишет в `Backend.detached`, дебаунс-сейв делает дренаж), по закрытию просит репин.
pub struct DetachedWindow {
    backend: Entity<Backend>,
    group: String,
    panel: String,
    content: AnyView,
}

impl DetachedWindow {
    fn new(
        backend: Entity<Backend>,
        group: String,
        panel: String,
        content: AnyView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Геометрия окна — causal bounds event, а не polling через render/backend pulse.
        cx.observe_window_bounds(window, |this, window, cx| {
            this.persist_geometry(window, cx);
        })
        .detach();
        // Закрытие окна → репин (вернуть панель в док окна-владельца). На выходе из
        // приложения дренаж уже не обрабатывает запрос → спека остаётся в detached.json
        // (панель восстановится отцепленной на следующем запуске).
        let (g, p) = (group.clone(), panel.clone());
        cx.on_release(move |this, app| {
            this.backend.update(app, |b, _| {
                b.repin_request.push((g.clone(), p.clone()));
            });
        })
        .detach();
        Self {
            backend,
            group,
            panel,
            content,
        }
    }

    fn persist_geometry(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(geom) = crate::windowing::window_geom(window) else {
            return;
        };
        let (group, panel) = (self.group.clone(), self.panel.clone());
        self.backend.update(cx, |bk, _| {
            if let Some(s) = bk
                .detached
                .iter_mut()
                .find(|s| s.group == group && s.panel == panel)
            {
                if (s.x, s.y, s.w, s.h) != geom {
                    s.x = geom.0;
                    s.y = geom.1;
                    s.w = geom.2;
                    s.h = geom.3;
                    bk.detached_dirty = true;
                }
            }
        });
    }
}

impl Render for DetachedWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::DETACHED_RENDER);
        // Активность Main для авто-закрытия по неактивности: откреплённое окно (Ордера/Лог/…)
        // несёт тот же `group`, что и Main, но это ОТДЕЛЬНОЕ ОС-окно — слушатель Shell его
        // движений не видит. Поэтому пишем активность группе и отсюда (мышь над любым
        // виджетом окна), пока это окно активно. Иначе график Main закрывался, хотя
        // пользователь активно работал в окне ордеров.
        {
            let backend = self.backend.clone();
            let group = self.group.clone();
            // CAPTURE-фаза (как в Shell): проходит до элементных bubble-обработчиков и не
            // подвержена их `stop_propagation` — отметка активности надёжна над любым виджетом.
            window.on_mouse_event::<MouseMoveEvent>(move |_e, phase, window, cx| {
                if phase == DispatchPhase::Capture && window.is_window_active() {
                    backend.update(cx, |b, _| b.note_main_input(&group));
                }
            });
        }
        let p = MoonPalette::active(cx);
        let title = format!("{} · {}", panel_title(&self.panel), self.group);
        v_flex()
            .size_full()
            .bg(rgb(p.shell))
            .text_color(rgb(p.text))
            .child(
                h_flex()
                    .h(crate::design::fit_h_px(cx, 34.0, 13.0, 10.5))
                    .w_full()
                    .items_center()
                    .gap(crate::design::ui_px(cx, 8.0))
                    .pl(crate::design::ui_px(
                        cx,
                        crate::design::titlebar_leading_inset(),
                    ))
                    .pr(crate::design::ui_px(cx, 6.0))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.shell_high))
                    .child(
                        MoonWindowFrame::detached_panel("detached-panel-title-drag", 0.0)
                            .title_cluster(title, cx)
                            .h_full()
                            .flex_1()
                            .min_w_0()
                            .items_center(),
                    )
                    .when(crate::design::show_custom_window_controls(), |this| {
                        this.child(
                            MoonWindowFrame::detached_panel("detached-panel-window-controls", 0.0)
                                .header_height(34.0)
                                .show_controls(true)
                                .visual_controls(cx),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .child(self.content.clone()),
            )
    }
}

/// Открыть окно открепления для спеки (на старте — по каждой сохранённой спеке; при
/// клике «⧉» — по новой). Контент — свежая панель; геометрия — из спеки.
pub fn spawn(
    app: &mut App,
    backend: &Entity<Backend>,
    spec: &DetachedSpec,
    owner: Option<AnyWindowHandle>,
) -> anyhow::Result<WindowHandle<Root>> {
    let owner = owner.or_else(|| {
        backend
            .read(app)
            .group_windows
            .get(&spec.group)
            .copied()
            .map(Into::into)
    });
    let bounds = Bounds {
        origin: point(px(spec.x as f32), px(spec.y as f32)),
        size: size(px(spec.w as f32), px(spec.h as f32)),
    };
    let opts = crate::windowing::detached_panel_window_options(
        format!("{} — MoonTerminal", panel_title(&spec.panel)),
        WindowBounds::Windowed(bounds),
        None,
        owner,
    );
    let backend = backend.clone();
    let spec = spec.clone();
    app.open_window(opts, move |window, cx| {
        crate::windowing::configure_shell_clear_color(window, cx);
        let content: AnyView = match spec.panel.as_str() {
            "Orders" => cx
                .new(|cx| OrdersPanel::new(backend.clone(), spec.group.clone(), window, cx))
                .into(),
            "Log" => cx
                .new(|cx| LogPanel::new(backend.clone(), spec.group.clone(), window, cx))
                .into(),
            "Report" => cx
                .new(|cx| ReportPanel::new(backend.clone(), spec.group.clone(), window, cx))
                .into(),
            "Assets" => cx
                .new(|cx| {
                    AssetsView::restored_group(backend.clone(), spec.group.clone(), window, cx)
                })
                .into(),
            _ => cx
                .new(|cx| {
                    StubPanel::new(
                        "?",
                        t!("dock.tab.generic").to_string(),
                        spec.group.clone(),
                        backend.clone(),
                        cx,
                    )
                })
                .into(),
        };
        let dw = cx.new(|cx| {
            DetachedWindow::new(
                backend.clone(),
                spec.group.clone(),
                spec.panel.clone(),
                content,
                window,
                cx,
            )
        });
        cx.new(|cx| Root::new(dw, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    })
}
