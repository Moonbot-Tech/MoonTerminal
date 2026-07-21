//! Панель «Ордера» — таблица открытых ордеров группы (все ядра), на всю ширину.
//! Колонки как в оригинале (egui), отрисованные в стиле MoonPalette:
//! Core · Side · Token · Size · SL · TS · Vstop · Buy · Cur.P · Fill · Strat.
//!
//! Сторона: BUY (лонг, ждёт) — зелёным, SHORT (шорт, ждёт) — красным, SELL
//! (исполнился — позиция открыта/продаётся) — синим. Эмуляторный — «(E)».
//! SL/TS/Vstop — флаги ON (зелёным) / OFF (тускло).
//!
//! По функционалу разнесено: состояние/вид/жизненный цикл — здесь, поля-списки и
//! меню сортировки — [`controls`], таблица/колонки/ячейки — [`table`].

mod controls;
mod table;

use std::collections::HashSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::*;
use moon_ui::{
    DockArea, MoonButtonSize, MoonButtonVariant, MoonDataCell, MoonDataRow, MoonDataTable,
    MoonDataTableColumn, MoonDataTableState, MoonDropdown, MoonMenuItem, MoonMenuSize, MoonPalette,
    MoonTone, Panel, PanelEvent, PanelInfo, PanelState, h_flex, v_flex,
};

use rust_i18n::t;

use crate::Backend;
use crate::core_order::{CoreOrder, OrderedCores};
use crate::design;
use crate::panels::{RenderGate, num};
use moon_core::feed::OrderRow;
use moon_core::session::CoreId;
// Не используется в этом файле, но table.rs берёт через `use super::*`.
use moon_core::symbol;

/// Одна строка таблицы ордеров с привязкой к ядру-источнику (порт `OrderEntry`).
/// Квоты рынка тут нет: ячейка токена режет её `symbol`-хелперами по имени рынка.
#[derive(Clone)]
pub(super) struct OrderEntry {
    pub(super) core: CoreId,
    pub(super) core_name: String,
    pub(super) row: OrderRow,
}

#[derive(Clone, PartialEq, Eq)]
struct OrdersCacheKey {
    data_sig: u64,
    view: OrdersViewState,
    /// Выбранные ядра фильтра (отсортировано) — их смена меняет набор строк.
    sel_cores: Vec<CoreId>,
    current: Option<(CoreId, String)>,
    /// Монеты, открытые в Main группы — их изменение меняет подсветку и порядок строк.
    main_open: Vec<(CoreId, String)>,
}

/// Первичный ключ сортировки (тогл-группа в меню).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PrimarySort {
    SellFirst,
    BuyFirst,
    Creation,
    /// Прибыльные первыми: сортировка по локальному PnL по убыванию (+500 → −20).
    ProfitFirst,
}

impl PrimarySort {
    /// Стабильный код для персиста (docks.json) — порт egui `to_u8`/`from_u8`.
    fn to_u8(self) -> u8 {
        match self {
            PrimarySort::Creation => 0,
            PrimarySort::SellFirst => 1,
            PrimarySort::BuyFirst => 2,
            PrimarySort::ProfitFirst => 3,
        }
    }
    fn from_u8(v: u8) -> Self {
        match v {
            1 => PrimarySort::SellFirst,
            2 => PrimarySort::BuyFirst,
            3 => PrimarySort::ProfitFirst,
            _ => PrimarySort::Creation,
        }
    }
}

/// Подъём строк, открытых на Main, наверх списка (per-окно, сохраняется). Две взаимоисключающие
/// галки в меню сортировки + возможность выключить.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MainOnTop {
    /// Не поднимать (обычная сортировка).
    Off,
    /// Весь тикер по всем ядрам (если монета на Main — наверх идут все её ордера всех ядер).
    AllTicker,
    /// Только выделенные строки — по одной на каждую (монета+ядро), что на Main.
    Highlighted,
}

impl MainOnTop {
    fn to_u8(self) -> u8 {
        match self {
            MainOnTop::Off => 0,
            MainOnTop::AllTicker => 1,
            MainOnTop::Highlighted => 2,
        }
    }
    fn from_u8(v: u8) -> Self {
        match v {
            1 => MainOnTop::AllTicker,
            2 => MainOnTop::Highlighted,
            _ => MainOnTop::Off,
        }
    }
}

/// Фильтр по типу ордера: все / реальные / эмуляторные.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum OrderKind {
    All,
    Real,
    Emu,
}

