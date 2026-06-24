//! Состояние вида графика — порт интерактива из MoonBot/WebGame:
//!   X (время): зум колесом вокруг курсора, пан ЛКМ/Shift-колесо. Live/latest
//!              определяется пространственно: правый край в пределах 5% окна
//!              от now снова якорится к «сейчас», таймера возврата нет.
//!   Y (цена):  авто/фикс-процент и ручной Y-pan/RMB-zoom живут отдельно от
//!              X-follow, чтобы горизонтальный просмотр истории не замораживал
//!              ценовую шкалу.

/// Прямоугольник в пикселях (top-left origin).
#[derive(Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

// --- Константы поведения ---
/// Скорость подгона авто-масштаба к видимому диапазону (доля за кадр).
const AUTO_LERP: f32 = 0.15;
/// Мёртвая зона центровки: пока цена в пределах ±BUFFER*range от центра — не
/// двигаем (иначе дёргалось бы каждый кадр). Буфер = доля текущего масштаба.
const CENTER_BUFFER: f32 = 0.10;
/// Скорость возврата центра к цене, когда она вышла за буфер (доля за кадр).
const TICK_LERP: f32 = 0.10;
/// Пикселей вертикального drag ПКМ на удвоение/деление диапазона Y.
const YSCALE_PX_PER_2X: f32 = 150.0;
/// Гистерезис диапазона Y: render_range держим, пока «гладкая» цель не уйдёт за
/// ±15% — тогда снап к цели. Между снапами масштаб Y постоянен (нужно, чтобы
/// scrollable canvas Stage 2 оставался валиден кадрами между прыжками).
const RANGE_HYST: f32 = 1.15;
/// Порог сдвига центра Y в пикселях: пока цена не уехала дальше — центр стоит.
const CENTER_SNAP_PX: f32 = 8.0;
/// Если правый live-якорь ближе этого расстояния к now, считаем вид снова live.
const LIVE_REJOIN_FRAC: f32 = 0.05;
/// Максимальное видимое окно времени, мс (Delphi MaxTimeRange=360 минут = 6 часов).
const MAX_WINDOW_MS: f32 = 21_600_000.0;
/// Дефолтное видимое окно, к которому выбираем пиксельно-гладкий live scale.
const DEFAULT_WINDOW_MS: f32 = 60_000.0;
/// Минимальное видимое окно при макс. зум-ин (раньше упирались в дефолтные 60 c).
const MIN_WINDOW_MS: f32 = 30_000.0;
/// Сколько держим ручной X-режим после последнего пана, прежде чем авто-вернуться в live.
const MANUAL_HOLD_MS: f64 = 3000.0;

#[derive(Clone)]
pub struct ChartView {
    /// Фиксированная точка отсчёта времени (unix ms), задаётся при старте.
    pub epoch_ms: f64,
    /// Зум по X: пикселей на миллисекунду.
    pub px_per_ms: f32,
    /// Время (unix ms) у правого «сейчас»-якоря области.
    pub right_time_ms: f64,
    /// Авто-следование за правым краем по времени (live).
    pub follow: bool,
    /// Доля ширины окна, оставляемая справа как «будущее» (аналог xRange*0.9).
    pub right_margin_frac: f32,

    /// Цена в центре области.
    pub center_price: f32,
    /// Видимый диапазон цены (единицы цены, сверху-вниз области).
    pub price_range: f32,
    /// Авто-подгон диапазона цены (кнопка «Авто»).
    pub auto_price: bool,
    /// Ручной Y-view после вертикального drag / RMB zoom. Сбрасывается кнопками
    /// масштаба, но не кнопкой Live: Live отвечает только за X/latest.
    pub manual_price: bool,
    /// Последний фикс-процент (range = center*percent), для дрейф-режима.
    pub scale_percent: f32,
    /// Производное: пикселей на единицу цены (кэш для пана/хит-теста).
    pub px_per_price: f32,

    /// Снапнутые (кусочно-постоянные) Y-параметры, которыми РЕАЛЬНО рисуем.
    /// Живой «гладкий» target — center_price/price_range; рендер берёт
    /// render_center/render_range и держит их стабильными между редкими
    /// прыжками → большинство кадров Y-маппинг не меняется (база для canvas).
    pub render_center: f32,
    pub render_range: f32,

