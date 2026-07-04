//! Интеракция слоя рисования фигур на панели чарта: режим-карандаш (узлы ЛКМ,
//! превью за курсором), hover/выделение/драг узлов и тела, удаление. Инструмент
//! (`Backend::fig_tool`) и выделение (`Backend::fig_selected`) глобальны —
//! тогглятся хоткеями в Shell; здесь — работа мыши на конкретной панели.
//!
//! ПКМ не используем (занят ордерами): создание — режим-карандаш, управление —
//! клик-выделение + драг + хоткей удаления.

use gpui::Context;

use moon_core::figures::{FigNode, Figure, FigureKind, FigureTool};
use moon_core::session::CoreId;

use super::ChartPanel;
use crate::chartdx::FigureVisual;

/// Порог хит-теста линии, px (до умножения на ppp), как у ордер-линий.
const HIT_PX: f32 = 6.0;
/// Дефолтный стиль новой фигуры (пока нет тулбара стиля): янтарный, 1.5 px.
const DEFAULT_COLOR: [u8; 4] = [255, 191, 64, 255];
const DEFAULT_THICKNESS: f32 = 1.5;

/// Фигура в процессе рисования на этой панели.
pub(super) struct FigDraft {
    pub pane: usize,
    pub core: CoreId,
    pub market: String,
    pub tool: FigureTool,
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
    pub grab: FigGrab,
    /// Смещение точки захвата от опорного узла (чтобы фигура не «прыгала» под курсор).
    pub grab_off_price: f64,
    pub grab_off_time: f64,
}