impl OrderKind {
    /// Стабильный код для персиста (docks.json) — порт egui `to_u8`/`from_u8`.
    fn to_u8(self) -> u8 {
        match self {
            OrderKind::All => 0,
            OrderKind::Real => 1,
            OrderKind::Emu => 2,
        }
    }
    fn from_u8(v: u8) -> Self {
        match v {
            1 => OrderKind::Real,
            2 => OrderKind::Emu,
            _ => OrderKind::All,
        }
    }
}

/// Колонки таблицы ордеров в каноничном порядке. Позиция в [`OrdCol::ALL`] = номер бита
/// в маске видимости [`OrdersViewState::columns`]; строковый [`OrdCol::key`] — стабильный
/// идентификатор для персиста (docks.json), НЕ завязан на порядок enum.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum OrdCol {
    Core,
    Side,
    Token,
    Size,
    Buy,
    CurP,
    /// Цена тейк-профита (целевая цена выходной ноги, `sell_price`).
    TpPrice,
    Fill,
    Pnl,
    PnlPct,
    /// PnL, если позиция закроется по цене тейка (`(tp − entry)·qty·dir`).
    PnlTp,
    Sl,
    Ts,
    Vstop,
    Strat,
}

impl OrdCol {
    // Стопы (SL/TS/Vstop) стоят СПРАВА, рядом со «Strat» (просьба пользователя: важные
    // цена/PnL — под рукой, стопы у стратегии). Цена TP — рядом с ценами (Buy/Cur.P),
    // PNL TP — рядом с PnL/PnL%.
    pub(super) const ALL: [OrdCol; 15] = [
        OrdCol::Core,
        OrdCol::Side,
        OrdCol::Token,
        OrdCol::Size,
        OrdCol::Buy,
        OrdCol::CurP,
        OrdCol::TpPrice,
        OrdCol::Fill,
        OrdCol::Pnl,
        OrdCol::PnlPct,
        OrdCol::PnlTp,
        OrdCol::Sl,
        OrdCol::Ts,
        OrdCol::Vstop,
        OrdCol::Strat,
    ];

    /// Стабильный ключ для персиста (docks.json) и ключей элементов меню.
    pub(super) fn key(self) -> &'static str {
        match self {
            OrdCol::Core => "core",
            OrdCol::Side => "side",
            OrdCol::Token => "token",
            OrdCol::Size => "size",
            OrdCol::Sl => "sl",
            OrdCol::Ts => "ts",
            OrdCol::Vstop => "vstop",
            OrdCol::Buy => "buy",
            OrdCol::CurP => "cur.p",
            OrdCol::TpPrice => "tp.p",
            OrdCol::Fill => "fill",
            OrdCol::Pnl => "pnl",
            OrdCol::PnlPct => "pnl.pct",
            OrdCol::PnlTp => "pnl.tp",
            OrdCol::Strat => "strat",
        }
    }

    /// Бит колонки в маске видимости (по позиции в `ALL`).
    pub(super) fn bit(self) -> u16 {
        let idx = OrdCol::ALL
            .iter()
            .position(|c| *c == self)
            .unwrap_or_default();
        1u16 << idx
    }

    fn from_key(key: &str) -> Option<OrdCol> {
        OrdCol::ALL.iter().copied().find(|c| c.key() == key)
    }
}

/// Маска «все колонки видимы» — дефолт вида.
pub(super) const ALL_COLUMNS_MASK: u16 = (1u16 << OrdCol::ALL.len()) - 1;

/// Состояние вида таблицы (источник + тип + фильтр + сортировка + видимые колонки). Своё у панели.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct OrdersViewState {
    pub(super) kind: OrderKind,
    pub(super) only_current_market: bool,
    pub(super) primary: PrimarySort,
    pub(super) newest_first: bool,
    /// Подъём открытых на Main строк наверх (выкл / весь тикер / только выделенные).
    pub(super) main_on_top: MainOnTop,
    /// Битовая маска видимых колонок (бит = `OrdCol::bit`). Персистится списком ключей.
    pub(super) columns: u16,
}

impl OrdersViewState {
    /// Видима ли колонка в текущем виде.
    pub(super) fn shows(&self, col: OrdCol) -> bool {
        self.columns & col.bit() != 0
    }

    /// Видимые колонки в каноничном порядке.
    pub(super) fn visible_columns(&self) -> Vec<OrdCol> {
        OrdCol::ALL
            .iter()
            .copied()
            .filter(|c| self.shows(*c))
            .collect()
    }
}

impl Default for OrdersViewState {
    fn default() -> Self {
        Self {
            // По умолчанию — только РЕАЛЬНЫЕ ордера (без эмулятора) и SELL (позиции в работе)
            // сверху: первым делом видно, что открыто/продаётся. Пользователь может сменить
            // в меню сортировки/типа — выбор персистится в docks.json.
            kind: OrderKind::Real,
            only_current_market: false,
            primary: PrimarySort::SellFirst,
            newest_first: true,
            main_on_top: MainOnTop::Highlighted,
            columns: ALL_COLUMNS_MASK,
        }
    }
}

