//! Свой таб-стрип чартов (порт egui-полоски чарт-вкладок): Main + AddToChart-N.
//! Полный контроль: активная вкладка, БЕЗ авто-
//! перехода при детекте, дабл-клик по чарту→Main, отцепление вкладки в ОС-окно.
//! Является Dock-панелью (center DockArea), внутри — своя полоска + активная
//! `ChartPanel`. Детекты/ордер/нижние вкладки — отдельные MoonPalette Dock-панели.
//!
//! Подсистема выносных ОС-окон откреп-вкладок (detach/restore/repin/персист + хост
//! `DetachedChartHost`) — в [`windows`].

mod add_stack;
mod coin_search;
mod custom;
mod detached_host;
mod ingest;
mod layout_popup;
mod main_stack;
mod settings;
mod sig;
mod stack;
mod strip;
mod windows;

use std::collections::HashMap;

pub(crate) use add_stack::AddChartStack;
pub(crate) use main_stack::MainChartStack;
use sig::{chart_tabs_sig, core_belongs_to_group};

use crate::chart_persist::StackLayoutMode;

use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonInputEvent, MoonInputState, Panel, PanelEvent, PanelState,
};
use rust_i18n::t;

use crate::Backend;
use crate::chart_persist;
use moon_core::config::{ChartBucket, ChartTheme};
use moon_core::session::CoreId;

/// Высота полоски чарт-вкладок (px). Должна РАВНЯТЬСЯ высоте таба `MoonTabStrip`
/// (`tab.rs`: `fit_height(28, 13, 7.5)`), иначе при изменении масштаба интерфейса/шрифта
/// (`scale.ui`/`font_delta`) полоса и линия под ней рассинхронятся с самими табами
/// (таб масштабируется через `fit_height`, а не через чистый `ui()`). При дефолте
/// (ui=1, font_delta=2) даёт 30 — прежнее значение константы.
pub(super) fn chart_tab_strip_h(cx: &App) -> f32 {
    crate::design::fit_h_value(cx, 28.0, 13.0, 7.5)
}

/// Идентичность вкладки чарта. Main — фуллскрин; Add(номер, bucket) — AddToChart-вкладка,
/// где `bucket` — куда сведены графики ядра внутри группы (своё ядро / общая / именованная
/// связка; см. `ChartBucket`). Порт egui `ContainerKind` (Main / Chart{num, bucket}).
#[derive(Clone, PartialEq, Eq)]
enum Tab {
    Main,
    Add(u32, ChartBucket),
    /// Сессионная вкладка из мульти-выбора монет (кнопка «Открыть в новой вкладке»). Та же
    /// форма, что `Add` (номер+bucket) → большинство веток сворачиваются `Add | Custom`. Номера
    /// идут с `CUSTOM_NUM_BASE` (не пересекаются с детект-номерами Add); НЕ персистится и НЕ
    /// наполняется ингестом (детекты в неё не текут — bucket синтетический `Shared`).
    Custom(u32, ChartBucket),
}

/// База номеров кастомных (session-only) вкладок — заведомо выше детект-номеров AddToChart,
/// чтобы `(num, bucket)` кастома не совпал с обычной Add-вкладкой.
const CUSTOM_NUM_BASE: u32 = 100_000;

