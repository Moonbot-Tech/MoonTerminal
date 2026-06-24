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

use std::rc::Rc;

use gpui::*;
use moon_ui::{
    DockArea, MoonButtonSize, MoonButtonVariant, MoonDataCell, MoonDataRow, MoonDataTable,
    MoonDataTableColumn, MoonDropdown, MoonMenuItem, MoonMenuSize, MoonPalette, MoonText, MoonTone,
    Panel, PanelEvent, PanelInfo, PanelState, h_flex, v_flex,
};

use rust_i18n::t;

use crate::Backend;
use crate::design;
use crate::panels::{RenderGate, num};
use moon_core::feed::OrderRow;
use moon_core::session::CoreId;
use moon_core::symbol;

pub use table::count_orders;

/// Одна строка таблицы ордеров с привязкой к ядру-источнику (порт `OrderEntry`).
#[derive(Clone)]
pub(super) struct OrderEntry {
    pub(super) core: CoreId,
    pub(super) core_name: String,
    pub(super) quote: String,
    pub(super) row: OrderRow,
}

#[derive(Clone, PartialEq, Eq)]
struct OrdersCacheKey {
    data_sig: u64,
    view: OrdersViewState,
    current: Option<(CoreId, String)>,
}

/// Первичный ключ сортировки (тогл-группа в меню).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PrimarySort {
    SellFirst,
    BuyFirst,
    Creation,
}

impl PrimarySort {
    /// Стабильный код для персиста (docks.json) — порт egui `to_u8`/`from_u8`.
    fn to_u8(self) -> u8 {
        match self {
            PrimarySort::Creation => 0,
            PrimarySort::SellFirst => 1,
            PrimarySort::BuyFirst => 2,
        }
    }
    fn from_u8(v: u8) -> Self {
        match v {
            1 => PrimarySort::SellFirst,
            2 => PrimarySort::BuyFirst,
            _ => PrimarySort::Creation,
        }
    }
}

/// Источник ордеров: все ядра группы или конкретное ядро.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum OrdersSource {
    All,
    Core(CoreId),
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

/// Состояние вида таблицы (источник + тип + фильтр + сортировка). Своё у панели.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct OrdersViewState {
    pub(super) source: OrdersSource,
    pub(super) kind: OrderKind,
    pub(super) only_current_market: bool,
    pub(super) primary: PrimarySort,
    pub(super) newest_first: bool,
}

impl Default for OrdersViewState {
    fn default() -> Self {
        Self {
            source: OrdersSource::All,
            kind: OrderKind::All,
            only_current_market: false,
            primary: PrimarySort::Creation,
            newest_first: true,
        }
    }
}

/// Вход (покупка) заполнен.
fn executed(r: &OrderRow) -> bool {
    r.fill_pct >= 99.95
}
/// SELL — исполненный ЛОНГ (куплен и выставлен на продажу). НЕ шорт.
pub(super) fn is_sell(r: &OrderRow) -> bool {
    !r.is_short && executed(r)
}
/// BUY — лонг, ещё не исполнен (ждёт покупки).
pub(super) fn is_buy(r: &OrderRow) -> bool {
    !r.is_short && !executed(r)
}

/// Панель «Ордера».
pub struct OrdersPanel {
    pub(super) backend: Entity<Backend>,
    pub(super) group: String,
    pub(super) view: OrdersViewState,
    /// Гейт перерисовки: ордерные ивенты летят часто, цены/P&L живут от рынка — общий
    /// `RenderGate` (сигнатура ИЛИ 1 Гц-тик, пол 250мс) экономит UI-поток на холостом ходу.
    gate: RenderGate,
    cache_key: Option<OrdersCacheKey>,
    cached_cores: Vec<(CoreId, String)>,
    cached_entries: Rc<Vec<OrderEntry>>,
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
        let mut this = Self {
            backend,
            group,
            view: OrdersViewState::default(),
            gate: RenderGate::default(),
            cache_key: None,
            cached_cores: Vec::new(),
            cached_entries: Rc::new(Vec::new()),
            dock: None,
            focus: cx.focus_handle(),
        };
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
            let quote = b
                .config
                .servers
                .iter()
                .find(|sv| sv.id == s.id)
                .map(|sv| symbol::resolve_quote(&sv.market))
                .unwrap_or_default();
            if let Some(d) = store.core(s.id) {
                for o in &d.orders {
                    rows.push(OrderEntry {
                        core: s.id,
                        core_name: s.name.clone(),
                        quote: quote.clone(),
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
            current: self
                .view
                .only_current_market
                .then(|| self.current_market(b))
                .flatten(),
        }
    }

    /// Имена ядер группы (id, имя) — для поля-списка источника.
    fn group_cores(&self, b: &Backend) -> Vec<(CoreId, String)> {
        b.session
            .sessions()
            .iter()
            .filter(|s| s.group == self.group)
            .map(|s| (s.id, s.name.clone()))
            .collect()
    }

    fn build_entries(
        &self,
        b: &Backend,
        view: &OrdersViewState,
        current: &Option<(CoreId, String)>,
    ) -> Vec<OrderEntry> {
        let mut entries = self.collect(b);
        entries.retain(|e| {
            let by_source = match view.source {
                OrdersSource::All => true,
                OrdersSource::Core(id) => e.core == id,
            };
            let by_kind = match view.kind {
                OrderKind::All => true,
                OrderKind::Real => !e.row.emulator,
                OrderKind::Emu => e.row.emulator,
            };
            by_source
                && by_kind
                && (!view.only_current_market
                    || match current {
                        Some((c, m)) => e.core == *c && &e.row.market == m,
                        None => true,
                    })
        });
        sort_entries(&mut entries, view);
        entries
    }

    fn rebuild_cache(&mut self, b: &Backend) {
        let key = self.cache_key(b);
        self.cached_cores = self.group_cores(b);
        self.cached_entries = Rc::new(self.build_entries(b, &key.view, &key.current));
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
        this
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
                this.view = next;
                let backend = this.backend.clone();
                this.rebuild_cache(backend.read(cx));
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
        PrimarySort::Creation => 0,
        PrimarySort::SellFirst => u8::from(!is_sell(r)),
        PrimarySort::BuyFirst => u8::from(!is_buy(r)),
    }
}

fn sort_entries(entries: &mut [OrderEntry], view: &OrdersViewState) {
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
    }
    v
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
        SharedString::from(t!("dock.tab.orders").to_string())
    }
    fn dump(&self, _cx: &App) -> PanelState {
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
        Some(vec![crate::panels::detach_button(
            "Orders",
            self.group.clone(),
            self.backend.clone(),
            self.dock.clone(),
        )])
    }
}

impl Render for OrdersPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::ORDERS_RENDER);
        let view = self.view;
        let cores = self.cached_cores.clone();
        let entries = self.cached_entries.clone();
        let shown = entries.len();
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
            .child(self.sort_menu(cx))
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child(format!("{shown}")),
            );
        if view.only_current_market {
            controls = controls.child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child(format!("· {}", t!("orders.only_current"))),
            );
        }

        // ── Виртуальная таблица в геометрии HTML-эталона ──
        let table = table::orders_table(entries, cx);

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