/// Вход исполнен (позиция открыта) — по АВТОРИТЕТНОМУ статусу воркера, а не по `fill_pct`
/// ноги. Стейт-машина фазовая для обоих направлений: `None`/`BuySet` = вход ещё ждёт;
/// `BuyDone` (вход залился) и любая `Sell*` фаза (выход выставлен/идёт/закрыт) = в позиции.
/// (Раньше брали `fill_pct` входной ноги, но для шорта она читалась из пустого `sell_order`
/// → шорт навсегда висел как `Short-S`.)
pub(super) fn executed(r: &OrderRow) -> bool {
    matches!(
        r.status.as_str(),
        "BuyDone" | "SellSet" | "SellDone" | "SellFail" | "SellCancel" | "SellAlmostDone"
    )
}
/// «Позиция в работе» (вход исполнен) — лонг ИЛИ шорт. Для сортировки SellFirst.
pub(super) fn is_sell(r: &OrderRow) -> bool {
    executed(r)
}
/// BUY — лонг, ещё не исполнен (ждёт покупки/входа). Только pending-вход лонга.
pub(super) fn is_buy(r: &OrderRow) -> bool {
    !r.is_short && !executed(r)
}

/// Панель «Ордера».
pub struct OrdersPanel {
    pub(super) backend: Entity<Backend>,
    pub(super) group: String,
    pub(super) view: OrdersViewState,
    /// Выбранные ядра фильтра (мультивыбор, как в «Отчёте»). Пусто = все ядра группы.
    pub(super) sel_cores: HashSet<CoreId>,
    /// Гейт перерисовки: ордерные ивенты летят часто, цены/P&L живут от рынка — общий
    /// `RenderGate` (сигнатура ИЛИ 1 Гц-тик, пол 250мс) экономит UI-поток на холостом ходу.
    gate: RenderGate,
    cache_key: Option<OrdersCacheKey>,
    cached_entries: Rc<Vec<OrderEntry>>,
    /// Retained-стейт таблицы (порядок/ширины колонок). Владеем сами — иначе порядок
    /// жил бы в анонимном `use_keyed_state` окна и его нельзя было бы ни засеять из
    /// `docks.json`, ни прочитать для персиста при drag-перестановке заголовков.
    table_state: Entity<MoonDataTableState>,
    /// Последний персистнутый порядок колонок — чтобы `observe` не дампил док на каждый
    /// `notify` стейта (выделение/ресайз), а только когда порядок реально сменился.
    col_order_cache: Vec<SharedString>,
    /// Id хранилища ширин колонок с контекстом (`orders-table:dock`/`:win`). В доке = `:dock`,
    /// в откреплённом окне переключается на `:win` (`mark_table_detached`) — свои ширины на режим.
    widths_id: String,
    /// Монеты, открытые в стеке Main этой группы (`(ядро, рынок)`). Используется для сортировки
    /// (поднять наверх). Обновляется в `rebuild_cache`.
    main_open: Rc<HashSet<(CoreId, String)>>,
    /// `(ядро, uid)` ПЕРВОГО ордера каждой Main-открытой пары — ровно эти строки подсвечиваем
    /// (по одной на пару, а не все ордера монеты). Обновляется в `rebuild_cache`.
    highlight: Rc<HashSet<(CoreId, u64)>>,
    /// Оптимистичная отрисовка тоглов SL/TS/Vstop: клик СРАЗУ рисует целевое состояние, не
    /// дожидаясь пересборки строк от feed (та едет по ордер-событиям). Ключ (ядро, uid,
    /// stop_tag) → (целевой флаг, момент клика). Запись живёт ≤3с — дальше истина сервера
    /// (строки) главнее; сходятся обычно за тик, т.к. feed держит одноимённый override.
    pub(super) stop_overlay: std::collections::HashMap<(CoreId, u64, u8), (bool, Instant)>,
    /// Счётчики для нижнего футера: реальные/эмуляторные ордера ДО фильтра типа (но после
    /// фильтра ядра/текущего маркета). Обновляются в [`Self::rebuild_cache`].
    count_real: usize,
    count_emu: usize,
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl OrdersPanel {
    pub fn new(
        backend: Entity<Backend>,
        group: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Перерисовка по дренажу backend — ТОЛЬКО когда реально изменились ордера.
        cx.observe(&backend, |this, backend, cx| {
            crate::diag::bump(&crate::diag::ORDERS_OBS_FIRE);
            let now = moon_chart::paint::now_unix_ms();
            let b = backend.read(cx);
            let key = this.cache_key(b);
            let changed = this.cache_key.as_ref() != Some(&key);
            let due = this.gate.should_notify(key.data_sig, now);
            if changed || due {
                this.rebuild_cache(b);
                crate::diag::bump(&crate::diag::ORDERS_OBS_NOTIFY);
                cx.notify();
            }
        })
        .detach();
        // Перестановка/ресайз колонок мутирует `table_state` и шлёт `notify`. Ловим его и,
        // если СПИСОК ПОРЯДКА сменился, дампим док (персист в `docks.json`). Дамп читает эту
        // же панель → откладываем через `cx.defer`, вне текущего borrow (как в `mutate`).
        let widths_id = crate::table_persist::ctx_id("orders-table", false);
        let saved_widths = crate::table_persist::saved(backend.read(cx), &widths_id);
        let table_state = cx.new(|_| {
            let mut s = MoonDataTableState::new();
            s.column_widths = saved_widths;
            s
        });
        cx.observe(&table_state, |this, state, cx| {
            // Ресайз колонки мутирует `table_state` → сохраняем ширины (универсальный сейвер).
            crate::table_persist::persist(&this.backend, &this.widths_id, &state, cx);
            let cur = state.read(cx).column_order.clone();
            if cur != this.col_order_cache {
                this.col_order_cache = cur;
                let view = cx.entity();
                cx.defer(move |app| Self::persist(&view, app));
            }
        })
        .detach();
        let mut this = Self {
            backend,
            group,
            view: OrdersViewState::default(),
            sel_cores: HashSet::new(),
            gate: RenderGate::default(),
            cache_key: None,
            cached_entries: Rc::new(Vec::new()),
            table_state,
            widths_id,
            col_order_cache: Vec::new(),
            main_open: Rc::new(HashSet::new()),
            highlight: Rc::new(HashSet::new()),
            stop_overlay: std::collections::HashMap::new(),
            count_real: 0,
            count_emu: 0,
            dock: None,
            focus: cx.focus_handle(),
        };
        this.apply_ctx_columns(cx);
        let backend_for_initial_cache = this.backend.clone();
        this.rebuild_cache(backend_for_initial_cache.read(cx));
        this
    }

    /// Открытые ордера ядер группы (с именем ядра и quote) — порт `collect_orders`.
    fn collect(&self, b: &Backend) -> Vec<OrderEntry> {
        let store = b.session.store();
        let mut rows = Vec::new();
        for s in b
            .session
            .sessions()
            .iter()
            .filter(|s| s.group == self.group)
        {
            if let Some(d) = store.core(s.id) {
                for o in &d.orders {
                    rows.push(OrderEntry {
                        core: s.id,
                        core_name: s.name.clone(),
                        row: o.clone(),
                    });
                }
            }
        }
        rows
    }

    /// (ядро, маркет) монеты, открытой на Main группы — для фильтра «только текущий».
    fn current_market(&self, b: &Backend) -> Option<(CoreId, String)> {
        b.main_chart_target(&self.group)
    }

    fn cache_key(&self, b: &Backend) -> OrdersCacheKey {
        OrdersCacheKey {
            data_sig: orders_sig(b, &self.group),
            view: self.view,
            sel_cores: {
                let mut v: Vec<CoreId> = self.sel_cores.iter().copied().collect();
                v.sort_unstable();
                v
            },
            current: self
                .view
                .only_current_market
                .then(|| self.current_market(b))
                .flatten(),
            // Монеты, открытые в стеке Main (1 фулскрин или несколько в стеке). Подсветим по
            // ОДНОЙ строке на каждую (монета+ядро); сортировка поднимает их наверх.
            main_open: b.main_open_markets(&self.group).to_vec(),
        }
    }

    /// Return the group's cores in canonical order for the source selector.
    ///
    /// Built on render so an open panel immediately reflects sort-mode changes.
    fn group_cores(&self, b: &Backend) -> OrderedCores {
        CoreOrder::new(&b.config).from_sessions(b.session.sessions(), |s| s.group == self.group)
    }

    /// Тумблер выбранного ядра фильтра (мультивыбор, как в «Отчёте»). `None` — пункт «Все»
    /// (все выбраны → очистить в «пусто = все»; иначе выбрать все ядра группы). `Some(id)` —
    /// тогл одного ядра. Не персистится (как и прежний одиночный источник — сброс на «Все»).
    pub(super) fn toggle_core(&mut self, id: Option<CoreId>, cx: &mut Context<Self>) {
        let all: HashSet<CoreId> = self
            .backend
            .read(cx)
            .session
            .sessions()
            .iter()
            .filter(|s| s.group == self.group)
            .map(|s| s.id)
            .collect();
        match id {
            None => {
                if !all.is_empty() && self.sel_cores.len() == all.len() {
                    self.sel_cores.clear();
                } else {
                    self.sel_cores = all;
                }
            }
            Some(id) => {
                if !self.sel_cores.remove(&id) {
                    self.sel_cores.insert(id);
                }
            }
        }
        let backend = self.backend.clone();
        self.rebuild_cache(backend.read(cx));
        cx.notify();
    }

    /// Клик по ячейке «Ядро»: выставить фильтр РОВНО на это ядро; повторный клик по уже
    /// единственному выбранному ядру — сброс на «Все». Не мультитогл (в отличие от
    /// [`Self::toggle_core`]), а «set-to-single / clear».
    pub(super) fn filter_to_core(&mut self, id: CoreId, cx: &mut Context<Self>) {
        if self.sel_cores.len() == 1 && self.sel_cores.contains(&id) {
            self.sel_cores.clear();
        } else {
            self.sel_cores = HashSet::from([id]);
        }
        let backend = self.backend.clone();
        self.rebuild_cache(backend.read(cx));
        cx.notify();
    }

    /// Собрать и отфильтровать строки. Возвращает `(строки, реальных, эму)`, где счётчики
    /// считаются ПОСЛЕ фильтра ядра/текущего маркета, но ДО фильтра типа (Все/Реальные/
    /// Эмуляторные) — для нижнего футера «Всего N (real/emu)».
    fn build_entries(
        &self,
        b: &Backend,
        view: &OrdersViewState,
        current: &Option<(CoreId, String)>,
    ) -> (Vec<OrderEntry>, usize, usize) {
        let mut entries = self.collect(b);
        // Фильтр ядра (мультивыбор, пусто = все) + текущего маркета — БЕЗ фильтра типа.
        entries.retain(|e| {
            let by_source = self.sel_cores.is_empty() || self.sel_cores.contains(&e.core);
            by_source
                && (!view.only_current_market
                    || match current {
                        Some((c, m)) => e.core == *c && &e.row.market == m,
                        None => true,
                    })
        });
        // Разбивка реальные/эму по этому набору (до фильтра типа) — для футера.
        let count_real = entries.iter().filter(|e| !e.row.emulator).count();
        let count_emu = entries.len() - count_real;
        // Фильтр типа ордера (Все / Реальные / Эмуляторные).
        entries.retain(|e| match view.kind {
            OrderKind::All => true,
            OrderKind::Real => !e.row.emulator,
            OrderKind::Emu => e.row.emulator,
        });
        sort_entries(&mut entries, view);
        (entries, count_real, count_emu)
    }

    fn rebuild_cache(&mut self, b: &Backend) {
        let key = self.cache_key(b);
        self.main_open = Rc::new(key.main_open.iter().cloned().collect());
        // Базовый порядок (первичная + новые/старые) + счётчики real/emu для футера.
        let (mut entries, count_real, count_emu) = self.build_entries(b, &key.view, &key.current);
        self.count_real = count_real;
        self.count_emu = count_emu;
        // Подсветка: ПЕРВАЯ строка каждой Main-открытой (монета+ядро) в базовом порядке — одна
        // строка на пару (не все ордера монеты).
        let mut seen: HashSet<(CoreId, String)> = HashSet::new();
        let mut highlight: HashSet<(CoreId, u64)> = HashSet::new();
        for e in entries.iter() {
            let pair = (e.core, e.row.market.clone());
            if self.main_open.contains(&pair) && seen.insert(pair) {
                highlight.insert((e.core, e.row.uid));
            }
        }
        // Подъём «Main сверху» — стабильно поверх базового порядка (внутри групп порядок сохранён).
        match key.view.main_on_top {
            MainOnTop::Off => {}
            MainOnTop::Highlighted => {
                entries.sort_by_key(|e| u8::from(!highlight.contains(&(e.core, e.row.uid))));
            }
            MainOnTop::AllTicker => {
                let markets: HashSet<&str> =
                    self.main_open.iter().map(|(_, m)| m.as_str()).collect();
                entries.sort_by_key(|e| u8::from(!markets.contains(e.row.market.as_str())));
            }
        }
        self.highlight = Rc::new(highlight);
        self.cached_entries = Rc::new(entries);
        self.cache_key = Some(key);
    }

    /// Реконструкция из `docks.json`: как `new`, но применяет сохранённое состояние
    /// вида (сортировка/тип/фильтр) из `PanelInfo`. `source` (ядро) не персистится —
    /// сбрасывается на «Все ядра» (как в egui-оригинале).
    pub fn restored(
        backend: Entity<Backend>,
        group: String,
        info: &PanelInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::new(backend, group, window, cx);
        this.view = view_from_info(info);
        // Per-контекст набор полей (единый дескриптор) ПЕРЕКРЫВАЕТ старый docks.json-набор:
        // если пользователь настроил колонки в новом хранилище — оно и выигрывает; иначе остаётся
        // docks.json-набор как миграционный сид (перепишется в universal при первом тогле).
        this.apply_ctx_columns(cx);
        // Порядок колонок (drag) персистится отдельно от `view` (это не Copy-список).
        let order = column_order_from_info(info);
        if !order.is_empty() {
            this.col_order_cache = order.clone();
            this.table_state.update(cx, |s, _| s.column_order = order);
        }
        this
    }

    /// Пометить панель как открытую в ОТКРЕПЛЁННОМ окне (зовётся из `detached::spawn` сразу
    /// после создания): переключает контекст хранилища ширин/полей на `:win` и пере-засеивает
    /// ширины И набор видимых колонок из сохранённого набора этого режима. Так у докнутой вкладки
    /// и раскрытого окна свои ширины и свои поля.
    /// Стейт таблицы — для кнопки «⤢ авто» в заголовке откреп-окна (detached.rs).
    pub(crate) fn table_state(&self) -> Entity<MoonDataTableState> {
        self.table_state.clone()
    }

    pub(crate) fn mark_table_detached(&mut self, cx: &mut Context<Self>) {
        self.widths_id = crate::table_persist::ctx_id("orders-table", true);
        let saved = crate::table_persist::saved(self.backend.read(cx), &self.widths_id);
        self.table_state.update(cx, |s, c| {
            s.column_widths = saved;
            c.notify();
        });
        self.apply_ctx_columns(cx);
        let backend = self.backend.clone();
        self.rebuild_cache(backend.read(cx));
        cx.notify();
    }

    /// Применить сохранённый per-контекст набор видимых колонок (`table_visible_columns` по
    /// `widths_id`), если он есть. Единый дескриптор полей: набор различается на режим (докнутая
    /// вкладка `:dock` vs откреплённое окно `:win`). Пустая/битая запись (0 валидных ключей) →
    /// оставляем текущий вид (иначе показали бы пустую таблицу).
    fn apply_ctx_columns(&mut self, cx: &App) {
        if let Some(keys) = crate::table_persist::visible(self.backend.read(cx), &self.widths_id) {
            let mask = keys
                .iter()
                .filter_map(|k| OrdCol::from_key(k))
                .fold(0u16, |m, c| m | c.bit());
            if mask != 0 {
                self.view.columns = mask;
            }
        }
    }

    /// Сохранить текущий набор видимых колонок в per-контекст хранилище (`widths_id`). Зовётся из
    /// [`Self::mutate`] при изменении маски видимости.
    fn save_ctx_columns(&self, cx: &mut App) {
        let keys: Vec<String> = self
            .view
            .visible_columns()
            .iter()
            .map(|c| c.key().to_string())
            .collect();
        crate::table_persist::set_visible(&self.backend, &self.widths_id, keys, cx);
    }

    /// Единая точка изменения состояния вида: применяет `f`, и ЕСЛИ вид изменился —
    /// перерисовывает и ПЕРСИСТИТ (дамп дока в `dock_states` + `dock_dirty`). Дамп
    /// делаем на уровне `App` (вне borrow самой панели) — иначе ре-энтранси при
    /// `dock.dump()`, который читает в т.ч. эту панель. `OrdersViewState: Copy`.
    pub(super) fn mutate(view: &Entity<Self>, app: &mut App, f: impl FnOnce(&mut OrdersViewState)) {
        let changed = view.update(app, |this, cx| {
            let mut next = this.view;
            f(&mut next);
            if next != this.view {
                let cols_changed = next.columns != this.view.columns;
                this.view = next;
                let backend = this.backend.clone();
                this.rebuild_cache(backend.read(cx));
                // Смена набора видимых колонок → персист в единый per-контекст дескриптор.
                if cols_changed {
                    this.save_ctx_columns(cx);
                }
                cx.notify();
                true
            } else {
                false
            }
        });
        if changed {
            Self::persist(view, app);
        }
    }

    /// Дамп текущей раскладки дока окна в backend (→ `docks.json`). Смена вида ордеров
    /// не эмитит `DockEvent`, поэтому состояние вида сохраняем сами — иначе сортировка
    /// сбрасывалась при переоткрытии.
    fn persist(view: &Entity<Self>, app: &mut App) {
        let (dock, group, backend) = {
            let p = view.read(app);
            (p.dock.clone(), p.group.clone(), p.backend.clone())
        };
        let Some(dock) = dock.and_then(|d| d.upgrade()) else {
            return;
        };
        let state = dock.read(app).dump(app);
        backend.update(app, |b, _| {
            b.dock_states.insert(group, state);
            b.dock_dirty = true;
        });
    }
}

/// Ключ группировки по ОТОБРАЖАЕМОЙ стороне (с учётом исполнения → SELL). 0 = выше.
fn primary_key(p: PrimarySort, r: &OrderRow) -> u8 {
    match p {
        // ProfitFirst сюда не доходит (обрабатывается отдельно в sort_entries), группа нейтральна.
        PrimarySort::Creation | PrimarySort::ProfitFirst => 0,
        PrimarySort::SellFirst => u8::from(!is_sell(r)),
        PrimarySort::BuyFirst => u8::from(!is_buy(r)),
    }
}

/// Базовая сортировка: первичная (SELL/BUY/Creation/Profit) + новые/старые. Подъём
/// «Main сверху» применяется отдельно поверх (стабильно) в `rebuild_cache`.
fn sort_entries(entries: &mut [OrderEntry], view: &OrdersViewState) {
    // «Profit первые» — отдельная ветка: сортируем по PnL по убыванию (прибыльные вверху),
    // без позиции (`None`) — вниз. Тай-брейк — по uid (новые/старые), как у других режимов.
    if view.primary == PrimarySort::ProfitFirst {
        entries.sort_by(|a, b| {
            let pa = table::order_pnl(&a.row);
            let pb = table::order_pnl(&b.row);
            // None считаем как −∞ (внизу списка).
            let key = |v: Option<f64>| v.unwrap_or(f64::NEG_INFINITY);
            key(pb)
                .partial_cmp(&key(pa))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    let c = a.row.uid.cmp(&b.row.uid);
                    if view.newest_first { c.reverse() } else { c }
                })
        });
        return;
    }
    entries.sort_by(|a, b| {
        let ka = primary_key(view.primary, &a.row);
        let kb = primary_key(view.primary, &b.row);
        ka.cmp(&kb).then_with(|| {
            let c = a.row.uid.cmp(&b.row.uid);
            if view.newest_first { c.reverse() } else { c }
        })
    });
}

