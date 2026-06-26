//! Общий слой вертикального стека чартов (Main + AddToChart): единый тип записи,
//! хелперы масштаба/очистки и 3-режимная раскладка (FIT/SCROLL/COMPRESS), параметризованная
//! фабрикой плитки. Нюансы Main (fullscreen / active / ПКМ-возврат) остаются в `MainChartStack`.

use std::time::{Duration, Instant};

use gpui::*;
use moon_ui::{MoonScrollbarVisibility, MoonVirtualList, MoonVirtualListScrollHandle, h_flex, v_flex};

use crate::chart_persist::StackLayoutMode;
use crate::panels::ChartPanel;
use moon_core::session::CoreId;

/// Одна запись (слот) стека: рынок ядра + его отдельный `ChartPanel`.
pub(super) struct ChartStackEntry {
    pub core: CoreId,
    pub market: String,
    pub panel: Entity<ChartPanel>,
    /// Когда график появился в слоте (для подсветки «нового» — пульс рамки `HIGHLIGHT`).
    pub arrived_at: Instant,
    /// Слот пуст (график закрылся/истёк по TTL), но держится позиционно — только COMPRESS
    /// (Fit+пиксели): соседи не сдвигаются и не меняют размер; новый занимает первый пустой;
    /// сброс всех слотов — когда пустыми стали ВСЕ. Рисуется прозрачной плашкой.
    pub vacated: bool,
}

impl ChartStackEntry {
    pub(super) fn new(core: CoreId, market: String, panel: Entity<ChartPanel>) -> Self {
        Self {
            core,
            market,
            panel,
            arrived_at: Instant::now(),
            vacated: false,
        }
    }
}

/// Дефолтная высота слота в режиме Scroll (px), когда у вкладки нет своей.
pub(super) const DEFAULT_SCROLL_HEIGHT: u16 = 300;

/// Длительность подсветки рамки только что появившегося графика (пульс).
pub(super) const HIGHLIGHT: Duration = Duration::from_millis(2600);

/// Разрешить раскладку стека из per-tab настроек вкладки в `(scroll, compress, высота_слота)`:
/// - `Fit` + высота 0 → растяжение (делят высоту окна): `(false, false, _)`;
/// - `Fit` + высота ≥20 → COMPRESS (фикс. высота, без скролла, сжатие): `(true, true, h)`;
/// - `Scroll` → фикс. высота + скролл: `(true, false, h)`.
pub(super) fn resolve_layout(
    mode: Option<StackLayoutMode>,
    height_fit: Option<u16>,
    height_scroll: Option<u16>,
) -> (bool, bool, f32) {
    match mode.unwrap_or(StackLayoutMode::Fit) {
        StackLayoutMode::Fit => {
            let hf = height_fit.unwrap_or(0);
            if hf == 0 {
                (false, false, 0.0)
            } else {
                (true, true, hf.clamp(20, 4000) as f32)
            }
        }
        StackLayoutMode::Scroll => {
            let hs = height_scroll
                .unwrap_or(DEFAULT_SCROLL_HEIGHT)
                .clamp(20, 4000);
            (true, false, hs as f32)
        }
    }
}

/// Применить масштаб ко всем панелям стека.
pub(super) fn set_panels_scale<S: 'static>(
    entries: &[ChartStackEntry],
    pct: Option<f32>,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel.update(cx, |p, pcx| p.set_scale(pct, pcx));
    }
}

/// Применить вкл/выкл стакана ко всем панелям стека.
pub(super) fn set_panels_orderbook_enabled<S: 'static>(
    entries: &[ChartStackEntry],
    enabled: bool,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel
            .update(cx, |p, pcx| p.set_orderbook_enabled(enabled, pcx));
    }
}

/// Применить вкл/выкл заливки зоны управления ко всем панелям стека.
pub(super) fn set_panels_show_zone<S: 'static>(
    entries: &[ChartStackEntry],
    show: bool,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel.update(cx, |p, pcx| p.set_show_zone(show, pcx));
    }
}

/// Применить вкл/выкл авто-пина при ордере ко всем панелям стека.
pub(super) fn set_panels_auto_pin<S: 'static>(
    entries: &[ChartStackEntry],
    on: bool,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel.update(cx, |p, pcx| p.set_auto_pin(on, pcx));
    }
}

/// Убрать из стека панели без графиков. Возвращает true, если состав изменился.
pub(super) fn retain_nonempty_panels(entries: &mut Vec<ChartStackEntry>, cx: &App) -> bool {
    let before = entries.len();
    entries.retain(|e| e.panel.read(cx).pane_count() > 0);
    entries.len() != before
}

