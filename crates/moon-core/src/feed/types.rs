//! Доменные типы, которыми backend кормит UI. Не зависят от moonproto,
//! чтобы UI/render-слой ничего не знал о транспорте.

/// Сторона сделки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

/// Идентификатор биржи ядра — байт `ExchangeCode` из moonproto (спот/фьючи — РАЗНЫЕ
/// коды: Binance=3, FBinance=4, ByBit=7, FBybit=2 …). Ключ дедупа рыночных данных:
/// ядра с одинаковым `ExchangeId` видят идентичный рынок. Держим как голый байт,
/// чтобы доменные типы оставались независимыми от moonproto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExchangeId(pub u8);

/// Один тик (сделка) — семантическая точка графика.
#[derive(Debug, Clone, Copy)]
pub struct Tick {
    /// Unix-время в миллисекундах (из core: row.unix_millis()).
    pub time_ms: f64,
    pub price: f32,
    /// Абсолютный объём сделки в базовой валюте.
    pub qty: f32,
    pub side: Side,
}

/// Retained price-line source kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceLineKind {
    Last,
    Mark,
}

/// Точка retained price-line (LastPrice / MarkPrice), уже в unix ms.
#[derive(Debug, Clone, Copy)]
pub struct PricePoint {
    pub time_ms: f64,
    pub price: f32,
}

/// Уровень стакана.
#[derive(Debug, Clone, Copy)]
pub struct Level {
    pub price: f32,
    pub qty: f32,
}

/// Снимок верхушки стакана (bids/asks).
#[derive(Debug, Clone, Default)]
pub struct OrderBook {
    /// Биды — по убыванию цены.
    pub bids: Vec<Level>,
    /// Аски — по возрастанию цены.
    pub asks: Vec<Level>,
}

/// Точка серверной ордерной трассы для чарта, уже в unix ms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrderTracePoint {
    pub time_ms: f64,
    pub price: f32,
}

/// Серверная polyline-трасса buy/sell линии ордера. Moonproto остаётся внутри
/// feed-слоя; UI получает только доменную структуру.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OrderTrace {
    pub points: Vec<OrderTracePoint>,
    pub tmp_point: Option<OrderTracePoint>,
    pub stop_price: Option<f32>,
    pub stop_time_ms: Option<f64>,
}

/// Открытый ордер (для нижнего дока).
#[derive(Debug, Clone)]
pub struct OrderRow {
    /// Имя рынка (монета).
    pub market: String,
    /// true = Short, false = Long.
    pub is_short: bool,
    /// Размер входной ноги (buy для long / sell для short), в базовой валюте.
    pub size: f64,
    pub sl_on: bool,
    pub ts_on: bool,
    pub vstop_on: bool,
    /// Цена входа (buy_price).
    pub buy_price: f64,
    /// Цена продажи (sell_price); 0 = не выставлена.
    pub sell_price: f64,
    /// Время создания ордера, unix мс (начало линии). 0 = неизвестно.
    pub create_time_ms: f64,
    /// Текущая цена рынка (p_last).
    pub price: f32,
    /// Заполнение входной ноги, %.
    pub fill_pct: f32,
    /// Имя/тип стратегии ордера (вместо числового strat_id).
    pub strat: String,
    /// uid ордера (task id) — монотонен с созданием: больше = новее. Для сортировки
    /// «по созданию / новые-старые первые» в окне ордеров.
    pub uid: u64,
    /// Эмуляторный ордер (не реальный) — для фильтра и пометки «(E)».
    pub emulator: bool,
    /// Ордер терминальный (`job_is_done` у ядра) — исполнен/отменён, ждёт deferred-removal.
    /// АВТОРИТЕТНЫЙ флаг закрытия (как MoonBot `o.IsClosed`): стор помечает линию закрытой
    /// по нему СРАЗУ, пока ордер ещё в снимке, а не по исчезновению+грейс.
    pub job_is_done: bool,

