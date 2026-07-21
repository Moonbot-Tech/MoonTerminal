//! Per-core АККАУНТНЫЕ данные: статус, ордера, детекты, стратегии. Свои у каждого
//! ядра. Рыночные данные (крестики/стакан) — общие для биржи и живут отдельно в
//! `crate::market::MarketStore` (дедуп по ядру-провайдеру), сюда не попадают.
//!
//! Версии (revision) заменяют dirty-флаги: каждая панель сама решает, когда
//! перезаливать данные (важно, когда одно ядро показано в нескольких панелях).

use std::collections::{HashMap, VecDeque};

use crate::applog::LogLine;
use crate::feed::{
    AssetsSnapshot, ChartAlertUpdate, ClientSettings, ConnStatus, DetectRow, EngineActionResult,
    FeedMsg, LevManageState, LicenseState, OrderRow, RuntimeState, StrategyRow,
    StrategySchemaModel, TransferAssetsSnapshot,
};
use crate::session::order_lines::OrderLineStore;
use crate::util::now_unix_ms_i64;

/// Сколько последних детектов держим в памяти на ядро.
const MAX_DETECTS: usize = 2000;

/// Сколько последних строк серверного лога держим в памяти на ядро (для живого
/// просмотра/поиска). История глубже — в файлах logs/<дата>_<ядро>.log.
const MAX_LOG: usize = 5000;

/// Кап очереди недоставленных тостов Engine-действий (копятся, пока ни одно окно
/// не активно; потребитель — Shell активного окна).
const MAX_ENGINE_ACTIONS: usize = 64;

pub type CoreId = u64;

/// The store's best available trust classification for a core's USD balance figures.
///
/// The classification lives here, next to the inputs it reads, because the raw numbers
/// alone cannot be rendered honestly: missing pricing can produce a finite zero or partial sum,
/// and a retained snapshot survives a reconnect. Every consumer of `assets.global` must agree
/// about that, so they all go through [`CoreData::balance_state`] instead of re-deriving the rule
/// from `status`/`assets_rev`/`usd_rate_known` on their own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BalanceState {
    /// A snapshot exists, the connection is ready, no stale marker remains, and the USD
    /// valuation is valid. See [`CoreData::assets_stale`] for the freshness limit.
    Live,
    /// The connection is not ready, or it became ready but still awaits a fresh snapshot.
    /// Retained figures may be shown only with an explicit stale marker.
    Stale,
    /// No snapshot has arrived: the balance is UNKNOWN, not zero.
    Awaiting,
    /// A snapshot exists but its free/total USD valuation is incomplete or non-finite.
    /// The figures must render as unavailable rather than as a zero or partial balance.
    Unpriced,
}

impl BalanceState {
    /// Whether there is a usable number to render and to sum.
    pub fn has_value(self) -> bool {
        matches!(self, BalanceState::Live | BalanceState::Stale)
    }

    /// Whether the store classifies the number as current enough to show without a stale marker.
    ///
    /// This is the companion to [`Self::has_value`]: one asks whether there is a figure, the
    /// other whether the available freshness signals classify it as live. The known limit on
    /// [`CoreData::assets_stale`] still applies.
    pub fn is_current(self) -> bool {
        matches!(self, BalanceState::Live)
    }

    /// Stable small integer for hashing this state into a render signature.
    ///
    /// Exists so consumers do not invent their own numbering: the exhaustive match keeps a new
    /// variant a compile error here rather than a silently unhashed state somewhere downstream.
    pub fn code(self) -> u64 {
        match self {
            BalanceState::Live => 1,
            BalanceState::Stale => 2,
            BalanceState::Awaiting => 3,
            BalanceState::Unpriced => 4,
        }
    }
}

