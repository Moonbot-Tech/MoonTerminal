//! Панель/окно «Активы». Сверху — полоса ядер (баланс USDT) + таблица позиций/балансов
//! по всем ядрам охвата (стоимость/итоги в USDT, фильтр >1 USDT + галка «показать всё»).
//! Снизу (только в отдельном окне) — список ядер слева (свободно/итого) и 3 контейнера
//! кошельков (Спот/Фьючерсы/Квартальные) справа: перетаскивание монеты между ними
//! открывает диалог количества (дефолт — всё свободное) и выполняет перенос.
//!
//! Один и тот же `AssetsView` живёт двумя способами:
//! - как dock-панель в окне группы (`AssetsScope::Group`) — активы ядер группы;
//! - как глобальное singleton-окно (`AssetsScope::All`, открывается кнопкой «⧉») —
//!   активы ВСЕХ подключённых ядер. Дедуп окна — в `Backend.assets_window` (как «Стратегии»).
//!
//! По функционалу разнесено: состояние/данные/жизненный цикл/окно — здесь, верхняя
//! таблица и полоса/список ядер — [`table`], 3 контейнера кошельков и диалог переноса
//! (drag&drop) — [`wallets`].

mod table;
mod wallets;

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    DockArea, MoonBackgroundPolicy, MoonButton, MoonButtonSize, MoonCheckbox, MoonCheckboxSize,
    MoonDataCell, MoonDataRow, MoonDataTable, MoonDataTableColumn, MoonInput, MoonInputState,
    MoonPalette, MoonTone, MoonWindowFrame, Panel, PanelEvent, PanelState, Root, h_flex, v_flex,
};

use crate::Backend;
use crate::design;
use crate::panels::{RenderGate, num};
use moon_core::feed::{AssetRow, ResetProfitKind, TransferAssetRow, WalletKind};
use moon_core::session::CoreId;
use rust_i18n::t;

use wallets::PendingTransfer;

/// Высота титлбара окна «Активы» (как у окна «Стратегии»).
const ASSETS_HEADER_H: f32 = 32.0;

/// Область охвата панели «Активы».
#[derive(Clone, PartialEq, Eq)]
enum AssetsScope {
    /// Dock-панель окна группы — ядра этой группы.
    Group(String),
    /// Глобальное окно — все подключённые ядра.
    All,
}

/// Строка таблицы активов с привязкой к ядру + посчитанная USDT-стоимость.
#[derive(Clone)]
pub(super) struct AssetEntry {
    pub(super) core_name: String,
    pub(super) row: AssetRow,
    /// Текущая стоимость в USDT.
    pub(super) value: f64,
}

/// Подытог по ядру: баланс свободно/итого в USDT (для полосы ядер и левого списка).
#[derive(Clone)]
pub(super) struct CoreAgg {
    pub(super) id: CoreId,
    pub(super) name: String,
    /// Свободный баланс в USDT (btc_total * курс).
    pub(super) free: f64,
    /// Итоговый баланс в USDT (btc_full * курс, с нереализ. PnL).
    pub(super) total: f64,
    /// PnL ядра в USDT (серверный, значение шапки «PnL»).
    pub(super) pnl: f64,
}

#[derive(Clone)]
pub(super) struct WalletColumnSnapshot {
    pub(super) kind: WalletKind,
    pub(super) total_count: usize,
    pub(super) rows: Vec<TransferAssetRow>,
}