    // --- Цены линий на чарте (категория C: горизонтали по цене) ---
    // Считаются в feed-слое (live.rs) из StopSettings/buy_price/market-liq: проценты
    // приводятся к абсолютной цене ТАМ, а рендер получает готовые цены и только
    // маппит их в пиксели через shader-uniform. `None` = линия не активна.
    /// Ордер ещё не исполнен (pending) — линию входа рисуем пунктиром.
    pub pending: bool,
    /// Входная нога исполнена (позиция открыта) — гейт для стоп/трейлинг/liq линий.
    pub filled: bool,
    /// Стоп-лосс (абсолютная цена).
    pub stop_loss: Option<f64>,
    /// Трейлинг-стоп (абсолютная цена; для %-режима — оценка от входа).
    pub trailing: Option<f64>,
    /// Тейк-профит (абсолютная цена).
    pub take_profit: Option<f64>,
    /// VStop (абсолютная цена уровня).
    pub vstop: Option<f64>,
    /// Цена условия pending-ордера (BuyCondPrice).
    pub pending_cond: Option<f64>,
    /// Цена ликвидации позиции (из рынка, по стороне).
    pub liq: Option<f64>,
    /// Локальный/серверный PanicSell флаг.
    pub panic_sell: bool,
    /// Moon-shot corridor active marker.
    pub is_moon_shot: bool,
    /// Corridor price band from server, 0/NaN means absent.
    pub corridor_price_down: f32,
    pub corridor_price_up: f32,
    /// Серверная трасса buy-линии (если ядро её уже построило).
    pub buy_trace: Option<OrderTrace>,
    /// Серверная трасса sell-линии (если ядро её уже построило).
    pub sell_trace: Option<OrderTrace>,
}

/// Один детект ядра (для тулбара/истории). Декаплено от moonproto.
#[derive(Debug, Clone)]
pub struct DetectRow {
    /// Монотонный per-core номер (курсор ингеста в ленту детектов).
    pub seq: u64,
    /// Рынок (монета).
    pub market: String,
    /// Unix-время приёма, мс.
    pub time_ms: f64,
    /// У стратегии-источника включён звук-алерт (SoundAlert=Yes) — только такие
    /// детекты показываем кнопкой в ленте.
    pub sound_alert: bool,
    /// Сколько секунд держать кнопку (KeepAlert стратегии; дефолт 60).
    pub keep_alert_secs: u32,
    /// AddToChart у стратегии — НОМЕР чарта-вкладки (1,2,3…), куда авто-добавить
    /// график монеты. 0 = не добавлять (обычный детект-кнопка в ленте).
    pub add_to_chart: u32,
    /// KeepInChart, сек — сколько держать авто-график монеты во вкладке, прежде
    /// чем закрыть (вкладка остаётся). Дефолт 60.
    pub keep_in_chart_secs: u32,
}

/// Одна строка серверного лога ядра (`Event::ServerLog`). Декаплено от moonproto.
#[derive(Debug, Clone)]
pub struct CoreLogLine {
    /// Unix-время строки, мс (из `ServerLogEvent::unix_millis`).
    pub time_ms: i64,
    /// Локальное время терминала, когда feed-поток принял эту строку, мс unix.
    pub recv_ms: i64,
    pub msg: String,
}

/// Одна стратегия ядра (для окна стратегий). Декаплено от moonproto.
#[derive(Debug, Clone)]
pub struct StrategyRow {
    pub id: u64,
    /// Имя стратегии (StrategyName) или fallback.
    pub name: String,
    /// Тип (вид) стратегии — человекочитаемо.
    pub kind: String,
    /// Ordinal вида (для связи со схемой при показе секций/полей).
    pub kind_ordinal: u8,
    /// Путь папки в дереве стратегий (например "test cpu/20").
    pub folder_path: String,
    /// Отмечена (checked) = запущена.
    pub checked: bool,
    pub is_short: bool,
    /// Значения полей стратегии (имя → форматированная строка) для read-only плашек.
    pub fields: Vec<(String, String)>,
}

/// Вид виджета поля схемы (из moonproto `StrategyFieldUiKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaFieldUi {
    Edit,
    Checkbox,
    Combo,
    Color,
}

/// Описание одного поля схемы стратегий (декаплено от moonproto).
#[derive(Debug, Clone)]
pub struct SchemaField {
    pub name: String,
    /// Имя типа ("Bool"/"Int32"/"Double"/"String"/…) из схемы ядра. Используется в UI,
    /// чтобы числовые поля не рисовались как многострочный memo (см. `is_memo_field`).
    pub type_name: String,
    pub ui: SchemaFieldUi,
    /// Статический список значений (для Combo).
    /// Пока не читается: нужно этапу полного редактирования полей стратегий.
    #[allow(dead_code)]
    pub picklist: Vec<String>,
    /// Значение по умолчанию (форматированное), если есть в схеме.
    pub default: Option<String>,
}

