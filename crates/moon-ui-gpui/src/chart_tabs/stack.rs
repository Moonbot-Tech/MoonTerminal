//! Общий слой вертикального стека чартов (Main + AddToChart): единый тип записи,
//! хелперы масштаба/очистки и 3-режимная раскладка (FIT/SCROLL/COMPRESS), параметризованная
//! фабрикой плитки. Нюансы Main (fullscreen / active / ПКМ-возврат) остаются в `MainChartStack`.

use std::time::{Duration, Instant};

use gpui::*;
use moon_ui::{MoonScrollbarVisibility, MoonVirtualList, MoonVirtualListScrollHandle, v_flex};

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

/// Убрать из стека панели без графиков. Возвращает true, если состав изменился.
pub(super) fn retain_nonempty_panels(entries: &mut Vec<ChartStackEntry>, cx: &App) -> bool {
    let before = entries.len();
    entries.retain(|e| e.panel.read(cx).pane_count() > 0);
    entries.len() != before
}

/// 3-режимная вертикальная раскладка стека (режим — из Настроек):
///  • scroll=false               → FIT: панели делят высоту окна;
///  • scroll=true, compress=false → SCROLL: фикс. высота `cfg_h`, `MoonVirtualList` со скроллом;
///  • scroll=true, compress=true  → COMPRESS: фикс. высота, без скролла, сжатие при переполнении.
///
/// `panel_at` достаёт панель по индексу, `tile` строит одну плитку (Main — с ПКМ-возвратом,
/// Add — простую). FIT/COMPRESS итерируют переданный `s` (это `&self` вызывающего стека), а
/// SCROLL берёт панели через weak-entity в App-контексте — поэтому own-entity не читается
/// через `cx` (иначе RefCell-паника «already mutably borrowed» во время render).
#[allow(clippy::too_many_arguments)]
pub(super) fn render_chart_stack<S, P, T>(
    base_id: &str,
    s: &S,
    entity: Entity<S>,
    count: usize,
    scroll: bool,
    compress: bool,
    cfg_h: f32,
    scroll_handle: &MoonVirtualListScrollHandle,
    border: Rgba,
    panel_at: P,
    tile: T,
) -> AnyElement
where
    S: Render + 'static,
    P: Fn(&S, usize) -> Option<Entity<ChartPanel>> + Copy + 'static,
    T: Fn(&S, usize, Entity<ChartPanel>, Option<f32>, bool, Rgba, Entity<S>) -> AnyElement
        + Copy
        + 'static,
{
    if scroll && !compress {
        // SCROLL: фикс. высота, виртуальный список со скроллбаром. Плитку строим через
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
                tile(s, ix, panel, Some(cfg_h), false, border, ent.clone())
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

    // FIT / COMPRESS: v_flex на всю высоту окна, без скролла.
    // COMPRESS: каждый слот flex с cap = cfg_h (height=Some+flex=true → max_h в плитке): мало
    // графиков — каждый по cfg_h (низ пустой), много — сжимаются до window/count. FIT: flex без cap.
    let mut tiles: Vec<AnyElement> = Vec::with_capacity(count);
    for ix in 0..count {
        let (height, flex) = if compress {
            (Some(cfg_h), true)
        } else {
            (None, true)
        };
        match panel_at(s, ix) {
            Some(panel) => tiles.push(tile(s, ix, panel, height, flex, border, entity.clone())),
            None => {
                // Пустой (держащийся) слот COMPRESS — прозрачная плашка тех же размеров.
                let mut e = div().w_full().relative().overflow_hidden();
                if flex {
                    e = e.flex_1().min_h(px(0.0));
                    if let Some(h) = height {
                        e = e.max_h(px(h));
                    }
                } else if let Some(h) = height {
                    e = e.h(px(h)).min_h(px(0.0));
                }
                tiles.push(e.into_any_element());
            }
        }
    }
    div()
        .id(format!("{base_id}-fit"))
        .relative()
        .size_full()
        .overflow_hidden()
        .child(v_flex().size_full().children(tiles))
        .into_any_element()
}