/// Восстановить сохранённое состояние вида из `PanelInfo` (docks.json). Отсутствующие
/// поля → дефолт. `source` не персистится (см. `dump`), всегда «Все ядра».
fn view_from_info(info: &PanelInfo) -> OrdersViewState {
    let mut v = OrdersViewState::default();
    if let PanelInfo::Panel(j) = info {
        if let Some(p) = j.get("primary").and_then(|x| x.as_u64()) {
            v.primary = PrimarySort::from_u8(p as u8);
        }
        if let Some(k) = j.get("kind").and_then(|x| x.as_u64()) {
            v.kind = OrderKind::from_u8(k as u8);
        }
        if let Some(n) = j.get("newest_first").and_then(|x| x.as_bool()) {
            v.newest_first = n;
        }
        if let Some(o) = j.get("only_current").and_then(|x| x.as_bool()) {
            v.only_current_market = o;
        }
        if let Some(m) = j.get("main_on_top").and_then(|x| x.as_u64()) {
            v.main_on_top = MainOnTop::from_u8(m as u8);
        }
        // Видимые колонки: список ключей → маска. Пустой/пропущенный список или ноль
        // валидных ключей → оставляем дефолт (все видимы), чтобы не показать пустую таблицу.
        if let Some(arr) = j.get("columns").and_then(|x| x.as_array()) {
            let mask = arr
                .iter()
                .filter_map(|x| x.as_str())
                .filter_map(OrdCol::from_key)
                .fold(0u16, |m, c| m | c.bit());
            if mask != 0 {
                v.columns = mask;
            }
        }
    }
    v
}

