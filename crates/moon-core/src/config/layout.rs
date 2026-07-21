//! Раскладка окон — отдельный переносимый `layout.toml` рядом с exe (как
//! `theme.toml`). Хранит позиции/размеры окон групп, свёрнут ли док и активную
//! вкладку, а также список откреплённых окон (какая вкладка, из какой группы,
//! геометрия). Общая на всех (один файл). Битый/отсутствующий файл → дефолт.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::paths;

/// Панели окна «Стратегии»: ширины (дерево/версии/разделы) + свёрнутость версий.
/// Клампы — на стороне окна при применении.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct StrategiesPanels {
    pub tree_w: f32,
    pub versions_w: f32,
    pub sections_w: f32,
    pub versions_collapsed: bool,
}

impl Default for StrategiesPanels {
    fn default() -> Self {
        Self {
            tree_w: 418.0,
            versions_w: 166.0,
            sections_w: 264.0,
            // По умолчанию колонка версий свёрнута в полоску со счётчиком.
            versions_collapsed: true,
        }
    }
}

/// Геометрия+состояние окна группы (ключ карты — имя группы).
#[derive(Clone, Serialize, Deserialize)]
pub struct GroupLayout {
    /// Внешняя позиция окна (физ. пиксели десктопа).
    pub x: i32,
    pub y: i32,
    /// Внутренний размер (физ. пиксели).
    pub w: u32,
    pub h: u32,
    #[serde(default)]
    pub maximized: bool,
    /// macOS «на весь экран» (WindowBounds::Fullscreen). Отдельно от `maximized`:
    /// зелёная кнопка на macOS даёт Fullscreen, а не Maximized, и его надо
    /// восстанавливать своим вариантом, иначе окно откроется обычным.
    #[serde(default)]
    pub fullscreen: bool,
    #[serde(default)]
    pub collapsed: bool,
    /// Индекс активной вкладки дока (см. `DockTab::idx`).
    #[serde(default)]
    pub tab: u8,
    /// Высота развёрнутого дока (точки egui). 0 = не задано → дефолт.
    #[serde(default)]
    pub dock_h: f32,
    /// Сортировка ордеров: 0=по созданию, 1=Sell первые, 2=Buy первые.
    #[serde(default)]
    pub orders_primary: u8,
    /// Сортировка ордеров по времени: новые первыми.
    #[serde(default = "def_true")]
    pub orders_newest_first: bool,
    /// Фильтр ордеров «только текущий маркет».
    #[serde(default)]
    pub orders_only_current: bool,
    /// Фильтр типа ордеров: 0=все, 1=реальные, 2=эмуляторные.
    #[serde(default)]
    pub orders_kind: u8,
    /// UUID монитора окна (`PlatformDisplay::uuid`), строкой. На macOS координаты окна
    /// per-display-относительные — восстановить монитор по x/y нельзя, только по uuid;
    /// contains-детект по точке остаётся фолбэком для старых layout без поля.
    #[serde(default)]
    pub display_uuid: Option<String>,
}

fn def_true() -> bool {
    true
}

/// Visible-column masks of the Tuning strategy list, ONE PER AXIS.
///
/// The list stands beside a different tool in each mode, so it is asked a different question in
/// each: "By coin" wants the strategy's coin-list counts, the other two want the width those
/// columns take. Named fields rather than an array — the axes are an enum, and an index would
/// silently re-point every saved mask the day their order changes.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct StratColsByMode {
    pub filter: u16,
    pub coins: u16,
    pub time: u16,
}

impl Default for StratColsByMode {
    /// Zero is a legitimate mask ("no toggleable column"), so the absent-key default cannot be
    /// `0` — the UI substitutes its own defaults when the whole key is missing instead.
    fn default() -> Self {
        Self {
            filter: 0,
            coins: 0,
            time: 0,
        }
    }
}

