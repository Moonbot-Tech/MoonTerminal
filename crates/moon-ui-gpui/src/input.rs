//! Ввод чарт-области в GPUI — порт `src/chart/input.rs` egui-версии 1:1, но
//! winit-free: вместо winit-типов берёт plain `dy: f32` (колесо) и локальный
//! [`Btn`]. Всё в ДЕВАЙС-пикселях (та же шкала, в которой движок рендерит offscreen
//! и в которой живёт `ChartView`). Конвертацию из лог. px окна делает вызывающий
//! (Shell): `(pos − slot_origin) × scale_factor`.

use crate::chartdx::pane::Container;
use moon_chart::paint::now_unix_ms;
use moon_chart::view::{ChartView, Rect};
use moon_chart::{GLASS_ZONE_PX, PRICE_AXIS_W};
use moon_core::session::CoreId;

/// Кнопка мыши (вместо winit::MouseButton).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Btn {
    Left,
    Right,
}

const WHEEL_THRESHOLD: f32 = 100.0;
const ANCHOR_BREAK_PCT: f32 = 0.10;
const RMB_ZOOM_START_PX: f32 = 4.0;

#[derive(Default)]
pub struct ChartInput {
    /// Позиция перекрестия (девайс-px); None — вне зоны графика.
    pub cursor: Option<(f32, f32)>,
    /// Последняя позиция указателя (девайс-px).
    pub last_ptr: (f32, f32),
    /// Панель под курсором (индекс в контейнере).
    pub hovered_pane: Option<usize>,
    /// Раскладка панелей (девайс-px) прошлого кадра — hit-тест ввода.
    pub pane_rects: Vec<(usize, Rect)>,
    /// Двойной клик по чарту → отправить монету на Main (владелец забирает).
    pub pending_to_main: Option<(CoreId, String)>,

    lmb_down: bool,
    lmb_x_active: bool,
    drag_pane: Option<usize>,
    drag_accum: (f32, f32),
    wheel_accum: f32,
    wheel_pane: Option<usize>,
    rmb_down: bool,
    /// ПКМ сдвинулся за порог → зум-перетаскивание цены.
    rmb_moved: bool,
    rmb_start_y: f32,
    rmb_start_range: f32,
    rmb_start_center: f32,
    /// Время/позиция прошлого ЛКМ-нажатия — для детекта двойного клика.
    last_lmb_ms: f64,
    last_lmb_pos: (f32, f32),
}

impl ChartInput {
    fn plot_metrics_for(&self, pane: Option<usize>, fallback_w: f32, ppp: f32) -> (f32, f32) {
        let Some(idx) = pane else {
            return (
                fallback_w.max(1.0),
                self.last_ptr.0.clamp(0.0, fallback_w.max(1.0)),
            );
        };
        let Some((_, r)) = self.pane_rects.iter().find(|(i, _)| *i == idx) else {
            return (
                fallback_w.max(1.0),
                self.last_ptr.0.clamp(0.0, fallback_w.max(1.0)),
            );
        };
        let price_axis_w = PRICE_AXIS_W * ppp;
        let glass_w = GLASS_ZONE_PX.min(r.w * 0.5);
        let plot_w = (r.w - price_axis_w - glass_w).max(1.0);
        let cursor_x = (self.last_ptr.0 - r.x - price_axis_w).clamp(0.0, plot_w);
        (plot_w, cursor_x)
    }