pub struct CoreData {
    pub status: ConnStatus,
    /// Открытые ордера ядра (все рынки).
    pub orders: Vec<OrderRow>,
    /// Ретейн-стор линий ордеров для чарта (история + закрытые, всю сессию).
    pub order_lines: OrderLineStore,
    /// Последние детекты ядра (кольцо, обрезается до MAX_DETECTS).
    pub detects: VecDeque<DetectRow>,
    /// Стратегии ядра (последний снимок; для окна стратегий).
    pub strategies: Vec<StrategyRow>,
    /// Схема стратегий ядра (секции/поля по видам). None пока не пришла.
    pub schema: Option<StrategySchemaModel>,
    /// Активы/позиции ядра (последний снимок; для окна «Активы»).
    pub assets: AssetsSnapshot,
    /// Whether a non-`Ready` status occurred after the latest assets message. The snapshot and
    /// `assets_rev` remain retained across reconnect, so returning to `Ready` cannot establish
    /// freshness by itself.
    ///
    /// KNOWN LIMIT: the marker is cleared by the next `FeedMsg::Assets`, and the live feed emits
    /// those on ANY domain event by REBUILDING the retained snapshot — so arrival proves the core
    /// is talking, not that the balances behind it are current. After a reconnect the figures can
    /// therefore read as `Live` while still being pre-outage. The feed requests a balance refresh
    /// on reconnect, but a failed request or missing response leaves the window unbounded. Closing
    /// it properly needs a connection generation / balance revision carried on the payload, so
    /// this flag can be cleared only by data proven to be current.
    pub assets_stale: bool,
    /// Transfer-активы ядра по кошелькам (для дерева переноса). Пусто, пока не запрошено.
    pub transfer_assets: TransferAssetsSnapshot,
    /// License/Free-PRO/MoonCredits state ядра. None, пока ядро не ответило.
    pub license: Option<LicenseState>,
    /// Снимок настроек клиента ядра (TP/SL/sell/iceberg/…). None, пока не пришёл.
    pub client_settings: Option<ClientSettings>,
    /// Снимок управления плечом ядра. None, пока не пришёл.
    pub lev_manage: Option<LevManageState>,
    /// Runtime/passive-mode state ядра. None, пока не пришёл.
    pub runtime_state: Option<RuntimeState>,
    /// Hedge-mode аккаунта (dual-side позиции). None, пока ядро не ответило.
    pub hedge_mode: Option<bool>,
    /// Непоказанные результаты Engine-действий (для тостов). Дренируется Shell'ом
    /// активного окна через [`CoreData::take_engine_actions`].
    engine_actions: VecDeque<EngineActionResult>,
    /// Авторитетные chart-алерты ядра: (market, obj_uid) → blob (`TChartObject.Save()`,
    /// непрозрачный). Набор ведёт сервер; после реконнекта фид запрашивает снапшот,
    /// который приходит теми же Upserted (перезапись по ключу). Blob нужен для
    /// re-upsert (вкл/выкл алерта) и реверса формата.
    pub chart_alerts: HashMap<(String, u64), Vec<u8>>,
    /// Последние строки серверного лога ядра (кольцо, обрезается до MAX_LOG).
    pub log: VecDeque<LogLine>,
    /// Сырые строки серверного лога с временем приёма терминалом. Нужны diagnostic/FireTest
    /// замерам; UI продолжает читать форматированный `log`.
    pub server_log_raw: VecDeque<crate::feed::CoreLogLine>,
    /// Последний снимок системных метрик ядра (CPU/память), вытащенный из строк лога
    /// (`sys_status`). moonproto их типизированно не шлёт — только текстом в `ServerLog`.
    pub sys: crate::session::sys_status::CoreSysStatus,
    /// Растёт при каждом новом снимке открытых ордеров; этим гейтится таблица Orders.
    pub orders_table_rev: u64,
    /// Растёт только при изменении геометрии/состояния ордерных линий на графике.
    pub order_lines_rev: u64,
    /// Локальное время последнего bump `order_lines_rev`.
    pub order_lines_rev_ms: i64,
    pub detects_rev: u64,
    pub strategies_rev: u64,
    pub schema_rev: u64,
    pub assets_rev: u64,
    pub transfer_rev: u64,
    pub license_rev: u64,
    pub client_settings_rev: u64,
    pub lev_manage_rev: u64,
    pub runtime_state_rev: u64,
    pub hedge_mode_rev: u64,
    pub log_rev: u64,
    pub chart_alerts_rev: u64,
    /// Растёт при обновлении системных метрик (`sys`) из строк лога — гейт таблицы «Ядра».
    pub sys_rev: u64,
}