/// Что именно тащим.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum FigGrab {
    NodeA,
    NodeB,
    /// Узел ширины канала.
    Width,
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

    /// ЛКМ-down. true = клик съеден слоем фигур (режим рисования или захват фигуры).
    pub(super) fn try_fig_click(&mut self, pos: (f32, f32), cx: &mut Context<Self>) -> bool {
        let tool = self.backend.read(cx).fig_tool;
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
        let node = map.node_at(pos);
        if let Some(tool) = tool {
            self.fig_draw_click(pane, tool, node, cx);
            return true;
        }
        // Вне режима: захват узла/тела фигуры или выделение.
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
        // Начатый на другой панели драфт сбрасываем — рисуем там, где кликнули.
        if self
            .fig_draft
            .as_ref()
            .is_some_and(|d| d.pane != pane || d.core != core || d.market != market)
        {
            self.fig_draft = None;
        }
        let finished: Option<FigureKind> = match (&mut self.fig_draft, tool) {
            (_, FigureTool::HLine) => Some(FigureKind::HLine { price: node.price }),
            (None, _) => {
                self.fig_draft = Some(FigDraft {
                    pane,
                    core,
                    market: market.clone(),
                    tool,
                    first: Some(node),
                    base_b: None,
                    cursor: node,
                });
                None
            }
            (Some(d), FigureTool::Segment) => {
                let a = d.first.take().unwrap_or(node);
                Some(FigureKind::Segment { a, b: node })
            }
            (Some(d), FigureTool::Channel) => {
                if d.base_b.is_none() {
                    // Второй клик: фиксируем базовый отрезок, дальше растяжка ширины.
                    d.base_b = Some(node);
                    d.cursor = node;
                    None
                } else {
                    let a = d.first.unwrap_or(node);
                    let b = d.base_b.unwrap();
                    // Ширина = вертикальное отклонение курсора от базовой линии в точке клика.
                    let dprice = node.price - price_on_line(a, b, node.time_ms);
                    Some(FigureKind::Channel { a, b, dprice })
                }
            }
        };
        if let Some(kind) = finished {
            self.fig_draft = None;
            let fig = Figure {
                id: 0,
                kind,
                color: DEFAULT_COLOR,
                thickness: DEFAULT_THICKNESS,
                dashed: false,
                created_ms: moon_core::util::now_unix_ms_i64(),
                alert: false,
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
        // Инструмент сняли (Esc) — гасим драфт.
        if self.backend.read(cx).fig_tool.is_none() && self.fig_draft.is_some() {
            self.fig_draft = None;
            self.sync_fig_visual(cx);
        }
        if !within {
            return false;
        }
        // Активный драг фигуры: правим стор на месте.
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
                let (core, market, id, grab) =
                    (drag.core, drag.market.clone(), drag.id, drag.grab);
                let edited = self.backend.read(cx).figures.borrow_mut().edit(
                    core,
                    &market,
                    id,
                    |fig| apply_drag(fig, grab, target),
                );
                if edited {
                    // Стор правится мимо GPUI-notify — пересобираем userdata сразу,
                    // иначе драг «догоняет» только на тиках данных.
                    self.fig_resync(cx);
                    cx.notify();
                }
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
        // Hover фигуры (только вне режима рисования).
        let hover = if self.backend.read(cx).fig_tool.is_none() {
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

    /// Mouse-up: завершить драг фигуры. true = ап съеден.
    pub(super) fn finish_fig_drag(&mut self, cx: &mut Context<Self>) -> bool {
        if self.fig_drag.take().is_none() {
            return false;
        }
        self.sync_fig_visual(cx);
        true
    }

    /// Прокинуть интерактив фигур в движок (превью/hover/выделение).
    pub(super) fn sync_fig_visual(&mut self, cx: &mut Context<Self>) {
        let b = self.backend.read(cx);
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
    fn fig_resync(&mut self, cx: &Context<Self>) {
        let b = self.backend.read(cx);
        self.chart.sync_orders_if_visible(&b.session, false);
    }
}

/// Цена базовой линии (a,b) в точке времени t (линейная интерполяция/экстраполяция).
fn price_on_line(a: FigNode, b: FigNode, t_ms: f64) -> f64 {
    let dt = b.time_ms - a.time_ms;
    if dt.abs() < 1.0 {
        return a.price;
    }
    a.price + (b.price - a.price) * ((t_ms - a.time_ms) / dt)
}

/// Превью-фигура из драфта (то, что рисуем за курсором).
fn draft_preview(d: &FigDraft) -> Option<Figure> {
    let kind = match (d.tool, d.first, d.base_b) {
        (FigureTool::HLine, ..) => FigureKind::HLine {
            price: d.cursor.price,
        },
        (FigureTool::Segment, Some(a), _) => FigureKind::Segment { a, b: d.cursor },
        (FigureTool::Channel, Some(a), None) => FigureKind::Segment { a, b: d.cursor },
        (FigureTool::Channel, Some(a), Some(b)) => FigureKind::Channel {
            a,
            b,
            dprice: d.cursor.price - price_on_line(a, b, d.cursor.time_ms),
        },
        _ => return None,
    };
    Some(Figure {
        id: 0,
        kind,
        color: DEFAULT_COLOR,
        thickness: DEFAULT_THICKNESS,
        dashed: false,
        created_ms: 0,
        alert: false,
    })
}

/// Расстояние от точки до тела фигуры, px.
fn hit_body(fig: &Figure, pos: (f32, f32), map: &Map) -> f32 {
    match &fig.kind {
        FigureKind::HLine { price } => (pos.1 - map.y_of_price(*price)).abs(),
        FigureKind::Segment { a, b } => seg_dist(pos, node_px(a, map), node_px(b, map)),
        FigureKind::Channel { a, b, dprice } => {
            let d1 = seg_dist(pos, node_px(a, map), node_px(b, map));
            let a2 = shifted(a, *dprice);
            let b2 = shifted(b, *dprice);
            let d2 = seg_dist(pos, node_px(&a2, map), node_px(&b2, map));
            d1.min(d2)
        }
    }
}

/// Узел фигуры под курсором (для драга у выделенной).
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
        FigureKind::Channel { a, b, dprice } => {
            let mid = FigNode {
                time_ms: (a.time_ms + b.time_ms) * 0.5,
                price: (a.price + b.price) * 0.5 + dprice,
            };
            if near(a) {
                Some(FigGrab::NodeA)
            } else if near(b) {
                Some(FigGrab::NodeB)
            } else if near(&mid) {
                Some(FigGrab::Width)
            } else {
                None
            }
        }
    }
}

fn node_px(n: &FigNode, map: &Map) -> (f32, f32) {
    (map.x_of_time(n.time_ms), map.y_of_price(n.price))
}

fn shifted(n: &FigNode, dprice: f64) -> FigNode {
    FigNode {
        time_ms: n.time_ms,
        price: n.price + dprice,
    }
}

/// Смещение точки захвата от опорного узла (grab-offset): драг не телепортирует фигуру.
fn grab_offset(fig: &Figure, grab: FigGrab, map: &Map, pos: (f32, f32)) -> (f64, f64) {
    let cur = FigNode {
        time_ms: map.time_at_x(pos.0),
        price: map.price_at_y(pos.1),
    };
    let anchor = match (&fig.kind, grab) {
        (FigureKind::HLine { price }, _) => FigNode {
            time_ms: cur.time_ms,
            price: *price,
        },
        (FigureKind::Segment { a, .. }, FigGrab::NodeA)
        | (FigureKind::Channel { a, .. }, FigGrab::NodeA) => *a,
        (FigureKind::Segment { b, .. }, FigGrab::NodeB)
        | (FigureKind::Channel { b, .. }, FigGrab::NodeB) => *b,
        (FigureKind::Channel { a, b, dprice }, FigGrab::Width) => FigNode {
            time_ms: (a.time_ms + b.time_ms) * 0.5,
            price: (a.price + b.price) * 0.5 + dprice,
        },
        (FigureKind::Segment { a, .. }, FigGrab::Body)
        | (FigureKind::Channel { a, .. }, FigGrab::Body) => *a,
        // Width у Segment не бывает (hit_node не возвращает) — берём курсор (нулевой сдвиг).
        (FigureKind::Segment { .. }, FigGrab::Width) => cur,
    };
    (cur.price - anchor.price, cur.time_ms - anchor.time_ms)
}

/// Применить драг к фигуре. `target` — новая позиция опорного узла.
fn apply_drag(fig: &mut Figure, grab: FigGrab, target: FigNode) -> bool {
    match (&mut fig.kind, grab) {
        (FigureKind::HLine { price }, _) => {
            if *price == target.price {
                return false;
            }
            *price = target.price;
        }
        (FigureKind::Segment { a, .. }, FigGrab::NodeA)
        | (FigureKind::Channel { a, .. }, FigGrab::NodeA) => {
            if *a == target {
                return false;
            }
            *a = target;
        }
        (FigureKind::Segment { b, .. }, FigGrab::NodeB)
        | (FigureKind::Channel { b, .. }, FigGrab::NodeB) => {
            if *b == target {
                return false;
            }
            *b = target;
        }
        (FigureKind::Channel { a, b, dprice }, FigGrab::Width) => {
            let mid_price = (a.price + b.price) * 0.5;
            let new_dp = target.price - mid_price;
            if *dprice == new_dp {
                return false;
            }
            *dprice = new_dp;
        }
        (FigureKind::Segment { a, b }, FigGrab::Body)
        | (FigureKind::Channel { a, b, .. }, FigGrab::Body) => {
            let (dp, dt) = (target.price - a.price, target.time_ms - a.time_ms);
            if dp == 0.0 && dt == 0.0 {
                return false;
            }
            a.price += dp;
            a.time_ms += dt;
            b.price += dp;
            b.time_ms += dt;
        }
        // Width у Segment не бывает (hit_node не возвращает).
        (FigureKind::Segment { .. }, FigGrab::Width) => return false,
    }
    true
}