/// Прямоугольник окна (внешняя позиция + внутренний размер, физ. пиксели).
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct GeomRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Одно откреплённое окно вкладки.
#[derive(Clone, Serialize, Deserialize)]
pub struct DetachedLayout {
    /// Индекс вкладки (см. `DockTab::idx`).
    pub tab: u8,
    /// Имя группы-владельца (для Orders — чьи ордера; для глобальных — откуда открыт).
    pub owner_group: String,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Полная раскладка окон.
///
/// Every field is `Option` or carries `#[serde(default)]` on purpose, and prefers a type wider
/// than its values need. This struct is deserialized as a WHOLE, so a single value that does not
/// fit its field's type fails the entire layout — and `load` below passes a no-op corruption
/// handler, so nothing quarantines the file and the first dirty save rewrites it with defaults.
/// One out-of-type integer therefore costs every window position, column width and detached
/// window slot in the file, permanently. Keep that in mind when adding a field.
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct WindowLayout {
    /// Окна групп по имени группы.
    #[serde(default)]
    pub groups: HashMap<String, GroupLayout>,
    /// Открытые откреплённые окна вкладок.
    #[serde(default)]
    pub detached: Vec<DetachedLayout>,
    /// Запомненная геометрия окон открепления по ключу (даже после закрытия) —
    /// чтобы повторное открепление той же вкладки вставало на прежнее место.
    /// Ключ: `g:<idx>` для глобальных, `o:<idx>:<группа>` для Orders (см. App).
    #[serde(default)]
    pub detached_geom: HashMap<String, GeomRect>,
    /// Геометрия окна «Стратегии» (отдельное окно) — чтобы открывалось на прежнем месте.
    #[serde(default)]
    pub strategies_window: Option<GeomRect>,
    /// Панели окна «Стратегии»: ширины колонок (лог. px, тянутся сплиттерами)
    /// и свёрнутость колонки «Версии» — как персист ширин колонок таблиц.
    #[serde(default)]
    pub strategies_panels: StrategiesPanels,
    /// Геометрия глобального окна «Активы» (singleton) — чтобы открывалось на прежнем месте.
    #[serde(default)]
    pub assets_window: Option<GeomRect>,
    /// Порог «скрывать активы дешевле N $» (слайдер верхней полосы «Активов»). Общий на все
    /// окна/вкладки «Активов» (один на всех — не плодим ключи на охват). `0` = показать всё.
    /// `None` (файл старой версии / поле не писалось) → дефолт 1$ на стороне панели.
    #[serde(default)]
    pub assets_min_value: Option<f64>,
    /// Геометрия окна «Настройки» (отдельное окно) — чтобы открывалось на прежнем месте.
    #[serde(default)]
    pub settings_window: Option<GeomRect>,
    /// Геометрия окна «Скринер» (singleton) — чтобы открывалось на прежнем месте.
    #[serde(default)]
    pub screener_window: Option<GeomRect>,
    /// Геометрия окна «Аналитика» (singleton) — чтобы открывалось на прежнем месте.
    #[serde(default)]
    pub analytics_window: Option<GeomRect>,
    /// Выбранный пресет периода «Аналитики» (id вида "p-cur-month") — окно
    /// открывается с прошлым выбором. None = дефолт («Тек. месяц»).
    #[serde(default)]
    pub analytics_period: Option<String>,
    /// Режим тепловой карты «Аналитики»: "year" (GitHub-обзор) / "month"
    /// (крупные карточки-дни). None = дефолт («Месяц»).
    #[serde(default)]
    pub analytics_heat_mode: Option<String>,
    /// Выбранный пресет периода вкладки «Тюнинг стратегий» — СВОЙ, независимый
    /// от «Сводки» (у каждой вкладки своё окно времени). None = дефолт.
    #[serde(default)]
    pub analytics_strat_period: Option<String>,
    /// Bitmask of the visible columns in the Tuning strategy list (the ▦ selector).
    /// None = default (all columns).
    ///
    /// Version 2 of the key. The bit layout is positional (metric columns sit at
    /// `2 + index`), so adding the coin-list columns MOVED every bit above them: a mask
    /// saved under the old layout would silently switch columns on and off rather than
    /// restore what the user chose. A new key is the honest migration — an old config
    /// still loads, and simply falls back to "all columns" once.
    ///
    /// Superseded by [`Self::analytics_strat_cols_modes`], which keeps one mask PER AXIS.
    /// Kept as its seed: a user who already picked their columns carries that pick into all
    /// three axes instead of being reset a second time.
    #[serde(default)]
    pub analytics_strat_cols2: Option<u16>,
    /// Restart count of the "By filter" tuner's threshold search. None = the tuner's default.
    /// Values from an externally edited file are clamped to the range owned by
    /// `db::tuner_smart` when the tuner loads.
    #[serde(default)]
    pub analytics_tuner_iters: Option<u32>,
    /// Quantile depth of the "By filter" tuner's threshold search. None or a value absent from
    /// the dropdown selects the tuner's default.
    #[serde(default)]
    pub analytics_tuner_edges: Option<u32>,
    /// Visible columns of the Tuning strategy list, per axis. None = the UI's own defaults.
    #[serde(default)]
    pub analytics_strat_cols_modes: Option<StratColsByMode>,
    /// Analytics: attribute LIQUIDATION trades to the strategy named in the row.
    ///
    /// Off by default. It moves money between strategies retroactively (measured: 291 of 319
    /// liquidations attach, −4582.89 USDT leaves "Manual"), so it is a decision the user makes
    /// rather than something that quietly changes their history on an update. The Report
    /// window deliberately does NOT follow it.
    #[serde(default)]
    pub analytics_attribute_liq: bool,
    /// The "closed trades the core never dated" banner: the count it was dismissed at.
    ///
    /// `None` — never dismissed, so it shows whenever there is anything to say. Otherwise it
    /// comes back only once MORE such trades appear: the same count is the same news, already
    /// read and put away.
    #[serde(default)]
    pub analytics_undated_hidden_n: Option<i64>,
    /// Видимые колонки скринера (ключи в каноничном порядке). None = все.
    #[serde(default)]
    pub screener_columns: Option<Vec<String>>,
    /// Тикер курса в шапке (слева после логотипа): выбранные ядро+рынок. `None` = дефолт
    /// (первое подключённое ядро; BTCUSDT, на Hyperliquid-подобных — UBTCUSDC).
    #[serde(default)]
    pub header_ticker: Option<HeaderTicker>,
    /// Часы в правом углу шапки: смещение отображаемого времени от UTC в минутах.
    /// 0 = UTC (дефолт → метка «(UTC)»). Если совпадает с системным поясом (отображаемое =
    /// системному времени) — метку пояса скрываем. Общее на все окна.
    #[serde(default)]
    pub header_clock_offset_min: i32,
    /// Отображение свечей/трейдов на чартах (ТФ, режим, зона трейдов, контур…) —
    /// ГЛОБАЛЬНЫЙ ДЕФОЛТ (вкладки могут переопределять в спеке charts.json).
    #[serde(default)]
    pub candle_view: crate::market::candles::CandleViewCfg,
    // Бывший `detect_view_by_group` переехал в отдельный `detects_view.toml`
    // (см. `detect_view::DetectViewFile`); старый ключ в layout.toml просто игнорируется.
    /// Временной X-масштаб чартов (px на мс) ПО ОКНАМ ГРУПП: [Shift+СКМ] на графике
    /// синхронизирует и сохраняет масштаб для чартов СВОЕГО окна; новые чарты окна
    /// наследуют. Нет записи = 60-секундный дефолт. Выносные окна хранят свой в
    /// спеке вкладки (charts.json).
    #[serde(default)]
    pub chart_x_ppm_by_group: HashMap<String, f32>,
    /// Универсальное сохранение ширин колонок таблиц: `id таблицы → (ключ колонки → ширина px)`.
    /// Любая `MoonDataTable` персистит сюда свои `column_widths` по стабильному id (`orders-table`
    /// и т.п.); при открытии панели ширины засеиваются обратно. Пусто = дефолтные ширины.
    #[serde(default)]
    pub table_column_widths: HashMap<String, HashMap<String, f32>>,
    /// Универсальное сохранение НАБОРА видимых колонок таблиц: `id таблицы (с контекстом
    /// `:dock`/`:win`) → список ключей видимых колонок в каноничном порядке`. Аналог
    /// `table_column_widths`, но для видимости полей — у докнутой вкладки и откреплённого окна
    /// свои наборы. Отсутствие записи = дефолт таблицы (обычно «все видимы»).
    #[serde(default)]
    pub table_visible_columns: HashMap<String, Vec<String>>,
    /// Индекс вкладки панели в её «домашней» tab-полосе на момент ОТКРЕПЛЕНИЯ — чтобы возврат в
    /// док встал НА ТО ЖЕ место, а не на каноничную priority-позицию. Ключ `группа:панель`
    /// (напр. `default:Orders`). Отсутствие → возврат по priority.
    #[serde(default)]
    pub dock_tab_index: HashMap<String, usize>,
    /// Имя ЛЕВОГО СОСЕДА панели во вкладочной полосе на момент ОТКРЕПЛЕНИЯ (пустая строка = панель
    /// была крайней слева). Возврат вставляет панель СРАЗУ ПОСЛЕ этого соседа в ЖИВОЙ полосе — так
    /// место не съезжает, даже если между откреплением и возвратом полоса менялась (сырой индекс
    /// [`Self::dock_tab_index`] в таком случае протухает). Ключ `группа:панель`. Фолбэк — индекс.
    #[serde(default)]
    pub dock_tab_left: HashMap<String, String>,
    /// Split-слот панели на момент ОТКРЕПЛЕНИЯ, если она стояла ОТДЕЛЬНЫМ листом в сплите (рядом
    /// с соседом, а не в общей линии вкладок). Открепление такой панели схлопывает сплит, поэтому
    /// возврат должен пере-создать сплит рядом с соседом. Ключ `группа:панель`. Взаимоисключающ с
    /// [`Self::dock_tab_index`] (панель либо в сплите, либо во вкладках).
    #[serde(default)]
    pub dock_split_slot: HashMap<String, DockSplitSlot>,
}

/// Запомненное split-размещение панели: в каком сплите (по соседям-якорям), на каком индексе, с
/// какой стороны и с какими размерами слотов она стояла — чтобы вернуть на ТО ЖЕ место и в
/// прежней пропорции (важно для сплитов из 3+ панелей).
#[derive(Clone, Serialize, Deserialize)]
pub struct DockSplitSlot {
    /// Все соседи по сплиту (кроме самой панели) — якоря для поиска нужного сплита при возврате;
    /// любой присутствующий в доке годится. В каноничном порядке сплита.
    #[serde(default)]
    pub siblings: Vec<String>,
    /// Панели СОСЕДНЕГО слота (рядом с которым стояла панель) — этот слот мог быть вложенным
    /// сплитом (столбцом), его оборачиваем целиком при пере-создании сплита. Пусто → как siblings.
    #[serde(default)]
    pub slot_panels: Vec<String>,
    /// Индекс панели в сплите на момент открепления — чтобы вставить обратно на то же место
    /// (клампится к числу слотов). Важно для сплитов 3+.
    #[serde(default)]
    pub index: usize,
    /// Сторона панели относительно соседа для СХЛОПНУТОГО сплита (2 панели): 0=Left, 1=Right,
    /// 2=Top, 3=Bottom (совпадает с `moon_ui::DockSplitPlacement`).
    pub placement: u8,
    /// Пиксельный размер слота ПАНЕЛИ вдоль оси сплита на момент открепления. 0.0 = flex (без
    /// фиксированного размера).
    #[serde(default)]
    pub size: f32,
    /// Пиксельный размер слота СОСЕДА вдоль оси сплита (для схлопнутого сплита). 0.0 = flex.
    #[serde(default)]
    pub sibling_size: f32,
}

/// Выбор источника тикера курса в шапке. Ядро храним по стабильному `uid` сервера
/// (переживает переупорядочивание конфига), рынок — каноничным именем ядра.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderTicker {
    pub core_uid: u64,
    pub market: String,
}

impl WindowLayout {
    /// Загрузить layout.toml. Нет файла → дефолт; битый → лог + дефолт.
    pub fn load() -> Self {
        super::toml_io::load_or_default(&paths::layout_path(), "layout.toml", |_| {})
    }

    /// Highest core uid this layout still references.
    ///
    /// Feeds the durable uid high-water mark: the header ticker is stored by uid, so reissuing
    /// one a saved layout still names would silently rebind that ticker to the new core.
    pub fn max_core_uid(&self) -> Option<u64> {
        self.header_ticker.as_ref().map(|t| t.core_uid)
    }

    /// Записать layout.toml (не фатально: при ошибке только лог).
    pub fn save(&self) {
        if let Err(e) = super::toml_io::save(&paths::layout_path(), self, "layout.toml") {
            log::warn!("{e:#}");
        }
    }
}