/// Разбить целую часть на тройки пробелом: "1111" → "1 111".
fn group_thousands(int: &str) -> String {
    let len = int.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

/// Денежный формат USDT: тысячи через пробел, 1 знак после запятой (через `,`),
/// знак `$` в конце. Пример: `1 111,1$`.
pub(super) fn money(v: f64) -> String {
    let neg = v < 0.0;
    let s = format!("{:.1}", v.abs()); // "1111.1"
    let (int, frac) = s.split_once('.').unwrap_or((s.as_str(), "0"));
    format!(
        "{}{},{frac}$",
        if neg { "-" } else { "" },
        group_thousands(int)
    )
}

/// Человекочитаемая категория рынка: listed-байт + quote.
pub(super) fn kind_label(row: &AssetRow) -> String {
    let k = match row.listed {
        1 => "spot",
        2 => "fut",
        3 => "both",
        _ => "?",
    };
    format!("{k}·{}", row.quote)
}

/// Окно/панель «Активы».
pub struct AssetsView {
    pub(super) backend: Entity<Backend>,
    scope: AssetsScope,
    /// true = вид рисует СВОЮ рамку ОС-окна (титлбар + системные контролы) и персистит
    /// свою геометрию. Глобальное окно = true; откреп-окно (рамку даёт `DetachedWindow`)
    /// и dock-вкладка = false.
    windowed: bool,
    /// Показывать нижние контейнеры переноса (список ядер + Спот/Фьючи/Квартальные).
    /// true в любом отдельном окне (глобальном/откреплённом), false во вкладке дока.
    show_wallets: bool,
    /// Выбранное ядро для нижних контейнеров кошельков.
    pub(super) selected_core: Option<CoreId>,
    /// Показывать ВСЁ (иначе только балансы >1 USDT с известной ценой).
    pub(super) show_all: bool,
    /// Свёрнута ли полоса плашек ядер (свёрнуто = только строка-итог Σ баланс/PnL).
    pub(super) plates_collapsed: bool,
    /// Открытый диалог переноса (количество) + поле ввода. Тип `PendingTransfer`
    /// приватен для `wallets`, поэтому поле тоже приватное (доступно потомкам модуля).
    pending_transfer: Option<PendingTransfer>,
    transfer_input: Option<Entity<MoonInputState>>,
    /// Гейт перерисовки (сигнатура assets_rev/transfer_rev ИЛИ 1 Гц-тик, пол 250мс).
    gate: RenderGate,
    cache_sig: Option<(u64, bool)>,
    cached_cores: Vec<(CoreId, String)>,
    cached_entries: Rc<Vec<AssetEntry>>,
    cached_aggs: Rc<Vec<CoreAgg>>,
    cached_wallet_key: Option<(Option<CoreId>, u64, bool)>,
    cached_wallets: Rc<Vec<WalletColumnSnapshot>>,
    cached_total_value: f64,
    cached_total_pnl: f64,
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl AssetsView {
    fn new(
        backend: Entity<Backend>,
        scope: AssetsScope,
        windowed: bool,
        show_wallets: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Перерисовка по дренажу backend — только при изменении активов (rev) или раз в сек.
        cx.observe(&backend, |this, backend, cx| {
            let now = moon_chart::paint::now_unix_ms();
            let b = backend.read(cx);
            let sig = this.assets_sig(b);
            let key = (sig, this.show_all);
            let changed = this.cache_sig != Some(key);
            let due = this.gate.should_notify(sig, now);
            if changed || due {
                this.rebuild_cache(b);
                cx.notify();
            }
        })
        .detach();

        // Только отдельное окно сохраняет свою геометрию (dock-панель живёт в окне группы).
        if windowed {
            cx.observe_window_bounds(window, |this, window, cx| {
                let Some((x, y, w, h)) = crate::windowing::window_geom(window) else {
                    return;
                };
                this.backend.update(cx, |b, _| {
                    if b.layout.assets_window.map(|g| (g.x, g.y, g.w, g.h)) != Some((x, y, w, h)) {
                        b.layout.assets_window =
                            Some(moon_core::config::layout::GeomRect { x, y, w, h });
                        b.layout_dirty = true;
                    }
                });
            })
            .detach();
        }

        let mut this = Self {
            backend,
            scope,
            windowed,
            show_wallets,
            selected_core: None,
            show_all: false,
            plates_collapsed: true,
            pending_transfer: None,
            transfer_input: None,
            gate: RenderGate::default(),
            cache_sig: None,
            cached_cores: Vec::new(),
            cached_entries: Rc::new(Vec::new()),
            cached_aggs: Rc::new(Vec::new()),
            cached_wallet_key: None,
            cached_wallets: Rc::new(Vec::new()),
            cached_total_value: 0.0,
            cached_total_pnl: 0.0,
            dock: None,
            focus: cx.focus_handle(),
        };
        // Выбрать первое ядро охвата и запросить его transfer-активы для контейнеров.
        let first = this
            .scope_cores(this.backend.read(cx))
            .first()
            .map(|(id, _)| *id);
        if let Some(core) = first {
            this.selected_core = Some(core);
            if let Err(error) = this.backend.read(cx).session.refresh_transfer_assets(core) {
                log::warn!("assets initial refresh failed for core {core}: {error}");
            }
        }
        let backend_for_initial_cache = this.backend.clone();
        this.rebuild_cache(backend_for_initial_cache.read(cx));
        this
    }

    /// Реконструкция dock-панели из `docks.json` (группа из state) — вкладка, без контейнеров.
    pub fn restored_group(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(backend, AssetsScope::Group(group), false, false, window, cx)
    }

    /// Контент откреплённого окна (`DetachedWindow` даёт рамку) — ядра группы + нижние
    /// контейнеры переноса, но без собственной рамки/персиста геометрии.
    pub fn detached_group(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(backend, AssetsScope::Group(group), false, true, window, cx)
    }

    /// Ядра охвата (id, имя): группа → ядра группы; глобально → все подключённые.
    pub(super) fn scope_cores(&self, b: &Backend) -> Vec<(CoreId, String)> {
        b.session
            .sessions()
            .iter()
            .filter(|s| match &self.scope {
                AssetsScope::Group(g) => &s.group == g,
                AssetsScope::All => true,
            })
            .map(|s| (s.id, s.name.clone()))
            .collect()
    }

    /// Сигнатура активов охвата (assets_rev/transfer_rev ядер) — гейт перерисовки.
    fn assets_sig(&self, b: &Backend) -> u64 {
        let store = b.session.store();
        self.scope_cores(b)
            .iter()
            .filter_map(|(id, _)| store.core(*id))
            .fold(0u64, |a, c| {
                a.wrapping_mul(31)
                    .wrapping_add(c.assets_rev)
                    .wrapping_mul(31)
                    .wrapping_add(c.transfer_rev)
            })
    }

    /// Строки таблицы по всем ядрам охвата (с USDT-стоимостью), отсортированные по
    /// убыванию стоимости. По умолчанию — только >1 USDT (или открытая позиция); галка
    /// «показать всё» снимает фильтр.
    fn collect(&self, b: &Backend) -> Vec<AssetEntry> {
        let store = b.session.store();
        let mut out = Vec::new();
        for (id, name) in self.scope_cores(b) {
            let Some(cd) = store.core(id) else { continue };
            for row in &cd.assets.rows {
                let value = row.value_usdt;
                let keep = self.show_all || value > 1.0 || row.pos_size != 0.0;
                if !keep {
                    continue;
                }
                out.push(AssetEntry {
                    core_name: name.clone(),
                    row: row.clone(),
                    value,
                });
            }
        }
        sort_by_value(&mut out);
        out
    }

    /// Балансы по каждому ядру охвата (свободно/итого в USDT, посчитаны на ядре).
    pub(super) fn per_core(&self, b: &Backend) -> Vec<CoreAgg> {
        let store = b.session.store();
        self.scope_cores(b)
            .into_iter()
            .map(|(id, name)| {
                let mut free = 0.0;
                let mut total = 0.0;
                let mut pnl = 0.0;
                if let Some(cd) = store.core(id) {
                    // USDT-баланс уже посчитан на ядре с учётом базовой валюты.
                    free = cd.assets.global.free_usdt;
                    total = cd.assets.global.total_usdt;
                    pnl = cd.assets.global.pnl_usdt;
                }
                CoreAgg {
                    id,
                    name,
                    free,
                    total,
                    pnl,
                }
            })
            .collect()
    }

    fn rebuild_cache(&mut self, b: &Backend) {
        let sig = self.assets_sig(b);
        let cores = self.scope_cores(b);
        let selected_valid = self
            .selected_core
            .is_some_and(|core| cores.iter().any(|(id, _)| *id == core));
        if !selected_valid {
            self.selected_core = cores.first().map(|(id, _)| *id);
            self.cached_wallet_key = None;
        }
        self.cached_cores = cores;
        self.cached_entries = Rc::new(self.collect(b));
        self.cached_aggs = Rc::new(self.per_core(b));
        self.rebuild_wallet_cache(b);
        self.cached_total_value = self.cached_entries.iter().map(|e| e.value).sum();
        self.cached_total_pnl = self
            .cached_cores
            .iter()
            .filter_map(|(id, _)| b.session.store().core(*id))
            .map(|cd| cd.assets.global.pnl_usdt)
            .sum();
        self.cache_sig = Some((sig, self.show_all));
    }

    fn wallet_cache_key(&self, b: &Backend) -> (Option<CoreId>, u64, bool) {
        let transfer_rev = self
            .selected_core
            .and_then(|core| b.session.store().core(core).map(|cd| cd.transfer_rev))
            .unwrap_or(0);
        (self.selected_core, transfer_rev, self.show_all)
    }

    fn rebuild_wallet_cache(&mut self, b: &Backend) {
        let key = self.wallet_cache_key(b);
        if self.cached_wallet_key == Some(key) {
            return;
        }
        let Some(core) = key.0 else {
            self.cached_wallets = Rc::new(Vec::new());
            self.cached_wallet_key = Some(key);
            return;
        };
        let Some(cd) = b.session.store().core(core) else {
            self.cached_wallets = Rc::new(Vec::new());
            self.cached_wallet_key = Some(key);
            return;
        };
        let mut snapshots = Vec::new();
        for kind in WalletKind::ALL {
            let all_items = cd.transfer_assets.wallet(kind).to_vec();
            let total_count = all_items.len();
            let mut rows: Vec<TransferAssetRow> = all_items
                .into_iter()
                .filter(|a| self.show_all || a.value_usdt > 1.0)
                .collect();
            rows.sort_by(|a, b| {
                b.value_usdt
                    .partial_cmp(&a.value_usdt)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            snapshots.push(WalletColumnSnapshot {
                kind,
                total_count,
                rows,
            });
        }
        self.cached_wallets = Rc::new(snapshots);
        self.cached_wallet_key = Some(key);
    }
}

/// Сортировка строк по убыванию USDT-стоимости (самые большие сверху).
pub(super) fn sort_by_value(out: &mut [AssetEntry]) {
    out.sort_by(|a, b| {
        b.value
            .partial_cmp(&a.value)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

impl EventEmitter<PanelEvent> for AssetsView {}
impl Focusable for AssetsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for AssetsView {
    fn panel_name(&self) -> &'static str {
        "Assets"
    }
    fn closable(&self, _cx: &App) -> bool {
        true
    }
    fn show_dock_header(&self, _cx: &App) -> bool {
        true
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from(t!("dock.tab.assets").to_string())
    }
    fn dump(&self, _cx: &App) -> PanelState {
        let group = match &self.scope {
            AssetsScope::Group(g) => g.clone(),
            AssetsScope::All => String::new(),
        };
        crate::dock_persist::panel_state_with_group("Assets", &group)
    }
    fn on_added_to(
        &mut self,
        dock_area: WeakEntity<DockArea>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.dock = Some(dock_area);
    }
    /// Кнопка «⧉»: открыть ГЛОБАЛЬНОЕ окно «Активы» (все ядра, singleton) — в отличие
    /// от Orders это не per-group detach, а отдельное окно (как «Стратегии»).
    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Vec<AnyElement>> {
        let backend = self.backend.clone();
        Some(vec![
            MoonButton::new("assets-open-global")
                .ghost()
                .size(MoonButtonSize::Action)
                .label("⧉")
                .on_click(move |_, window, app| {
                    open(backend.clone(), Some(window.window_handle()), app);
                })
                .render()
                .into_any_element(),
        ])
    }
}

impl Render for AssetsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cores = self.cached_cores.clone();
        let entries = self.cached_entries.clone();
        let p = MoonPalette::active(cx);
        let windowed = self.windowed;

        let count = entries.len();
        // Натуральная высота таблицы = шапка + строки (пусто → 0). Ограничивает max_h
        // обёртки, чтобы таблица росла под контент, а не тянулась на всю панель.
        let table_natural_h = if count == 0 {
            0.0
        } else {
            design::TABLE_HEAD_H + count as f32 * design::TABLE_ROW_H
        };
        let total_value = self.cached_total_value;
        let total_pnl = self.cached_total_pnl;

        let aggs = self.cached_aggs.clone();
        let controls = self.controls(count, total_value, cx);
        let plates = self.core_strip(&aggs, total_pnl, cx);
        // Контейнеры переноса (список ядер + кошельки) — в отдельном ОКНЕ (глобальном или
        // откреплённом); во вкладке дока показываем только позиции/балансы (таблица шире).
        let wallets = self.cached_wallets.clone();
        let tree_section = self
            .show_wallets
            .then(|| self.bottom(&cores, &aggs, &wallets, cx).into_any_element());
        let table = table::assets_table("assets-table", entries, cx);

        // Ширина окна для хит-оверлея титлбара (drag/resize/контролы) — как у «Стратегий».
        let chrome_width = match window.window_bounds() {
            WindowBounds::Windowed(bb)
            | WindowBounds::Maximized(bb)
            | WindowBounds::Fullscreen(bb) => f32::from(bb.size.width),
        };

        let mut root = v_flex()
            .id("assets-panel")
            .size_full()
            .relative()
            .min_h(px(0.0))
            .overflow_hidden()
            .track_focus(&self.focus)
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .bg(rgb(p.table_body))
            .when(windowed, |this| this.child(assets_header(p, cx)))
            .child(controls)
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            // Таблица позиций РЕЗЕРВИРУЕТ высоту по содержимому: пусто → 0 (полностью
            // схлопнута), N строк → высота под N строк. Высота-basis (`.h`, а НЕ `flex_1`)
            // → плашки не могут ужать её при разворачивании: она уступает (скроллится
            // внутри) только если строк реально не влезает по высоте панели.
            .child(
                v_flex()
                    .h(px(table_natural_h))
                    .min_h(px(0.0))
                    .w_full()
                    .overflow_hidden()
                    .child(table),
            )
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .child(plates)
            .children(tree_section);
        if windowed {
            root = root.child(
                MoonWindowFrame::tool("assets-window-frame-hit", chrome_width)
                    .header_height(ASSETS_HEADER_H)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            );
        }
        root
    }
}

/// Титлбар окна «Активы» (drag-кластер слева + системные контролы справа).
fn assets_header(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .id("assets-window-header")
        .relative()
        .flex_none()
        .w_full()
        .h(design::fit_h_px(cx, ASSETS_HEADER_H, 14.0, 9.0))
        .justify_between()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .bg(rgb(p.shell_high))
        .border_b(px(1.0))
        .border_color(rgb(p.border))
        .child(
            MoonWindowFrame::tool("assets-titlebar-title", 0.0)
                .title_cluster(t!("dock.tab.assets").to_string(), cx)
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .when(design::show_custom_window_controls(), |this| {
            this.child(
                MoonWindowFrame::tool("assets-window-frame-visual", 0.0)
                    .header_height(ASSETS_HEADER_H)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}

/// Открыть глобальное окно «Активы» (tool/secondary singleton, все ядра).
/// Дедуп — в `Backend.assets_window`.
pub fn open(backend: Entity<Backend>, owner: Option<AnyWindowHandle>, cx: &mut App) {
    // Уже открыто → сфокусировать.
    if let Some(handle) = backend.read(cx).assets_window {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let saved = backend.read(cx).layout.assets_window;
    let bounds = saved.map_or(
        Bounds {
            origin: point(px(140.0), px(110.0)),
            size: size(px(1180.0), px(720.0)),
        },
        |g| Bounds {
            origin: point(px(g.x as f32), px(g.y as f32)),
            size: size(px(g.w as f32), px(g.h as f32)),
        },
    );
    let display_id = saved.and_then(|g| {
        let origin = point(px(g.x as f32), px(g.y as f32));
        cx.displays()
            .into_iter()
            .find(|d| d.bounds().contains(&origin))
            .map(|d| d.id())
    });
    let mut opts = crate::windowing::tool_window_options(
        t!("assets.window_title").to_string(),
        WindowBounds::Windowed(bounds),
        Some(size(px(900.0), px(560.0))),
        owner,
    );
    opts.display_id = display_id;
    let b = backend.clone();
    if let Ok(handle) = cx.open_window(opts, move |window, cx| {
        crate::windowing::configure_shell_clear_color(window, cx);
        let view = cx.new(|cx| AssetsView::new(b, AssetsScope::All, true, true, window, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    }) {
        backend.update(cx, |bk, _| bk.assets_window = Some(handle));
    }
}
