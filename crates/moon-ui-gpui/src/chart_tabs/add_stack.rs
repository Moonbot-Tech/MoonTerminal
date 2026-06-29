//! AddToChart-вкладка: визуально один список графиков, архитектурно — отдельный `ChartPanel`
//! на каждый график. Вынесено из `chart_tabs` как самостоятельная вью-модель; общий рендер
//! стека — в [`super::stack`]. Используется и полоской вкладок, и выносными окнами ([`super::windows`]).

use std::time::{Duration, Instant};

use gpui::*;
use moon_ui::MoonVirtualListScrollHandle;

use super::stack::{
    COMPACT_STABLE, ChartStackEntry, HIGHLIGHT, apply_setting, chart_stack_card, compare_role,
    render_chart_stack, resolve_layout, set_panels_action_btn_pos, set_panels_auto_pin,
    set_panels_cursor_labels, set_panels_line_labels, set_panels_liquidations,
    set_panels_orderbook_enabled,
    set_panels_price_axis_pos, set_panels_scale, set_panels_show_zone, set_panels_time_axis_visible,
    sync_compare,
};
use crate::Backend;
use crate::chart_persist::{ChartBtnPos, PriceAxisPos, StackLayoutMode, StackOrientation};
use crate::panels::ChartPanel;
use moon_core::config::{ChartBucket, ChartTheme};
use moon_core::session::CoreId;

/// AddToChart-вкладка: визуально это один список графиков, но архитектурно каждый график —
/// отдельный `ChartPanel`/`gpu_canvas`/dirty entity. Не возвращаемся к старой модели
/// `ChartPanel -> Container.panes`, где mousemove одного графика перерисовывал overlay всех.
pub(crate) struct AddChartStack {
    backend: Entity<Backend>,
    num: u32,
    bucket: ChartBucket,
    epoch: f64,
    theme: ChartTheme,
    charts: Vec<ChartStackEntry>,
    scale: Option<f32>,
    /// Per-tab режим раскладки (Fit/Scroll; None = дефолт Fit).
    layout_mode: Option<StackLayoutMode>,
    /// Высота слота для Fit: 0 = растяжение, ≥20 = compress. None = дефолт.
    layout_height_fit: Option<u16>,
    /// Высота слота для Scroll. None = дефолт.
    layout_height_scroll: Option<u16>,
    /// Показывать ли стакан на графиках вкладки (per-окно). None = дефолт (вкл).
    orderbook_enabled: Option<bool>,
    /// Рисовать ли трейды ликвидаций (per-окно). None = дефолт (вкл).
    liquidations_enabled: Option<bool>,
    /// Показывать ли заливку зоны управления (per-окно). None = дефолт (вкл).
    show_zone: Option<bool>,
    /// Авто-пин графика при выставлении ордера (per-окно). None = дефолт (выкл).
    auto_pin: Option<bool>,
    /// Ориентация стека (per-окно). None = дефолт (Vertical).
    layout_orientation: Option<StackOrientation>,
    /// Позиции кнопок Cancel Buy / Panic Sell в зоне чарта (per-окно). None = дефолт (Right).
    cancel_buy_pos: Option<ChartBtnPos>,
    panic_sell_pos: Option<ChartBtnPos>,
    /// Положение оси цен (Left/Right/Hide) для графиков стека (per-окно). None = дефолт (Left).
    price_axis_pos: Option<PriceAxisPos>,
    /// Видимость оси времени для графиков стека (per-окно). None = дефолт (вкл).
    time_axis_visible: Option<bool>,
    /// Видимость подписей у линий для графиков стека (per-окно). None = дефолт (вкл).
    line_labels: Option<bool>,
    /// Видимость подписей у перекрестия для графиков стека (per-окно). None = дефолт (вкл).
    cursor_labels: Option<bool>,
    /// Подписки на стаканы временно приостановлены (вкладка не в фокусе > 5с). Эффективный
    /// стакан = `orderbook_enabled ∧ !suspended` — не затирает пользовательскую галку «Стакан».
    /// Откреплённые в окно вкладки никогда не suspend (окно само держит спрос).
    orderbook_suspended: bool,
    /// Якорь режима сравнения `(core, market)` — ведущий по цене чарт (замок горит, стоит слева).
    /// None = сравнение выключено. Активно только в горизонтальной ориентации.
    compare_anchor: Option<(CoreId, String)>,
    /// Общее Y-окно сравнения `(center, range)` — следует за последней изменённой панелью.
    compare_y: Option<(f32, f32)>,
    /// Режим метлы: соседи якоря показывают «только стакан» (чарт+ось цен скрыты).
    compare_orderbook_only: bool,
    /// Держать ли пустой слот при выбытии графика (COMPRESS-реюз для авто-AddToChart: место
    /// сохраняется под следующий детект). У КАСТОМНЫХ вкладок = false: закрыл график → слот
    /// удаляется сразу, соседи перераспределяются по раскладке.
    hold_vacated: bool,
    /// Момент последнего изменения числа открытых графиков (появился/исчез/реюз слота). Debounce
    /// для COMPRESS-компакции: пустые `vacated`-слоты схлопываются, только когда с этого момента
    /// прошло `COMPACT_STABLE` (число графиков стабильно). См. `compact_vacated_if_stable`.
    last_count_change: Instant,
    /// Армирован ли ~1Гц таймер debounce-компакции COMPRESS (self-rearming, как idle-таймер Main).
    compact_timer_armed: bool,
    /// Скролл-хэндл вертикального MoonVirtualList (scroll-режим стека).
    scroll: MoonVirtualListScrollHandle,
}

