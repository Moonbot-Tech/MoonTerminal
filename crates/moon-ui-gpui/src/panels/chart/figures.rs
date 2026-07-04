//! Интеракция слоя рисования фигур на панели чарта: режим-карандаш (Ctrl+ЛКМ рисует),
//! hover/выделение/драг узлов и тела, контекст-меню по ПКМ (Alert/Удалить). Инструмент
//! (`Backend::fig_tool`), режим (`fig_draw_mode`) и выделение (`fig_selected`) глобальны.
//! Всё работает ТОЛЬКО в области чарта (не в зоне стакана — там свои кнопки).

use gpui::{Context, Pixels, Point, Window};
use rust_i18n::t;

use moon_ui::{MoonContextMenuWindowExt as _, MoonMenuItem, MoonWindowExt as _};

use moon_core::figures::{FigNode, Figure, FigureKind, FigureTool};
use moon_core::session::CoreId;

use super::ChartPanel;
use crate::chartdx::FigureVisual;

/// Порог хит-теста линии, px (до умножения на ppp), как у ордер-линий.
const HIT_PX: f32 = 6.0;

/// Фигура в процессе рисования на этой панели.
pub(super) struct FigDraft {
    pub pane: usize,
    pub core: CoreId,
    pub market: String,
    pub tool: FigureTool,
    /// Стиль (цвет/толщина/пунктир) — снимок `Backend::fig_style` на начало рисования.
    pub color: [u8; 4],
    pub thickness: f32,
    pub kind: moon_core::figures::LineKind,
    /// Первый узел (второй — курсор). Для канала после второго клика — Some(b) в `base_b`.
    pub first: Option<FigNode>,
    /// Канал: зафиксированный базовый отрезок (стадия растяжки ширины).
    pub base_b: Option<FigNode>,
    /// Текущая позиция курсора в координатах данных.
    pub cursor: FigNode,
}

/// Драг существующей фигуры.
pub(super) struct FigDrag {
    pub core: CoreId,
    pub market: String,
    pub id: u64,
    pub pane: usize,
    pub grab: FigGrab,
    /// Смещение точки захвата от опорного узла (чтобы фигура не «прыгала» под курсор).
    pub grab_off_price: f64,
    pub grab_off_time: f64,
}

/// Что именно тащим.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum FigGrab {
    /// Первый узел (a) / первая цена канала.
    NodeA,
    /// Второй узел (b) / вторая цена канала.
    NodeB,
    /// Третий узел (c) треугольника.
    NodeC,
    /// Всё тело (сдвиг по цене и времени).
    Body,
}

/// Пиксельный маппинг плота панели: данные ↔ device px.
struct Map {
    plot: moon_chart::view::Rect,
    epoch_ms: f64,
    left_rel: f32,
    window_ms: f32,
    center: f32,
    range: f32,
}

impl Map {
    fn time_at_x(&self, x: f32) -> f64 {
        let rel = self.left_rel + (x - self.plot.x) / self.plot.w.max(1.0) * self.window_ms;
        self.epoch_ms + rel as f64
    }
    fn x_of_time(&self, time_ms: f64) -> f32 {
        let rel = (time_ms - self.epoch_ms) as f32;
        self.plot.x + (rel - self.left_rel) / self.window_ms.max(1e-3) * self.plot.w
    }
    fn price_at_y(&self, y: f32) -> f64 {
        let rel_y = ((y - self.plot.y) / self.plot.h.max(1.0)).clamp(0.0, 1.0);
        (self.center + (0.5 - rel_y) * self.range) as f64
    }
    fn y_of_price(&self, price: f64) -> f32 {
        let rel_y = 0.5 - (price as f32 - self.center) / self.range.max(1e-9);
        self.plot.y + rel_y * self.plot.h
    }
    fn node_at(&self, pos: (f32, f32)) -> FigNode {
        FigNode {
            time_ms: self.time_at_x(pos.0),
            price: self.price_at_y(pos.1),
        }
    }
}