    /// Полуразмер крестика, px.
    pub marker_half_px: f32,

    /// Временной scale стоит в default-фазовом режиме: можно пересчитать его при
    /// resize/present-rate change. Первый ручной zoom выводит из этого режима.
    x_default_scale: bool,
    /// Ещё не подгоняли зум под дефолтное окно (делается раз, по реальной ширине
    /// зоны графика в первом кадре).
    x_init_pending: bool,
    last_phase_area_w: f32,
    last_phase_present_hz: f32,
    phase_default_px_per_ms: f32,
    /// Время прошлого `update_y` (unix мс) — для нормировки Y-сглаживания по реальному
    /// dt, а не "за кадр" (иначе скорость авто-Y зависела бы от частоты подготовки).
    last_update_ms: f64,
    /// До этого момента (unix мс) держим ручной X-режим после пана; по истечении
    /// `tick_auto_live` авто-возвращает live. 0 = нет отложенного возврата (или уже live).
    manual_until: f64,
}

impl ChartView {
    pub fn new(epoch_ms: f64) -> Self {
        Self {
            epoch_ms,
            px_per_ms: 0.05, // ~20 секунд видимого окна на 1000 px
            right_time_ms: epoch_ms,
            follow: true,
            right_margin_frac: 0.10, // поле «будущего» справа как в moonweb (xRange*0.9)
            center_price: 0.0,
            price_range: 1.0,
            auto_price: true,
            manual_price: false,
            scale_percent: 0.10,
            px_per_price: 0.5,
            render_center: 0.0,
            render_range: 1.0,
            marker_half_px: 3.5, // крест 7px (NormalX MoonBot)
            x_default_scale: true,
            x_init_pending: true,
            last_phase_area_w: f32::NAN,
            last_phase_present_hz: f32::NAN,
            phase_default_px_per_ms: 0.0,
            last_update_ms: 0.0,
            manual_until: 0.0,
        }
    }

    fn phase_clean_default_px_per_ms(area_w: f32, present_hz: f32) -> f32 {
        let dt_ms = 1000.0 / present_hz.max(1.0);
        let s0 = area_w.max(1.0) * dt_ms / DEFAULT_WINDOW_MS;
        let shift_px = if s0 >= 1.0 {
            s0.round().max(1.0)
        } else {
            let n = (1.0 / s0.max(1e-9)).round().max(1.0);
            1.0 / n
        };
        (shift_px / dt_ms).max(1e-9)
    }

    /// Подгоняет default time window к ближайшей фазо-чистой точке вокруг 60 c:
    /// целое число px/frame или 1 px за N кадров. Пересчитывается только пока
    /// scale остаётся default/reset-to-live, при первом кадре/resize/present change.
    pub fn ensure_default_window(&mut self, area_w: f32, present_hz: f32) {
        if area_w < 1.0 {
            return;
        }
        let present_hz = present_hz.max(1.0);
        let default_px_per_ms = Self::phase_clean_default_px_per_ms(area_w, present_hz);
        let phase_changed = (area_w - self.last_phase_area_w).abs() >= 0.5
            || (present_hz - self.last_phase_present_hz).abs() >= 0.5;
        if phase_changed || self.x_init_pending {
            self.phase_default_px_per_ms = default_px_per_ms;
            self.last_phase_area_w = area_w;
            self.last_phase_present_hz = present_hz;
        }
        if !self.x_init_pending && !(self.x_default_scale && phase_changed) {
            return;
        }
        self.px_per_ms = default_px_per_ms;
        self.x_default_scale = true;
        self.x_init_pending = false;
    }

    /// Лайв сейчас?
    pub fn is_live(&self, now_ms: f64) -> bool {
        let _ = now_ms;
        self.follow
    }