/// Сохранённый порядок колонок (drag) из `PanelInfo` → список ключей. Берём только
/// валидные `OrdCol`-ключи (устойчиво к мусору/переименованиям). Пусто → дефолт `ALL`.
fn column_order_from_info(info: &PanelInfo) -> Vec<SharedString> {
    let PanelInfo::Panel(j) = info else {
        return Vec::new();
    };
    j.get("column_order")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str())
                .filter(|s| OrdCol::from_key(s).is_some())
                .map(SharedString::from)
                .collect()
        })
        .unwrap_or_default()
}

impl EventEmitter<PanelEvent> for OrdersPanel {}
impl Focusable for OrdersPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for OrdersPanel {
    fn panel_name(&self) -> &'static str {
        "Orders"
    }
    /// Visible tab caption. `panel_name` is the stable persistence key and stays untouched.
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        crate::panel_meta::tab_label(self.panel_name())
    }
    // × не удаляет панель, а возвращает её в нижнюю строку (см. Shell: PanelCloseRequested).
    fn closable(&self, _cx: &App) -> bool {
        true
    }
    // Вынесенная в split одиночная панель показывает заголовок (drag-ручка + ×), иначе у неё
    // нет ни места тянуть, ни кнопки закрыть.
    fn show_dock_header(&self, _cx: &App) -> bool {
        true
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::panel_meta::panel_title(self.panel_name())
    }
    fn dump(&self, cx: &App) -> PanelState {
        // Группа (для реконструкции) + состояние вида: сортировка/тип/фильтр. `source`
        // (ядро) не сохраняем — id ядра не стабилен между запусками (как в egui).
        PanelState {
            panel_name: "Orders".to_string(),
            children: Vec::new(),
            info: PanelInfo::panel(serde_json::json!({
                "group": self.group,
                "primary": self.view.primary.to_u8(),
                "kind": self.view.kind.to_u8(),
                "newest_first": self.view.newest_first,
                "only_current": self.view.only_current_market,
                "main_on_top": self.view.main_on_top.to_u8(),
                // Видимые колонки — списком стабильных ключей (не маской: устойчиво к смене
                // порядка enum). Отсутствие поля при restore → все колонки видимы.
                "columns": self
                    .view
                    .visible_columns()
                    .iter()
                    .map(|c| c.key())
                    .collect::<Vec<_>>(),
                // Порядок колонок после drag-перестановки заголовков (стабильные ключи).
                // Живёт в `table_state`; читаем напрямую. Пустой → дефолтный порядок `ALL`.
                "column_order": self
                    .table_state
                    .read(cx)
                    .column_order
                    .iter()
                    .map(|k| k.to_string())
                    .collect::<Vec<_>>(),
            })),
        }
    }
    fn on_added_to(
        &mut self,
        dock_area: WeakEntity<DockArea>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.dock = Some(dock_area);
    }
    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Vec<AnyElement>> {
        Some(vec![
            crate::table_persist::reset_button("orders-reset-widths", &self.table_state),
            crate::panels::detach_button(
                "Orders",
                self.group.clone(),
                self.backend.clone(),
                self.dock.clone(),
            ),
        ])
    }
}