/// Расстояние от точки до отрезка (px).
fn seg_dist(px: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 <= 1e-6 {
        0.0
    } else {
        (((px.0 - a.0) * dx + (px.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
    };
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    ((px.0 - cx).powi(2) + (px.1 - cy).powi(2)).sqrt()
}

impl ChartPanel {
    /// Пиксельный маппинг плота панели (None — панель без вида).
    fn fig_map(&self, pane: usize) -> Option<Map> {
        let plot = self.local_plot_rect(pane)?;
        let (epoch_ms, left_rel, window_ms, center, range) =
            self.chart.with_container(|container| {
                container.pane(pane).map(|p| {
                    let (l, w) = p.view.visible_x(plot.w);
                    (
                        p.view.epoch_ms,
                        l,
                        w,
                        p.view.render_center,
                        p.view.render_range,
                    )
                })
            })?;
        if !(range > 0.0) || window_ms <= 0.0 {
            return None;
        }
        Some(Map {
            plot,
            epoch_ms,
            left_rel,
            window_ms,
            center,
            range,
        })
    }

    /// Ключ чарта панели по индексу pane.
    fn fig_pane_key(&self, pane: usize) -> Option<(CoreId, String)> {
        self.chart
            .with_container(|c| c.pane(pane).map(|p| (p.core, p.market.clone())))
    }

    /// ЛКМ-down. true = клик съеден слоем фигур. Работает ТОЛЬКО в режиме рисования
    /// (карандаш нажат): `ctrl` зажат → рисуем `fig_tool`; без Ctrl → выделяем/двигаем
    /// существующую фигуру. Вне режима — фигуры не трогаем (клик идёт в торговлю/чарт).
    pub(super) fn try_fig_click(
        &mut self,
        pos: (f32, f32),
        ctrl: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.backend.read(cx).fig_draw_mode {
            return false;
        }
        let Some(pane) = self.input.pane_at(pos.0, pos.1) else {
            return false;
        };
        // В зоне управления (стакан/резерв) фигуры не рисуем и не хватаем — там торговля.
        if self.glass_pane_at(pos).is_some() {
            return false;
        }
        let Some(map) = self.fig_map(pane) else {
            return false;
        };
        if ctrl {
            let tool = self.backend.read(cx).fig_tool;
            let node = map.node_at(pos);
            self.fig_draw_click(pane, tool, node, cx);
            return true;
        }
        // Без Ctrl: захват узла/тела фигуры или выделение (пусто → клик не съеден,
        // работает пан).
        self.try_fig_grab(pane, pos, &map, cx)
    }

    /// Клик в режиме рисования: ставит узел/завершает фигуру.
    fn fig_draw_click(
        &mut self,
        pane: usize,
        tool: FigureTool,
        node: FigNode,
        cx: &mut Context<Self>,
    ) {
        let Some((core, market)) = self.fig_pane_key(pane) else {
            return;
        };
        let style = self.backend.read(cx).fig_style;
        // Начатый на другой панели драфт сбрасываем — рисуем там, где кликнули.
        if self
            .fig_draft
            .as_ref()
            .is_some_and(|d| d.pane != pane || d.core != core || d.market != market)
        {
            self.fig_draft = None;
        }
        let finished: Option<FigureKind> = match (&mut self.fig_draft, tool) {
            // Один клик — сразу готово.
            (_, FigureTool::HLine) => Some(FigureKind::HLine { price: node.price }),
            // Первый клик любого многоточечного инструмента — заводим драфт.
            (None, _) => {
                self.fig_draft = Some(FigDraft {
                    pane,
                    core,
                    market: market.clone(),
                    tool,
                    color: style.color,
                    thickness: style.thickness,
                    kind: style.kind,
                    first: Some(node),
                    base_b: None,
                    cursor: node,
                });
                None
            }
            // Отрезок: второй клик завершает.
            (Some(d), FigureTool::Segment) => {
                let a = d.first.take().unwrap_or(node);
                Some(FigureKind::Segment { a, b: node })
            }
            // Канал: 2 клика по ЦЕНАМ (время не важно) — горизонтальный коридор.
            (Some(d), FigureTool::Channel) => {
                let a = d.first.take().unwrap_or(node);
                Some(FigureKind::Channel {
                    price1: a.price,
                    price2: node.price,
                })
            }
            // Треугольник: 3 клика (a, b, c).
            (Some(d), FigureTool::Triangle) => {
                if d.base_b.is_none() {
                    // Второй клик — вторая вершина; ждём третью.
                    d.base_b = Some(node);
                    d.cursor = node;
                    None
                } else {
                    let a = d.first.unwrap_or(node);
                    let b = d.base_b.unwrap();
                    Some(FigureKind::Triangle { a, b, c: node })
                }
            }
        };
        if let Some(kind) = finished {
            self.fig_draft = None;
            let style = self.backend.read(cx).fig_style;
            let fig = Figure {
                id: 0,
                kind,
                color: style.color,
                thickness: style.thickness,
                line_kind: style.kind,
                created_ms: moon_core::util::now_unix_ms_i64(),
                alert: false,
                strategy_id: 0,
                from_server: false,
            };
            let id = self
                .backend
                .read(cx)
                .figures
                .borrow_mut()
                .add(core, &market, fig);
            // Новая фигура сразу выделена — видно узлы, можно удалить/подвинуть.
            self.backend.update(cx, |b, bcx| {
                b.fig_selected = Some((core, market.clone(), id));
                bcx.notify();
            });
        }
        self.sync_fig_visual(cx);
    }

    /// Захват фигуры вне режима рисования: узлы (у выделенной), затем тело ближайшей.
    fn try_fig_grab(
        &mut self,
        pane: usize,
        pos: (f32, f32),
        map: &Map,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((core, market)) = self.fig_pane_key(pane) else {
            return false;
        };
        let threshold = HIT_PX * self.last_ppp.max(1.0);
        let b = self.backend.read(cx);
        let store = b.figures.borrow();
        let figures = store.figures(core, &market);
        if figures.is_empty() {
            return false;
        }
        let selected = b
            .fig_selected
            .as_ref()
            .filter(|(c, m, _)| *c == core && *m == market)
            .map(|(_, _, id)| *id);
        // 1. Узлы выделенной фигуры (приоритет над телом).
        if let Some(sel_id) = selected {
            if let Some(fig) = figures.iter().find(|f| f.id == sel_id) {
                if let Some(grab) = hit_node(fig, pos, map, threshold) {
                    let (off_p, off_t) = grab_offset(fig, grab, map, pos);
                    drop(store);
                    self.fig_drag = Some(FigDrag {
                        core,
                        market,
                        id: sel_id,
                        pane,
                        grab,
                        grab_off_price: off_p,
                        grab_off_time: off_t,
                    });
                    return true;
                }
            }
        }
        // 2. Тело ближайшей фигуры: выделение + драг тела.
        let mut best: Option<(u64, f32)> = None;
        for fig in figures {
            let d = hit_body(fig, pos, map);
            if d <= threshold && best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((fig.id, d));
            }
        }
        let Some((id, _)) = best else {
            drop(store);
            let _ = b;
            // Клик мимо фигур — снимаем выделение (не съедая клик: пан работает).
            if self.backend.read(cx).fig_selected.is_some() {
                self.backend.update(cx, |b, bcx| {
                    b.fig_selected = None;
                    bcx.notify();
                });
                self.sync_fig_visual(cx);
            }
            return false;
        };
        let fig = figures.iter().find(|f| f.id == id).unwrap();
        let (off_p, off_t) = grab_offset(fig, FigGrab::Body, map, pos);
        drop(store);
        let _ = b;
        self.backend.update(cx, |b, bcx| {
            b.fig_selected = Some((core, market.clone(), id));
            bcx.notify();
        });
        self.fig_drag = Some(FigDrag {
            core,
            market,
            id,
            pane,
            grab: FigGrab::Body,
            grab_off_price: off_p,
            grab_off_time: off_t,
        });
        self.sync_fig_visual(cx);
        true
    }

    /// Mouse-move: превью драфта, драг фигуры, hover. `pressed_left` — кнопка зажата.
    pub(super) fn update_fig_pointer(
        &mut self,
        pos: (f32, f32),
        within: bool,
        pressed_left: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        // Режим рисования выключили (Esc/кнопка) — гасим драфт.
        if !self.backend.read(cx).fig_draw_mode && self.fig_draft.is_some() {
            self.fig_draft = None;
            self.sync_fig_visual(cx);
        }
        if !within {
            return false;
        }
        // Активный драг фигуры: правим стор на месте + принудительная пересборка (как
        // ордер-драг: force=true), иначе линия «догоняет» рывками только на тиках данных.
        if pressed_left {
            if let Some(drag) = &self.fig_drag {
                let pane = self.input.pane_at(pos.0, pos.1);
                let Some(map) = pane.and_then(|p| self.fig_map(p)) else {
                    return false;
                };
                let target = FigNode {
                    time_ms: map.time_at_x(pos.0) - drag.grab_off_time,
                    price: map.price_at_y(pos.1) - drag.grab_off_price,
                };
                let (core, market, id, grab, dpane) =
                    (drag.core, drag.market.clone(), drag.id, drag.grab, drag.pane);
                let edited = self.backend.read(cx).figures.borrow_mut().edit(
                    core,
                    &market,
                    id,
                    |fig| apply_drag(fig, grab, target),
                );
                if edited {
                    self.fig_resync(cx);
                    cx.notify();
                }
                // Курсор-перекрестие ведём за мышью, как при драге ордера.
                self.input.cursor = Some(pos);
                self.input.hovered_pane = Some(dpane);
                self.sync_native_cursor();
                return true;
            }
            return false;
        }
        // Драфт: вторая точка следует за курсором.
        let mut changed = false;
        let draft_pane = self.fig_draft.as_ref().map(|d| d.pane);
        if let Some(dp) = draft_pane {
            if self.input.pane_at(pos.0, pos.1) == Some(dp) {
                if let Some(map) = self.fig_map(dp) {
                    let node = map.node_at(pos);
                    if let Some(d) = &mut self.fig_draft {
                        if d.cursor != node {
                            d.cursor = node;
                            changed = true;
                        }
                    }
                }
            }
        }
        // Hover фигуры (в режиме рисования — чтобы видеть, что выделишь/потянешь).
        let hover = if self.backend.read(cx).fig_draw_mode {
            self.fig_hit_at(pos, cx)
        } else {
            None
        };
        if hover != self.fig_hover {
            self.fig_hover = hover;
            changed = true;
        }
        if changed {
            self.sync_fig_visual(cx);
        }
        changed
    }

    /// id фигуры под курсором (тело или узел выделенной).
    fn fig_hit_at(&self, pos: (f32, f32), cx: &Context<Self>) -> Option<u64> {
        let pane = self.input.pane_at(pos.0, pos.1)?;
        if self.glass_pane_at(pos).is_some() {
            return None;
        }
        let (core, market) = self.fig_pane_key(pane)?;
        let map = self.fig_map(pane)?;
        let threshold = HIT_PX * self.last_ppp.max(1.0);
        let b = self.backend.read(cx);
        let store = b.figures.borrow();
        let mut best: Option<(u64, f32)> = None;
        for fig in store.figures(core, &market) {
            let d = hit_body(fig, pos, &map);
            if d <= threshold && best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((fig.id, d));
            }
        }
        best.map(|(id, _)| id)
    }

    /// ПКМ по фигуре (только в режиме рисования, только в области чарта) → контекст-меню
    /// «Alert / Удалить». true = меню открыто (вызывающий гасит fullscreen-тоггл). В зоне
    /// стакана и вне режима возвращает false → ПКМ работает как обычно.
    pub(super) fn try_open_figure_menu(
        &mut self,
        local_pos: (f32, f32),
        menu_pos: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.backend.read(cx).fig_draw_mode {
            return false;
        }
        // fig_hit_at сам исключает зону стакана (glass_pane_at).
        let Some(id) = self.fig_hit_at(local_pos, cx) else {
            return false;
        };
        let Some(pane) = self.input.pane_at(local_pos.0, local_pos.1) else {
            return false;
        };
        let Some((core, market)) = self.fig_pane_key(pane) else {
            return false;
        };
        let armed = self
            .backend
            .read(cx)
            .figures
            .borrow()
            .get(core, &market, id)
            .map(|f| f.alert)
            .unwrap_or(false);
        // ПКМ по фигуре её выделяет (и показывает узлы).
        self.backend.update(cx, |b, bcx| {
            b.fig_selected = Some((core, market.clone(), id));
            bcx.notify();
        });
        self.sync_fig_visual(cx);
        let alert_label = if armed {
            t!("chart.fig_menu.alert_off")
        } else {
            t!("chart.fig_menu.alert_on")
        }
        .to_string();
        let backend_alert = self.backend.clone();
        let backend_del = self.backend.clone();
        let market_del = market.clone();
        let items: Vec<MoonMenuItem> = vec![
            MoonMenuItem::with_key("fig-alert", alert_label).on_click(move |_, window, app| {
                window.close_context_menu(app);
                backend_alert.update(app, |b, _| {
                    b.toggle_selected_figure_alert();
                });
            }),
            MoonMenuItem::with_key("fig-delete", t!("chart.fig_menu.delete").to_string()).on_click(
                move |_, window, app| {
                    window.close_context_menu(app);
                    backend_del.update(app, |b, _| {
                        b.remove_figure(core, &market_del, id);
                    });
                },
            ),
        ];
        window.open_moon_context_menu(cx, "chart-fig-menu", menu_pos, items, 160.0);
        cx.notify();
        true
    }

    /// Mouse-up: завершить драг фигуры. true = ап съеден. Если фигура заармлена —
    /// шлём свежий blob в ядро (координаты изменились).
    pub(super) fn finish_fig_drag(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.fig_drag.take() else {
            return false;
        };
        self.backend.update(cx, |b, _| {
            b.reupsert_figure_alert(drag.core, &drag.market, drag.id);
        });
        self.sync_fig_visual(cx);
        true
    }

    /// Прокинуть интерактив фигур в движок (режим/превью/hover/выделение). Зовётся на
    /// мышь-событиях И на observe бэкенда (тоггл карандаша сразу скрывает/показывает слой).
    pub(super) fn sync_fig_visual(&mut self, cx: &mut Context<Self>) {
        let b = self.backend.read(cx);
        let draw_mode = b.fig_draw_mode;
        let key = self
            .fig_draft
            .as_ref()
            .map(|d| (d.core, d.market.clone()))
            .or_else(|| {
                b.fig_selected
                    .as_ref()
                    .map(|(c, m, _)| (*c, m.clone()))
            })
            .or_else(|| {
                self.input
                    .hovered_pane
                    .and_then(|p| self.fig_pane_key(p))
            });
        let draft = self.fig_draft.as_ref().and_then(draft_preview);
        let selected = b
            .fig_selected
            .as_ref()
            .filter(|(c, m, _)| key.as_ref().is_some_and(|(kc, km)| kc == c && km == m))
            .map(|(_, _, id)| *id);
        let visual = FigureVisual {
            draw_mode,
            key,
            draft,
            hovered: self.fig_hover,
            selected,
        };
        let _ = b;
        if self.chart.set_figure_visual(visual) {
            // Userdata пересобирается только в sync_orders_*: дёргаем сразу, иначе
            // превью/подсветка ждали бы следующего тика данных/notify бэкенда.
            self.fig_resync(cx);
            cx.notify();
        }
    }

    /// Немедленная пересборка userdata (фигуры едут вместе с ордерными слоями).
    /// `force=true` как у ордер-драга — иначе гейт пропускает часть кадров и драг рвётся.
    fn fig_resync(&mut self, cx: &Context<Self>) {
        let b = self.backend.read(cx);
        self.chart.sync_orders_if_visible(&b.session, true);
    }
}

/// Превью-фигура из драфта (то, что рисуем за курсором).
fn draft_preview(d: &FigDraft) -> Option<Figure> {
    let kind = match (d.tool, d.first, d.base_b) {
        (FigureTool::HLine, ..) => FigureKind::HLine {
            price: d.cursor.price,
        },
        (FigureTool::Segment, Some(a), _) => FigureKind::Segment { a, b: d.cursor },
        // Канал: превью = 2 горизонтали (первая цена + цена курсора).
        (FigureTool::Channel, Some(a), _) => FigureKind::Channel {
            price1: a.price,
            price2: d.cursor.price,
        },
        // Треугольник: после 1-го клика — ребро a→курсор; после 2-го — треугольник.
        (FigureTool::Triangle, Some(a), None) => FigureKind::Segment { a, b: d.cursor },
        (FigureTool::Triangle, Some(a), Some(b)) => FigureKind::Triangle {
            a,
            b,
            c: d.cursor,
        },
        _ => return None,
    };
    Some(Figure {
        id: 0,
        kind,
        color: d.color,
        thickness: d.thickness,
        line_kind: d.kind,
        created_ms: 0,
        alert: false,
        strategy_id: 0,
        from_server: false,
    })
}

/// Расстояние от точки до тела фигуры, px.
fn hit_body(fig: &Figure, pos: (f32, f32), map: &Map) -> f32 {
    match &fig.kind {
        FigureKind::HLine { price } => (pos.1 - map.y_of_price(*price)).abs(),
        FigureKind::Segment { a, b } => seg_dist(pos, node_px(a, map), node_px(b, map)),
        FigureKind::Triangle { a, b, c } => {
            let (pa, pb, pc) = (node_px(a, map), node_px(b, map), node_px(c, map));
            seg_dist(pos, pa, pb)
                .min(seg_dist(pos, pb, pc))
                .min(seg_dist(pos, pc, pa))
        }
        FigureKind::Channel { price1, price2 } => {
            (pos.1 - map.y_of_price(*price1))
                .abs()
                .min((pos.1 - map.y_of_price(*price2)).abs())
        }
    }
}

/// Узел/линия фигуры под курсором (для драга у выделенной).
fn hit_node(fig: &Figure, pos: (f32, f32), map: &Map, threshold: f32) -> Option<FigGrab> {
    let near = |n: &FigNode| {
        let p = node_px(n, map);
        ((pos.0 - p.0).powi(2) + (pos.1 - p.1).powi(2)).sqrt() <= threshold
    };
    match &fig.kind {
        FigureKind::HLine { .. } => None,
        FigureKind::Segment { a, b } => {
            if near(a) {
                Some(FigGrab::NodeA)
            } else if near(b) {
                Some(FigGrab::NodeB)
            } else {
                None
            }
        }
        FigureKind::Triangle { a, b, c } => {
            if near(a) {
                Some(FigGrab::NodeA)
            } else if near(b) {
                Some(FigGrab::NodeB)
            } else if near(c) {
                Some(FigGrab::NodeC)
            } else {
                None
            }
        }
        // Канал: цепляемся за БЛИЖАЙШУЮ горизонталь по Y (свой узел-маркер не рисуем).
        FigureKind::Channel { price1, price2 } => {
            let d1 = (pos.1 - map.y_of_price(*price1)).abs();
            let d2 = (pos.1 - map.y_of_price(*price2)).abs();
            if d1 <= threshold && d1 <= d2 {
                Some(FigGrab::NodeA)
            } else if d2 <= threshold {
                Some(FigGrab::NodeB)
            } else {
                None
            }
        }
    }
}

fn node_px(n: &FigNode, map: &Map) -> (f32, f32) {
    (map.x_of_time(n.time_ms), map.y_of_price(n.price))
}

/// Смещение точки захвата от опорного узла (grab-offset): драг не телепортирует фигуру.
/// Возвращает `(price_off, time_off)`. У горизонтали/канала время не используется (0).
fn grab_offset(fig: &Figure, grab: FigGrab, map: &Map, pos: (f32, f32)) -> (f64, f64) {
    let cur = FigNode {
        time_ms: map.time_at_x(pos.0),
        price: map.price_at_y(pos.1),
    };
    // Опорная цена по типу+захвату; время опоры = время курсора (сдвиг времени 0),
    // кроме узлов отрезка/треугольника, где нужна реальная точка.
    let anchor = match (&fig.kind, grab) {
        (FigureKind::HLine { price }, _) => (*price, cur.time_ms),
        (FigureKind::Channel { price1, .. }, FigGrab::NodeA)
        | (FigureKind::Channel { price1, .. }, FigGrab::Body) => (*price1, cur.time_ms),
        (FigureKind::Channel { price2, .. }, FigGrab::NodeB) => (*price2, cur.time_ms),
        (FigureKind::Channel { price1, .. }, FigGrab::NodeC) => (*price1, cur.time_ms),
        (FigureKind::Segment { a, .. }, FigGrab::NodeA)
        | (FigureKind::Triangle { a, .. }, FigGrab::NodeA)
        | (FigureKind::Segment { a, .. }, FigGrab::Body)
        | (FigureKind::Triangle { a, .. }, FigGrab::Body) => (a.price, a.time_ms),
        (FigureKind::Segment { b, .. }, FigGrab::NodeB)
        | (FigureKind::Triangle { b, .. }, FigGrab::NodeB) => (b.price, b.time_ms),
        (FigureKind::Triangle { c, .. }, FigGrab::NodeC) => (c.price, c.time_ms),
        // Недостижимые сочетания (NodeC у отрезка/канала-NodeC уже покрыт) — курсор.
        (FigureKind::Segment { .. }, FigGrab::NodeC) => (cur.price, cur.time_ms),
    };
    (cur.price - anchor.0, cur.time_ms - anchor.1)
}

/// Применить драг к фигуре. `target` — новая позиция опорного узла/цены.
fn apply_drag(fig: &mut Figure, grab: FigGrab, target: FigNode) -> bool {
    match (&mut fig.kind, grab) {
        (FigureKind::HLine { price }, _) => {
            if *price == target.price {
                return false;
            }
            *price = target.price;
        }
        (FigureKind::Channel { price1, .. }, FigGrab::NodeA) => {
            if *price1 == target.price {
                return false;
            }
            *price1 = target.price;
        }
        (FigureKind::Channel { price2, .. }, FigGrab::NodeB) => {
            if *price2 == target.price {
                return false;
            }
            *price2 = target.price;
        }
        (FigureKind::Channel { price1, price2 }, FigGrab::Body) => {
            let dp = target.price - *price1;
            if dp == 0.0 {
                return false;
            }
            *price1 += dp;
            *price2 += dp;
        }
        (FigureKind::Segment { a, .. }, FigGrab::NodeA)
        | (FigureKind::Triangle { a, .. }, FigGrab::NodeA) => {
            if *a == target {
                return false;
            }
            *a = target;
        }
        (FigureKind::Segment { b, .. }, FigGrab::NodeB)
        | (FigureKind::Triangle { b, .. }, FigGrab::NodeB) => {
            if *b == target {
                return false;
            }
            *b = target;
        }
        (FigureKind::Triangle { c, .. }, FigGrab::NodeC) => {
            if *c == target {
                return false;
            }
            *c = target;
        }
        (FigureKind::Segment { a, b }, FigGrab::Body) => {
            let (dp, dt) = (target.price - a.price, target.time_ms - a.time_ms);
            if dp == 0.0 && dt == 0.0 {
                return false;
            }
            a.price += dp;
            a.time_ms += dt;
            b.price += dp;
            b.time_ms += dt;
        }
        (FigureKind::Triangle { a, b, c }, FigGrab::Body) => {
            let (dp, dt) = (target.price - a.price, target.time_ms - a.time_ms);
            if dp == 0.0 && dt == 0.0 {
                return false;
            }
            for n in [a, b, c] {
                n.price += dp;
                n.time_ms += dt;
            }
        }
        // Недостижимо: NodeC у отрезка/канала.
        (FigureKind::Segment { .. }, FigGrab::NodeC)
        | (FigureKind::Channel { .. }, FigGrab::NodeC) => return false,
    }
    true
}