/// Секция (раздел) полей одного вида стратегии (main/filters/…).
#[derive(Debug, Clone)]
pub struct SchemaSection {
    pub title: String,
    pub fields: Vec<SchemaField>,
}

/// Схема одного вида стратегии: его секции.
#[derive(Debug, Clone)]
pub struct SchemaKind {
    pub ordinal: u8,
    /// Имя вида из схемы ядра (авторитетнее хардкода strat_kind_name).
    /// Пока не читается: нужно этапу полного редактирования полей стратегий.
    #[allow(dead_code)]
    pub name: String,
    pub sections: Vec<SchemaSection>,
}

/// Полная схема стратегий ядра (все виды). Шлётся при смене revision схемы.
#[derive(Debug, Clone, Default)]
pub struct StrategySchemaModel {
    pub kinds: Vec<SchemaKind>,
}

/// Кошелёк биржи (для дерева переноса активов). Зеркало moonproto `ExchangeKind`
/// (Spot=0/Futures=1/Quarterly=2), но декаплено — UI/стор не зависят от moonproto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WalletKind {
    Spot,
    Futures,
    Quarterly,
}

impl WalletKind {
    /// Все кошельки в порядке отображения (как ветки дерева).
    pub const ALL: [WalletKind; 3] = [WalletKind::Spot, WalletKind::Futures, WalletKind::Quarterly];

    /// Человекочитаемое имя ветки.
    pub fn label(self) -> &'static str {
        match self {
            WalletKind::Spot => "Спот",
            WalletKind::Futures => "Фьючерсы",
            WalletKind::Quarterly => "Квартальные",
        }
    }

    /// Стабильный код для персиста (раскрытые ветки/выбор).
    pub fn to_u8(self) -> u8 {
        match self {
            WalletKind::Spot => 0,
            WalletKind::Futures => 1,
            WalletKind::Quarterly => 2,
        }
    }

    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => WalletKind::Futures,
            2 => WalletKind::Quarterly,
            _ => WalletKind::Spot,
        }
    }
}

/// Один актив/позиция ядра по рынку (для окна «Активы»). Декаплено от moonproto.
/// USDT-нормализация стоимости и отсечение пыли делаются на стороне UI — в сторе
/// держим полные данные.
#[derive(Debug, Clone)]
pub struct AssetRow {
    /// Имя рынка ядра, напр. "ADAUSDT".
    pub market: String,
    /// Базовая монета (актив), напр. "ADA".
    pub coin: String,
    /// Котировочная валюта рынка, напр. "USDT"/"BTC".
    pub quote: String,
    /// ListedType рынка: 0 unknown / 1 spot / 2 futures / 3 both.
    pub listed: u8,
    /// Баланс актива (asset_balance), в базовой монете.
    pub qty: f64,
    /// Полный баланс актива (asset_balance_full), в базовой монете.
    pub qty_full: f64,
    /// Текущая цена рынка (p_last, в quote).
    pub price: f64,
    /// Текущая стоимость баланса монеты в USDT (qty * price * курс quote/USDT).
    /// Считается на ядре через `base_currency_price`. 0 = курс неизвестен.
    pub value_usdt: f64,
    /// Mark-цена (для фьюч; 0 если нет).
    pub mark_price: f64,
    /// Размер позиции (pos_size).
    pub pos_size: f64,
    /// Цена позиции (pos_price).
    pub pos_price: f64,
    /// Цена ликвидации позиции (liq_price; 0 если нет).
    pub liq_price: f64,
    /// Плечо рынка на ЭТОМ ядре (`Market.leverage_x`). Per-core account-поле — показывается
    /// в тулбаре (Lev зависит от ядра и монеты). 0 = неизвестно.
    pub leverage: i32,
    /// Профит позиции: b (баланс) / l (long) / s (short) — как в ядре.
    pub profit_b: f64,
    pub profit_l: f64,
    pub profit_s: f64,
}