    /// Якорит правый край к `edge_ms`, если идёт лайв. Smooth wall-clock режим
    /// (CHART_RENDERING_TZ): зовётся с now_ms → правый край = «сейчас», гладкий
    /// скролл по времени. Дешевизна кадра обеспечивается canvas UV-scroll +
    /// egui-mesh cache (Stage 2b/2c), а не пропуском кадров.
    pub fn follow_edge(&mut self, edge_ms: f64, now_ms: f64) {
        if self.is_live(now_ms) {
            self.right_time_ms = self.quantize_edge_ms(edge_ms);
        }
    }

    /// Снап правого края на ЦЕЛЫЙ пиксель (аналог MoonBot `NowPhase`): между кадрами
    /// меняется только целое число пикселей, поэтому тонкие элементы (кресты трейдов,
    /// линии ордеров, last/mark) не дрожат субпиксельно. Контринтуитивно, но дискретный
    /// шаг = гладко для чёткого 2D (на 60+ Гц шаг в 1 px глаз не ловит). ТУ ЖЕ формулу
    /// применяет own-pass callback, двигая камеру на каждый present (см. chartdx).
    pub fn quantize_edge_ms(&self, edge_ms: f64) -> f64 {
        let ppm = self.px_per_ms.max(1e-9) as f64;
        let rel = edge_ms - self.epoch_ms;
        (rel * ppm).round() / ppm + self.epoch_ms
    }

    /// Немедленный возврат к лайву (кнопка Live): к «сейчас».
    pub fn resume_live(&mut self, now_ms: f64) {
        self.follow = true;
        self.right_time_ms = now_ms;
        self.manual_until = 0.0;
    }

    /// Явное (постоянное) выключение live — кнопка тулбара: без авто-возврата.
    pub fn set_manual_persistent(&mut self) {
        self.follow = false;
        self.manual_until = 0.0;
    }

    /// Истёк ли ручной hold после пана → авто-возврат в live (П.9). Драйвится таймером
    /// (own-pass двигает камеру, но prepare в покое не тикает — см. panels/chart.rs).
    /// Возвращает true, если возобновили live.
    pub fn tick_auto_live(&mut self, now_ms: f64) -> bool {
        if !self.follow && self.manual_until > 0.0 && now_ms >= self.manual_until {
            self.resume_live(now_ms);
            true
        } else {
            false
        }
    }

    /// Ближайший дедлайн авто-возврата (unix мс), если ожидается — для арминга таймера.
    pub fn auto_live_deadline_ms(&self) -> Option<f64> {
        if !self.follow && self.manual_until > 0.0 {
            Some(self.manual_until)
        } else {
            None
        }
    }

    pub fn reset_default_window_on_next_prepare(&mut self) {
        self.x_default_scale = true;
        self.x_init_pending = true;
    }

    pub fn snap_to_live_if_near(&mut self, now_ms: f64, area_w: f32) -> bool {
        if self.follow {
            return false;
        }
        let tolerance_ms =
            (area_w.max(1.0) * LIVE_REJOIN_FRAC) as f64 / self.px_per_ms.max(1e-6) as f64;
        if now_ms - self.right_time_ms <= tolerance_ms {
            self.resume_live(now_ms);
            true
        } else {
            false
        }
    }

    /// Видимое окно по X: (время у левого края, ширина окна в мс).
    /// Единый источник X-геометрии для uniform и для куллинга видимых тиков.
    pub fn visible_x(&self, area_w: f32) -> (f32, f32) {
        let window_ms = area_w / self.px_per_ms.max(1e-6);
        let right_rel =
            (self.right_time_ms - self.epoch_ms) as f32 + window_ms * self.right_margin_frac;
        (right_rel - window_ms, window_ms)
    }

    // ── Масштаб Y (кнопки тулбара) ────────────────────────────────────────────

    /// Кнопка «Авто» — динамический подгон под видимый диапазон.
    pub fn set_auto(&mut self) {
        self.auto_price = true;
        self.manual_price = false;
    }

    /// Фикс-процент: видимый диапазон = цена*percent (как ZoomBar moonweb).
    pub fn set_scale_percent(&mut self, percent: f32) {
        self.auto_price = false;
        self.manual_price = false;
        self.scale_percent = percent;
        let base = if self.center_price.abs() > 1e-6 {
            self.center_price.abs()
        } else {
            self.price_range
        };
        self.price_range = (base * percent).max(1e-6);
        self.render_range = self.price_range;
        self.render_center = self.center_price;
    }

