// GUI-приложение: не открывать окно консоли при запуске (без мелькания чёрного окна).
// В честной debug-сборке (debug_assertions=true) консоль остаётся — видны логи env_logger;
// в обычной/release сборке консоли нет, логи идут в файл (см. applog::set_file_logging).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! MoonTerminal — GPUI-оболочка (миграция с egui), этап 1: каркас.
//!
//! Поднимает реальный backend из `moon-core` (конфиг → SessionManager по ядру на
//! сервер) и открывает ПО ОКНУ НА ГРУППУ (как egui-версия). Каждое окно показывает
//! живой статус подключения группы (ready/total + кто «лежит») и метрики CPU/RAM —
//! данные тянутся из общего `Entity<Backend>`, который дренится таймером на
//! UI-потоке и через `notify` будит наблюдателей-окна.
//!
//! Цель этапа — доказать сквозную связку config→сессии→окна→живые данные→GPUI.
//! Чарт/dock/таблицы/настройки — следующие этапы.

mod axes;
mod chart_persist;
mod chart_tabs;
mod chartdx;
mod controls;
mod crash;
mod debug_window;
mod design;
mod detached;
mod diag;
mod dock_persist;
mod firetest;
mod group_window;
mod icons;
mod input;
mod panels;
mod settings;
mod shell;
mod strategies;
mod terminal_chrome;
mod windowing;

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::*;

use chartdx::ChartDataHandle;

use moon_ui::{DockAreaState, MoonTheme, MoonThemeConfig, Root, ThemeMode, init as init_moon_ui};

use moon_core::config::{AppConfig, UiThemeMode, WindowLayout};
use moon_core::metrics::{Metrics, MetricsSnapshot};
use moon_core::session::{CoreId, SessionManager};

// Локализация: грузит корневые `locales/*.yml` (путь относительно манифеста крейта).
// `t!("ключ")` тянет строку из этого набора; язык — `rust_i18n::set_locale` (глобальный,
// общий с MoonUI). Fallback на английский, если ключа нет в выбранной локали.
rust_i18n::i18n!("../../locales", fallback = "en");

fn embedded_fonts() -> Vec<Cow<'static, [u8]>> {
    vec![
        include_bytes!("../../../assets/fonts/Inter-400.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/Inter-500.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/Inter-600.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/GeistMono-400.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/GeistMono-500.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/GeistMono-600.ttf")
            .as_slice()
            .into(),
    ]
}

pub(crate) fn moon_theme_config_for(cfg: &AppConfig) -> MoonThemeConfig {
    let mut theme = match cfg.ui_theme_mode {
        UiThemeMode::Dark => MoonThemeConfig::moon_terminal(),
        UiThemeMode::Light => MoonThemeConfig::moon_light(),
    };
    theme.mode = match cfg.ui_theme_mode {
        UiThemeMode::Dark => ThemeMode::Dark,
        UiThemeMode::Light => ThemeMode::Light,
    };
    theme
        .with_font_delta(cfg.ui_font_delta)
        .with_ui_scale(cfg.ui_scale)
}

pub(crate) fn install_moon_theme_for_config(cfg: &AppConfig, cx: &mut App) {
    MoonTheme::install_config(moon_theme_config_for(cfg), cx);
}

/// Запрос «применить раскладку ко всем вкладкам/окнам группы» (из выносного окна чарта).
pub(crate) struct ChartApplyAll {
    pub group: String,
    /// Включать ли Main-вкладку. true — из попапа Main (ко всем окнам); false — из чартов.
    pub include_main: bool,
    pub mode: Option<chart_persist::StackLayoutMode>,
    pub height_fit: Option<u16>,
    pub height_scroll: Option<u16>,
    /// Копируем ВСЕ настройки вкладки-источника: масштаб цены + галка стакана.
    pub scale: Option<f32>,
    pub orderbook: Option<bool>,
    pub liquidations: Option<bool>,
    pub show_zone: Option<bool>,
    pub auto_pin: Option<bool>,
    pub orientation: Option<chart_persist::StackOrientation>,
    pub cancel_pos: Option<chart_persist::ChartBtnPos>,
    pub panic_pos: Option<chart_persist::ChartBtnPos>,
    pub price_axis_pos: Option<chart_persist::PriceAxisPos>,
    pub time_axis_visible: Option<bool>,
    pub line_labels: Option<bool>,
    pub cursor_labels: Option<bool>,
}

