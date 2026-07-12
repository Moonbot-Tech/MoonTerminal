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
//!
//! Здесь (в крейт-руте) остаются объявления модулей, struct `Backend` (его приватные
//! поля видны всем модулям крейта — правило «потомок видит приватное предка») и тонкий
//! `main()`. Методы `Backend` — в [`backend`], тело старта — в [`startup`].

mod axes;
mod backend;
mod chart_persist;
mod detect_sound;
mod chart_tabs;
mod chartdx;
mod clock;
mod coin_icons;
mod controls;
mod core_settings_popup;
mod crash;
mod debug_window;
mod design;
mod detached;
mod diag;
mod dock_persist;
mod figures_backend;
mod firetest;
mod group_window;
mod hotkeys;
mod icons;
mod input;
mod panels;
mod screener;
mod settings;
mod sound;
mod shell;
mod startup;
mod strategies;
mod table_persist;
mod terminal_chrome;
mod windowing;

pub(crate) use startup::install_moon_theme_for_config;

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use gpui::*;

use chartdx::ChartDataHandle;

use moon_ui::{DockAreaState, Root};

use moon_core::config::{AppConfig, WindowLayout};
use moon_core::metrics::{Metrics, MetricsSnapshot};
use moon_core::session::{CoreId, SessionManager};