impl CoreData {
    /// Create an empty per-core store in the connecting state.
    pub fn new() -> Self {
        Self {
            status: ConnStatus::Connecting,
            orders: Vec::new(),
            order_lines: OrderLineStore::default(),
            detects: VecDeque::new(),
            strategies: Vec::new(),
            schema: None,
            assets: AssetsSnapshot::default(),
            transfer_assets: TransferAssetsSnapshot::default(),
            license: None,
            client_settings: None,
            lev_manage: None,
            runtime_state: None,
            hedge_mode: None,
            engine_actions: VecDeque::new(),
            chart_alerts: HashMap::new(),
            log: VecDeque::new(),
            server_log_raw: VecDeque::new(),
            sys: crate::session::sys_status::CoreSysStatus::default(),
            orders_table_rev: 0,
            order_lines_rev: 0,
            order_lines_rev_ms: 0,
            detects_rev: 0,
            strategies_rev: 0,
            schema_rev: 0,
            assets_stale: false,
            assets_rev: 0,
            transfer_rev: 0,
            license_rev: 0,
            client_settings_rev: 0,
            lev_manage_rev: 0,
            runtime_state_rev: 0,
            hedge_mode_rev: 0,
            log_rev: 0,
            chart_alerts_rev: 0,
            sys_rev: 0,
        }
    }

    /// Снимок последних `max` строк лога ядра (старые→новые) для панели лога.
    pub fn log_snapshot(&self, max: usize) -> Vec<LogLine> {
        let start = self.log.len().saturating_sub(max);
        self.log.iter().skip(start).cloned().collect()
    }

    /// Забирает накопленные результаты Engine-действий (очередь дренируется:
    /// потребитель один — Shell активного окна, тосты показываются один раз).
    pub fn take_engine_actions(&mut self) -> Vec<EngineActionResult> {
        self.engine_actions.drain(..).collect()
    }

    /// Снимок сырых строк серверного лога (старые→новые) для diagnostic замеров.
    pub fn raw_server_log_snapshot(&self, max: usize) -> Vec<crate::feed::CoreLogLine> {
        let start = self.server_log_raw.len().saturating_sub(max);
        self.server_log_raw.iter().skip(start).cloned().collect()
    }