    // ── Пан / зум мышью ─────────────────────────────────────────────────────────
    // X-drag отрывает view от live сразу; re-anchor проверяется отдельно на mouse-up.

    /// Пан по X на dx пикселей (drag ЛКМ / Shift-колесо).
    pub fn pan_x_px(&mut self, dx: f32, now_ms: f64, area_w: f32) {
        let dt_ms = dx as f64 / self.px_per_ms.max(1e-6) as f64;
        self.right_time_ms = (self.right_time_ms - dt_ms).min(now_ms);
        self.follow = false;
        // П.9: пан не выключает live навсегда — заводим окно ручного удержания, по
        // истечении которого `tick_auto_live` снова якорится к «сейчас». Каждый кадр
        // пана сдвигает дедлайн вперёд, поэтому возврат идёт через ~3 c ПОСЛЕ отпускания.
        self.manual_until = now_ms + MANUAL_HOLD_MS;
        let _ = area_w;
    }

    /// Пан по Y на dy пикселей (drag ЛКМ).
    pub fn pan_y_px(&mut self, dy: f32, now_ms: f64) {
        let _ = now_ms;
        self.center_price += dy / self.px_per_price.max(1e-6);
        self.manual_price = true;
        self.render_center = self.center_price;
    }

    /// Зум по X. В live сохраняем live-якорь (как WebGame/MoonBot); в ручном X-view
    /// сохраняем время под курсором и после дискретного шага можем re-anchor к live.
    pub fn zoom_x_at(&mut self, factor: f32, area_w: f32, cursor_x: f32, now_ms: f64) {
        let was_follow = self.follow;
        let old_px = self.px_per_ms.max(1e-6);
        let cursor_x = cursor_x.clamp(0.0, area_w.max(1.0));
        let (old_left, _) = self.visible_x(area_w);
        let cursor_time = self.epoch_ms + old_left as f64 + cursor_x as f64 / old_px as f64;
        let next = self.px_per_ms * factor;
        let lo = if area_w >= 1.0 {
            area_w / MAX_WINDOW_MS
        } else {
            0.0005
        };
        // Зум-ин разрешаем глубже дефолта (60 c) до MIN_WINDOW_MS (30 c): база — фазо-чистый
        // дефолт, домноженный на DEFAULT/MIN = 2×. Иначе максимум упирался в 1 мин (П.7).
        let hi = (if self.phase_default_px_per_ms > 0.0 {
            self.phase_default_px_per_ms
        } else {
            Self::phase_clean_default_px_per_ms(area_w, 60.0)
        } * (DEFAULT_WINDOW_MS / MIN_WINDOW_MS))
            .max(lo);
        self.px_per_ms = next.clamp(lo, hi);
        self.x_default_scale = (self.px_per_ms - self.phase_default_px_per_ms).abs() <= 1e-9;
        if was_follow {
            self.right_time_ms = now_ms;
            self.follow = true;
            return;
        }
        let new_window = area_w / self.px_per_ms.max(1e-6);
        let left = cursor_time - self.epoch_ms - cursor_x as f64 / self.px_per_ms as f64;
        self.right_time_ms =
            (self.epoch_ms + left + new_window as f64 * (1.0 - self.right_margin_frac as f64))
                .min(now_ms);
        self.snap_to_live_if_near(now_ms, area_w);
    }

    /// Зум по Y (drag ПКМ) от снимка на момент нажатия. up=zoom out, down=zoom in.
    pub fn rmb_zoom(&mut self, start_center: f32, start_range: f32, cum_dy: f32, now_ms: f64) {
        let factor = 2f32.powf(-cum_dy / YSCALE_PX_PER_2X);
        let r = (start_range * factor).clamp(start_range * 0.25, start_range * 4.0);
        self.center_price = start_center;
        self.price_range = r.max(1e-6);
        self.manual_price = true;
        self.render_center = self.center_price;
        self.render_range = self.price_range;
        let _ = now_ms;
    }