pub struct ChartTabs {
    backend: Entity<Backend>,
    group: String,
    epoch: f64,
    theme: ChartTheme,
    /// Main-чарты: несколько рынков как stack отдельных `ChartPanel`, активный — fullscreen.
    main: Entity<MainChartStack>,
    /// AddToChart-вкладки (номер, bucket, стек графиков), отсортированы по (номер, bucket).
    add: Vec<(u32, ChartBucket, Entity<AddChartStack>)>,
    /// Сессионные кастомные вкладки из мульти-выбора монет (та же тройка). Не персистятся, не
    /// наполняются ингестом. Лейблы — отдельно в `custom_labels`.
    custom: Vec<(u32, ChartBucket, Entity<AddChartStack>)>,
    /// Лейблы кастомных вкладок по номеру (показ в стрипе).
    custom_labels: HashMap<u32, String>,
    /// Следующий номер кастомной вкладки.
    next_custom_num: u32,
    /// Отмеченные чекбоксами монеты в выпадашке поиска (для «Открыть в новой вкладке»).
    coin_selected: std::collections::HashSet<(CoreId, String)>,
    /// Поколение «гейта стаканов» по номеру кастомной вкладки — отменяет устаревшие 5с-таймеры
    /// suspend (ушли→вернулись→снова ушли: считается только последний таймер).
    custom_gate_gen: HashMap<u32, u64>,
    /// Откреплённые в своё ОС-окно вкладки — держим Entity, чтобы при закрытии окна
    /// вернуть панель в стрип (repin) и чтобы новые детекты этого номера шли в неё.
    detached: Vec<(u32, ChartBucket, Entity<AddChartStack>)>,
    /// Активная вкладка.
    active: Tab,
    /// Сколько монет на вкладке (num, bucket) пользователь уже «видел» (был на ней активен).
    /// Бейдж = pane_count - seen (новые с момента ухода). На активной вкладке seen догоняет
    /// pane_count → бейджа нет. Уходишь → seen заморожен → новые детекты растят бейдж.
    seen: HashMap<(u32, ChartBucket), usize>,
    /// Per-core курсор учтённых AddToChart-детектов.
    add_seq: HashMap<CoreId, u64>,
    /// Сигнатура входов, которые реально меняют tab-strip: AddToChart-детекты,
    /// split-настройка и явный запрос открыть монету на Main.
    last_sig: u64,
    /// Последняя виденная `price_scale_rev` тулбара — применяем масштаб к АКТИВНОЙ панели
    /// только когда rev вырос (юзер выбрал), иначе синхроним показ масштаба активной вкладки.
    last_scale_rev: u64,
    /// Откреп-вкладки на восстановление при загрузке (из charts.json): создаём их пустыми и
    /// открываем окна на ПЕРВОМ render (не в конструкторе окна группы — нельзя вложенно).
    restore_pending: Vec<(u32, ChartBucket, chart_persist::WinGeom, Option<f32>)>,
    /// Handle окна группы. Backend-observe callbacks не получают `&mut Window`, но open/activate
    /// и restore detached окон должны жить вне `render()`.
    window_handle: AnyWindowHandle,
    focus: FocusHandle,
    /// In-scene попап настроек раскладки активной вкладки (кнопка ⚙). Popup должен жить
    /// в обычной GPUI scene: chart text находится ниже scene, поэтому отдельное ОС-окно не нужно.
    layout_popup_open: bool,
    /// Был ли курсор внутри popup-а. Уход после первого входа закрывает popup и коммитит ввод.
    layout_popup_hovered: bool,
    /// Поле высоты режима Fit.
    layout_fit_input: Entity<MoonInputState>,
    /// Поле высоты режима Scroll.
    layout_scroll_input: Entity<MoonInputState>,
    /// Поле имени кастомной вкладки (в попапе ⚙, только для Custom).
    custom_name_input: Entity<MoonInputState>,
    /// Поле ввода монеты (поиск) полоски вкладок — своё на окно; набор монет зависит от ядер
    /// АКТИВНОЙ вкладки (см. [`coin_search`]).
    coin_input: Entity<MoonInputState>,
    /// Текущий текст в поле монеты (зеркало `coin_input`, обновляется по `Change`).
    coin_query: String,
    /// Открыт ли выпадающий список совпадений монеты.
    coin_popup_open: bool,
}