impl AddChartStack {
    pub(super) fn new(
        backend: Entity<Backend>,
        num: u32,
        bucket: ChartBucket,
        epoch: f64,
        theme: ChartTheme,
    ) -> Self {
        Self {
            backend,
            num,
            bucket,
            epoch,
            theme,
            charts: Vec::new(),
            scale: None,
            layout_mode: None,
            layout_height_fit: None,
            layout_height_scroll: None,
            orderbook_enabled: None,
            liquidations_enabled: None,
            show_zone: None,
            auto_pin: None,
            layout_orientation: None,
            cancel_buy_pos: None,
            panic_sell_pos: None,
            price_axis_pos: None,
            time_axis_visible: None,
            line_labels: None,
            cursor_labels: None,
            orderbook_suspended: false,
            compare_anchor: None,
            compare_y: None,
            compare_orderbook_only: false,
            hold_vacated: true,
            last_count_change: Instant::now(),
            compact_timer_armed: false,
            scroll: MoonVirtualListScrollHandle::new(),
        }
    }

    /// Отметить изменение числа открытых графиков (появился/исчез/реюз слота) → перезапустить
    /// debounce-таймер COMPRESS-компакции (5с стабильности до схлопывания пустых слотов).
    fn touch_count_change(&mut self) {
        self.last_count_change = Instant::now();
    }

