//! Общий слой вертикального стека чартов (Main + AddToChart): единый тип записи,
//! хелперы масштаба/очистки и 3-режимная раскладка (FIT/SCROLL/COMPRESS), параметризованная
//! фабрикой плитки. Нюансы Main (fullscreen / active / ПКМ-возврат) остаются в `MainChartStack`.

use std::ops::Range;
use std::time::{Duration, Instant};

use gpui::*;
use moon_ui::{
    MoonPalette, MoonScrollableElement, MoonScrollbarVisibility, MoonVirtualList,
    MoonVirtualListScrollHandle, h_flex, v_flex,
};

use crate::chart_persist::{StackLayoutMode, StackOrientation};
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

/// Ширина узкого слота соседа якоря в режиме метлы (= ширина стакана `GLASS_ZONE_PX` + рамки).
pub(super) const COMPARE_BOOK_W: f32 = moon_chart::GLASS_ZONE_PX + 2.0;

/// Мин. ширина слота ЯКОРЯ при метле в FIT-stretch (width=0): сам график ≥ 1.5× стакана, плюс ось
/// цен и собственный стакан якоря (`1.5·GLASS + PRICE_AXIS_W + GLASS`). Якорь flex (растёт), но не
/// ужимается ниже этого минимума.
pub(super) const COMPARE_ANCHOR_MIN_W: f32 =
    moon_chart::GLASS_ZONE_PX * 2.5 + moon_chart::PRICE_AXIS_W;

/// Роль слота в режиме сравнения (для размеров при метле). `Normal` — обычный размер.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CompareRole {
    Normal,
    Anchor,
    Follower,
}

/// Длительность подсветки рамки только что появившегося графика (пульс).
pub(super) const HIGHLIGHT: Duration = Duration::from_millis(2600);

/// Пауза стабильности числа открытых графиков, после которой COMPRESS схлопывает придержанные
/// пустые слоты — оставшиеся графики растягиваются на освободившееся место. Сбрасывается любым
/// появлением/исчезновением графика (см. `AddChartStack::touch_count_change`).
pub(super) const COMPACT_STABLE: Duration = Duration::from_millis(5000);

const STACK_GUTTER: f32 = 8.0;
const STACK_HEADER_H: f32 = 20.0;

type VisibleRangeHandler = Box<dyn Fn(Range<usize>, &mut Window, &mut App)>;

/// Визуальная оболочка одного chart-host в stack-режиме.
///
/// Важно: body вокруг `ChartPanel` намеренно без `.bg()`. Chart own-pass рисуется через
/// UnderScene, и любой непрозрачный quad над plot-зоной закроет график. Красим только header,
/// border и отдельный gutter вне plot-зоны.
pub(super) fn chart_stack_card(
    id: SharedString,
    label: impl Into<SharedString>,
    panel: Entity<ChartPanel>,
    p: MoonPalette,
    border: Rgba,
) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id)
        .w_full()
        .relative()
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(STACK_GUTTER))
                .bg(rgb(p.gutter)),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom(px(STACK_GUTTER))
                .overflow_hidden()
                .border_1()
                .border_color(border)
                .child(
                    h_flex()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(STACK_HEADER_H))
                        .pl(px(11.0))
                        .pr(px(8.0))
                        .items_center()
                        .overflow_hidden()
                        .bg(rgb(p.panel_head))
                        .border_b_1()
                        .border_color(border)
                        .child(
                            div()
                                .font_family(crate::design::mono())
                                .text_size(px(10.0))
                                .text_color(rgb(p.text_soft))
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .child(label),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(STACK_HEADER_H))
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .overflow_hidden()
                        .child(panel),
                ),
        )
}

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

/// Применить позиции кнопок рыночных действий (Cancel Buy / Panic Sell) ко всем панелям стека.
pub(super) fn set_panels_action_btn_pos<S: 'static>(
    entries: &[ChartStackEntry],
    cancel: crate::chart_persist::ChartBtnPos,
    panic: crate::chart_persist::ChartBtnPos,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel
            .update(cx, |p, pcx| p.set_action_btn_pos(cancel, panic, pcx));
    }
}