    /// `view` панели под курсором (для пан/зум). None — курсор не над панелью.
    pub fn hovered_view_mut<'c>(&self, container: &'c mut Container) -> Option<&'c mut ChartView> {
        self.view_mut(container, self.hovered_pane)
    }

    fn view_mut<'c>(
        &self,
        container: &'c mut Container,
        pane: Option<usize>,
    ) -> Option<&'c mut ChartView> {
        let idx = pane?;
        container.view_mut(idx)
    }

    /// Двойной ЛКМ по чарту (не стакану) панели под курсором → запомнить монету.
    fn try_dblclick_to_main(&mut self, container: &Container) {
        let Some(idx) = self.hovered_pane else { return };
        let Some((_, r)) = self.pane_rects.iter().find(|(i, _)| *i == idx) else {
            return;
        };
        // В стакане (правая зона GLASS_ZONE_PX) дабл-клик игнорируем.
        let glass_w = GLASS_ZONE_PX.min(r.w * 0.5);
        if self.last_ptr.0 >= r.x + r.w - glass_w {
            return;
        }
        self.pending_to_main = container.target(idx);
    }

    /// Колесо: зум по X (или пан по X при Shift) — у панели под курсором.
    /// `dy` — знак/величина прокрутки (lines), `gate_ok` — указатель в зоне графика.
    /// Возвращает «нужен кадр».
    pub fn wheel(
        &mut self,
        dy: f32,
        shift: bool,
        gate_ok: bool,
        container: &mut Container,
        fallback_w: f32,
        ppp: f32,
    ) -> bool {
        if !gate_ok || dy == 0.0 {
            return false;
        }
        if self.wheel_pane != self.hovered_pane {
            self.wheel_accum = 0.0;
            self.wheel_pane = self.hovered_pane;
        }
        let (plot_w, cursor_x) = self.plot_metrics_for(self.hovered_pane, fallback_w, ppp);
        let now = now_unix_ms();
        if let Some(view) = self.hovered_view_mut(container) {
            if shift {
                view.pan_x_px(-dy.signum() * 60.0, now, plot_w);
            } else {
                self.wheel_accum += dy * 40.0;
                if self.wheel_accum.abs() < WHEEL_THRESHOLD {
                    return false;
                }
                // Terminal UX: wheel up zooms in, wheel down zooms out.
                let factor = if self.wheel_accum > 0.0 { 2.0 } else { 0.5 };
                self.wheel_accum = 0.0;
                view.zoom_x_at(factor, plot_w, cursor_x, now);
            }
            return true;
        }
        false
    }

    /// Нажатие/отпускание кнопки. `gate_ok` — указатель в зоне графика (гейтит
    /// только нажатия). ПКМ-drag = зум цены; короткий ПКМ-клик ничего не переключает,
    /// потому что один `ChartEngine` больше не имеет внутреннего tiled-режима.
    /// `allow_dbl_to_main` — разрешён ли дабл-клик→Main. Возврат: «нужен кадр».
    pub fn mouse_button(
        &mut self,
        button: Btn,
        pressed: bool,
        gate_ok: bool,
        allow_dbl_to_main: bool,
        container: &mut Container,
        ppp: f32,
        fallback_w: f32,
    ) -> bool {
        let mut changed = false;
        match button {
            Btn::Left => {
                if pressed {
                    if !gate_ok {
                        return false; // клик по UI-панелям — не таскаем график
                    }
                    let now = now_unix_ms();
                    let (px, py) = self.last_ptr;
                    let dbl = now - self.last_lmb_ms < 400.0
                        && (px - self.last_lmb_pos.0).abs() < 28.0
                        && (py - self.last_lmb_pos.1).abs() < 28.0;
                    self.last_lmb_ms = now;
                    self.last_lmb_pos = (px, py);
                    if dbl && allow_dbl_to_main {
                        self.try_dblclick_to_main(container);
                    }
                    self.lmb_down = true;
                    self.lmb_x_active = false;
                    self.drag_pane = self.hovered_pane;
                    self.drag_accum = (0.0, 0.0);
                } else {
                    if self.lmb_down && self.lmb_x_active {
                        let target = self.drag_pane;
                        let (plot_w, _) = self.plot_metrics_for(target, fallback_w, ppp);
                        let now = now_unix_ms();
                        if let Some(view) = self.view_mut(container, target) {
                            changed |= view.snap_to_live_if_near(now, plot_w);
                        }
                    }
                    self.lmb_down = false;
                    self.lmb_x_active = false;
                    if !self.rmb_down {
                        self.drag_pane = None;
                    }
                }
            }
            Btn::Right => {
                if pressed {
                    if !gate_ok {
                        return false;
                    }
                    self.rmb_down = true;
                    self.rmb_moved = false;
                    self.drag_pane = self.hovered_pane;
                    self.rmb_start_y = self.last_ptr.1;
                    let snap = self
                        .view_mut(container, self.drag_pane)
                        .map(|v| (v.render_range, v.render_center));
                    if let Some((r, c)) = snap {
                        self.rmb_start_range = r;
                        self.rmb_start_center = c;
                    }
                } else {
                    self.rmb_down = false;
                    if !self.lmb_down {
                        self.drag_pane = None;
                    }
                }
            }
        }
        changed
    }

    /// Drag-часть движения: ЛКМ (пан X/Y) и ПКМ (вертикальный зум цены от снимка).
    /// Сам обновляет `last_ptr`. Возвращает «нужен кадр» (идёт перетаскивание).
    pub fn pointer_drag(
        &mut self,
        x: f32,
        y: f32,
        container: &mut Container,
        ppp: f32,
        fallback_w: f32,
    ) -> bool {
        let dx = x - self.last_ptr.0;
        let dy = y - self.last_ptr.1;
        self.last_ptr = (x, y);
        let mut changed = false;

        // ЛКМ: горизонталь → пан по времени, вертикаль → пан по цене.
        if self.lmb_down {
            let target = self.drag_pane.or(self.hovered_pane);
            let (plot_w, _) = self.plot_metrics_for(target, fallback_w, ppp);
            self.drag_accum.0 += dx;
            self.drag_accum.1 += dy;
            let now = now_unix_ms();
            if let Some(view) = self.view_mut(container, target) {
                if dy != 0.0 {
                    view.pan_y_px(dy, now);
                    changed = true;
                }
                if !self.lmb_x_active
                    && self.drag_accum.0.abs() >= plot_w * ANCHOR_BREAK_PCT
                    && self.drag_accum.0.abs() >= self.drag_accum.1.abs()
                {
                    self.lmb_x_active = true;
                }
                if self.lmb_x_active && dx != 0.0 {
                    view.pan_x_px(dx, now, plot_w);
                    changed = true;
                }
            }
        }

        // ПКМ: вертикальный зум по цене от снимка нажатия.
        if self.rmb_down {
            let cum = y - self.rmb_start_y;
            if cum.abs() > RMB_ZOOM_START_PX {
                self.rmb_moved = true;
            }
            if self.rmb_moved {
                let (c, r) = (self.rmb_start_center, self.rmb_start_range);
                let now = now_unix_ms();
                if let Some(view) = self.view_mut(container, self.drag_pane.or(self.hovered_pane)) {
                    view.rmb_zoom(c, r, cum, now);
                    changed = true;
                }
            }
        }

        changed
    }

    /// Синхронизировать зажатость кнопок из факта move-события. GPUI шлёт mouse_up
    /// только над элементом → отпускание вне слота теряется и drag «залипает».
    /// Move сообщает реально зажатую кнопку — сбрасываем залипшее состояние.
    pub fn sync_pressed(&mut self, left_held: bool, right_held: bool) {
        if !left_held {
            self.lmb_down = false;
            self.lmb_x_active = false;
        }
        if !right_held {
            self.rmb_down = false;
        }
        if !left_held && !right_held {
            self.drag_pane = None;
        }
    }

    /// Сдвигался ли ПКМ за порог зум-перетаскивания цены с момента нажатия. true =
    /// это был зум-drag, а не короткий клик (нужно, чтобы возврат из фулскрина по ПКМ
    /// не срабатывал после зума цены).
    pub fn rmb_moved(&self) -> bool {
        self.rmb_moved
    }

    /// Hit-тест панели под точкой по `pane_rects`.
    pub fn pane_at(&self, x: f32, y: f32) -> Option<usize> {
        self.pane_rects
            .iter()
            .find(|(_, r)| x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h)
            .map(|(i, _)| *i)
    }
}