impl ChartTabs {
    pub fn new(
        backend: Entity<Backend>,
        group: String,
        focus_open: Option<(CoreId, String)>,
        epoch: f64,
        theme: ChartTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let main = cx.new(|cx| {
            MainChartStack::new(
                backend.clone(),
                group.clone(),
                focus_open,
                epoch,
                theme.clone(),
                cx,
            )
        });
        let initial_sig = chart_tabs_sig(backend.read(cx), &group);
        #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
        {
            if let Some(main_handle) = main.read(cx).debug_data_handle(cx) {
                backend.update(cx, |b, _| {
                    b.register_debug_main_chart(group.clone(), main_handle);
                });
            }
        }
        // Из charts.json: масштаб Main (num=0) и список откреп-вкладок этой группы на
        // восстановление (создадим пустыми на первом render → ждут детект).
        #[allow(clippy::type_complexity)]
        let (
            main_scale,
            main_layout,
            main_orderbook,
            main_liquidations,
            main_show_zone,
            main_auto_pin,
            main_action_pos,
            main_axis_pos,
            main_time_axis,
            main_line_labels,
            main_cursor_labels,
            restore_pending,
        ): (
            Option<f32>,
            (Option<StackLayoutMode>, Option<u16>, Option<u16>),
            Option<bool>,
            Option<bool>,
            Option<bool>,
            Option<bool>,
            (
                Option<chart_persist::ChartBtnPos>,
                Option<chart_persist::ChartBtnPos>,
            ),
            Option<chart_persist::PriceAxisPos>,
            Option<bool>,
            Option<bool>,
            Option<bool>,
            Vec<_>,
        ) = {
            let specs = &backend.read(cx).chart_specs;
            let main_spec = specs.iter().find(|s| s.group == group && s.num == 0);
            let main_scale = main_spec.and_then(|s| s.scale);
            let main_layout = main_spec.map_or((None, None, None), |s| {
                (s.layout_mode, s.layout_height_fit, s.layout_height_scroll)
            });
            let main_orderbook = main_spec.and_then(|s| s.orderbook_enabled);
            let main_liquidations = main_spec.and_then(|s| s.liquidations_enabled);
            let main_show_zone = main_spec.and_then(|s| s.show_zone);
            let main_auto_pin = main_spec.and_then(|s| s.auto_pin);
            let main_action_pos =
                main_spec.map_or((None, None), |s| (s.cancel_buy_pos, s.panic_sell_pos));
            let main_axis_pos = main_spec.and_then(|s| s.price_axis_pos);
            let main_time_axis = main_spec.and_then(|s| s.time_axis_visible);
            let main_line_labels = main_spec.and_then(|s| s.line_labels);
            let main_cursor_labels = main_spec.and_then(|s| s.cursor_labels);
            let pending = specs
                .iter()
                .filter(|s| s.group == group && s.num >= 1 && s.detached.is_some())
                .map(|s| (s.num, s.bucket(), s.detached.unwrap(), s.scale))
                .collect();
            (
                main_scale,
                main_layout,
                main_orderbook,
                main_liquidations,
                main_show_zone,
                main_auto_pin,
                main_action_pos,
                main_axis_pos,
                main_time_axis,
                main_line_labels,
                main_cursor_labels,
                pending,
            )
        };
        if main_scale.is_some() {
            main.update(cx, |p, pcx| p.set_scale(main_scale, pcx));
        }
        if main_layout.0.is_some() || main_layout.1.is_some() || main_layout.2.is_some() {
            main.update(cx, |p, pcx| {
                p.set_layout(main_layout.0, main_layout.1, main_layout.2, pcx)
            });
        }
        if main_orderbook.is_some() {
            main.update(cx, |p, pcx| p.set_orderbook_enabled(main_orderbook, pcx));
        }
        if main_liquidations.is_some() {
            main.update(cx, |p, pcx| p.set_liquidations_enabled(main_liquidations, pcx));
        }
        if main_show_zone.is_some() {
            main.update(cx, |p, pcx| p.set_show_zone(main_show_zone, pcx));
        }
        if main_auto_pin.is_some() {
            main.update(cx, |p, pcx| p.set_auto_pin(main_auto_pin, pcx));
        }
        if main_action_pos.0.is_some() || main_action_pos.1.is_some() {
            main.update(cx, |p, pcx| {
                p.set_action_btn_pos(main_action_pos.0, main_action_pos.1, pcx)
            });
        }
        if main_axis_pos.is_some() {
            main.update(cx, |p, pcx| p.set_price_axis_pos(main_axis_pos, pcx));
        }
        if main_time_axis.is_some() {
            main.update(cx, |p, pcx| p.set_time_axis_visible(main_time_axis, pcx));
        }
        if main_line_labels.is_some() {
            main.update(cx, |p, pcx| p.set_line_labels(main_line_labels, pcx));
        }
        if main_cursor_labels.is_some() {
            main.update(cx, |p, pcx| p.set_cursor_labels(main_cursor_labels, pcx));
        }
        cx.observe(&backend, |this, backend, cx| {
            // Запросы «применить ко всем» из выносных окон — до early-return по sig (они sig не меняют).
            this.drain_apply_all(cx);
            let sig = chart_tabs_sig(backend.read(cx), &this.group);
            if sig == this.last_sig {
                return;
            }
            this.last_sig = sig;
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            this.drain_debug_fill_main_chart(cx);
            this.handle_open_request(cx);
            this.ingest(cx);
            this.drain_chart_repin(cx);
            this.sync_active_scale(cx);
            this.sync_main_chart_target(cx);
            this.sync_seen_for_active(cx);
            this.persist_scales(cx);
            this.sync_inactive_chart_visibility(cx);
            this.last_sig = chart_tabs_sig(backend.read(cx), &this.group);
            cx.notify();
        })
        .detach();
        let coin_input = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .placeholder(rust_i18n::t!("chart.coin.search").to_string())
        });
        // Печать в поле монеты → обновить запрос и (пере)открыть список совпадений. Render читает
        // `coin_query`, а не сам инпут как источник событий (мирроринг StrategiesView).
        cx.subscribe(&coin_input, |this, input, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let value = input.read(cx).value().to_string();
                if this.coin_query != value {
                    this.coin_popup_open = !value.trim().is_empty();
                    this.coin_query = value;
                    cx.notify();
                }
            }
        })
        .detach();
        let layout_fit_input = cx.new(|cx| MoonInputState::new(window, cx));
        let layout_scroll_input = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(
            &layout_fit_input,
            |this, _input, ev: &MoonInputEvent, cx| {
                if this.layout_popup_open
                    && matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. })
                {
                    this.commit_layout_popup(cx);
                }
            },
        )
        .detach();
        cx.subscribe(
            &layout_scroll_input,
            |this, _input, ev: &MoonInputEvent, cx| {
                if this.layout_popup_open
                    && matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. })
                {
                    this.commit_layout_popup(cx);
                }
            },
        )
        .detach();
        // Поле имени кастомной вкладки в попапе ⚙: коммит по Blur/Enter.
        let custom_name_input = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(
            &custom_name_input,
            |this, input, ev: &MoonInputEvent, cx| {
                if this.layout_popup_open
                    && matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. })
                {
                    let name = input.read(cx).value().to_string();
                    this.rename_active_custom(name, cx);
                }
            },
        )
        .detach();
        let mut this = Self {
            backend,
            group,
            epoch,
            theme,
            main,
            add: Vec::new(),
            custom: Vec::new(),
            custom_labels: HashMap::new(),
            next_custom_num: CUSTOM_NUM_BASE,
            coin_selected: std::collections::HashSet::new(),
            custom_gate_gen: HashMap::new(),
            detached: Vec::new(),
            active: Tab::Main,
            seen: HashMap::new(),
            add_seq: HashMap::new(),
            last_sig: initial_sig,
            last_scale_rev: 0,
            restore_pending,
            window_handle: window.window_handle(),
            focus: cx.focus_handle(),
            layout_popup_open: false,
            layout_popup_hovered: false,
            layout_fit_input,
            layout_scroll_input,
            custom_name_input,
            coin_input,
            coin_query: String::new(),
            coin_popup_open: false,
        };
        this.restore_detached(cx);
        this.restore_custom_tabs(cx);
        this.sync_active_scale(cx);
        this.sync_main_chart_target(cx);
        this.persist_scales(cx);
        this
    }

    fn handle_open_request(&mut self, cx: &mut Context<Self>) {
        let pending = {
            let b = self.backend.read(cx);
            b.open_request
                .as_ref()
                .cloned()
                .filter(|(core, _)| core_belongs_to_group(b, self.group.as_str(), *core))
        };
        let Some((pending_core, pending_market)) = pending else {
            return;
        };
        let req = self.backend.update(cx, |b, _| {
            if b.open_request
                .as_ref()
                .is_some_and(|(core, market)| *core == pending_core && market == &pending_market)
            {
                let activate = b.open_request_activate;
                b.open_request_activate = false;
                b.open_request.take().map(|(c, m)| (c, m, activate))
            } else {
                None
            }
        });
        if let Some((core, market, activate)) = req {
            self.main
                .update(cx, |p, pcx| p.open_or_focus(core, market, pcx));
            self.active = Tab::Main;
            self.last_sig = chart_tabs_sig(self.backend.read(cx), self.group.as_str());
            // П.1: поднимаем/фокусируем окно Main ТОЛЬКО для дабл-клика по чарту
            // (open_request_activate). Клики в Ордерах/Детектах открывают монету, но окно
            // не активируют — иначе любой клик дёргал бы окно на передний план.
            if activate {
                let handle = self.window_handle;
                cx.defer(move |app| {
                    let _ = handle.update(app, |_, window, _| window.activate_window());
                });
            }
            self.sync_inactive_chart_visibility(cx);
            self.sync_seen_for_active(cx);
            self.sync_active_scale(cx);
            self.sync_main_chart_target(cx);
            self.persist_scales(cx);
        }
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    fn drain_debug_fill_main_chart(&mut self, cx: &mut Context<Self>) {
        let requested = self.backend.update(cx, |b, _| {
            if b.debug_fill_main_chart_group.as_deref() == Some(self.group.as_str()) {
                b.debug_fill_main_chart_group = None;
                Some(b.debug_fill_main_chart_rev)
            } else {
                None
            }
        });
        if requested.is_some() {
            let rev = requested.unwrap_or_default();
            let filled = self
                .main
                .update(cx, |panel, pcx| panel.debug_fill_history_to_capacity(pcx));
            if filled {
                log::info!(
                    "debug fill main chart: delivered group={} rev={} result=ok",
                    self.group,
                    rev
                );
            } else {
                log::warn!(
                    "debug fill main chart: delivered group={} rev={} result=failed",
                    self.group,
                    rev
                );
            }
            self.active = Tab::Main;
            self.last_sig = chart_tabs_sig(self.backend.read(cx), self.group.as_str());
            cx.notify();
        }
    }

    /// Активная панель (Main или AddToChart/Custom stack) для показа.
    fn active_element(&self) -> AnyElement {
        match &self.active {
            Tab::Main => self.main.clone().into_any_element(),
            Tab::Add(n, bucket) | Tab::Custom(n, bucket) => self
                .add_stack(*n, bucket)
                .map(|p| p.into_any_element())
                .unwrap_or_else(|| self.main.clone().into_any_element()),
        }
    }

    fn active_scale(&self, cx: &App) -> Option<f32> {
        match &self.active {
            Tab::Main => self.main.read(cx).scale(),
            Tab::Add(n, bucket) | Tab::Custom(n, bucket) => self
                .add_stack(*n, bucket)
                .map(|p| p.read(cx).scale())
                .unwrap_or_else(|| self.main.read(cx).scale()),
        }
    }

    fn main_chart_target(&self, cx: &App) -> Option<(CoreId, String)> {
        // Залоченный якорь сравнения действует как Main-фулскрин для торговли: его (core, market)
        // становится таргетом группы → хоткеи F1-F6/S1-S6 и cancel_buy идут на него.
        if let Some(stack) = self.active_stack() {
            if let Some(anchor) = stack.read(cx).compare_anchor() {
                return Some(anchor);
            }
        }
        self.main.read(cx).active_target(cx)
    }

    fn sync_main_chart_target(&self, cx: &mut Context<Self>) {
        let target = self.main_chart_target(cx);
        self.backend
            .update(cx, |b, _| b.set_main_chart_target(&self.group, target));
        #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
        {
            if let Some(handle) = self.main.read(cx).debug_data_handle(cx) {
                self.backend.update(cx, |b, _| {
                    b.register_debug_main_chart(self.group.clone(), handle)
                });
            }
        }
    }

    fn set_active_scale(&self, pct: Option<f32>, cx: &mut Context<Self>) {
        match &self.active {
            Tab::Main => self.main.update(cx, |p, pcx| p.set_scale(pct, pcx)),
            Tab::Add(n, bucket) | Tab::Custom(n, bucket) => {
                if let Some(stack) = self.add_stack(*n, bucket) {
                    stack.update(cx, |p, pcx| p.set_scale(pct, pcx));
                }
            }
        }
    }

    /// Выбор масштаба из дропдауна в полоске вкладок (рядом с ⚙): применяется ТОЛЬКО к
    /// активной вкладке этого окна (Main — к Main) и сохраняется per-вкладочно. В отличие
    /// от старого тулбар-дропдауна не трогает глобальный `price_scale_rev` → другие вкладки
    /// и выносные окна не затрагиваются.
    pub(crate) fn pick_active_scale(&mut self, pct: Option<f32>, cx: &mut Context<Self>) {
        self.set_active_scale(pct, cx);
        self.persist_scales(cx);
        cx.notify();
    }

    /// Совпадения поля монеты для АКТИВНОЙ вкладки (Main/Custom → все ядра группы; Add → ядра

    /// Метка вкладки (П.4): «номер-группа», «номер-группа-ядро» (своё ядро) или
    /// «номер-группа-связка» (именованная связка).
    fn add_label(&self, n: u32, bucket: &ChartBucket, cx: &App) -> String {
        chart_pane_label(&self.backend, &self.group, n, bucket, cx)
    }

    /// Неактивные вкладки отсутствуют в текущей GPUI scene, значит их chart data observe не должен
    /// гонять CPU prepare. Активная/откреплённая панель сама выставит visible=true в своём render.
    fn sync_inactive_chart_visibility(&self, cx: &mut Context<Self>) {
        let active = self.active.clone();
        if matches!(active, Tab::Main) {
            self.main
                .update(cx, |panel, pcx| panel.set_scene_visible(true, pcx));
        } else {
            self.main
                .update(cx, |panel, pcx| panel.set_scene_visible(false, pcx));
        }
        for (n, c, panel) in &self.add {
            if Tab::Add(*n, c.clone()) != active {
                panel.update(cx, |panel, pcx| panel.set_scene_visible(false, pcx));
            }
        }
        for (n, c, panel) in &self.custom {
            let visible = Tab::Custom(*n, c.clone()) == active;
            panel.update(cx, |panel, pcx| panel.set_scene_visible(visible, pcx));
        }
    }

    fn sync_seen_for_active(&mut self, cx: &App) {
        if let Tab::Add(n, c) = self.active.clone() {
            if let Some((_, _, panel)) = self.add.iter().find(|(num, cc, _)| *num == n && *cc == c)
            {
                let cnt = panel.read(cx).pane_count(cx);
                self.seen.insert((n, c), cnt);
            }
        }
    }

    fn sync_active_scale(&mut self, cx: &mut Context<Self>) {
        let (rev, want) = {
            let b = self.backend.read(cx);
            (b.price_scale_rev, b.price_scale)
        };
        if rev != self.last_scale_rev {
            self.last_scale_rev = rev;
            self.set_active_scale(want, cx);
        } else {
            let cur = self.active_scale(cx);
            self.backend.update(cx, |b, _| {
                if b.price_scale != cur {
                    b.price_scale = cur;
                }
            });
        }
    }
}