/// Применить положение оси цен (Left/Right/Hide) ко всем панелям стека.
pub(super) fn set_panels_price_axis_pos<S: 'static>(
    entries: &[ChartStackEntry],
    pos: crate::chart_persist::PriceAxisPos,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel.update(cx, |p, pcx| p.set_price_axis_pos(pos, pcx));
    }
}

/// Применить видимость оси времени ко всем панелям стека.
pub(super) fn set_panels_time_axis_visible<S: 'static>(
    entries: &[ChartStackEntry],
    visible: bool,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel
            .update(cx, |p, pcx| p.set_time_axis_visible(visible, pcx));
    }
}

/// Применить видимость подписей у линий ко всем панелям стека.
pub(super) fn set_panels_line_labels<S: 'static>(
    entries: &[ChartStackEntry],
    show: bool,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel.update(cx, |p, pcx| p.set_line_labels(show, pcx));
    }
}

/// Применить вкл/выкл трейдов ликвидаций ко всем панелям стека.
pub(super) fn set_panels_liquidations<S: 'static>(
    entries: &[ChartStackEntry],
    enabled: bool,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel
            .update(cx, |p, pcx| p.set_liquidations_enabled(enabled, pcx));
    }
}

/// Применить видимость подписей у перекрестия ко всем панелям стека.
pub(super) fn set_panels_cursor_labels<S: 'static>(
    entries: &[ChartStackEntry],
    show: bool,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel.update(cx, |p, pcx| p.set_cursor_labels(show, pcx));
    }
}

/// Обработать клики по замку (режим сравнения): забрать pending у всех панелей. Если кликнули —
/// переключить якорь: повторный клик по текущему якорю снимает сравнение; иначе назначить новый
/// якорь и переставить его в индекс 0 (крайний левый). Возвращает true при изменении якоря/порядка.
pub(super) fn handle_compare_lock_requests<S: 'static>(
    entries: &mut Vec<ChartStackEntry>,
    anchor: &mut Option<(CoreId, String)>,
    cx: &mut Context<S>,
) -> bool {
    let mut clicked: Option<(CoreId, String)> = None;
    for e in entries.iter() {
        if e.panel.update(cx, |p, _| p.take_compare_lock_request()) {
            clicked = Some((e.core, e.market.clone()));
        }
    }
    let Some(key) = clicked else {
        return false;
    };
    if anchor.as_ref() == Some(&key) {
        *anchor = None; // повторный клик по якорю → выключить сравнение
    } else {
        if let Some(pos) = entries
            .iter()
            .position(|e| e.core == key.0 && e.market == key.1)
        {
            let e = entries.remove(pos);
            entries.insert(0, e); // якорь — в начало ряда (налево)
        }
        *anchor = Some(key);
    }
    true
}

/// Обработать клики по метле (режим «только стакан» у соседей): забрать pending у всех панелей,
/// при клике — переключить `broom_on`. Возвращает true при изменении.
pub(super) fn handle_compare_broom_requests<S: 'static>(
    entries: &[ChartStackEntry],
    broom_on: &mut bool,
    cx: &mut Context<S>,
) -> bool {
    let mut clicked = false;
    for e in entries.iter() {
        if e.panel.update(cx, |p, _| p.take_compare_broom_request()) {
            clicked = true;
        }
    }
    if clicked {
        *broom_on = !*broom_on;
    }
    clicked
}