    /// Применяет только аккаунтные сообщения. Identity/CoreBase/MarketDataChanged
    /// маршрутизируются координатором мимо CoreData.
    pub fn apply(&mut self, msg: FeedMsg) {
        match msg {
            FeedMsg::Status(s) => {
                // Any non-Ready status marks the retained snapshot stale, so a reconnect cannot
                // promote pre-outage figures on the strength of the status alone. What clears the
                // marker is documented on `assets_stale` — including what it does NOT prove.
                if !matches!(s, ConnStatus::Ready) {
                    self.assets_stale = true;
                }
                self.status = s;
            }
            FeedMsg::Orders(orders) => {
                // Сначала обновляем ретейн-стор линий (трассы/узлы/закрытия) по
                // свежему снимку, затем перемещаем его в список для таблицы.
                // Таблица и график гейтятся разными revision: таблице важен любой
                // новый snapshot, графику — только изменение линий.
                let changed = self.order_lines.update(&orders);
                self.orders = orders;
                self.orders_table_rev = self.orders_table_rev.wrapping_add(1);
                if changed {
                    self.order_lines_rev = self.order_lines_rev.wrapping_add(1);
                    self.order_lines_rev_ms = now_unix_ms_i64();
                }
            }
            FeedMsg::OrderLines(orders) => {
                let changed = self.order_lines.update(&orders);
                if changed {
                    self.order_lines_rev = self.order_lines_rev.wrapping_add(1);
                    self.order_lines_rev_ms = now_unix_ms_i64();
                }
            }
            FeedMsg::Detects(detects) => {
                if !detects.is_empty() {
                    // detect-diag: дошли до стора (CoreData) и бампаем detects_rev — этот rev
                    // дальше гейтит ChartTabs::ingest через chart_tabs_sig. (env MOON_DETECT_DIAG.)
                    crate::detect_diag::line(&format!(
                        "[store] +{} detects → rev={}",
                        detects.len(),
                        self.detects_rev.wrapping_add(1)
                    ));
                    for det in detects {
                        self.detects.push_back(det);
                    }
                    if self.detects.len() > MAX_DETECTS {
                        while self.detects.len() > MAX_DETECTS {
                            self.detects.pop_front();
                        }
                    }
                    self.detects_rev = self.detects_rev.wrapping_add(1);
                }
            }
            FeedMsg::Strategies(strategies) => {
                self.strategies = strategies;
                self.strategies_rev = self.strategies_rev.wrapping_add(1);
            }
            FeedMsg::StrategySchema(schema) => {
                self.schema = Some(schema);
                self.schema_rev = self.schema_rev.wrapping_add(1);
            }
            FeedMsg::Assets(assets) => {
                self.assets = assets;
                self.assets_stale = false;
                self.assets_rev = self.assets_rev.wrapping_add(1);
            }
            FeedMsg::TransferAssets(transfer) => {
                self.transfer_assets = transfer;
                self.transfer_rev = self.transfer_rev.wrapping_add(1);
            }
            FeedMsg::License(license) => {
                if self.license != Some(license) {
                    self.license = Some(license);
                    self.license_rev = self.license_rev.wrapping_add(1);
                }
            }
            FeedMsg::ClientSettings(settings) => {
                if self.client_settings.as_ref() != Some(&settings) {
                    self.client_settings = Some(settings);
                    self.client_settings_rev = self.client_settings_rev.wrapping_add(1);
                }
            }
            FeedMsg::LevManage(lev) => {
                if self.lev_manage.as_ref() != Some(&lev) {
                    self.lev_manage = Some(lev);
                    self.lev_manage_rev = self.lev_manage_rev.wrapping_add(1);
                }
            }
            FeedMsg::RuntimeState(state) => {
                if self.runtime_state != Some(state) {
                    self.runtime_state = Some(state);
                    self.runtime_state_rev = self.runtime_state_rev.wrapping_add(1);
                }
            }
            FeedMsg::HedgeMode(on) => {
                if self.hedge_mode != Some(on) {
                    self.hedge_mode = Some(on);
                    self.hedge_mode_rev = self.hedge_mode_rev.wrapping_add(1);
                }
            }
            FeedMsg::EngineActions(results) => {
                self.engine_actions.extend(results);
                while self.engine_actions.len() > MAX_ENGINE_ACTIONS {
                    self.engine_actions.pop_front();
                }
            }
            FeedMsg::ChartAlerts(updates) => {
                for u in updates {
                    match u {
                        ChartAlertUpdate::Upserted(row) => {
                            self.chart_alerts
                                .insert((row.market, row.obj_uid), row.blob);
                        }
                        ChartAlertUpdate::Deleted { market, obj_uid } => {
                            self.chart_alerts.remove(&(market, obj_uid));
                        }
                    }
                }
                self.chart_alerts_rev = self.chart_alerts_rev.wrapping_add(1);
            }
            FeedMsg::ServerLog(lines) => {
                if !lines.is_empty() {
                    for l in lines {
                        // Парсинг системных метрик (CoreSysStatus::parse_line) ВРЕМЕННО ОТКЛЮЧЁН:
                        // наполнять панель «Статус ядер» из строк лога нецелесообразно
                        // (редко/фрагментарно). Поле `sys`/`sys_rev` и модуль `sys_status`
                        // оставлены — вернём разбор, когда moonproto начнёт слать метрики
                        // типизированно. Тогда достаточно вернуть строку ниже:
                        //   if self.sys.parse_line(&l.msg, l.time_ms) { sys_changed = true; }
                        self.server_log_raw.push_back(l.clone());
                        self.log.push_back(LogLine::core(l.time_ms, l.msg));
                    }
                    if self.log.len() > MAX_LOG {
                        let drop = self.log.len() - MAX_LOG;
                        self.log.drain(0..drop);
                    }
                    if self.server_log_raw.len() > MAX_LOG {
                        let drop = self.server_log_raw.len() - MAX_LOG;
                        self.server_log_raw.drain(0..drop);
                    }
                    self.log_rev = self.log_rev.wrapping_add(1);
                }
            }
            // Идентификационные/рыночные wake-сообщения сюда не маршрутизируются.
            FeedMsg::Identity(_) | FeedMsg::CoreBase { .. } | FeedMsg::MarketDataChanged(_) => {}
        }
    }