impl EventEmitter<PanelEvent> for ChartTabs {}
impl Focusable for ChartTabs {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for ChartTabs {
    fn panel_name(&self) -> &'static str {
        "ChartTabs"
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from("Чарты")
    }
    fn dump(&self, _cx: &App) -> PanelState {
        // AddToChart-вкладки не сохраняем: они пересоздаются из детектов при работе.
        crate::dock_persist::panel_state_with_group("ChartTabs", &self.group)
    }
    fn background_policy(&self, _cx: &App) -> MoonBackgroundPolicy {
        MoonBackgroundPolicy::NoFill
    }
}

/// Осмысленная подпись AddToChart-графика (П.4 — порт egui): «номер-группа», а далее по
/// `bucket`: своё ядро → «номер-группа-ядро», именованная связка → «номер-группа-связка»,
/// общая → только «номер-группа». Пустая группа → только номер (старый фолбэк). Используется
/// и в стрипе вкладок, и в заголовке/титуле выносного окна.
fn chart_pane_label(
    backend: &Entity<Backend>,
    group: &str,
    n: u32,
    bucket: &ChartBucket,
    cx: &App,
) -> String {
    // Кастомная (мульти-монетная) вкладка: метка = её имя (custom_label) или дефолт «Набор N»,
    // а не сырой номер «100000-…». Узнаём по наличию custom_coins в спеке.
    {
        let specs = &backend.read(cx).chart_specs;
        if let Some(s) = specs
            .iter()
            .find(|s| s.matches(group, n, bucket))
        {
            if s.custom_coins.is_some() {
                return s.custom_label.clone().unwrap_or_else(|| {
                    t!("chart.tab.custom", n = n - CUSTOM_NUM_BASE + 1).to_string()
                });
            }
        }
    }
    let mut label = if group.is_empty() {
        n.to_string()
    } else {
        format!("{n}-{group}")
    };
    let suffix = match bucket {
        ChartBucket::Shared => String::new(),
        ChartBucket::Core(cid) => backend
            .read(cx)
            .session
            .sessions()
            .iter()
            .find(|s| s.id == *cid)
            .map(|s| s.name.clone())
            .unwrap_or_default(),
        ChartBucket::Bundle(name) => name.clone(),
    };
    if !suffix.is_empty() {
        label.push('-');
        label.push_str(&suffix);
    }
    label
}