impl Render for OrdersPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::ORDERS_RENDER);
        let view = self.view;
        let cores = self.group_cores(self.backend.read(cx));
        let entries = self.cached_entries.clone();
        let p = MoonPalette::active(cx);

        // ── Панель управления ──
        let mut controls = h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(self.source_combo(&cores, cx))
            .child(self.kind_combo(cx))
            .child(self.columns_menu(cx))
            .child(self.sort_menu(cx));
        if view.only_current_market {
            controls = controls.child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child(format!("· {}", t!("orders.only_current"))),
            );
        }

        // ── Виртуальная таблица в геометрии HTML-эталона ──
        // Подсвечиваем по одной строке на Main-открытую (монета+ядро) — см. table::orders_table.
        // Оптимистичные тоглы стопов: снимок живых (<3с) записей для ячеек SL/TS/Vstop.
        self.stop_overlay
            .retain(|_, (_, t)| t.elapsed() < Duration::from_secs(3));
        let stop_overlay: Rc<std::collections::HashMap<(CoreId, u64, u8), bool>> = Rc::new(
            self.stop_overlay
                .iter()
                .map(|(k, (v, _))| (*k, *v))
                .collect(),
        );
        let table = table::orders_table(
            entries,
            view.columns,
            &self.table_state,
            self.highlight.clone(),
            stop_overlay,
            cx,
        );

        // ── Футер: всего открытых ордеров (реальных/эму) ──
        let total = self.count_real + self.count_emu;
        let footer = h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(
                        t!(
                            "orders.footer_total",
                            total = total,
                            real = self.count_real,
                            emu = self.count_emu
                        )
                        .to_string(),
                    ),
            );

        v_flex()
            .id("orders-panel")
            .size_full()
            .min_h(px(0.0))
            .overflow_hidden()
            .track_focus(&self.focus)
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .bg(rgb(p.table_body))
            .child(controls)
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .child(table)
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .child(footer)
    }
}

/// Сигнатура таблицы ордеров группы. Это именно table-rev, не rev линий графика:
/// числовые поля/статусы в таблице должны обновляться независимо от userdata чарта.
fn orders_sig(b: &Backend, group: &str) -> u64 {
    let store = b.session.store();
    b.session
        .sessions()
        .iter()
        .filter(|s| s.group == group)
        .filter_map(|s| store.core(s.id))
        .fold(0u64, |a, c| {
            a.wrapping_mul(31).wrapping_add(c.orders_table_rev)
        })
}