// Локализация: грузит корневые `locales/*.yml` (путь относительно манифеста крейта).
// `t!("ключ")` тянет строку из этого набора; язык — `rust_i18n::set_locale` (глобальный,
// общий с MoonUI). Fallback на английский, если ключа нет в выбранной локали.
rust_i18n::i18n!("../../locales", fallback = "en");

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
    /// Запрос «открыть монету в новой кастомной вкладке в режиме сравнения» (ПКМ по
    /// детекту): якорь = монета детекта + та же монета с других ядер группы (дедуп по
    /// бирже), замок+метла. Читает ChartTabs группы (см. `open_compare_tab`).
    open_compare_request: Option<(CoreId, String)>,
    /// Ревизия `open_compare_request` — будит ChartTabs через сигнатуру (как `open_request_rev`).
    open_compare_request_rev: u64,
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
    /// Кэш ДЕФОЛТНОГО источника тикера шапки (нет сохранённого выбора): (ядро, рынок).
    /// Резолвится лениво при первом успешном поиске BTCUSDT/UBTCUSDC; не персистится.
    header_ticker_default: Option<(CoreId, String)>,
    last_header_ticker_refresh: Option<Instant>,
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
    /// Группа окна, чей Main-стек должен переключить активный чарт (хоткей `switch_charts`).
    switch_charts_group: Option<String>,
    /// Ревизия запроса «переключить активный чарт»: ++ на каждое нажатие. ChartTabs своей группы
    /// листает активный чарт Main-стека, когда rev вырос (одноразово, не каждый кадр).
    switch_charts_rev: u64,
    /// Ревизия запроса «закрыть все графики» (встроенный Shift+Esc). ГЛОБАЛЬНАЯ (без группы):
    /// каждый ChartTabs на её рост закрывает свой Main-стек. ++ из любого окна.
    close_all_charts_rev: u64,
    /// Группа окна, чей Main-стек должен закрыть ФУЛЛСКРИН-чарт (встроенный Esc). Адресная.
    close_active_chart_group: Option<String>,
    /// Ревизия запроса «закрыть фулскрин-чарт»: ++ на Esc. ChartTabs своей группы закрывает
    /// текущий фулскрин-график Main (и только его), когда rev вырос.
    close_active_chart_rev: u64,
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
    /// Оптимистичный локальный выбор ручной стратегии `(вкл, id)` — живой отклик тогла/пикера
    /// в шапке до echo ClientSettings от ядра.
    manual_strat_local: HashMap<CoreId, (bool, u64)>,
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
    /// Запрос «показать стратегию в окне Стратегий»: (ядро, id стратегии). Ставится из
    /// ПКМ по линии ордера на чарте / клика по колонке Strat в «Ордерах»; дренит
    /// `StrategiesView` в render (снимает «только активные», раскрывает и выбирает).
    strategies_goto: Option<(CoreId, u64)>,
    /// Глобальное окно «Активы» (singleton, все ядра) — дедуп/фокус.
    assets_window: Option<WindowHandle<Root>>,
    /// Окно «Скринер» (singleton, все биржи с дедупом по провайдеру) — дедуп/фокус.
    screener_window: Option<WindowHandle<Root>>,
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
    /// Локальный тоггл «исключить ЧС из рыночной дельты» per-core. У ядра нет read-back этого
    /// флага (локальное действие Active Lib), поэтому храним выбор сами; дефолт — выкл.
    exclude_bl_delta: std::collections::HashMap<CoreId, bool>,
    /// Время последнего закрытия графика крестиком — ГЛОБАЛЬНО (общее для всех панелей).
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
    /// Пользовательские фигуры чарта (слой рисования; ключ = ядро+монета) — общий стор
    /// всех панелей (Rc клонится в движки чартов). Персист figures.json, дебаунс-сейв
    /// коорд-тиком по `dirty` стора.
    figures: std::rc::Rc<std::cell::RefCell<moon_core::figures::FigureStore>>,
    /// Режим рисования включён (кнопка-карандаш нажата, ДЕФОЛТ — вкл). В нём: Ctrl+ЛКМ
    /// рисует фигуру `fig_tool`, простой ЛКМ выделяет/двигает существующую. Выкл — фигуры
    /// СКРЫТЫ и заморожены, чарт работает как обычно.
    fig_draw_mode: bool,
    /// Выбранный инструмент рисования (что рисует Ctrl+ЛКМ). Выбор в попапе карандаша.
    fig_tool: moon_core::figures::FigureTool,
    /// Стиль новых фигур (цвет/толщина/пунктир) — правится в попапе карандаша.
    fig_style: moon_core::figures::DrawStyle,
    /// Выделенная фигура (ядро, монета, id) — одна на приложение: подсветка+узлы на
    /// чарте, хоткеи удаления/алерта работают по ней из Shell.
    fig_selected: Option<(CoreId, String, u64)>,
    /// Чарт-панель ПОД КУРСОРОМ (одна на приложение) — ставится/снимается на её `on_hover`
    /// (enter/leave, редко). Курсор-зависимые хоткеи (new_long/new_short) через неё ставят
    /// ордер по цене под мышью, не завися от фокуса. Weak: панель может исчезнуть.
    hovered_chart: Option<WeakEntity<crate::panels::ChartPanel>>,
    /// Последняя виденная суммарная ревизия серверных chart-алертов (гейт реконсиляции
    /// remote-фигур в дренаж-пути).
    last_chart_alerts_activity: u64,
    /// Курсор последнего проигранного детекта по ядру (звук детектов/алертов).
    last_detect_seq: std::collections::HashMap<CoreId, u64>,
    /// Последняя виденная `detects_rev` по ядру — гейт `play_detect_sounds`: дренаж будит
    /// сотни раз/с, а детекты меняются редко; без гейта скан списка (до 2000/ядро) шёл бы
    /// на каждое пробуждение.
    last_detect_rev: std::collections::HashMap<CoreId, u64>,
    /// Звук по умолчанию для срабатывания алерта без стратегии («Выбор звука» в панели
    /// «Алерты»). Стем wav; см. `sound`/`detect_sound`.
    default_alert_sound: String,
    /// Конфиг изменён в памяти и ждёт дебаунс-сейва (правка размеров ордера колесом мыши —
    /// часто; на диск пишем раз за дренаж-тик). Дренаж зовёт `config.save()` и сбрасывает.
    config_dirty: bool,
    /// Приложение завершается (on_app_quit). На выходе закрытие откреп-окон НЕ должно репинить
    /// их (иначе detached сбросится в None и не восстановится) — дренаж репина это проверяет.
    quitting: bool,
}

fn main() -> anyhow::Result<()> {
    startup::run()
}