/// Account-итоги ядра (`GlobalBalance`). Декаплено от moonproto.
#[derive(Debug, Clone, Default)]
pub struct GlobalBalanceRow {
    /// BTC-эквивалент: доступно / заблокировано / полный (с нереализ. PnL).
    pub btc_total: f64,
    pub btc_locked: f64,
    pub btc_full: f64,
    /// special_coin_balance (USDT для фьюч, BUSD/USDC в MA-режиме и т.п.).
    pub special_coin: f64,
    /// Суммарный PnL ядра в БАЗОВОЙ валюте (серверный `total_pnl` = MoonBot RecalcTotalPnl:
    /// сумма `total_profit` ТОЛЬКО по рынкам базовой валюты `is_btc_market`). Это «реальный»
    /// PnL ядра — не равен сумме `profit_*` по всем строкам таблицы (там мешаются котировки).
    pub total_pnl: f64,
    /// Свободный баланс аккаунта в USDT (btc_balance_total × курс базовой валюты→USDT).
    /// Считается на ядре с УЧЁТОМ базовой валюты (для USDT-бота `btc_balance_*` уже в USDT,
    /// курс=1; для BTC-бота — ×BTCUSDT). 0 = курс неизвестен.
    pub free_usdt: f64,
    /// Итоговый баланс аккаунта в USDT (btc_balance_full × курс, с нереализ. PnL).
    pub total_usdt: f64,
    /// Серверный PnL ядра (`total_pnl`), пересчитанный в USDT той же базовой ставкой, что
    /// `free_usdt`/`total_usdt`. Это значение шапки «PnL» — берём с сервера, не суммируем сами.
    pub pnl_usdt: f64,
}

/// Снимок активов ядра (для окна «Активы»). Декаплено от moonproto.
#[derive(Debug, Clone, Default)]
pub struct AssetsSnapshot {
    pub rows: Vec<AssetRow>,
    pub global: GlobalBalanceRow,
    /// Плечо по рынку (`leverage_x`) — per-core, для ЛЮБОГО отслеживаемого рынка (не только с
    /// позицией): тулбар Lev читает её для монеты main-чарта. Не включаем рынки без account-
    /// данных (там ядро сбрасывает leverage_x в 1) — их плечо неизвестно, показываем «—».
    pub leverage: std::collections::HashMap<String, i32>,
}

/// Один transfer-актив кошелька (для дерева переноса). Декаплено от moonproto.
#[derive(Debug, Clone)]
pub struct TransferAssetRow {
    /// Валюта/монета, напр. "USDT"/"BTC".
    pub currency: String,
    /// Доступно к переносу (биржа).
    pub amount: f64,
    /// Всего на кошельке.
    pub total: f64,
    /// Стоимость `total` в USDT (через рынок `<currency>USDT`). 0 = курс неизвестен.
    pub value_usdt: f64,
}

/// Снимок transfer-активов ядра по кошелькам (Spot/Futures/Quarterly). Источник
/// дерева переноса; обновляется по запросу (`refresh_transfer_assets`).
#[derive(Debug, Clone, Default)]
pub struct TransferAssetsSnapshot {
    pub spot: Vec<TransferAssetRow>,
    pub futures: Vec<TransferAssetRow>,
    pub quarterly: Vec<TransferAssetRow>,
}

impl TransferAssetsSnapshot {
    /// Активы выбранного кошелька (ветки дерева).
    pub fn wallet(&self, kind: WalletKind) -> &[TransferAssetRow] {
        match kind {
            WalletKind::Spot => &self.spot,
            WalletKind::Futures => &self.futures,
            WalletKind::Quarterly => &self.quarterly,
        }
    }
}

/// License/module/MoonCredits state of one MoonBot core.
/// Декаплено от moonproto: UI видит только готовый аккаунтный snapshot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LicenseState {
    pub paid_version: bool,
    pub reg_id: i32,
    pub moon_credits: i32,
    pub moon_credits_hold: i32,
    pub moon_credits_auction: i32,
    pub can_use_watcher: bool,
}