/// 3-режимная раскладка стека (режим — из Настроек), ориентация `horizontal`:
///  • scroll=false               → FIT: панели делят высоту (верт.) / ширину (гор.) окна;
///  • scroll=true, compress=false → SCROLL: фикс. размер `cfg_h`, скролл по вертикали
///    (`MoonVirtualList`) либо по горизонтали (`overflow_x_scroll`-контейнер);
///  • scroll=true, compress=true  → COMPRESS: фикс. размер, без скролла, сжатие при переполнении.
///
/// `cfg_h` — фикс. размер слота вдоль оси стека (высота при верт., ширина при гор.). `panel_at`
/// достаёт панель по индексу, `tile(s, ix, panel, size, flex, horizontal, border, ent)` строит
/// одну плитку. FIT/COMPRESS итерируют переданный `s` (это `&self` вызывающего стека); вертикальный
/// SCROLL берёт панели через weak-entity в App-контексте (иначе RefCell-паника при render).
#[allow(clippy::too_many_arguments)]
pub(super) fn render_chart_stack<S, P, T>(
    base_id: &str,
    s: &S,
    entity: Entity<S>,
    count: usize,
    scroll: bool,
    compress: bool,
    horizontal: bool,
    cfg_h: f32,
    scroll_handle: &MoonVirtualListScrollHandle,
    border: Rgba,
    panel_at: P,
    tile: T,
) -> AnyElement
where
    S: Render + 'static,
    P: Fn(&S, usize) -> Option<Entity<ChartPanel>> + Copy + 'static,
    T: Fn(&S, usize, Entity<ChartPanel>, Option<f32>, bool, bool, Rgba, Entity<S>) -> AnyElement
        + Copy
        + 'static,
{
    if scroll && !compress {
        if horizontal {
            // Горизонтальный SCROLL: `MoonVirtualList` умеет только вертикаль (gpui `uniform_list`),
            // поэтому строим невиртуализированный ряд фикс-ширины в `overflow_x_scroll` (чартов
            // единицы — виртуализация не нужна). Каждая плитка: фикс. ШИРИНА cfg_h, full height.
            let mut tiles: Vec<AnyElement> = Vec::with_capacity(count);
            for ix in 0..count {
                if let Some(panel) = panel_at(s, ix) {
                    tiles.push(tile(s, ix, panel, Some(cfg_h), false, true, border, entity.clone()));
                }
            }
            return div()
                .id(format!("{base_id}-hscroll"))
                .relative()
                .size_full()
                .overflow_x_scroll()
                .child(h_flex().h_full().children(tiles))
                .into_any_element();
        }
        // Вертикальный SCROLL: фикс. высота, виртуальный список со скроллбаром. Плитку строим через
        // weak-entity (фабрика `MoonVirtualList` отдаёт `App`, а не `Context`).
        let weak = entity.downgrade();
        let list = MoonVirtualList::new(
            format!("{base_id}-vlist"),
            count,
            cfg_h,
            move |ix, _window, app| {
                let Some(ent) = weak.upgrade() else {
                    return div().into_any_element();
                };
                let s = ent.read(app);
                let Some(panel) = panel_at(s, ix) else {
                    return div().into_any_element();
                };
                tile(s, ix, panel, Some(cfg_h), false, false, border, ent.clone())
            },
        )
        .track_scroll(scroll_handle)
        .surface(false)
        .border(false)
        .radius(0.0)
        .scrollbar_visibility(MoonScrollbarVisibility::Scrolling);
        return div()
            .id(format!("{base_id}-scroll"))
            .relative()
            .size_full()
            .child(list)
            .into_any_element();
    }

    // FIT / COMPRESS: v_flex (верт.) / h_flex (гор.) на всё окно, без скролла.
    // COMPRESS: каждый слот flex с cap = cfg_h (size=Some+flex=true → max по оси в плитке): мало
    // графиков — каждый по cfg_h (хвост пустой), много — сжимаются до window/count. FIT: flex без cap.
    let mut tiles: Vec<AnyElement> = Vec::with_capacity(count);
    for ix in 0..count {
        let (size, flex) = if compress {
            (Some(cfg_h), true)
        } else {
            (None, true)
        };
        match panel_at(s, ix) {
            Some(panel) => {
                tiles.push(tile(s, ix, panel, size, flex, horizontal, border, entity.clone()))
            }
            None => {
                // Пустой (держащийся) слот COMPRESS — прозрачная плашка тех же размеров (по оси).
                let mut e = div().relative().overflow_hidden();
                e = if horizontal { e.h_full() } else { e.w_full() };
                if flex {
                    e = e.flex_1();
                    e = if horizontal { e.min_w(px(0.0)) } else { e.min_h(px(0.0)) };
                    if let Some(v) = size {
                        e = if horizontal { e.max_w(px(v)) } else { e.max_h(px(v)) };
                    }
                } else if let Some(v) = size {
                    e = if horizontal {
                        e.w(px(v)).min_w(px(0.0))
                    } else {
                        e.h(px(v)).min_h(px(0.0))
                    };
                }
                tiles.push(e.into_any_element());
            }
        }
    }
    let inner = if horizontal {
        h_flex().size_full().children(tiles)
    } else {
        v_flex().size_full().children(tiles)
    };
    div()
        .id(format!("{base_id}-fit"))
        .relative()
        .size_full()
        .overflow_hidden()
        .child(inner)
        .into_any_element()
}