/// Применить состояние сравнения к панелям: `compare_eligible = horizontal` на всех; при активном
/// сравнении (horizontal И есть якорь) — пометить якорь и навязать ВСЕМ его Y-окно; иначе снять
/// lock. Ведущее окно — ВСЕГДА у якоря (он стабилен между проходами observe, поэтому синхронизация
/// сходится за пару проходов без notify-петли). Пан/зум по якорю двигает всех; драг соседа
/// возвращается к окну якоря (синхрон от ведущего — пан-везде это отдельный шаг, см. docs-internal).
///
/// ВАЖНО: НЕ берём окно соседей как «ведущее» — `set_locked_y` пишет только поле панели, в движок
/// применяется на render; в синхронном цикле observe→notify их `y_window()` ещё старое, и любая
/// детекция «кто подвигал» по нему даёт скачущее окно → бесконечный цикл (зависание).
pub(super) fn apply_compare<S: 'static>(
    entries: &[ChartStackEntry],
    anchor: &Option<(CoreId, String)>,
    shared: &mut Option<(f32, f32)>,
    horizontal: bool,
    orderbook_only: bool,
    cx: &mut Context<S>,
) {
    let key = anchor
        .as_ref()
        .filter(|k| entries.iter().any(|e| e.core == k.0 && e.market == k.1));
    let active = horizontal && key.is_some();
    if !active {
        *shared = None;
        for e in entries {
            e.panel.update(cx, |p, c| {
                p.set_compare_eligible(horizontal, c);
                p.set_compare_anchor(false, c);
                p.set_locked_y(None, c);
                p.set_orderbook_only(false, c);
                p.set_compare_broom_on(false, c);
            });
        }
        return;
    }
    let key = key.unwrap();
    // Ведущее окно — текущее окно ЯКОРЯ (стабильно в пределах цикла observe → сходимость).
    // Якорь НЕ лочим: он остаётся в своём режиме (масштаб вкладки/авто/пан), а соседи копируют
    // его живое окно. Иначе lock на якоре заморозил бы Y и масштаб/авто перестали бы работать.
    let window = entries
        .iter()
        .find(|e| e.core == key.0 && e.market == key.1)
        .and_then(|e| e.panel.read(cx).y_window());
    *shared = window;
    for e in entries {
        let is_anchor = e.core == key.0 && e.market == key.1;
        e.panel.update(cx, |p, c| {
            p.set_compare_eligible(true, c);
            p.set_compare_anchor(is_anchor, c);
            // Якорь свободен (respects scale/auto); соседи залочены на его окно.
            p.set_locked_y(if is_anchor { None } else { window }, c);
            // Метла: «только стакан» у соседей; якорь полноценный, на нём горит кнопка-метла.
            p.set_orderbook_only(!is_anchor && orderbook_only, c);
            p.set_compare_broom_on(is_anchor && orderbook_only, c);
        });
    }
}

/// Роль слота `ix` в режиме метлы: `Normal` если «только стакан» выкл; иначе `Anchor` для слота-якоря
/// (по `(core, market)`), `Follower` для остальных. Общая для Main/AddToChart стеков.
pub(super) fn compare_role(
    entries: &[ChartStackEntry],
    anchor: &Option<(CoreId, String)>,
    orderbook_only: bool,
    ix: usize,
) -> CompareRole {
    if !orderbook_only {
        return CompareRole::Normal;
    }
    match entries.get(ix) {
        Some(e) => {
            let is_anchor = anchor
                .as_ref()
                .is_some_and(|k| k.0 == e.core && k.1 == e.market);
            if is_anchor {
                CompareRole::Anchor
            } else {
                CompareRole::Follower
            }
        }
        None => CompareRole::Normal,
    }
}

/// Синхронизировать режим сравнения стека (общая для Main/AddToChart): в вертикали снять якорь;
/// забрать клики замка/метлы у панелей (сменить/снять якорь, переставить влево; переключить «только
/// стакан»); при снятом якоре выключить «только стакан»; навязать панелям общее Y-окно/флаги.
/// Возвращает true, если якорь/порядок изменились (нужен notify стека).
pub(super) fn sync_compare<S: 'static>(
    entries: &mut Vec<ChartStackEntry>,
    anchor: &mut Option<(CoreId, String)>,
    shared: &mut Option<(f32, f32)>,
    orderbook_only: &mut bool,
    orientation: Option<StackOrientation>,
    cx: &mut Context<S>,
) -> bool {
    let horizontal = orientation
        .unwrap_or(StackOrientation::Vertical)
        .is_horizontal();
    if !horizontal {
        *anchor = None;
    }
    let mut changed = handle_compare_lock_requests(entries, anchor, cx);
    changed |= handle_compare_broom_requests(entries, orderbook_only, cx);
    if anchor.is_none() {
        *orderbook_only = false;
    }
    apply_compare(entries, anchor, shared, horizontal, *orderbook_only, cx);
    changed
}