/// Снимок настроек клиента ядра (moonproto `ClientSettings`) — плоская проекция для UI
/// (TP/SL/sell-пресеты в тулбаре). Декаплено от moonproto: raw-поля `s_price`/`sb_num`/…
/// в проде `pub(crate)`, поэтому читаем их ТОЛЬКО через публичные хелперы команды.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientSettings {
    /// Эффективный тейк-профит, % (`effective_take_profit_percent`).
    pub take_profit_pct: f64,
    /// Расширенный диапазон TP (флаг `x_tmode`, «s9»): off = 0..100%, on = 100..900%
    /// (на проводе хранится как `x_sell` ×10). Определяет диапазон слайдера и галку в попапе.
    pub take_profit_extended: bool,
    /// Режим fixed-sell включён.
    pub fixed_sell_mode: bool,
    /// Stop-loss / price-drop level, % (`price_drop_level`).
    pub stop_loss_pct: f32,
    /// Трейлинг-стоп, % (`trailing_drop`).
    pub trailing_drop_pct: f32,
    /// Глобальный тейк-профит включён (`use_g_take_profit`) + значение, % (`g_take_profit`).
    pub use_global_take_profit: bool,
    pub global_take_profit_pct: f64,
    /// Паника при падении цены (`panic_if_price_drop`).
    pub panic_if_price_drop: bool,
    /// Режим эмулятора (`emu_mode`).
    pub emu_mode: bool,
    pub buy_iceberg: bool,
    pub sell_iceberg: bool,
    pub sign_orders: bool,
    pub use_stop_market: bool,
    /// 6 fixed-sell пресетов как видимые проценты (кнопки S1-S6).
    pub fixed_sell_pcts: [f64; 6],
    /// Выбранный fixed-sell слот, 1..=6 (`selected_fixed_sell_slot`).
    pub fixed_sell_slot: usize,
}

/// Настройки управления плечом ядра (moonproto `LevManage`). Отдельный снимок, как в MoonBot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LevManageState {
    pub auto_max_order: bool,
    pub auto_lev_up: bool,
    pub auto_isolated: bool,
    pub auto_cross: bool,
    pub auto_fix_lev: bool,
    pub fix_lev: i32,
    pub tlg_report: bool,
    pub lev_control: String,
}

/// Runtime-состояние ядра (moonproto `RuntimeState`): запущен ли рынок-рантайм и активна
/// ли авто-детекция (false = passive mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RuntimeState {
    pub is_started: bool,
    pub auto_detect_active: bool,
}

/// Точечная правка `ClientSettings` из тулбара. Применяется на фид-стороне к УДЕРЖАННОМУ
/// moonproto-снимку (`client.snapshot().settings().client_settings`) через его хелперы —
/// так сохраняются append-only tail/AutoStart-blob'ы, которые UI не видит. Затем снимок
/// целиком уходит обратно в ядро (`settings().send`).
#[derive(Debug, Clone, Copy)]
pub enum ClientSettingsEdit {
    /// Главный тейк-профит, % + режим расширенного диапазона (`x_tmode`/«s9»). При
    /// `extended` пишем `x_tmode=true`, `x_sell=round(pct/10)` (100..900%); иначе
    /// `x_tmode=false`, `x_sell=round(pct)` (1..100%). Снимает fixed-sell/scalp.
    TakeProfit { pct: f64, extended: bool },
    /// Stop-loss / price-drop level, % (знаковый: -20..+1 на ядре).
    StopLossPct(f32),
    /// Скальп-тейк (суб-процентный TP через `x_sell_scalp`, x_sell=0): файн-слайдер TP.
    /// На ядре шаг реально 1/50 = 0.02%. Снимает fixed-sell.
    ScalpTakeProfit(f64),
    /// Выбрать fixed-sell слот 1..=6 (клик по S1-S6).
    SelectFixedSellSlot(usize),
    /// Значение fixed-sell пресета: слот 1..=6, видимый процент (колесо/инлайн-правка S-кнопки).
    SetFixedSellPct { slot: usize, pct: f64 },
}

/// Точечная правка управления плечом (moonproto `LevManage`). Применяется к удержанному
/// снимку и уходит через `settings().manage_leverage`.
#[derive(Debug, Clone, Copy)]
pub enum LevManageEdit {
    /// Зафиксировать целевое плечо: `auto_fix_lev=true` + `fix_lev=n`.
    FixLev(i32),
}