/// Общий backend: живёт в одном `Entity`, дренится таймером, будит окна по notify.
struct Backend {
    session: SessionManager,
    /// Единая точка отсчёта времени (epoch_ms сессий/чарт-вью). Нужна при пересоздании
    /// сессии после сохранения настроек (`SettingsView::save` → рестарт). Порт
    /// egui `App.epoch_ms`.
    epoch: f64,
    /// БД отчётов: канал записи (ядро шлёт close-report → writer пишет в SQLite) +
    /// счётчик-генерация (окно «Отчёт» по нему перезапрашивает). None = БД недоступна.
    /// Порт egui `App.reports`. Держим целиком: `tx` нужен сессии (start/reconnect),
    /// `generation` — панели отчётов.
    reports: Option<moon_core::db::ReportsHandle>,
    metrics: Metrics,
    snap: MetricsSnapshot,
    /// Желаемые открытые рынки (ядро, рынок) — derived view из `chart_market_refs`.
    /// Снаружи чарт-панели держат owner/refcount, а не мутируют этот список руками.
    desired: Vec<(CoreId, String)>,
    chart_market_refs: HashMap<(CoreId, String), usize>,
    chart_market_refs_epoch: u64,
    /// Рынки, которым нужен стакан = есть ≥1 видимая панель с включённым стаканом. Параллельный
    /// refcount к `chart_market_refs`, но считает только панели с orderbook on. `desired_orderbook`
    /// — derived список; идёт в `set_open` отдельным набором (Stage 2: не подписываться, если никто
    /// не хочет стакан).
    chart_orderbook_refs: HashMap<(CoreId, String), usize>,
    desired_orderbook: Vec<(CoreId, String)>,
    desired_open_dirty: bool,
    last_open_sync: Instant,
    /// Main fullscreen chart target by group. Panels such as Orders use this for
    /// "current market"; AddToChart stacks are deliberately not part of that filter.
    main_chart_targets: HashMap<String, (CoreId, String)>,
    /// Монеты, открытые в стеке вкладки Main каждой группы (`group → [(ядро, рынок)]`) — то, что
    /// пользователь открыл на Main. Окно «Ордера» подсвечивает по одной строке на каждую пару.
    main_open_markets: HashMap<String, Vec<(CoreId, String)>>,
    /// Ручной выбор «активного торгового ядра» в шапке (группа → ядро). Sticky-override:
    /// перекрывает авто-следование за ядром фуллскрин-чарта, пока ядро в группе и юзер не
    /// открыл фуллскрином чарт ДРУГОГО ядра (тогда сбрасывается в авто). См. `active_trade_core`.
    trade_core_override: HashMap<String, CoreId>,
    /// Закоммиченный конфиг (тема/ордер-стиль/серверы) — то, что сохранено на диск.
    config: AppConfig,
    /// Черновик окна настроек (draft) — Some, пока окно открыто. Группы-окна, если
    /// он есть, рисуют чарт ИМ (живой предпросмотр); «Сохранить» коммитит его в
    /// config+диск; закрытие окна без сохранения сбрасывает (→ откат к config). 1:1
    /// с egui (SettingsState.draft).
    preview: Option<AppConfig>,
    /// Запрос «открыть монету на Main» (клик по детекту в DetectsPanel) — Shell
    /// читает и открывает в своём чарте. Порт egui open_detect→host.
    open_request: Option<(CoreId, String)>,
    /// Ревизия `open_request`: нужна, чтобы ChartTabs просыпался по конкретному
    /// запросу открытия, а не по страховочному backend-render.
    open_request_rev: u64,
    /// Активировать ли окно Main при выполнении `open_request`. true ТОЛЬКО для дабл-клика
    /// по чарту (открытие монеты на Main); клики Ордеров/Детектов открывают без подъёма окна.
    /// Ставится одновременно с каждым `open_request`, чтобы не рассинхронилось.
    open_request_activate: bool,
    /// Диагностический автозапуск графика для runtime-счётчиков. Off по умолчанию;
    /// включается только env `MOON_RENDER_DIAG_OPEN_FIRST_MARKET`.
    diag_open_first_market: bool,
    diag_open_done: bool,
    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    diag_open_10_btc: bool,
    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    diag_open_10_btc_done: bool,
    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    debug_fill_main_chart_group: Option<String>,
    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    debug_fill_main_chart_rev: u64,
    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    debug_main_chart_handles: HashMap<String, ChartDataHandle>,
    /// Раскладка окон (геометрия по группам) — load на старте, save на изменении
    /// (дебаунс через дренаж-таймер). Порт egui WindowLayout/layout.toml.
    layout: WindowLayout,
    layout_dirty: bool,
    /// Раскладка доков (группа → DockAreaState) — load на старте, save по
    /// DockEvent::LayoutChanged (дебаунс тем же таймером). Пишется в docks.json.
    dock_states: HashMap<String, DockAreaState>,
    dock_dirty: bool,
    /// Масштаб цены (Y) АКТИВНОГО чарта окна: None = «Авто». Теперь МАСШТАБ ПО-ВКЛАДОЧНЫЙ —
    /// это поле = «масштаб активной вкладки» (ChartTabs синхронит для показа в тулбаре; тулбар
    /// при выборе бампает `price_scale_rev` → ChartTabs применяет к активной панели).
    price_scale: Option<f32>,
    /// Группа окна, для которой сделан последний toolbar scale request.
    price_scale_group: Option<String>,
    /// Ревизия запроса масштаба из тулбара: ++ при выборе в дропдауне. ChartTabs применяет
    /// `price_scale` к АКТИВНОЙ панели, когда rev вырос (а не каждый кадр).
    price_scale_rev: u64,
    /// Live-follow тулбара: true = вид бежит за «сейчас», false = пауза (заморозка).
    follow: bool,
    /// Выбранный пресет размера ручного ордера (индекс кнопки F1-F6, 0..=5) НА ЯДРО:
    /// база разная (BTC vs USDT) → значения и выбор per-core. Значение размера для
    /// `PlaceOrder` = `ServerConfig::order_sizes_or_default(base)[sel]`. Нет записи = дефолт.
    order_size_sel: HashMap<CoreId, usize>,
    /// Ревизия выбора размера ордера (++ при клике в тулбаре) — для notify/перерисовки.
    order_size_rev: u64,
    /// Запрос инлайн-редактирования значения кнопки размера (дабл-клик в тулбаре):
    /// `(ядро, индекс F1-F6)`. Shell забирает его в render, открывает инпут поверх кнопки
    /// и фокусирует; по Blur/Enter пишет значение в `ServerConfig.order_sizes` + save.
    order_size_edit_req: Option<(CoreId, usize)>,
    /// Запрос инлайн-редактирования значения fixed-sell пресета (дабл-клик по S-кнопке):
    /// `(ядро, индекс S1-S6)`. По Blur/Enter Shell шлёт `SetFixedSellPct` в ядро.
    sell_edit_req: Option<(CoreId, usize)>,
    /// Оптимистичный локальный кэш fixed-sell процентов `(ядро, индекс)→%`. Колесо/правка пишут
    /// сюда СРАЗУ (дисплей живой), параллельно шлём в ядро. Иначе значение обновлялось бы только
    /// эхом сервера (`send_settings` локальный снимок не трогает) — для sell это незаметно/лаг.
    sell_pct_local: HashMap<(CoreId, usize), f64>,
    /// Оптимистичный локальный выбор fixed-sell слота. `Some(slot)` = горит S1-S6,
    /// `None` = горит основной TP. Без этого клик визуально ждёт echo ClientSettings от ядра.
    sell_slot_local: HashMap<CoreId, Option<usize>>,
    /// «Паник-селл взведён» по (ядро, рынок) — локальный тоггл кнопки Panic Sell на чарте
    /// (визуальная подсветка + on/off, без ожидания эха от ядра).
    panic_armed: HashSet<(CoreId, String)>,
    /// Backend-level notify is only for slow GPUI chrome/status/overlays. High-rate chart
    /// data goes straight into retained chart handles and must not dirty the whole tree.
    backend_dirty_since_notify: bool,
    last_backend_notify: Option<Instant>,
    /// Запросы реконнекта ядра (кнопка ↻ в «Подключениях») — дренаж зовёт
    /// `session.reconnect`. Порт egui `SettingsActions.reconnect`.
    reconnect_request: Vec<CoreId>,
    /// Запросы «показать окно группы» (кнопка 👁) — дренаж открывает/фокусирует окно.
    /// Порт egui `SettingsActions.show_group`.
    show_group_request: Vec<String>,
    /// Открытые окна групп (группа → handle) — фокус по 👁, дедуп окон.
    group_windows: HashMap<String, WindowHandle<Root>>,
    /// Окно «Настройки» (floating tool-window) — дедуп/фокус.
    settings_window: Option<WindowHandle<Root>>,
    /// Окно «Стратегии» (отдельное ОС-окно, общее на приложение) — дедуп/фокус.
    strategies_window: Option<WindowHandle<Root>>,
    /// Глобальное окно «Активы» (singleton, все ядра) — дедуп/фокус.
    assets_window: Option<WindowHandle<Root>>,
    /// Built-in debug scenario runner (`--debug-script chart-smoke`). None in normal app runs.
    firetest: Option<firetest::Runtime>,
    /// Откреплённые dock-панели (какая панель, из какой группы, геометрия окна) — load
    /// на старте, save при изменении. Порт egui `WindowLayout.detached`/`detached.rs`.
    detached: Vec<detached::DetachedSpec>,
    detached_dirty: bool,
    /// Запросы «вернуть панель в док» (закрыли окно открепления) — (группа, panel_name).
    /// Дренит `Shell` своей группы: добавляет панель в свой `DockArea` + убирает спеку.
    repin_request: Vec<(String, String)>,
    /// Запросы «вернуть чарт-вкладку в стрип» (закрыли окно откреп-вкладки) —
    /// (группа, номер, bucket). Дренит `ChartTabs` своей группы: панель detached→add.
    chart_repin_request: Vec<(String, u32, moon_core::config::ChartBucket)>,
    /// Запросы «применить раскладку ко всем» из выносного окна чарта (там нет доступа к стекам
    /// группы) — дренит `ChartTabs` своей группы. `include_main=false` для запросов с чартов
    /// (Main не трогаем). Из самого `ChartTabs` применяется напрямую, без очереди.
    chart_apply_all: Vec<ChartApplyAll>,
    /// Откреплённые в ОС-окна чарт-вкладки, по группе (группа → handle окна). Закрытие
    /// окна группы закрывает принадлежащие ей откреп-чарты; при закрытии самого откреп-окна
    /// чистится по window_id. (Отдельно от `detached` — то про dock-панели, это про чарты.)
    detached_chart_windows: Vec<(String, WindowHandle<Root>)>,
    /// Время последнего «активного» ввода главного окна группы (движение мыши при фокусе),
    /// по группе. Авто-закрытие Main по неактивности (config `main_idle_close_secs`) меряет
    /// от него: окно теряет фокус / мышь не двигается → значение «замораживается», и графики
    /// закроются через N сек. Обновляется только при активном окне (см. Shell on_mouse_move).
    last_main_input: std::collections::HashMap<String, std::time::Instant>,
    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    debug_window: Option<WindowHandle<Root>>,
    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    debug_chart_windows: Vec<WindowHandle<Root>>,
    /// Visible chart consumers for account/order overlays. Live market frames pull
    /// `MarketDataSource` directly from `gpu_canvas.frame()`.
    chart_consumers: Vec<ChartDataHandle>,
    /// Персист чарт-вкладок (масштаб по вкладке + геометрия откреп-окон) — charts.json.
    /// Дебаунс-сейв делает дренаж по `chart_specs_dirty`. См. `chart_persist`.
    chart_specs: Vec<chart_persist::ChartTabSpec>,
    chart_specs_dirty: bool,
    /// Конфиг изменён в памяти и ждёт дебаунс-сейва (правка размеров ордера колесом мыши —
    /// часто; на диск пишем раз за дренаж-тик). Дренаж зовёт `config.save()` и сбрасывает.
    config_dirty: bool,
    /// Приложение завершается (on_app_quit). На выходе закрытие откреп-окон НЕ должно репинить
    /// их (иначе detached сбросится в None и не восстановится) — дренаж репина это проверяет.
    quitting: bool,
}