    /// Армировать (если ещё нет) ~1Гц таймер debounce-компакции COMPRESS. Тикает, пока есть
    /// графики; сам пере-армится в колбэке. Зовётся из render и `add_coin` (идемпотентно).
    /// Образец — `MainChartStack::arm_idle_timer`.
    fn arm_compact_timer(&mut self, cx: &mut Context<Self>) {
        if self.compact_timer_armed || self.charts.is_empty() {
            return;
        }
        self.compact_timer_armed = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(Duration::from_secs(1)).await;
            let _ = cx.update(|cx| {
                this.update(cx, |this, cx| {
                    this.compact_timer_armed = false;
                    this.compact_vacated_if_stable(cx);
                    this.arm_compact_timer(cx);
                })
                .is_ok()
            });
        })
        .detach();
    }

    /// COMPRESS: если число открытых графиков стабильно `COMPACT_STABLE` (никто не появился и не
    /// исчез) и есть придержанные пустые слоты — убрать их, чтобы оставшиеся графики растянулись
    /// на освободившееся место. В других режимах / на кастомных вкладках — no-op.
    fn compact_vacated_if_stable(&mut self, cx: &mut Context<Self>) {
        let (_, compress, _) = resolve_layout(
            self.layout_mode,
            self.layout_height_fit,
            self.layout_height_scroll,
        );
        if !compress
            || !self.hold_vacated
            || self.last_count_change.elapsed() < COMPACT_STABLE
        {
            return;
        }
        let before = self.charts.len();
        self.charts.retain(|e| !e.vacated);
        if self.charts.len() != before {
            self.sync_compare(cx);
            cx.notify();
        }
    }

    /// Кастомная вкладка: НЕ держать пустые слоты (закрыл график → перераспределить остальные).
    pub(crate) fn set_hold_vacated(&mut self, hold: bool) {
        self.hold_vacated = hold;
    }

    pub(crate) fn compare_anchor(&self) -> Option<(CoreId, String)> {
        self.compare_anchor.clone()
    }

    pub(crate) fn compare_orderbook_only(&self) -> bool {
        self.compare_orderbook_only
    }

    /// Восстановить состояние сравнения из charts.json (якорь + режим метлы) и применить.
    pub(crate) fn restore_compare(
        &mut self,
        anchor: Option<(CoreId, String)>,
        orderbook_only: bool,
        cx: &mut Context<Self>,
    ) {
        self.compare_anchor = anchor;
        self.compare_orderbook_only = orderbook_only;
        self.sync_compare(cx);
    }

    /// Синхронизировать режим сравнения: забрать клики замка/метлы (сменить/снять якорь, переставить
    /// влево; переключить «только стакан»), затем навязать общее Y-окно/флаги панелям. В вертикали
    /// сравнение выключено.
    fn sync_compare(&mut self, cx: &mut Context<Self>) {
        sync_compare(
            &mut self.charts,
            &mut self.compare_anchor,
            &mut self.compare_y,
            &mut self.compare_orderbook_only,
            self.layout_orientation,
            cx,
        );
    }

    pub(super) fn add_coin(
        &mut self,
        core: CoreId,
        market: &str,
        ttl_ms: f64,
        cx: &mut Context<Self>,
    ) {
        let (_, compress, _) = resolve_layout(
            self.layout_mode,
            self.layout_height_fit,
            self.layout_height_scroll,
        );

        // Уже есть такой график → продлить TTL.
        if let Some(i) = self
            .charts
            .iter()
            .position(|e| e.core == core && e.market == market)
        {
            if self.charts[i].vacated {
                self.charts[i].vacated = false;
                self.charts[i].arrived_at = Instant::now();
                self.touch_count_change(); // график снова открыт → сброс debounce
            }
            let panel = self.charts[i].panel.clone();
            panel.update(cx, |panel, pcx| panel.add_coin(core, market, ttl_ms, pcx));
            cx.notify();
            return;
        }

        // COMPRESS (только авто-AddToChart): новый занимает ПЕРВЫЙ пустой держащийся слот (без
        // сдвига/смены размера соседей). Кастомные держат hold_vacated=false → этот путь не нужен.
        if compress && self.hold_vacated {
            if let Some(i) = self.charts.iter().position(|e| e.vacated) {
                self.charts[i].core = core;
                self.charts[i].market = market.to_string();
                self.charts[i].arrived_at = Instant::now();
                self.charts[i].vacated = false;
                self.touch_count_change(); // новый график занял пустой слот → сброс debounce
                let panel = self.charts[i].panel.clone();
                panel.update(cx, |panel, pcx| panel.add_coin(core, market, ttl_ms, pcx));
                cx.notify();
                return;
            }
        }

        // Новый график — в конец (в FIT-stretch запиненные всплывут при сортировке в render).
        let backend = self.backend.clone();
        let num = self.num;
        let bucket = self.bucket.clone();
        let epoch = self.epoch;
        let theme = self.theme.clone();
        let scale = self.scale;
        let panel = cx.new(|cx| ChartPanel::new_addto(backend, num, bucket, epoch, theme, cx));
        // Любое изменение панели (вкл. переключение пина ●/○) → перерисовать стек: prune пустых +
        // пере-сортировка запиненных наверх происходит в render.
        cx.observe(&panel, |this, _, cx| {
            this.prune_or_hold(cx);
            this.sync_compare(cx);
            cx.notify();
        })
        .detach();
        if scale.is_some() {
            panel.update(cx, |panel, pcx| panel.set_scale(scale, pcx));
        }
        // Эффективный стакан (учитывая suspend-гейт): новый чарт не должен подписываться, если
        // вкладка сейчас приостановлена (или галка «Стакан» снята).
        panel.update(cx, |panel, pcx| {
            panel.set_orderbook_enabled(self.effective_orderbook(), pcx)
        });
        if let Some(sz) = self.show_zone {
            panel.update(cx, |panel, pcx| panel.set_show_zone(sz, pcx));
        }
        if let Some(ap) = self.auto_pin {
            panel.update(cx, |panel, pcx| panel.set_auto_pin(ap, pcx));
        }
        panel.update(cx, |panel, pcx| {
            panel.set_action_btn_pos(
                self.cancel_buy_pos.unwrap_or_default(),
                self.panic_sell_pos.unwrap_or_default(),
                pcx,
            )
        });
        panel.update(cx, |panel, pcx| {
            panel.set_price_axis_pos(self.price_axis_pos.unwrap_or_default(), pcx)
        });
        panel.update(cx, |panel, pcx| {
            panel.set_time_axis_visible(self.time_axis_visible.unwrap_or(true), pcx)
        });
        panel.update(cx, |panel, pcx| {
            panel.set_line_labels(self.line_labels.unwrap_or(true), pcx)
        });
        panel.update(cx, |panel, pcx| {
            panel.set_cursor_labels(self.cursor_labels.unwrap_or(true), pcx)
        });
        panel.update(cx, |panel, pcx| {
            panel.set_liquidations_enabled(self.liquidations_enabled.unwrap_or(true), pcx)
        });
        panel.update(cx, |panel, pcx| panel.add_coin(core, market, ttl_ms, pcx));
        self.charts
            .push(ChartStackEntry::new(core, market.to_string(), panel));
        self.touch_count_change(); // появился новый график → сброс debounce
        self.arm_compact_timer(cx); // запустить таймер компакции (если ещё не армирован)
        // Новый тикер: в режиме сравнения сразу получает eligible + общее Y-окно якоря.
        self.sync_compare(cx);
        cx.notify();
    }

    /// Реакция на выбытие графиков (TTL истёк → пустая панель).
    /// - **FIT-stretch / Scroll**: удаляем пустые сразу (стабильность даёт пин — сортировка в render).
    /// - **COMPRESS (Fit+пиксели)**: слот НЕ удаляем — помечаем `vacated` (держит позицию и размер
    ///   соседей). Сброс ВСЕХ слотов — только когда пустыми стали все (→ вернётся дефолтная высота).
    fn prune_or_hold(&mut self, cx: &App) -> bool {
        let (_, compress, _) = resolve_layout(
            self.layout_mode,
            self.layout_height_fit,
            self.layout_height_scroll,
        );
        // FIT-stretch / Scroll, ИЛИ кастомная вкладка (hold_vacated=false): пустые удаляем сразу
        // → соседи перераспределяются. Держим слот только в COMPRESS у авто-AddToChart.
        if !compress || !self.hold_vacated {
            let before = self.charts.len();
            self.charts.retain(|e| e.panel.read(cx).pane_count() > 0);
            return self.charts.len() != before;
        }
        let mut changed = false;
        for e in self.charts.iter_mut() {
            let empty = e.panel.read(cx).pane_count() == 0;
            if empty != e.vacated {
                e.vacated = empty;
                changed = true;
            }
        }
        if !self.charts.is_empty() && self.charts.iter().all(|e| e.vacated) {
            self.charts.clear();
            changed = true;
        }
        if changed {
            // График исчез (слот стал пустым) — число открытых изменилось → сброс debounce:
            // придержанные пустые слоты схлопнутся только после 5с стабильности.
            self.touch_count_change();
        }
        changed
    }

    pub(crate) fn pane_count(&self, cx: &App) -> usize {
        self.charts
            .iter()
            .filter(|entry| entry.panel.read(cx).pane_count() > 0)
            .count()
    }

    pub(crate) fn scale(&self) -> Option<f32> {
        self.scale
    }

    pub(crate) fn set_scale(&mut self, pct: Option<f32>, cx: &mut Context<Self>) {
        apply_setting(&mut self.scale, pct, &self.charts, cx, |c, cx| {
            set_panels_scale(c, pct, cx)
        });
    }

    pub(crate) fn orderbook_enabled(&self) -> Option<bool> {
        self.orderbook_enabled
    }

    /// Эффективный стакан = пользовательская галка (None→вкл) И не приостановлен по фокусу.
    fn effective_orderbook(&self) -> bool {
        self.orderbook_enabled.unwrap_or(true) && !self.orderbook_suspended
    }

    /// Вкл/выкл стакан для всех графиков стека (per-окно). Применяется с учётом suspend-гейта.
    pub(crate) fn set_orderbook_enabled(&mut self, enabled: Option<bool>, cx: &mut Context<Self>) {
        if self.orderbook_enabled == enabled {
            return;
        }
        self.orderbook_enabled = enabled;
        set_panels_orderbook_enabled(&self.charts, self.effective_orderbook(), cx);
        cx.notify();
    }

    /// Приостановить/возобновить подписки на стаканы по фокусу вкладки (гейтинг кастомных
    /// вкладок: ушли > 5с → suspend=true → отписка; вернулись → resume). Не трогает галку «Стакан».
    pub(crate) fn set_orderbook_suspended(&mut self, suspended: bool, cx: &mut Context<Self>) {
        if self.orderbook_suspended == suspended {
            return;
        }
        self.orderbook_suspended = suspended;
        set_panels_orderbook_enabled(&self.charts, self.effective_orderbook(), cx);
        self.backend.update(cx, |b, _| b.rebuild_orderbook_wanted());
        cx.notify();
    }

    pub(crate) fn show_zone(&self) -> Option<bool> {
        self.show_zone
    }

    /// Вкл/выкл заливку зоны управления для всех графиков стека (per-окно).
    pub(crate) fn set_show_zone(&mut self, show: Option<bool>, cx: &mut Context<Self>) {
        apply_setting(&mut self.show_zone, show, &self.charts, cx, |c, cx| {
            set_panels_show_zone(c, show.unwrap_or(true), cx)
        });
    }

    pub(crate) fn auto_pin(&self) -> Option<bool> {
        self.auto_pin
    }

    pub(crate) fn action_btn_pos(&self) -> (Option<ChartBtnPos>, Option<ChartBtnPos>) {
        (self.cancel_buy_pos, self.panic_sell_pos)
    }

    /// Позиции кнопок Cancel Buy / Panic Sell для всех графиков стека (per-окно).
    pub(crate) fn set_action_btn_pos(
        &mut self,
        cancel: Option<ChartBtnPos>,
        panic: Option<ChartBtnPos>,
        cx: &mut Context<Self>,
    ) {
        if self.cancel_buy_pos == cancel && self.panic_sell_pos == panic {
            return;
        }
        self.cancel_buy_pos = cancel;
        self.panic_sell_pos = panic;
        set_panels_action_btn_pos(
            &self.charts,
            cancel.unwrap_or_default(),
            panic.unwrap_or_default(),
            cx,
        );
        cx.notify();
    }

    pub(crate) fn price_axis_pos(&self) -> Option<PriceAxisPos> {
        self.price_axis_pos
    }

    /// Положение оси цен (Left/Right/Hide) для всех графиков стека (per-окно).
    pub(crate) fn set_price_axis_pos(&mut self, pos: Option<PriceAxisPos>, cx: &mut Context<Self>) {
        apply_setting(&mut self.price_axis_pos, pos, &self.charts, cx, |c, cx| {
            set_panels_price_axis_pos(c, pos.unwrap_or_default(), cx)
        });
    }

    pub(crate) fn time_axis_visible(&self) -> Option<bool> {
        self.time_axis_visible
    }

    /// Видимость оси времени для всех графиков стека (per-окно).
    pub(crate) fn set_time_axis_visible(&mut self, visible: Option<bool>, cx: &mut Context<Self>) {
        apply_setting(&mut self.time_axis_visible, visible, &self.charts, cx, |c, cx| {
            set_panels_time_axis_visible(c, visible.unwrap_or(true), cx)
        });
    }

    pub(crate) fn line_labels(&self) -> Option<bool> {
        self.line_labels
    }

    /// Видимость подписей у линий для всех графиков стека (per-окно).
    pub(crate) fn set_line_labels(&mut self, show: Option<bool>, cx: &mut Context<Self>) {
        apply_setting(&mut self.line_labels, show, &self.charts, cx, |c, cx| {
            set_panels_line_labels(c, show.unwrap_or(true), cx)
        });
    }

    pub(crate) fn cursor_labels(&self) -> Option<bool> {
        self.cursor_labels
    }

    /// Видимость подписей у перекрестия для всех графиков стека (per-окно).
    pub(crate) fn set_cursor_labels(&mut self, show: Option<bool>, cx: &mut Context<Self>) {
        apply_setting(&mut self.cursor_labels, show, &self.charts, cx, |c, cx| {
            set_panels_cursor_labels(c, show.unwrap_or(true), cx)
        });
    }

    pub(crate) fn liquidations_enabled(&self) -> Option<bool> {
        self.liquidations_enabled
    }

    /// Вкл/выкл трейды ликвидаций для всех графиков стека (per-окно).
    pub(crate) fn set_liquidations_enabled(&mut self, enabled: Option<bool>, cx: &mut Context<Self>) {
        apply_setting(
            &mut self.liquidations_enabled,
            enabled,
            &self.charts,
            cx,
            |c, cx| set_panels_liquidations(c, enabled.unwrap_or(true), cx),
        );
    }

    /// Вкл/выкл авто-пин при ордере для всех графиков стека (per-окно).
    pub(crate) fn set_auto_pin(&mut self, on: Option<bool>, cx: &mut Context<Self>) {
        apply_setting(&mut self.auto_pin, on, &self.charts, cx, |c, cx| {
            set_panels_auto_pin(c, on.unwrap_or(false), cx)
        });
    }

    pub(crate) fn layout_orientation(&self) -> Option<StackOrientation> {
        self.layout_orientation
    }

    /// Сменить ориентацию стека (per-окно). Перестраивает текущее отображение.
    pub(crate) fn set_orientation(
        &mut self,
        orientation: Option<StackOrientation>,
        cx: &mut Context<Self>,
    ) {
        if self.layout_orientation == orientation {
            return;
        }
        self.layout_orientation = orientation;
        // Ориентация влияет на доступность сравнения (вертикаль выключает lock).
        self.sync_compare(cx);
        cx.notify();
    }

    pub(crate) fn layout_mode(&self) -> Option<StackLayoutMode> {
        self.layout_mode
    }

    pub(crate) fn layout_height_fit(&self) -> Option<u16> {
        self.layout_height_fit
    }

    pub(crate) fn layout_height_scroll(&self) -> Option<u16> {
        self.layout_height_scroll
    }

    /// Применить per-tab раскладку (режим + раздельные высоты Fit/Scroll) к этому стеку.
    pub(crate) fn set_layout(
        &mut self,
        mode: Option<StackLayoutMode>,
        height_fit: Option<u16>,
        height_scroll: Option<u16>,
        cx: &mut Context<Self>,
    ) {
        if self.layout_mode == mode
            && self.layout_height_fit == height_fit
            && self.layout_height_scroll == height_scroll
        {
            return;
        }
        self.layout_mode = mode;
        self.layout_height_fit = height_fit;
        self.layout_height_scroll = height_scroll;
        // Слоты держатся только в COMPRESS. При переключении в другой режим пустые слоты убираем,
        // чтобы FIT-stretch/Scroll не показывали пустые плашки.
        let (_, compress, _) = resolve_layout(mode, height_fit, height_scroll);
        if !compress {
            self.charts.retain(|e| !e.vacated);
        }
        cx.notify();
    }

    /// Текущий список тикеров стека `(core, market)` — для персиста кастомной вкладки.
    /// Пустые/освобождённые слоты пропускаем.
    pub(crate) fn coins(&self, cx: &App) -> Vec<(CoreId, String)> {
        self.charts
            .iter()
            .filter(|e| !e.vacated && e.panel.read(cx).pane_count() > 0)
            .map(|e| (e.core, e.market.clone()))
            .collect()
    }

    /// Закрепить (pin) все графики стека — для кастомной вкладки (чарты сразу запинены).
    pub(crate) fn pin_all(&mut self, cx: &mut Context<Self>) {
        for e in &self.charts {
            e.panel.update(cx, |p, pcx| p.ensure_pinned(pcx));
        }
    }

    pub(super) fn set_scene_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        for entry in &self.charts {
            entry
                .panel
                .update(cx, |panel, _| panel.set_scene_visible(visible));
        }
    }

    pub(crate) fn close_all_panes(&mut self, cx: &mut Context<Self>) {
        for entry in &self.charts {
            entry
                .panel
                .update(cx, |panel, pcx| panel.close_all_panes(pcx));
        }
        self.charts.clear();
        cx.notify();
    }
}