/// Статус соединения с ядром.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnStatus {
    Connecting,
    /// Промежуточная стадия подключения/инициализации (текст для бейджа).
    Stage(String),
    Ready,
    Failed(String),
    Disconnected,
}

/// Market-data domains that can wake a visible chart.
///
/// The payload is intentionally small: data rows stay in MoonProto/MarketStore,
/// while the terminal keeps causal per-market revisions and pulls only visible
/// chart targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketDirtyFlags(u8);

impl MarketDirtyFlags {
    pub const HISTORY: Self = Self(1 << 0);
    pub const ORDERBOOK: Self = Self(1 << 1);
    pub const MARKET_META: Self = Self(1 << 2);
    pub const ALL: Self = Self(Self::HISTORY.0 | Self::ORDERBOOK.0 | Self::MARKET_META.0);

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl std::ops::BitOr for MarketDirtyFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.union(rhs)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketDirty {
    pub market: String,
    pub flags: MarketDirtyFlags,
}

impl MarketDirty {
    pub fn new(market: impl Into<String>, flags: MarketDirtyFlags) -> Self {
        Self {
            market: market.into(),
            flags,
        }
    }
}

/// Сообщение от backend к UI.
///
/// Аккаунтные сообщения (Status/Orders/Detects/Strategies) несут готовый UI state
/// конкретного ядра. Рыночные тики/стакан/price-lines через этот канал не едут:
/// feed thread публикует их в MoonProto/MarketStore и шлёт только лёгкий
/// [`MarketDataChanged`] wake для consumer-side pull.
#[derive(Debug, Clone)]
pub enum FeedMsg {
    Status(ConnStatus),
    /// Биржа ядра (из server_info после BaseCheck). Шлётся один раз.
    Identity(ExchangeId),
    /// Базовая валюта аккаунта ядра ("USDT"/"BTC"/…) из `server_info`. Шлётся один раз
    /// (рядом с `Identity`). Нужна UI для дефолтов размера ордера по базе (BTC vs USDT).
    CoreBase {
        base: String,
    },
    /// Рыночный read-model изменился. Это лёгкий пинок consumer-side pull:
    /// `SessionManager` отмечает dirty конкретных рынков, а видимые графики
    /// сами подтягивают нужный snapshot. Сами тики/стакан через UI-channel не едут.
    MarketDataChanged(Vec<MarketDirty>),
    /// Открытые ордера ядра (все рынки).
    Orders(Vec<OrderRow>),
    /// Быстрый снимок ордеров только для chart/order-line стора. Таблица Orders
    /// остаётся загейтена `Orders`, но график не должен терять короткий terminal
    /// status между `OrderEvent::Updated` и deferred-removal.
    OrderLines(Vec<OrderRow>),
    /// Пачка новых детектов (накопленных за тик дренажа событий).
    Detects(Vec<DetectRow>),
    /// Пачка новых строк серверного лога ядра (за тик дренажа событий).
    ServerLog(Vec<CoreLogLine>),
    /// Снимок стратегий ядра (шлётся при изменении сигнатуры).
    Strategies(Vec<StrategyRow>),
    /// Схема стратегий ядра (секции/поля по видам). Шлётся при смене revision.
    StrategySchema(StrategySchemaModel),
    /// Снимок активов/позиций ядра (для окна «Активы»). Шлётся ~1 Гц по событию.
    Assets(AssetsSnapshot),
    /// Снимок transfer-активов ядра по кошелькам (для дерева переноса). Шлётся при
    /// смене revision (обновляется по запросу `RefreshTransferAssets`).
    TransferAssets(TransferAssetsSnapshot),
    /// License/Free-PRO/MoonCredits state ядра.
    License(LicenseState),
    /// Снимок настроек клиента ядра (TP/SL/sell/iceberg/…). Шлётся при `ClientSettingsUpdated`.
    ClientSettings(ClientSettings),
    /// Снимок управления плечом ядра. Шлётся при `LevManageUpdated`.
    LevManage(LevManageState),
    /// Runtime/passive-mode state ядра. Шлётся при `RuntimeStateUpdated`.
    RuntimeState(RuntimeState),
    /// Hedge-mode аккаунта ядра (dual-side позиции вкл/выкл). Шлётся при `HedgeModeUpdated`.
    HedgeMode(bool),
}