impl Backend {
    fn manual_order_size_state(&self, core: CoreId) -> ([f64; 6], usize) {
        const DEFAULT_SEL: usize = 2;

        let base = self.session.core_base(core).unwrap_or("");
        let sizes = self
            .config
            .servers
            .iter()
            .find(|s| s.id == core)
            .map(|s| s.order_sizes_or_default(base))
            .unwrap_or_else(|| moon_core::config::servers::default_order_sizes(base));
        let sel = self
            .order_size_sel
            .get(&core)
            .copied()
            .unwrap_or(DEFAULT_SEL)
            .min(sizes.len().saturating_sub(1));
        (sizes, sel)
    }

    pub(crate) fn manual_order_size(&self, core: CoreId) -> f64 {
        let (sizes, sel) = self.manual_order_size_state(core);
        sizes[sel]
    }

    /// Прогнозный размер ручного ордера (s1-s6 активного ядра) в USD: размер в базовой валюте
    /// аккаунта × курс базы→USD. None — нет ядра/размера/курса. Для подписи на перекрестии чарта.
    pub(crate) fn prospective_order_usd(&self, core: CoreId) -> Option<f64> {
        let size = self.manual_order_size(core);
        if !(size > 0.0) {
            return None;
        }
        let base = self.session.core_base(core).unwrap_or("");
        let rate = self.session.market_source().currency_usd_rate(core, base)?;
        (rate > 0.0).then_some(size * rate)
    }

    /// Значение пресета размера `ix` (F1-F6) ядра — из конфига (или дефолт по базе).
    fn order_size_value(&self, core: CoreId, ix: usize) -> f64 {
        let (sizes, _) = self.manual_order_size_state(core);
        sizes[ix.min(sizes.len().saturating_sub(1))]
    }

