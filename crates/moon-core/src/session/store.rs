//! Per-core АККАУНТНЫЕ данные: статус, ордера, детекты, стратегии. Свои у каждого
//! ядра. Рыночные данные (крестики/стакан) — общие для биржи и живут отдельно в
//! `crate::market::MarketStore` (дедуп по ядру-провайдеру), сюда не попадают.
//!
//! Версии (revision) заменяют dirty-флаги: каждая панель сама решает, когда
//! перезаливать данные (важно, когда одно ядро показано в нескольких панелях).

use std::collections::{HashMap, VecDeque};

use crate::applog::LogLine;
use crate::feed::{
    AssetsSnapshot, ClientSettings, ConnStatus, DetectRow, FeedMsg, LevManageState, LicenseState,
    OrderRow, RuntimeState, StrategyRow, StrategySchemaModel, TransferAssetsSnapshot,
};
use crate::session::order_lines::OrderLineStore;
use crate::util::now_unix_ms_i64;

/// Сколько последних детектов держим в памяти на ядро.
const MAX_DETECTS: usize = 2000;

/// Сколько последних строк серверного лога держим в памяти на ядро (для живого
/// просмотра/поиска). История глубже — в файлах logs/<дата>_<ядро>.log.
const MAX_LOG: usize = 5000;

pub type CoreId = u64;

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
    /// Последние строки серверного лога ядра (кольцо, обрезается до MAX_LOG).
    pub log: VecDeque<LogLine>,
    /// Сырые строки серверного лога с временем приёма терминалом. Нужны diagnostic/FireTest
    /// замерам; UI продолжает читать форматированный `log`.
    pub server_log_raw: VecDeque<crate::feed::CoreLogLine>,
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
}

impl CoreData {
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
            log: VecDeque::new(),
            server_log_raw: VecDeque::new(),
            orders_table_rev: 0,
            order_lines_rev: 0,
            order_lines_rev_ms: 0,
            detects_rev: 0,
            strategies_rev: 0,
            schema_rev: 0,
            assets_rev: 0,
            transfer_rev: 0,
            license_rev: 0,
            client_settings_rev: 0,
            lev_manage_rev: 0,
            runtime_state_rev: 0,
            hedge_mode_rev: 0,
            log_rev: 0,
        }
    }

    /// Снимок последних `max` строк лога ядра (старые→новые) для панели лога.
    pub fn log_snapshot(&self, max: usize) -> Vec<LogLine> {
        let start = self.log.len().saturating_sub(max);
        self.log.iter().skip(start).cloned().collect()
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
            FeedMsg::Status(s) => self.status = s,
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
            FeedMsg::ServerLog(lines) => {
                if !lines.is_empty() {
                    for l in lines {
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

    /// Суммарная ревизия лога всех ядер — дёшево ловит «появились новые строки лога
    /// хоть у какого-то ядра» (App форсит кадр окнам с активной вкладкой «Лог»).
    pub fn log_activity(&self) -> u64 {
        self.cores
            .values()
            .fold(0u64, |a, c| a.wrapping_add(c.log_rev))
    }
}