    // ── Обновление шкалы цены раз в кадр ─────────────────────────────────────────

    /// Подгоняет центр/диапазон цены. X-follow влияет только на выбор target:
    /// live центрируется по последней цене, manual-X — по видимому диапазону.
    /// Ручной Y-pan/RMB-zoom (`manual_price`) замораживает Y до выбора масштаба.
    pub fn update_y(
        &mut self,
        now_ms: f64,
        area_h: f32,
        visible: Option<(f32, f32)>,
        last_price: Option<f32>,
    ) {
        let live = self.is_live(now_ms);
        // Сглаживание Y по РЕАЛЬНОМУ dt, а не "за кадр": prepare теперь тикает с плавающей
        // частотой (камеру двигает own-pass на vblank, данные — реже), и константа "доля за
        // кадр" давала бы разную скорость авто-зума/центровки на разных машинах. Переводим
        // per-frame-коэффициенты к фактическому интервалу: f = 1 - (1-base)^(dt/16.67мс).
        let dt_ms = if self.last_update_ms > 0.0 {
            (now_ms - self.last_update_ms).clamp(1.0, 250.0)
        } else {
            1000.0 / 60.0
        };
        self.last_update_ms = now_ms;
        let frame_ref = 1000.0 / 60.0;
        let auto_lerp = (1.0 - (1.0 - AUTO_LERP as f64).powf(dt_ms / frame_ref)) as f32;
        let tick_lerp = (1.0 - (1.0 - TICK_LERP as f64).powf(dt_ms / frame_ref)) as f32;
        if !self.manual_price {
            let visible_mid = visible.map(|(lo, hi)| (lo + hi) * 0.5);
            let target_center = if live {
                last_price.or(visible_mid)
            } else {
                visible_mid.or(last_price)
            };
            let target_range = match (self.auto_price, visible, target_center) {
                (true, Some((lo, hi)), Some(c)) if live => {
                    // В live держим последнюю цену в центре и симметрично расширяем range,
                    // чтобы ни хвосты тиков, ни ордерные линии не обрезались. ×1.20 = поле
                    // ~10% по краям (линии не впритирку к верх/низ).
                    let half = (c - lo).max(hi - c).max(c.abs() * 0.0005 + 1e-6);
                    Some(half * 2.0 * 1.20)
                }
                (true, Some((lo, hi)), Some(c)) => {
                    Some((hi - lo).abs().max(c.abs() * 0.0005 + 1e-6) * 1.20)
                }
                (true, None, Some(c)) => Some((c.abs() * 0.001).max(1e-6)),
                (false, _, Some(c)) => Some((c.abs() * self.scale_percent).max(1e-6)),
                _ => None,
            };

            if let Some(r) = target_range {
                if live && self.auto_price && self.center_price != 0.0 && self.price_range > 0.0 {
                    // Асимметрия (П.5): РАСШИРЯЕМ диапазон мгновенно — иначе видимые тики/
                    // ордер-линии обрезаются на десятки кадров, пока медленный lerp догоняет
                    // (симптом: видно только линию покупки + стакан, тики за экраном). СУЖАЕМ
                    // плавно — нет дёрганья при кратковременных всплесках цены.
                    if r > self.price_range {
                        self.price_range = r;
                    } else {
                        self.price_range += (r - self.price_range) * auto_lerp;
                    }
                } else {
                    self.price_range = r;
                }
            }

            if let Some(c) = target_center {
                if self.center_price == 0.0 || !live {
                    self.center_price = c;
                } else if self.price_range > 1e-9 {
                    let drift = (c - self.center_price).abs() / self.price_range;
                    if drift > CENTER_BUFFER {
                        self.center_price += (c - self.center_price) * tick_lerp;
                    }
                }
            }
        }
        if !(self.price_range > 1e-9) {
            self.price_range = self.center_price.abs() * 0.10 + 1.0;
        }
        // Снап Y. В лайве render_* кусочно-постоянны: range держим, пока цель не
        // ушла за ±RANGE_HYST; центр — пока цена не уехала > CENTER_SNAP_PX px.
        // В ручном режиме следуем точно за вводом (drag отзывчив; canvas
        // пере-бейкается — это transient на время взаимодействия).
        if live && !self.manual_price {
            let target = self.price_range.max(1e-9);
            if !self.auto_price
                || !(self.render_range > 1e-9)
                || target > self.render_range * RANGE_HYST
                || target < self.render_range / RANGE_HYST
            {
                self.render_range = target;
            }
            let ppp = (area_h / self.render_range.max(1e-9)).max(1e-6);
            if (self.center_price - self.render_center).abs() * ppp > CENTER_SNAP_PX {
                self.render_center = self.center_price;
            }
        } else {
            self.render_range = self.price_range.max(1e-9);
            self.render_center = self.center_price;
        }
        self.px_per_price = (area_h / self.render_range.max(1e-9)).max(1e-6);
    }
}