    /// Best available trust classification for this core's `assets.global` USD figures.
    ///
    /// `Unpriced` outranks `Stale`: an unpriced figure has no number to show at all, so its
    /// freshness is moot. Staleness needs BOTH inputs — `assets_stale` covers the reconnect
    /// window (status returns to `Ready` before the new snapshot lands), while the `status`
    /// check covers a snapshot that arrived before the link ever reached `Ready`. The generation
    /// ambiguity documented on [`Self::assets_stale`] prevents this from proving freshness.
    pub fn balance_state(&self) -> BalanceState {
        if self.assets_rev == 0 {
            BalanceState::Awaiting
        } else if !self.assets.global.usd_rate_known {
            BalanceState::Unpriced
        } else if self.assets_stale || !matches!(self.status, ConnStatus::Ready) {
            BalanceState::Stale
        } else {
            BalanceState::Live
        }
    }
}

impl Default for CoreData {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default)]
pub struct CoreStore {
    cores: HashMap<CoreId, CoreData>,
}

impl CoreStore {
    pub fn ensure(&mut self, id: CoreId) {
        self.cores.entry(id).or_default();
    }

    /// Удалить аккаунтные данные ядра (сервер убран из конфига). Live-подписки/окна
    /// чистит вызывающий (`SessionManager::reconcile`).
    pub fn remove(&mut self, id: CoreId) {
        self.cores.remove(&id);
    }

    pub fn core(&self, id: CoreId) -> Option<&CoreData> {
        self.cores.get(&id)
    }

    pub fn core_mut(&mut self, id: CoreId) -> Option<&mut CoreData> {
        self.cores.get_mut(&id)
    }

    /// Снимок статусов всех ядер (id → клон статуса) — для бейджей в Настройках.
    pub fn statuses(&self) -> impl Iterator<Item = (CoreId, ConnStatus)> + '_ {
        self.cores.iter().map(|(id, d)| (*id, d.status.clone()))
    }

    /// Итератор по ядрам (id, данные) — для реконсиляции chart-алертов и т.п.
    pub fn cores(&self) -> impl Iterator<Item = (CoreId, &CoreData)> + '_ {
        self.cores.iter().map(|(id, d)| (*id, d))
    }

    /// Суммарная ревизия chart-алертов всех ядер — дёшево ловит «серверный набор
    /// алертов изменился хоть у какого-то ядра» (гейт реконсиляции remote-фигур).
    pub fn chart_alerts_activity(&self) -> u64 {
        self.cores
            .values()
            .fold(0u64, |a, c| a.wrapping_add(c.chart_alerts_rev))
    }

    /// Суммарная ревизия лога всех ядер — дёшево ловит «появились новые строки лога
    /// хоть у какого-то ядра» (App форсит кадр окнам с активной вкладкой «Лог»).
    pub fn log_activity(&self) -> u64 {
        self.cores
            .values()
            .fold(0u64, |a, c| a.wrapping_add(c.log_rev))
    }
}

#[cfg(test)]
/// Checks for the balance trust classifier every UI surface reads through.
mod tests;
