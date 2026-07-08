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
    MoonDataCell, MoonDataRow, MoonDataTable, MoonDataTableColumn, MoonDataTableState, MoonInput,
    MoonInputState, MoonPalette, MoonTone, MoonWindowFrame, Panel, PanelEvent, PanelState, Root,
    h_flex, v_flex,
};

use crate::Backend;
use crate::design;
use crate::panels::{RenderGate, num};
use moon_core::feed::{AssetRow, TransferAssetRow, WalletKind};
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
    /// Ядро строки (для клика по тикеру → открыть чарт на Main и торговых кнопок).
    pub(super) core: CoreId,
    pub(super) core_name: String,
    pub(super) row: AssetRow,
    /// Текущая стоимость в USDT.
    pub(super) value: f64,
    /// Рынок строки (`row.market`) реально существует у ядра — гейт кнопки «Market sell»
    /// (у синтетических кошельковых строк рынка `<coin><quote>` может не быть, напр. USDTUSDC).
    pub(super) market_exists: bool,
}

/// Подытог по ядру: баланс свободно/итого в USDT (для левого списка ядер у «Кошельков»).
#[derive(Clone)]
pub(super) struct CoreAgg {
    pub(super) id: CoreId,
    /// Свободный баланс в USDT (btc_total * курс).
    pub(super) free: f64,
    /// Итоговый баланс в USDT (btc_full * курс, с нереализ. PnL).
    pub(super) total: f64,
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

/// Денежный формат USDT: тысячи через пробел, дробная через `,`, знак `$` в конце.
/// Точность — максимум сотые, минимум десятые (`fmt::usd`): `1 111,24$` / `1 111,0$`.
pub(super) fn money(v: f64) -> String {
    let neg = v < 0.0;
    let s = moon_core::util::fmt::usd(v.abs()); // "1111.24" / "1111.0"
    let (int, frac) = s.split_once('.').unwrap_or((s.as_str(), "0"));
    format!(
        "{}{},{frac}$",
        if neg { "-" } else { "" },
        group_thousands(int)
    )
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
    /// Свёрнута ли секция позиций/балансов (таблица сверху).
    pub(super) positions_collapsed: bool,
    /// Свёрнута ли секция кошельков (список ядер + Спот/Фьючерсы/Квартальные).
    pub(super) wallets_collapsed: bool,
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
    /// Состояние таблицы позиций (ширины/сортировка колонок) — своё, чтобы ширины
    /// персистились через [`crate::table_persist`].
    table_state: Entity<MoonDataTableState>,
    /// Id хранилища ширин с контекстом: dock-вкладка = `assets-table:dock`, отдельное/откреп.
    /// окно (`show_wallets`) = `:win`. Свои ширины на режим.
    widths_id: String,
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

        // Контекст ширин: отдельное/откреплённое окно (есть контейнеры кошельков) = `:win`,
        // dock-вкладка = `:dock`. Свои сохранённые ширины на каждый режим.
        let widths_id = crate::table_persist::ctx_id("assets-table", show_wallets);
        let saved_widths = crate::table_persist::saved(backend.read(cx), &widths_id);
        let table_state = cx.new(|_| {
            let mut s = MoonDataTableState::new();
            s.column_widths = saved_widths;
            s
        });
        // Ресайз колонки мутирует state → сохраняем ширины (универсальный сейвер).
        cx.observe(&table_state, |this, state, cx| {
            crate::table_persist::persist(&this.backend, &this.widths_id, &state, cx);
        })
        .detach();

        let mut this = Self {
            backend,
            scope,
            windowed,
            show_wallets,
            selected_core: None,
            show_all: false,
            positions_collapsed: false,
            wallets_collapsed: false,
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
            table_state,
            widths_id,
            dock: None,
            focus: cx.focus_handle(),
        };
        // Запросить transfer-активы у ВСЕХ ядер охвата: спотовые кошельки нужны не только
        // выбранному ядру (нижние контейнеры), но и таблице сверху — часть бирж (Bitget)
        // отдаёт купленные монеты ТОЛЬКО через transfer_assets, не в per-market балансах.
        let cores: Vec<CoreId> = this
            .scope_cores(this.backend.read(cx))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        this.selected_core = cores.first().copied();
        for core in &cores {
            if let Err(error) = this.backend.read(cx).session.refresh_transfer_assets(*core) {
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
            // ДИАГ (env MOON_ASSETS_DIAG): что реально пришло в balance_position ядра —
            // есть ли строка для рынка позиции и её поля. Показывает, скрыта ли поза
            // фильтром или её вообще нет в balance_position (тогда причина в источнике).
            if std::env::var_os("MOON_ASSETS_DIAG").is_some() {
                log::error!(
                    "[assets_diag] core={name} futures_acc={} rows={}",
                    cd.assets.futures_account,
                    cd.assets.rows.len()
                );
                for r in &cd.assets.rows {
                    log::error!(
                        "[assets_diag]   market={} coin={} pos_size={} qty={} qty_full={} value={:.2} min_lot={:.2} price={}",
                        r.market, r.coin, r.pos_size, r.qty, r.qty_full, r.value_usdt, r.min_lot_usd, r.price
                    );
                }
            }
            // Монеты, уже показанные из per-market строк — чтобы не задублировать их
            // спотовым кошельком (`transfer_assets`) ниже.
            let mut seen_coin: std::collections::HashSet<String> = std::collections::HashSet::new();
            for row in &cd.assets.rows {
                let row = row.clone();
                // Открытую позу показываем ЦЕЛИКОМ (как MoonBot): выставленное на закрытие
                // (sell/TP-ордера) из размера НЕ вычитаем — иначе поза, весь размер которой
                // висит в ордерах, обнуляется и пропадает из «Активов», хотя на чарте есть.
                let value = row.value_usdt;
                // Видимость (правила MoonBot): фьюч-ядра (вкл. CoinM) — ТОЛЬКО открытые
                // позиции (балансы там котируемые, не купленные монеты); спот — все
                // купленные монеты, КРОМЕ котируемой валюты аккаунта (USDT и т.п.) и
                // остатков дешевле минимального лота рынка (непродаваемая пыль; лот
                // неизвестен → старый порог 1$). «Показать всё» снимает фильтры.
                // Позиция-пыль (хвост округления после вычета «в работе» / частичного
                // закрытия, дешевле минимального лота — её не продать) — не позиция.
                let min_lot = if row.min_lot_usd > 0.0 {
                    row.min_lot_usd
                } else {
                    1.0
                };
                let is_position =
                    row.pos_size != 0.0 && row.pos_size.abs() * row.price >= min_lot;
                let spot_coin_visible =
                    !cd.assets.futures_account && !row.is_quote_asset && value >= min_lot;
                let keep = self.show_all || is_position || spot_coin_visible;
                if !keep {
                    continue;
                }
                seen_coin.insert(row.coin.to_ascii_uppercase());
                out.push(AssetEntry {
                    core: id,
                    core_name: name.clone(),
                    market_exists: cd.assets.markets.contains(&row.market),
                    row,
                    value,
                });
            }
            // Спот-холдинги из КОШЕЛЬКА (`transfer_assets`). У части бирж (Bitget и др.)
            // купленные монеты НЕ привязаны к per-market балансам (`assets.rows` пуст) —
            // они приходят только сюда. Показываем их как продаваемые спот-строки (как в
            // MoonBot: BGB/MAPO с кнопкой Market Sell). Только для СПОТ-аккаунтов, без
            // котируемой валюты и пыли; дедуп против уже показанных монет.
            if !cd.assets.futures_account {
                // Квота аккаунта = base_currency ядра (BaseCheck): у ядра, торгующего в
                // BTCUSDC/ETHUSDC, это USDC. Её баланс в кошельке — кэш, не купленная монета,
                // прячем (как делает ядро для per-market через is_quote_asset). Фолбэк на
                // квоту из конфига, если base_currency пуст (старый сервер).
                let quote = {
                    let base = cd.assets.base_currency.trim();
                    if base.is_empty() {
                        self.core_quote(b, id)
                    } else {
                        base.to_string()
                    }
                };
                let quote_up = quote.to_ascii_uppercase();
                for w in &cd.transfer_assets.spot {
                    let coin_up = w.currency.to_ascii_uppercase();
                    if seen_coin.contains(&coin_up) {
                        continue;
                    }
                    let is_quote = coin_up == quote_up;
                    // Реальное имя рынка из каталога ядра (форматы бирж разные) — нужно ДО
                    // фильтра: по нему вычитаем «в работе». Нет рынка → фолбэк-конкатенация
                    // для отображения, но market_exists=false (Sell скрыт).
                    let resolved = resolve_market(&cd.assets.markets, &w.currency, &quote);
                    // ПОЛНЫЙ удерживаемый остаток кошелька (как MoonBot): `total` = полный баланс
                    // (free + заблокированное в ордерах), `amount` = свободное. Выставленное на
                    // продажу НЕ вычитаем — открытую спот-позу, всё количество которой висит в
                    // TP-ордерах, показываем целиком (иначе строка с ~0 уходит под фильтр пыли).
                    // `value_usdt` кошелька уже посчитан от `total`.
                    let held_qty = w.total;
                    let held_value = w.value_usdt;
                    let keep = self.show_all || (!is_quote && held_value >= 1.0);
                    if !keep {
                        continue;
                    }
                    seen_coin.insert(coin_up);
                    let market_exists = resolved.is_some();
                    let market = resolved.unwrap_or_else(|| format!("{}{}", w.currency, quote));
                    let row = wallet_asset_row(w, &quote, is_quote, market, held_value, held_qty);
                    out.push(AssetEntry {
                        core: id,
                        core_name: name.clone(),
                        market_exists,
                        row,
                        value: held_value,
                    });
                }
            }
        }
        sort_by_value(&mut out);
        out
    }

    /// Котируемая валюта ядра (из его `market` в конфиге) — для сборки спотовых строк
    /// кошелька: символ рынка `<coin><quote>` и определение «это сама квота».
    fn core_quote(&self, b: &Backend, core: CoreId) -> String {
        b.config
            .servers
            .iter()
            .find(|sv| sv.id == core)
            .map(|sv| moon_core::symbol::resolve_quote(&sv.market))
            .unwrap_or_else(|| "USDT".to_string())
    }

    /// Балансы по каждому ядру охвата (свободно/итого в USDT, посчитаны на ядре).
    pub(super) fn per_core(&self, b: &Backend) -> Vec<CoreAgg> {
        let store = b.session.store();
        self.scope_cores(b)
            .into_iter()
            .map(|(id, _name)| {
                let mut free = 0.0;
                let mut total = 0.0;
                if let Some(cd) = store.core(id) {
                    // USDT-баланс уже посчитан на ядре с учётом базовой валюты.
                    free = cd.assets.global.free_usdt;
                    total = cd.assets.global.total_usdt;
                }
                CoreAgg { id, free, total }
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
        self.request_missing_transfers(b);
        self.cached_entries = Rc::new(self.collect(b));
        self.cached_aggs = Rc::new(self.per_core(b));
        self.rebuild_wallet_cache(b);
        self.cached_total_value = self.cached_entries.iter().map(|e| e.value).sum();
        self.cache_sig = Some((sig, self.show_all));
    }

    /// Дозапрос transfer-активов для ядер охвата, которые ещё НЕ прислали ни одного снимка
    /// (`transfer_rev == 0`). На старте ядра ещё не подключены и разовый запрос из `new()`
    /// уходит впустую — здесь ретраим (гейт rebuild ~1 Гц), пока ядро не ответит; после
    /// первого снимка (rev>0) запрос прекращается, даже если спот-кошелёк пуст. Нужно
    /// таблице сверху: часть бирж (Bitget) отдаёт купленные монеты только через transfer.
    fn request_missing_transfers(&self, b: &Backend) {
        let store = b.session.store();
        for (id, _) in &self.cached_cores {
            let rev = store.core(*id).map(|cd| cd.transfer_rev).unwrap_or(0);
            if rev == 0 {
                let _ = b.session.refresh_transfer_assets(*id);
            }
        }
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

/// Реальное имя рынка `<coin>/<quote>` из каталога ядра. Форматы бирж разные: Binance/Bitget
/// — конкатенация (`BTCUSDC`), Gate — с подчёркиванием (`SOVRN_USDT`). Возвращаем найденное
/// имя (для Market Sell / клика по тикеру) или `None`, если рынка нет.
fn resolve_market(markets: &std::collections::HashSet<String>, coin: &str, quote: &str) -> Option<String> {
    let concat = format!("{coin}{quote}");
    if markets.contains(&concat) {
        return Some(concat);
    }
    let under = format!("{coin}_{quote}");
    if markets.contains(&under) {
        return Some(under);
    }
    None
}

/// Синтетическая `AssetRow` из спотового кошелька (`transfer_assets`) — для монет, которых
/// нет в per-market балансах (Bitget и т.п.). `market` — реальное имя рынка из каталога
/// (или фолбэк-конкатенация, если рынка нет — кнопка Sell всё равно скрыта). Цену выводим
/// из стоимости. Позиции/PnL нет (чистый спот-баланс).
fn wallet_asset_row(
    w: &TransferAssetRow,
    quote: &str,
    is_quote: bool,
    market: String,
    free_value: f64,
    qty_free: f64,
) -> AssetRow {
    let price = if w.total.abs() > 0.0 {
        w.value_usdt / w.total
    } else {
        0.0
    };
    AssetRow {
        market,
        coin: w.currency.clone(),
        quote: quote.to_string(),
        listed: 1, // spot
        // Свободный остаток БЕЗ выставленного на продажу (остаток sell-ордеров вычтен
        // вызывающим — transfer-снимок сам заморозку не видит).
        qty: qty_free,
        qty_full: w.total,
        price,
        // Стоимость свободного остатка (без выставленного на продажу) — как в per-market.
        value_usdt: free_value,
        min_lot_usd: 0.0,
        is_quote_asset: is_quote,
        mark_price: 0.0,
        pos_size: 0.0,
        pos_price: 0.0,
        liq_price: 0.0,
        leverage: 0,
        pnl_usdt: 0.0,
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
            crate::table_persist::reset_button("assets-reset-widths", &self.table_state),
            MoonButton::new("assets-open-global")
                .ghost()
                .size(MoonButtonSize::Action)
                .label("⧉")
                .tooltip(t!("assets.open_global_hint").to_string())
                .on_click(move |_, window, app| {
                    let owner_display = window.display(app).map(|d| d.id());
                    open(
                        backend.clone(),
                        Some(window.window_handle()),
                        owner_display,
                        app,
                    );
                })
                .render()
                .into_any_element(),
        ])
    }
}

impl Render for AssetsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::ASSETS_RENDER);
        // Метка живости окна для feed-потоков: пока панель на экране (рендер ≥1 Гц от
        // RenderGate), build_assets идёт 1 Гц; без рендеров метка стареет → 1 раз в 5 с.
        moon_core::feed::note_assets_view_render();
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

        // ── Две сворачиваемые горизонтальные секции ─────────────────────────────────
        // 1 «Позиции» (таблица), 2 «Кошельки» (только в отдельном окне). Секции «Ядра»
        // (плашки) больше НЕТ — балансы ядер видны в списке слева у «Кошельков».
        let aggs = self.cached_aggs.clone();
        let controls = self.controls(count, total_value, cx);
        // Контейнеры переноса (список ядер + кошельки) — в отдельном ОКНЕ (глобальном или
        // откреплённом); во вкладке дока показываем только позиции/балансы (таблица шире).
        let wallets = self.cached_wallets.clone();
        let tree_section = self
            .show_wallets
            .then(|| self.bottom(&cores, &aggs, &wallets, cx).into_any_element());
        let table = table::assets_table("assets-table", entries, &self.table_state, cx);

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
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)));
        // Таблица позиций (контент секции 1): высота-basis по содержимому (пусто → 0,
        // N строк → под N строк), при нехватке места СЖИМАЕТСЯ (скролл внутри) — уступая
        // развёрнутым секциям ниже.
        if !self.positions_collapsed {
            root = root.child(
                v_flex()
                    .h(px(table_natural_h))
                    .min_h(px(0.0))
                    .w_full()
                    .overflow_hidden()
                    .child(table),
            );
        }
        root = root
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
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
pub fn open(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
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
    // Мультимонитор: монитор по сохранённой точке (не-мак) либо от владельца.
    let display_id = crate::windowing::saved_or_owner_display_id(
        saved.map(|g| point(px(g.x as f32), px(g.y as f32))),
        owner,
        owner_display,
        cx,
    );
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
        crate::windowing::activate_new_window(handle.into(), cx);
    }
}