    /// Записать значение пресета размера `ix` ядра в конфиг (правка колесом/инпутом). На диск
    /// НЕ сохраняем сразу — ставим `config_dirty`, дренаж сделает дебаунс-сейв.
    fn set_order_size_value(&mut self, core: CoreId, ix: usize, v: f64) {
        if ix >= 6 || !(v > 0.0) {
            return;
        }
        let base = self.session.core_base(core).unwrap_or("").to_string();
        if let Some(s) = self.config.servers.iter_mut().find(|s| s.id == core) {
            let mut arr = s
                .order_sizes
                .unwrap_or_else(|| moon_core::config::servers::default_order_sizes(&base));
            arr[ix] = v;
            s.order_sizes = Some(arr);
            self.config_dirty = true;
        }
    }

    /// Текущий видимый процент fixed-sell пресета `ix` (S1-S6) ядра: оптимистичный локальный
    /// кэш, если есть (свежая правка колесом/инпутом), иначе значение из снимка ClientSettings.
    fn fixed_sell_pct(&self, core: CoreId, ix: usize) -> f64 {
        if let Some(v) = self.sell_pct_local.get(&(core, ix)) {
            return *v;
        }
        self.session
            .store()
            .core(core)
            .and_then(|d| d.client_settings.as_ref())
            .map(|s| s.fixed_sell_pcts[ix.min(5)])
            .unwrap_or(0.0)
    }

    /// Записать оптимистичный локальный процент fixed-sell (живой дисплей до эха ядра).
    fn set_fixed_sell_pct_local(&mut self, core: CoreId, ix: usize, v: f64) {
        self.sell_pct_local.insert((core, ix), v);
    }

    /// Локальный кэш `(core,ix)`, иначе `fallback` (значение ядра) — для дисплея sell-полосы.
    fn fixed_sell_pct_with(&self, core: CoreId, ix: usize, fallback: f64) -> f64 {
        self.sell_pct_local
            .get(&(core, ix))
            .copied()
            .unwrap_or(fallback)
    }

    fn set_fixed_sell_slot_local(&mut self, core: CoreId, slot: Option<usize>) {
        self.sell_slot_local.insert(core, slot);
    }

    fn fixed_sell_slot_with(&self, core: CoreId, fallback: Option<usize>) -> Option<usize> {
        self.sell_slot_local.get(&core).copied().unwrap_or(fallback)
    }

    fn fixed_sell_mode_with(&self, core: CoreId, fallback: bool) -> bool {
        self.sell_slot_local
            .get(&core)
            .map(|slot| slot.is_some())
            .unwrap_or(fallback)
    }

    /// Взведён ли «паник-селл» по (ядро, рынок) — для подсветки кнопки Panic Sell.
    fn is_panic_armed(&self, core: CoreId, market: &str) -> bool {
        self.panic_armed.contains(&(core, market.to_string()))
    }

    /// Тоггл «паник-селл» по рынку: флипает локальный флаг и шлёт правку ядру. Возвращает
    /// новое состояние (true = взвели).
    fn toggle_panic_sell(&mut self, core: CoreId, market: String) -> bool {
        let key = (core, market.clone());
        let on = if self.panic_armed.remove(&key) {
            false
        } else {
            self.panic_armed.insert(key);
            true
        };
        if let Err(error) = self.session.panic_sell_market(core, market, on) {
            log::warn!("panic sell market failed: {error:#}");
        }
        on
    }

    fn cancel_buy_for_main_chart(&self, group: &str) -> usize {
        let Some((core, market)) = self.main_chart_target(group) else {
            log::warn!("cancel buy ignored: no open main chart for group={group}");
            return 0;
        };
        self.cancel_buy_orders(core, &market)
    }

    fn cancel_buy_orders(&self, core: CoreId, market: &str) -> usize {
        let Some(data) = self.session.store().core(core) else {
            log::warn!("cancel buy ignored: core={core} has no store");
            return 0;
        };

        let uids: Vec<u64> = data
            .orders
            .iter()
            .filter(|order| {
                // MoonBot DoCancel = CancelAllBuys(market) + CancelPendings(market).
                order.market == market
                    && !order.job_is_done
                    && ((!order.is_short && order.fill_pct < 99.95) || order.pending)
            })
            .map(|order| order.uid)
            .collect();

        for uid in &uids {
            if let Err(err) = self.session.cancel_order(core, *uid) {
                log::warn!("cancel buy failed: core={core} market={market} uid={uid}: {err:#}");
            }
        }
        if uids.is_empty() {
            log::info!("cancel buy: no active buy orders for core={core} market={market}");
        } else {
            log::info!(
                "cancel buy: requested {} orders for core={core} market={market}",
                uids.len()
            );
        }
        uids.len()
    }