#[cfg(test)]
mod tests {
    use super::{ChartView, MANUAL_HOLD_MS};

    fn default_window_sec(width: f32, present_hz: f32) -> f32 {
        let px_per_ms = ChartView::phase_clean_default_px_per_ms(width, present_hz);
        width / px_per_ms / 1000.0
    }

    #[test]
    fn default_time_window_snaps_to_phase_clean_values_around_60s() {
        let cases = [
            (1000.0, 66.66667),
            (1280.0, 64.0),
            (1920.0, 64.0),
            (2560.0, 42.66667),
        ];
        for (width, expected) in cases {
            let actual = default_window_sec(width, 60.0);
            assert!(
                (actual - expected).abs() < 0.01,
                "width={width}: got {actual}, expected {expected}"
            );
        }
    }

    #[test]
    fn x_pan_detaches_immediately_even_inside_live_snap_zone() {
        let now = 100_000.0;
        let mut view = ChartView::new(0.0);
        view.ensure_default_window(1000.0, 60.0);
        view.resume_live(now);

        view.pan_x_px(1.0, now, 1000.0);

        assert!(!view.follow);
        assert!(view.right_time_ms < now);
        assert!(view.snap_to_live_if_near(now, 1000.0));
        assert!(view.follow);
    }

    #[test]
    fn zoom_in_is_clamped_to_min_window_30s() {
        let now = 100_000.0;
        let mut view = ChartView::new(0.0);
        view.ensure_default_window(1000.0, 60.0);
        let default_px_per_ms = view.px_per_ms;

        // Один шаг зум-ин (×2) от дефолта (60 c) допускается до 30 c = 2× px/ms (П.7).
        view.zoom_x_at(2.0, 1000.0, 500.0, now);
        assert!((view.px_per_ms - default_px_per_ms * 2.0).abs() < 1e-9);
        assert!(view.follow);

        // Дальнейший зум-ин упирается в потолок (30 c), глубже нельзя.
        view.zoom_x_at(2.0, 1000.0, 500.0, now);
        assert!((view.px_per_ms - default_px_per_ms * 2.0).abs() < 1e-9);
    }

    #[test]
    fn pan_then_hold_auto_returns_to_live() {
        let now = 100_000.0;
        let mut view = ChartView::new(0.0);
        view.ensure_default_window(1000.0, 60.0);
        view.resume_live(now);

        view.pan_x_px(50.0, now, 1000.0);
        assert!(!view.follow);
        // Внутри окна удержания live не возобновляется.
        assert!(!view.tick_auto_live(now + 1000.0));
        assert!(!view.follow);
        // По истечении удержания — авто-возврат к live.
        assert!(view.tick_auto_live(now + MANUAL_HOLD_MS + 1.0));
        assert!(view.follow);
    }

    #[test]
    fn explicit_follow_off_has_no_auto_return() {
        let now = 100_000.0;
        let mut view = ChartView::new(0.0);
        view.resume_live(now);
        // Явное выключение (кнопка Live) — без отложенного возврата.
        view.set_manual_persistent();
        assert!(!view.follow);
        assert!(view.auto_live_deadline_ms().is_none());
        assert!(!view.tick_auto_live(now + 10_000.0));
        assert!(!view.follow);
    }
}