impl Render for AddChartStack {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Запустить (если надо) debounce-таймер COMPRESS-компакции — идемпотентно, дёшево.
        self.arm_compact_timer(cx);
        let palette = moon_ui::MoonPalette::active(cx);
        if self.charts.is_empty() {
            // Непрозрачный фон: в выносном окне Root=NoFill и own-pass нет → без фона
            // сквозь логотип просвечивает белая подложка окна.
            return div()
                .size_full()
                .bg(rgb(palette.chart_bg))
                .flex()
                .items_center()
                .justify_center()
                .child(crate::design::logo_glow_sized(cx, 220.0))
                .into_any_element();
        }

        // Stack: per-tab раскладка (FIT/SCROLL/COMPRESS + высота), иначе глобальный дефолт.
        // ВАЖНО: чарт-слоты ПРОЗРАЧНЫЕ. own-pass (combo/стакан) — слой GpuCanvasLayer::UnderScene
        // (под сценой); любой непрозрачный `.bg()` над слотом его перекрывает. Разделитель — рамка.
        let (scroll, compress, cfg_h) = resolve_layout(
            self.layout_mode,
            self.layout_height_fit,
            self.layout_height_scroll,
        );
        // Запиненные наверх кластером — ТОЛЬКО НЕ в COMPRESS (там слоты позиционно стабильны,
        // сортировка их бы двигала). В FIT-stretch/Scroll пин поднимает график к запиненным.
        if !compress {
            self.charts.sort_by_key(|e| !e.panel.read(cx).is_pinned());
        }
        let count = self.charts.len();
        let border = rgb(palette.border);
        let accent = rgb(palette.accent);
        let base_id = format!("add-chart-stack-{}", self.num);
        let horizontal = self
            .layout_orientation
            .unwrap_or(StackOrientation::Vertical)
            .is_horizontal();
        let entity = cx.entity();
        let p = palette;
        render_chart_stack(
            &base_id,
            self,
            entity,
            count,
            scroll,
            compress,
            horizontal,
            cfg_h,
            &self.scroll,
            border,
            // Пустой (держащийся) COMPRESS-слот → None: render покажет прозрачную плашку.
            |s, ix| {
                s.charts
                    .get(ix)
                    .filter(|e| !e.vacated)
                    .map(|e| e.panel.clone())
            },
            move |s, ix, panel, size, flex, min_w, horizontal, border, _ent| {
                let (id, label, fresh) = match s.charts.get(ix) {
                    Some(e) => (
                        format!("add-chart-stack-tile-{}-{}-{}", s.num, e.core, e.market),
                        e.market.clone(),
                        e.arrived_at.elapsed() < HIGHLIGHT,
                    ),
                    None => (
                        format!("add-chart-stack-tile-{}-{ix}", s.num),
                        "Chart".to_string(),
                        false,
                    ),
                };
                let mut tile =
                    chart_stack_card(SharedString::from(id.clone()), label, panel, p, border);
                // Поперёк оси — на всю ширину/высоту; вдоль оси — flex+cap (COMPRESS до size, сжатие),
                // фикс (size без flex) или растяжение (FIT). Гор: ось = X (ширина), верт: ось = Y.
                tile = if horizontal {
                    tile.h_full()
                } else {
                    tile.w_full()
                };
                if flex {
                    tile = tile.flex_1();
                    let m = min_w.unwrap_or(0.0);
                    tile = if horizontal {
                        tile.min_w(px(m))
                    } else {
                        tile.min_h(px(m))
                    };
                    if let Some(v) = size {
                        tile = if horizontal {
                            tile.max_w(px(v))
                        } else {
                            tile.max_h(px(v))
                        };
                    }
                } else if let Some(v) = size {
                    // Фикс. БЕЗ сжатия (min=max=v): в SCROLL тайлы переполняют контейнер → скролл.
                    tile = if horizontal {
                        tile.w(px(v)).min_w(px(v))
                    } else {
                        tile.h(px(v)).min_h(px(v))
                    };
                }
                // Подсветка только что появившегося графика: яркая акцентная рамка поверх, пульс
                // (3 мигания за HIGHLIGHT). Сдвинута внутрь на 1px, чтобы overflow_hidden её не
                // срезал; opacity не падает в 0 на пике, чтобы было хорошо видно. gpui гонит кадры.
                let highlight = fresh.then(|| {
                    div()
                        .absolute()
                        .top(px(1.0))
                        .left(px(1.0))
                        .right(px(1.0))
                        .bottom(px(9.0))
                        .border_2()
                        .border_color(accent)
                        .rounded(px(2.0))
                        .with_animation(
                            SharedString::from(format!("{id}-arrive")),
                            Animation::new(HIGHLIGHT),
                            |el, delta| {
                                // 3 чётких мигания: 0 → 1 → 0, повторённые.
                                let pulse = (delta * std::f32::consts::PI * 3.0).sin().abs();
                                el.opacity(pulse)
                            },
                        )
                });
                tile.children(highlight).into_any_element()
            },
            |s, ix| compare_role(&s.charts, &s.compare_anchor, s.compare_orderbook_only, ix),
        )
    }
}