/// Применить новое значение поля-настройки стека ко всем панелям: если не изменилось — выйти; иначе
/// записать поле, навязать панелям через `apply` и `cx.notify()`. Убирает повтор «if ==new return;
/// assign; set_panels_*; notify» в сеттерах Main/AddToChart. `field` и `entries` — РАЗНЫЕ поля
/// вызывающего стека (disjoint borrow), поэтому `apply` не захватывает `self`.
pub(super) fn apply_setting<S, T, F>(
    field: &mut T,
    new: T,
    entries: &[ChartStackEntry],
    cx: &mut Context<S>,
    apply: F,
) where
    S: 'static,
    T: PartialEq,
    F: FnOnce(&[ChartStackEntry], &mut Context<S>),
{
    if *field == new {
        return;
    }
    *field = new;
    apply(entries, cx);
    cx.notify();
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
pub(super) fn render_chart_stack<S, P, T, R>(
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
    role: R,
    on_visible_range: Option<VisibleRangeHandler>,
) -> AnyElement
where
    S: Render + 'static,
    P: Fn(&S, usize) -> Option<Entity<ChartPanel>> + Clone + 'static,
    // tile(s, ix, panel, size, flex, min_w, horizontal, border, ent)
    //   flex=true:  size → max_w, min_w → min_w (якорь-stretch).
    //   flex=false: size → фикс. ширина БЕЗ сжатия (SCROLL переполняет → скролл).
    T: Fn(
            &S,
            usize,
            Entity<ChartPanel>,
            Option<f32>,
            bool,
            Option<f32>,
            bool,
            Rgba,
            Entity<S>,
        ) -> AnyElement
        + Clone
        + 'static,
    // Роль слота в режиме метлы (Anchor берёт свою ширину, Follower — стакан). Normal = обычный.
    R: Fn(&S, usize) -> CompareRole + Clone + 'static,
{
    if scroll && !compress {
        if horizontal {
            // Горизонтальный SCROLL: `MoonVirtualList` умеет только вертикаль (gpui `uniform_list`),
            // поэтому строим невиртуализированный ряд фикс-ширины в `overflow_x_scroll` (чартов
            // единицы — виртуализация не нужна). Каждая плитка: фикс. ШИРИНА cfg_h, full height.
            let mut tiles: Vec<AnyElement> = Vec::with_capacity(count);
            for ix in 0..count {
                if let Some(panel) = panel_at(s, ix) {
                    // SCROLL+метла: сосед — фикс. ширина стакана; якорь/обычный — своя ширина cfg_h.
                    let w = if role(s, ix) == CompareRole::Follower {
                        COMPARE_BOOK_W
                    } else {
                        cfg_h
                    };
                    tiles.push(tile(
                        s,
                        ix,
                        panel,
                        Some(w),
                        false,
                        None,
                        true,
                        border,
                        entity.clone(),
                    ));
                }
            }
            // overflow_x_scrollbar(): гориз. скролл + ВИДИМЫЙ скроллбар (moonui). Тайлы не сжимаются
            // (min=max) → переполняют → есть что скроллить.
            return div()
                .relative()
                .size_full()
                .child(h_flex().h_full().children(tiles))
                .overflow_x_scrollbar()
                .into_any_element();
        }
        // Вертикальный SCROLL: фикс. высота, виртуальный список со скроллбаром. Плитку строим через
        // weak-entity (фабрика `MoonVirtualList` отдаёт `App`, а не `Context`).
        let weak = entity.downgrade();
        let panel_at_v = panel_at.clone();
        let tile_v = tile.clone();
        let list = MoonVirtualList::new(
            format!("{base_id}-vlist"),
            count,
            cfg_h,
            move |ix, _window, app| {
                let Some(ent) = weak.upgrade() else {
                    return div().into_any_element();
                };
                let s = ent.read(app);
                let Some(panel) = panel_at_v(s, ix) else {
                    return div().into_any_element();
                };
                tile_v(
                    s,
                    ix,
                    panel,
                    Some(cfg_h),
                    false,
                    None,
                    false,
                    border,
                    ent.clone(),
                )
            },
        )
        .track_scroll(scroll_handle)
        .surface(false)
        .border(false)
        .radius(0.0)
        .scrollbar_visibility(MoonScrollbarVisibility::Hover);
        let list = if let Some(on_visible_range) = on_visible_range {
            list.on_visible_range(on_visible_range)
        } else {
            list
        };
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
        // Размер слота вдоль оси. В режиме метлы:
        //  • Anchor берёт СВОЮ ширину: compress → flex+max(cfg); stretch(0) → flex (растёт).
        //  • Follower: stretch(0) → flex+max(стакан) (узкий, ужимается соразмерно — поведение при 0);
        //              compress → flex без cap (делит остаток окна между стаканами).
        //  • Normal — как обычно (compress → max cfg, иначе flex).
        // (size=max_w, flex, min_w). Метла:
        //  • Follower при width=0(stretch) → flex+max(стакан): ВСЕ стаканы равномерны, ужимаются.
        //  • Follower при width>0(compress) → flex без cap: делят остаток окна между собой.
        //  • Anchor при stretch → flex+min(1.5 стакана): остаётся больше, не схлопывается.
        //  • Anchor при compress → flex+max(cfg): берёт свою (заданную) ширину.
        //  • Normal — обычный (compress → max cfg, иначе flex).
        let (size, flex, min_w) = match role(s, ix) {
            // FIT width=0 (stretch): соседи равномерны (flex+max стакан), якорь больше (flex+min).
            CompareRole::Follower if !compress => (Some(COMPARE_BOOK_W), true, None),
            CompareRole::Anchor if !compress => (None, true, Some(COMPARE_ANCHOR_MIN_W)),
            // FIT width>0 (compress): якорь — ФИКС. заданная ширина px (без сжатия), соседи делят
            // остаток окна постоянно (flex без cap).
            CompareRole::Anchor => (Some(cfg_h), false, None),
            CompareRole::Follower => (None, true, None),
            // Обычный (не метла): COMPRESS → flex+max(cfg); FIT-stretch → flex.
            CompareRole::Normal => {
                if compress {
                    (Some(cfg_h), true, None)
                } else {
                    (None, true, None)
                }
            }
        };
        match panel_at(s, ix) {
            Some(panel) => tiles.push(tile(
                s,
                ix,
                panel,
                size,
                flex,
                min_w,
                horizontal,
                border,
                entity.clone(),
            )),
            None => {
                // Пустой (держащийся) слот COMPRESS — прозрачная плашка тех же размеров (по оси).
                let mut e = div().relative().overflow_hidden();
                e = if horizontal { e.h_full() } else { e.w_full() };
                if flex {
                    e = e.flex_1();
                    let m = min_w.unwrap_or(0.0);
                    e = if horizontal {
                        e.min_w(px(m))
                    } else {
                        e.min_h(px(m))
                    };
                    if let Some(v) = size {
                        e = if horizontal {
                            e.max_w(px(v))
                        } else {
                            e.max_h(px(v))
                        };
                    }
                } else if let Some(v) = size {
                    // Фикс. БЕЗ сжатия (min=max=v) — иначе в SCROLL flex ужмёт и не будет переполнения.
                    e = if horizontal {
                        e.w(px(v)).min_w(px(v))
                    } else {
                        e.h(px(v)).min_h(px(v))
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