    fn register_chart_consumer(&mut self, chart: ChartDataHandle) {
        if self
            .chart_consumers
            .iter()
            .any(|existing| existing == &chart)
        {
            return;
        }
        self.chart_consumers.push(chart);
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    fn register_debug_main_chart(&mut self, group: String, chart: ChartDataHandle) {
        self.debug_main_chart_handles.insert(group, chart);
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    fn debug_main_chart_shift_hz(&self, group: &str) -> Option<f32> {
        self.debug_main_chart_handles
            .get(group)
            .filter(|chart| chart.is_alive())
            .and_then(ChartDataHandle::camera_shift_hz)
    }

    fn live_chart_consumers(&mut self) -> Vec<ChartDataHandle> {
        self.chart_consumers.retain(ChartDataHandle::is_alive);
        self.chart_consumers.clone()
    }

    fn set_main_chart_target(&mut self, group: &str, target: Option<(CoreId, String)>) {
        // Открытие фуллскрином чарта ДРУГОГО ядра = «явная смена» → сбрасываем sticky-override,
        // чтобы шапка вернулась к авто-следованию за фуллскрином. Тот же core / снятие фуллскрина
        // override не трогают.
        if let Some((new_core, _)) = &target {
            let prev_core = self.main_chart_targets.get(group).map(|(c, _)| *c);
            if prev_core != Some(*new_core) {
                self.trade_core_override.remove(group);
            }
        }
        match target {
            Some(target) => {
                self.main_chart_targets.insert(group.to_string(), target);
            }
            None => {
                self.main_chart_targets.remove(group);
            }
        }
    }

    fn main_chart_target(&self, group: &str) -> Option<(CoreId, String)> {
        self.main_chart_targets.get(group).cloned()
    }

    /// Опубликовать монеты, открытые в стеке Main группы (из `MainChartStack`).
    fn set_main_open_markets(&mut self, group: &str, markets: Vec<(CoreId, String)>) {
        if markets.is_empty() {
            self.main_open_markets.remove(group);
        } else {
            self.main_open_markets.insert(group.to_string(), markets);
        }
    }

    /// Монеты, открытые в стеке Main группы (для подсветки/сортировки в «Ордерах»).
    pub(crate) fn main_open_markets(&self, group: &str) -> &[(CoreId, String)] {
        self.main_open_markets
            .get(group)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Отметить «активный» ввод в главном окне группы (движение мыши при сфокусированном окне).
    /// Сбрасывает таймер авто-закрытия Main по неактивности. Зовётся из Shell on_mouse_move
    /// ТОЛЬКО когда окно активно.
    pub(crate) fn note_main_input(&mut self, group: &str) {
        self.last_main_input
            .insert(group.to_string(), std::time::Instant::now());
    }

    /// Время последнего активного ввода в главном окне группы (для авто-закрытия по неактивности).
    pub(crate) fn main_input_at(&self, group: &str) -> Option<std::time::Instant> {
        self.last_main_input.get(group).copied()
    }

    /// Авто-закрытие Main по неактивности, сек (config; 0 = выключено).
    pub(crate) fn main_idle_close_secs(&self) -> u32 {
        self.preview
            .as_ref()
            .unwrap_or(&self.config)
            .main_idle_close_secs
    }

    /// Активное торговое ядро группы для шапки/тулбара: sticky-override (ручной выбор в
    /// шапке), если он ещё валиден (ядро в группе), иначе ядро фуллскрин-чарта, иначе первое
    /// ядро группы. Тулбар не должен превращаться в прочерки только потому, что чарт ещё не открыт.
    fn active_trade_core(&self, group: &str) -> Option<CoreId> {
        if let Some(&core) = self.trade_core_override.get(group) {
            let in_group = self
                .session
                .sessions()
                .iter()
                .any(|s| s.id == core && s.group == group);
            if in_group {
                return Some(core);
            }
        }
        self.main_chart_target(group)
            .map(|(core, _)| core)
            .or_else(|| {
                self.session
                    .sessions()
                    .iter()
                    .find(|s| s.group == group)
                    .map(|s| s.id)
            })
    }

    /// Записать ручной выбор активного торгового ядра (клик в селекторе шапки).
    fn set_trade_core_override(&mut self, group: &str, core: CoreId) {
        self.trade_core_override.insert(group.to_string(), core);
    }

    /// Ядра группы (id, имя) для селектора в шапке. Порядок — как в конфиге/сессиях.
    fn group_cores(&self, group: &str) -> Vec<(CoreId, String)> {
        self.session
            .sessions()
            .iter()
            .filter(|s| s.group == group)
            .map(|s| (s.id, s.name.clone()))
            .collect()
    }

    fn retain_chart_market(&mut self, core: CoreId, market: &str) {
        let key = (core, market.to_string());
        *self.chart_market_refs.entry(key).or_insert(0) += 1;
        self.rebuild_desired_markets();
    }

    fn release_chart_market(&mut self, core: CoreId, market: &str) {
        let key = (core, market.to_string());
        let mut remove = false;
        if let Some(count) = self.chart_market_refs.get_mut(&key) {
            debug_assert!(*count > 0, "chart market refcount over-release");
            *count = count.saturating_sub(1);
            remove = *count == 0;
        } else {
            debug_assert!(false, "chart market refcount release without owner");
        }
        if remove {
            self.chart_market_refs.remove(&key);
        }
        self.rebuild_desired_markets();
    }

    fn retain_chart_orderbook(&mut self, core: CoreId, market: &str) {
        let key = (core, market.to_string());
        *self.chart_orderbook_refs.entry(key).or_insert(0) += 1;
        self.rebuild_orderbook_wanted();
    }

    fn release_chart_orderbook(&mut self, core: CoreId, market: &str) {
        let key = (core, market.to_string());
        let mut remove = false;
        if let Some(count) = self.chart_orderbook_refs.get_mut(&key) {
            *count = count.saturating_sub(1);
            remove = *count == 0;
        }
        if remove {
            self.chart_orderbook_refs.remove(&key);
        }
        self.rebuild_orderbook_wanted();
    }

    /// Пересобрать `desired_orderbook` (рынки с ≥1 включённым стаканом). Меняется → dirty (re-send).
    fn rebuild_orderbook_wanted(&mut self) {
        let mut want: Vec<(CoreId, String)> = self
            .chart_orderbook_refs
            .iter()
            .filter_map(|((core, market), count)| (*count > 0).then(|| (*core, market.clone())))
            .collect();
        want.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        if self.desired_orderbook != want {
            self.desired_orderbook = want;
            self.desired_open_dirty = true;
        }
    }

    fn rebuild_desired_markets(&mut self) {
        let mut desired: Vec<(CoreId, String)> = self
            .chart_market_refs
            .iter()
            .filter_map(|((core, market), count)| (*count > 0).then(|| (*core, market.clone())))
            .collect();
        desired.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        if self.desired != desired {
            self.desired = desired;
            self.desired_open_dirty = true;
        }
    }

    fn sync_open_markets_if_due(&mut self) {
        let now = Instant::now();
        // The 1s fallback is intentional: provider-side linger/drop/failover is
        // wall-clock based. The hot path itself is the boolean dirty flag; we no
        // longer hash the whole desired market list every 100ms.
        let due = now.duration_since(self.last_open_sync) >= Duration::from_secs(1);
        if self.desired_open_dirty || due {
            self.desired_open_dirty = false;
            self.last_open_sync = now;
            self.session
                .set_open(&self.desired, &self.desired_orderbook);
        }
    }

    fn mark_backend_dirty(&mut self, cx: &mut Context<Self>) {
        self.backend_dirty_since_notify = true;
        self.flush_backend_notify(cx);
    }

    fn flush_backend_notify(&mut self, cx: &mut Context<Self>) {
        if !self.backend_dirty_since_notify {
            return;
        }
        let due = self
            .last_backend_notify
            .is_none_or(|last| last.elapsed() >= Duration::from_millis(250));
        if !due {
            return;
        }
        self.backend_dirty_since_notify = false;
        self.last_backend_notify = Some(Instant::now());
        crate::diag::bump(&crate::diag::BACKEND_NOTIFY);
        cx.notify();
    }

    fn maybe_diag_open_first_market(&mut self, cx: &mut Context<Self>) {
        if !self.diag_open_first_market || self.diag_open_done || self.open_request.is_some() {
            return;
        }
        if self.group_windows.is_empty() {
            return;
        }

        let candidate = self.config.servers.iter().find_map(|server| {
            let market = server.market.trim();
            let session_exists = self
                .session
                .sessions()
                .iter()
                .any(|session| session.id == server.id && session.group == server.group);
            (server.active
                && server.show_window
                && self.config.group(&server.group).active
                && self.group_windows.contains_key(&server.group)
                && !market.is_empty()
                && session_exists)
                .then(|| (server.id, market.to_string(), server.name.clone()))
        });

        let Some((core, market, name)) = candidate else {
            self.diag_open_done = true;
            log::warn!("diag auto-open: no active visible server with default market");
            return;
        };

        self.diag_open_done = true;
        self.open_request = Some((core, market.clone()));
        self.open_request_rev = self.open_request_rev.wrapping_add(1);
        self.open_request_activate = false;
        if std::env::var_os("MOON_RENDER_DIAG_PAUSE_AFTER_OPEN").is_some() {
            self.follow = false;
        }
        log::info!("diag auto-open: core={core} name={name} market={market}");
        cx.notify();
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    fn take_diag_open_10_btc(&mut self) -> bool {
        if !self.diag_open_10_btc || self.diag_open_10_btc_done {
            return false;
        }
        // Debug perf windows only need a live core id/group, not the main group window.
        // On headless Linux/X11 the main window can exist while the bookkeeping gate is
        // still false during early startup, which made MOON_RENDER_DIAG_OPEN_10_BTC
        // silently do nothing and broke automated perf runs.
        if self.session.sessions().is_empty() {
            return false;
        }
        if crate::debug_window::debug_chart_target(self).is_none() {
            return false;
        }
        self.diag_open_10_btc_done = true;
        true
    }
}

fn main() -> anyhow::Result<()> {
    // Строим env_logger как Logger (не .init()) и оборачиваем в TeeLogger — он
    // дублирует напечатанные записи в in-memory кольцо вкладки «Лог» (порт egui main).
    let env = env_logger::Builder::from_env(
        env_logger::Env::default()
            .default_filter_or("warn,moon_ui_gpui=info,moon_gpui=info,moon_core=info"),
    )
    .build();
    log::set_max_level(env.filter());
    if let Err(e) = log::set_boxed_logger(Box::new(moon_core::applog::TeeLogger::new(env))) {
        eprintln!("не удалось установить логгер: {e}");
    }
    log::info!(
        "build: moonterminal={} moonui={}",
        option_env!("MOONTERMINAL_GIT_REV").unwrap_or("unknown"),
        option_env!("MOONUI_GIT_REV").unwrap_or("unknown")
    );
    let firetest_config = firetest::Config::from_args(std::env::args())?;
    if firetest_config.is_some() {
        diag::force_enable();
    }

    // Нативные краши (access violation в DirectX/GPUI-форке, напр. present по протухшему
    // дескриптору окна при реконнекте) идут МИМО Rust-паник-хука — процесс умирает молча,
    // `panic.log` пуст. Ставим SEH-фильтр верхнего уровня, чтобы такой краш тоже попал в
    // `panic.log` с кодом/адресом/бэктрейсом. Раньше всего — до создания окон.
    crash::install_native_handler();

    // Паник-хук: GUI-приложение без консоли → stderr с сообщением паники теряется (и при
    // panic=abort это выглядит как нативный краш 0xc0000409 в ucrtbase). Пишем место+сообщение
    // паники в `panic.log` (cwd) и в общий лог ДО аборта — чтобы видеть точный source-локейшн.
    {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let loc = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "?".into());
            let payload = info
                .payload()
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| info.payload().downcast_ref::<String>().map(|s| s.as_str()))
                .unwrap_or("<non-string>");
            // Бэктрейс (force — без RUST_BACKTRACE): location у clamp-паник = внутренность core,
            // а нам нужен ВЫЗЫВАЮЩИЙ кадр в нашем коде.
            let bt = std::backtrace::Backtrace::force_capture();
            let line = format!("PANIC at {loc}: {payload}\n--- backtrace ---\n{bt}\n--- end ---");
            log::error!("PANIC at {loc}: {payload}");
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("panic.log")
            {
                let _ = writeln!(f, "{line}");
            }
            default_hook(info);
        }));
    }

    let cfg = AppConfig::load()?;
    // Язык интерфейса из конфига → глобальная локаль rust-i18n (для t! здесь и в MoonUI).
    rust_i18n::set_locale(cfg.language.code());
    // Файловый лог: режим из конфига + одноразовая чистка старых файлов при старте.
    moon_core::applog::set_file_logging(cfg.log_to_file, cfg.log_retention_days);
    moon_core::applog::purge_old();
    let group_list = crate::group_window::groups(&cfg);
    log::info!("groups: {group_list:?} (servers: {})", cfg.servers.len());

    // Единая точка отсчёта времени для сессий и чарт-вью (как epoch_ms в egui).
    let epoch = moon_chart::paint::now_unix_ms();

    // Регистрируем встроенные SVG-иконки MoonUI как AssetSource — без этого `IconName::*`
    // (напр. крестик очистки `cleanable` = CircleX) не находят svg и рисуются пустыми.
    let app = gpui_platform::application().with_assets(moon_ui::MoonAssets);
    app.run(move |cx| {
        init_moon_ui(cx);
        install_moon_theme_for_config(&cfg, cx);
        cx.text_system()
            .add_fonts(embedded_fonts())
            .expect("failed to add embedded MoonBot fonts");

        let layout = WindowLayout::load();
        let dock_states = dock_persist::load_all();
        let detached = detached::load_all();

        // Одноразовый ремап charts.json: до v11 схемы вкладки хранили ПОЗИЦИОННЫЕ CoreId,
        // а теперь CoreId = стабильный uid. Перепривязываем, пока порядок серверов тот же,
        // что был при записи файла (флаг взводится только при апгрейде со старой версии).
        let chart_specs = {
            let mut specs = chart_persist::load_all();
            if cfg.chart_core_remap_needed {
                chart_persist::remap_core_ids(&mut specs, &cfg.servers);
                chart_persist::save_all(&specs);
            }
            specs
        };

        // БД отчётов: поднимаем writer (как egui App). Его `tx` отдаём сессии (ядро
        // шлёт close-report → запись в SQLite), `generation` живёт в Backend для окна
        // «Отчёт». None = БД недоступна (окно отчётов покажет пусто).
        let reports = moon_core::db::spawn_writer();
        let (feed_wake_tx, feed_wake_rx) = std::sync::mpsc::channel::<()>();

        let backend = cx.new(|_| Backend {
            session: SessionManager::start(
                &cfg,
                epoch,
                reports.as_ref().map(|h| &h.tx),
                Some(feed_wake_tx.clone()),
            ),
            epoch,
            reports,
            metrics: Metrics::new(),
            snap: MetricsSnapshot::default(),
            // open = рынки ОТКРЫТЫХ чарт-панелей (как App::about_to_wait в egui).
            // Пусто на старте; наполнится при открытии монеты (порт чарт-панелей).
            // set_open всё равно избирает провайдера/биржу на старте → subscribe_all_trades
            // (ретейн всех трейдов биржи — как было; ради мгновенного открытия монеты).
            desired: Vec::new(),
            chart_market_refs: HashMap::new(),
            chart_market_refs_epoch: 0,
            chart_orderbook_refs: HashMap::new(),
            desired_orderbook: Vec::new(),
            desired_open_dirty: true,
            last_open_sync: Instant::now() - Duration::from_secs(10),
            main_chart_targets: HashMap::new(),
            main_open_markets: HashMap::new(),
            trade_core_override: HashMap::new(),
            config: cfg.clone(),
            preview: None,
            open_request: None,
            open_request_rev: 0,
            open_request_activate: false,
            diag_open_first_market: std::env::var_os("MOON_RENDER_DIAG_OPEN_FIRST_MARKET")
                .is_some(),
            diag_open_done: false,
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            diag_open_10_btc: std::env::var_os("MOON_RENDER_DIAG_OPEN_10_BTC").is_some(),
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            diag_open_10_btc_done: false,
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            debug_fill_main_chart_group: None,
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            debug_fill_main_chart_rev: 0,
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            debug_main_chart_handles: HashMap::new(),
            layout: layout.clone(),
            layout_dirty: false,
            dock_states,
            dock_dirty: false,
            price_scale: None,
            price_scale_group: None,
            price_scale_rev: 0,
            follow: true,
            order_size_sel: HashMap::new(),
            order_size_rev: 0,
            order_size_edit_req: None,
            sell_edit_req: None,
            sell_pct_local: HashMap::new(),
            sell_slot_local: HashMap::new(),
            panic_armed: HashSet::new(),
            backend_dirty_since_notify: false,
            last_backend_notify: None,
            reconnect_request: Vec::new(),
            show_group_request: Vec::new(),
            group_windows: HashMap::new(),
            settings_window: None,
            strategies_window: None,
            assets_window: None,
            firetest: firetest_config.clone().map(firetest::Runtime::new),
            detached,
            detached_dirty: false,
            repin_request: Vec::new(),
            chart_repin_request: Vec::new(),
            chart_apply_all: Vec::new(),
            detached_chart_windows: Vec::new(),
            last_main_input: std::collections::HashMap::new(),
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            debug_window: None,
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            debug_chart_windows: Vec::new(),
            chart_consumers: Vec::new(),
            chart_specs,
            chart_specs_dirty: false,
            config_dirty: false,
            quitting: false,
        });

        // Фабрики панелей для восстановления раскладки доков (PanelRegistry — глобален).
        dock_persist::register_panels(cx, backend.clone(), epoch);

        // Закрытие ГЛАВНОГО (группового) окна = полный выход: убираем закрытое окно из
        // group_windows, и если групповых окон не осталось — quit (закроет и откреплённые
        // чарт-окна). Детач-чарт окна сами quit не вызывают (их id нет в group_windows).
        let quit_backend = backend.clone();
        cx.on_window_closed(move |app, closed_id| {
            // Возвращаем (откреп-окна_на_закрытие, надо_ли_выйти).
            let (to_close, quit) = quit_backend.update(app, |b, _| {
                // Это окно группы? (его group, если да)
                let group = b
                    .group_windows
                    .iter()
                    .find(|(_, h)| h.window_id() == closed_id)
                    .map(|(g, _)| g.clone());
                if let Some(group) = group {
                    b.group_windows.remove(&group);
                    if b.group_windows.is_empty() {
                        // Последнее окно группы → полный выход (quit закроет всё, вкл. откреп).
                        return (Vec::new(), true);
                    }
                    // Иначе закрыть откреп-чарты ИМЕННО этой группы.
                    let close: Vec<WindowHandle<Root>> = b
                        .detached_chart_windows
                        .iter()
                        .filter(|(g, _)| *g == group)
                        .map(|(_, h)| *h)
                        .collect();
                    b.detached_chart_windows.retain(|(g, _)| *g != group);
                    (close, false)
                } else {
                    // Закрыли откреп-чарт-окно (или иное) — вычистить из трекинга.
                    b.detached_chart_windows
                        .retain(|(_, h)| h.window_id() != closed_id);
                    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
                    {
                        if b.debug_window
                            .as_ref()
                            .is_some_and(|h| h.window_id() == closed_id)
                        {
                            b.debug_window = None;
                        }
                        b.debug_chart_windows.retain(|h| h.window_id() != closed_id);
                    }
                    (Vec::new(), false)
                }
            });
            for h in to_close {
                h.update(app, |_, window, _| window.remove_window()).ok();
            }
            if quit {
                app.quit();
            }
        })
        .detach();

        // На выходе из приложения: пометить quitting и СРАЗУ сохранить charts.json. На старте
        // quit окна ещё не снесены → detached=Some; без этого закрытие откреп-окон при выходе
        // репинит их (detached→None) и они не восстанавливаются. quitting также глушит дренаж
        // репина (drain_chart_repin), чтобы он не сбросил detached.
        let app_quit_backend = backend.clone();
        cx.on_app_quit(move |cx| {
            moon_core::detect_diag::line("[quit] on_app_quit → сохраняю charts.json");
            app_quit_backend.update(cx, |b, _| {
                b.quitting = true;
                chart_persist::save_all(&b.chart_specs);
            });
            async move {}
        })
        .detach();

        // Feed event path: feed threads send causal wakes after real MoonProto events.
        // Market-only wakes update MarketDataSource/store; visible charts pull it from
        // gpu_canvas.frame() without dirtying Backend/Shell. Account/order wakes still notify
        // Backend through the slow gate and update only chart order overlays here.
        let data_backend = backend.clone();
        cx.spawn(async move |cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let mut feed_wake_rx = feed_wake_rx;
            loop {
                let (rx, woke) = executor
                    .spawn(async move {
                        let woke = feed_wake_rx.recv().is_ok();
                        (feed_wake_rx, woke)
                    })
                    .await;
                feed_wake_rx = rx;
                if !woke {
                    break;
                }
                while feed_wake_rx.try_recv().is_ok() {}

                cx.update(|cx| {
                    data_backend.update(cx, |b, cx| {
                        let drain = b.session.drain();
                        if !drain.any {
                            return;
                        }
                        if drain.chart_data {
                            if drain.ui_state {
                                let chart_consumers = b.live_chart_consumers();
                                for chart in chart_consumers {
                                    chart.sync_orders_if_visible(&b.session, false);
                                }
                            }
                        }
                        if drain.ui_state {
                            b.mark_backend_dirty(cx);
                        }
                    });
                });
            }
        })
        .detach();

        // Slow coordination path: provider roles, metrics, reconnects and persistence. This may
        // wake the GPUI tree through Backend notify, but it never stages high-rate chart pixels.
        let coord_backend = backend.clone();
        let coord_cfg = cfg.clone();
        let coord_layout = layout.clone();
        cx.spawn(async move |cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let mut last_report = Instant::now();
            loop {
                executor.timer(Duration::from_millis(100)).await;
                cx.update(|cx| {
                    let (show_reqs, open_debug_10) = coord_backend.update(cx, |b, cx| {
                        b.maybe_diag_open_first_market(cx);
                        b.sync_open_markets_if_due();
                        b.snap = b.metrics.sample(Instant::now());
                        crate::firetest::tick_backend(b, cx);

                        let recon: Vec<CoreId> = b.reconnect_request.drain(..).collect();
                        for id in recon {
                            b.session
                                .reconnect(id, &b.config, b.reports.as_ref().map(|h| &h.tx));
                        }
                        if b.layout_dirty {
                            b.layout.save();
                            b.layout_dirty = false;
                        }
                        if b.dock_dirty {
                            dock_persist::save_all(&b.dock_states);
                            b.dock_dirty = false;
                        }
                        if b.detached_dirty {
                            detached::save_all(&b.detached);
                            b.detached_dirty = false;
                        }
                        if b.chart_specs_dirty {
                            chart_persist::save_all(&b.chart_specs);
                            b.chart_specs_dirty = false;
                        }
                        if b.config_dirty {
                            // Дебаунс-сейв конфига (правка размеров колесом мыши пишет в память
                            // часто; на диск — раз за дренаж-тик, а не на каждый тик колеса).
                            if let Err(e) = b.config.save() {
                                log::warn!("config save (debounced) failed: {e}");
                            }
                            b.config_dirty = false;
                        }
                        b.flush_backend_notify(cx);
                        let reqs = std::mem::take(&mut b.show_group_request);
                        #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
                        let open_debug_10 = b.take_diag_open_10_btc();
                        #[cfg(not(any(
                            debug_assertions,
                            moon_profile_debug,
                            feature = "debug-tools"
                        )))]
                        let open_debug_10 = false;
                        (reqs, open_debug_10)
                    });

                    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
                    if open_debug_10 {
                        log::info!("diag auto-open: spawning 10 live-market chart windows");
                        crate::debug_window::spawn_debug_chart_windows(cx, coord_backend.clone());
                    }
                    for g in show_reqs {
                        crate::group_window::spawn_group_window(
                            cx,
                            &coord_backend,
                            &coord_cfg,
                            g,
                            epoch,
                            &coord_layout,
                            0.0,
                        );
                    }
                });
                if last_report.elapsed().as_millis() >= 1000 {
                    let ms = last_report.elapsed().as_secs_f64() * 1000.0;
                    last_report = Instant::now();
                    if let Some(sample) = crate::diag::take_sample(ms) {
                        crate::diag::write_sample(ms, &sample);
                        cx.update(|cx| {
                            coord_backend.update(cx, |b, _| {
                                crate::firetest::record_diag_sample(b, ms, &sample);
                            });
                        });
                    }
                }
            }
        })
        .detach();
        // По окну на группу (тем же helper'ом, что и кнопка 👁 «показать группу»).
        for (i, group) in group_list.into_iter().enumerate() {
            crate::group_window::spawn_group_window(
                cx,
                &backend,
                &cfg,
                group,
                epoch,
                &layout,
                i as f32 * 40.0,
            );
        }

        // Восстановить окна откреплённых панелей (панель уже не в доке — она была убрана
        // при откреплении, и dock_persist сохранил док без неё). Порт egui-восстановления
        // detached на старте.
        let specs = backend.read(cx).detached.clone();
        for spec in &specs {
            if let Err(err) = detached::spawn(cx, &backend, spec, None) {
                log::warn!(
                    "restore detached panel failed group={} panel={}: {err:#}",
                    spec.group,
                    spec.panel
                );
            }
        }
    });
    Ok(())
}
