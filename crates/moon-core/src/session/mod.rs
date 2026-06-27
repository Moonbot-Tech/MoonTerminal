//! SessionManager: по одному backend-потоку (ядру) на каждый сервер из конфига.
//!
//! Данные ядер делятся на два плана:
//! - АККАУНТНЫЙ (статус/ордера/детекты/стратегии) — свой у каждого ядра, лежит в
//!   `CoreStore` по CoreId;
//! - РЫНОЧНЫЙ (крестики/стакан) — общий для биржи, дедуплицируется по ядру-провайдеру
//!   и лежит в `MarketStore` (см. `crate::market`).
//!
//! Координатор (см. `coordinator.rs`) узнаёт биржу каждого ядра из `Identity`,
//! избирает провайдера на биржу и шлёт ядрам рыночную роль командой `SetMarket`.

pub mod coordinator;
pub mod order_lines;
pub mod store;

pub use store::{CoreId, CoreStore};

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use anyhow::{anyhow, Result};
use moonproto::state::OrderBookKind;

use crate::config::{AppConfig, ServerConfig};
use crate::data::OrderBookModel;
use crate::db::ReportTx;
use crate::feed::{
    self, ClientSettingsEdit, ConnStatus, CoreCmd, ExchangeId, FeedHandle, FeedMsg, FeedWakeTx,
    LevManageEdit, NewStrategySpec, OrderStopKind, WalletKind,
};
use crate::market::{MarketDataMode, MarketDataSource, MarketStore, SharedMarketStore};

pub struct CoreSession {
    pub id: CoreId,
    pub name: String,
    pub group: String,
    /// Сигнатура connection-relevant полей (key/feed/synthetic), с которыми поднят
    /// feed-поток. `reconcile` пере-поднимает ядро только если она изменилась —
    /// смена имени/группы/рынка/цвета такого не требует.
    conn_sig: u64,
    handle: FeedHandle,
}

/// Стабильный (в пределах процесса) хэш connection-relevant полей сервера. Меняется —
/// нужно пере-поднять feed-поток. Имя/группа/рынок/цвет/связка/размеры сюда НЕ входят:
/// их смена обновляется на месте без реконнекта.
fn conn_sig(server: &ServerConfig) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    server.key.expose().hash(&mut h);
    let f = server.feed;
    [
        f.orders,
        f.detects,
        f.reports,
        f.balance,
        f.strategies,
        f.log,
        f.alerts,
        f.arb,
    ]
    .hash(&mut h);
    server.synthetic.hash(&mut h);
    h.finish()
}

/// Сводка подключений для статус-бара: сколько ядер готово из общего числа +
/// список «лежащих» (имя, статус) для всплывающей подсказки.
pub struct ConnSummary {
    pub ready: usize,
    pub total: usize,
    /// Не-Ready ядра: (имя, статус). Для тултипа «кто не подключён и почему».
    pub down: Vec<(String, ConnStatus)>,
}

/// Сводка лицензий ядер одной группы для статус-бара окна.
#[derive(Clone, Debug, Default)]
pub struct LicenseSummary {
    pub total: usize,
    pub known: usize,
    pub paid: usize,
    pub free: usize,
    pub moon_credits: i64,
    pub moon_credits_hold: i64,
    pub moon_credits_auction: i64,
}

pub struct SessionManager {
    sessions: Vec<CoreSession>,
    feed_wake: Option<FeedWakeTx>,
    /// Аккаунтный план: статус/ордера/детекты/стратегии по ядру. Снаружи —
    /// только чтение через [`SessionManager::store`]; мутирует лишь сам менеджер.
    store: CoreStore,
    /// Рыночный план: общий буфер вне GPUI entity. Live-feed только будит; данные
    /// в буфер тянет `MarketDataSource` из MoonProto snapshots.
    market: SharedMarketStore,
    /// Pull/read-model bridge shared by UI listener and native chart frames.
    market_source: MarketDataSource,
    /// Режим источника рыночных данных (рубильник; пока дефолт Dedup).
    mode: MarketDataMode,
    /// Ядро → биржа (из `Identity`). Без идентичности провайдер не назначается.
    core_key: HashMap<CoreId, ExchangeId>,
    /// Ядро → базовая валюта аккаунта (из `CoreBase`): "USDT"/"BTC"/…. Нужна UI для
    /// дефолтов размера ордера по базе (BTC vs USDT). Пусто, пока ядро не идентифицировано.
    core_base: HashMap<CoreId, String>,
    /// Ядро → ядро-провайдер его рыночных данных (dedup: один на биржу; per-core: сам).
    core_provider: HashMap<CoreId, CoreId>,
    /// Биржа → избранный провайдер (для удержания/failover в режиме Dedup).
    providers: HashMap<ExchangeId, CoreId>,
    /// Провайдер → обслуживаемые рынки (union открытых чартов + linger).
    wanted: HashMap<CoreId, HashSet<String>>,
    /// (провайдер, рынок) → дедлайн снятия после закрытия последнего чарта (linger).
    pending_drop: HashMap<(CoreId, String), Instant>,
    /// Последняя посланная ядру роль `(provider, markets, orderbook_markets)` — чтобы не слать
    /// дубликаты команд. `orderbook_markets` — подмножество `markets`, которым нужен стакан.
    last_cmd: HashMap<CoreId, (bool, Vec<String>, Vec<String>)>,
}

#[derive(Clone, Debug, Default)]
pub struct DrainStats {
    /// At least one feed message was applied to session state.
    pub any: bool,
    /// Data visible to chart GPU state changed: market ticks/book/price-lines or order lines.
    pub chart_data: bool,
    /// Slow GPUI chrome/account state changed and the Backend entity should be notified.
    pub ui_state: bool,
}

impl SessionManager {
    /// Поднимает live-сессии по всем серверам конфига. Нет серверов — нет сессий.
    /// `reports` — общий канал к SQLite-writer'у (клонируется на каждое ядро).
    pub fn start(
        config: &AppConfig,
        epoch_ms: f64,
        reports: Option<&ReportTx>,
        feed_wake: Option<FeedWakeTx>,
    ) -> Self {
        let market = MarketStore::shared(epoch_ms);
        let market_source = MarketDataSource::new(market.clone());
        let mut store = CoreStore::default();
        let mut sessions = Vec::new();
        for s in config
            .servers
            .iter()
            .filter(|s| s.active && config.group(&s.group).active)
            .cloned()
        {
            store.ensure(s.id);
            let id = s.id;
            let name = s.name.clone();
            let group = s.group.clone();
            let sig = conn_sig(&s);
            let handle = feed::spawn(
                s,
                config.chart_memory_percent,
                reports.cloned(),
                feed_wake.clone(),
                Some(market.clone()),
            );
            market_source.set_client(id, handle.client.clone());
            sessions.push(CoreSession {
                id,
                name,
                group,
                conn_sig: sig,
                handle,
            });
            log::info!("session up: core={id}");
        }
        if sessions.is_empty() {
            log::warn!("нет серверов в конфиге — добавь ядра в Настройках");
        }
        Self {
            sessions,
            feed_wake,
            store,
            market,
            market_source,
            mode: MarketDataMode::default(),
            core_key: HashMap::new(),
            core_base: HashMap::new(),
            core_provider: HashMap::new(),
            providers: HashMap::new(),
            wanted: HashMap::new(),
            pending_drop: HashMap::new(),
            last_cmd: HashMap::new(),
        }
    }

    /// Инкрементально приводит набор живых сессий к конфигу БЕЗ полного рестарта:
    /// добавляет новые ядра, гасит удалённые/деактивированные, переподнимает только те,
    /// у кого сменились connection-поля (key/feed/synthetic). Неизменные ядра не трогает —
    /// их feed-поток, данные и подписки сохраняются (нет реконнект-флика и потери истории
    /// при добавлении соседнего сервера). Имя/группу обновляет на месте.
    ///
    /// Заменяет прежний `SessionManager::start` в пути применения настроек: раньше любое
    /// структурное изменение пере-поднимало ВСЕ ядра.
    pub fn reconcile(&mut self, config: &AppConfig, reports: Option<&ReportTx>) {
        let mem = config.chart_memory_percent;
        let desired: Vec<ServerConfig> = config
            .servers
            .iter()
            .filter(|s| s.active && config.group(&s.group).active)
            .cloned()
            .collect();
        let desired_ids: HashSet<CoreId> = desired.iter().map(|s| s.id).collect();

        // 1) Выбывшие ядра (удалены / сняли active / выключили группу) → погасить и вычистить.
        let removed: Vec<CoreId> = self
            .sessions
            .iter()
            .map(|s| s.id)
            .filter(|id| !desired_ids.contains(id))
            .collect();
        for id in removed {
            self.drop_core(id);
        }

        // 2) Новые ядра поднять, изменённые — переподнять, прочим обновить мету на месте.
        for s in &desired {
            let sig = conn_sig(s);
            match self.sessions.iter().position(|x| x.id == s.id) {
                None => self.spawn_core(s, sig, mem, reports),
                Some(idx) => {
                    self.sessions[idx].name = s.name.clone();
                    self.sessions[idx].group = s.group.clone();
                    if self.sessions[idx].conn_sig != sig {
                        self.respawn_core(s, sig, mem, reports);
                    }
                }
            }
        }
    }

    /// Поднять feed-поток нового ядра и зарегистрировать его.
    fn spawn_core(&mut self, s: &ServerConfig, sig: u64, mem: u16, reports: Option<&ReportTx>) {
        let id = s.id;
        self.store.ensure(id);
        let handle = feed::spawn(
            s.clone(),
            mem,
            reports.cloned(),
            self.feed_wake.clone(),
            Some(self.market.clone()),
        );
        self.market_source.set_client(id, handle.client.clone());
        self.sessions.push(CoreSession {
            id,
            name: s.name.clone(),
            group: s.group.clone(),
            conn_sig: sig,
            handle,
        });
        log::info!("session up: core={id}");
    }

    /// Пере-поднять feed-поток существующего ядра (сменились connection-поля). Дроп
    /// старого хэндла завершает старый поток; координация ядра сбрасывается под переизбор.
    fn respawn_core(&mut self, s: &ServerConfig, sig: u64, mem: u16, reports: Option<&ReportTx>) {
        let id = s.id;
        let handle = feed::spawn(
            s.clone(),
            mem,
            reports.cloned(),
            self.feed_wake.clone(),
            Some(self.market.clone()),
        );
        self.market_source.set_client(id, handle.client.clone());
        if let Some(sess) = self.sessions.iter_mut().find(|x| x.id == id) {
            sess.handle = handle;
            sess.conn_sig = sig;
            sess.name = s.name.clone();
            sess.group = s.group.clone();
        }
        self.store.ensure(id);
        if let Some(core) = self.store.core_mut(id) {
            core.status = ConnStatus::Connecting;
        }
        self.core_key.remove(&id);
        self.core_base.remove(&id);
        self.core_provider.remove(&id);
        self.providers.retain(|_, prov| *prov != id);
        self.last_cmd.remove(&id);
        log::info!("reconnect (config changed): core={id}");
    }

    /// Погасить ядро (сервер убран/деактивирован): дроп сессии завершает поток, чистим
    /// аккаунтные данные, рыночного клиента и всю координацию.
    fn drop_core(&mut self, id: CoreId) {
        self.sessions.retain(|s| s.id != id); // дроп FeedHandle → поток завершится
        self.store.remove(id);
        self.market_source.remove_client(id);
        self.core_key.remove(&id);
        self.core_base.remove(&id);
        self.core_provider.remove(&id);
        self.providers.retain(|_, prov| *prov != id);
        self.wanted.remove(&id);
        self.pending_drop.retain(|(core, _), _| *core != id);
        self.last_cmd.remove(&id);
        log::info!("session down: core={id}");
    }

    /// Дренирует все каналы ядер. Аккаунтные сообщения → CoreStore; market-data
    /// payload-и сюда не едут: live/synth публикуют их в read-model/MarketStore и
    /// шлют только лёгкий `MarketDataChanged` wake. Identity → core_key.
    /// Зовётся частым data-drain тиком перед `set_open`. Возвращает, что именно
    /// изменилось: общий UI-state и отдельно данные, которые могут менять GPU-пиксели чарта.
    pub fn drain(&mut self) -> DrainStats {
        let mut stats = DrainStats::default();
        for sess in &self.sessions {
            while let Ok(msg) = sess.handle.rx.try_recv() {
                stats.any = true;
                match msg {
                    FeedMsg::Identity(ex) => {
                        self.core_key.insert(sess.id, ex);
                        self.market_source
                            .set_orderbook_kind(sess.id, orderbook_kind_for_exchange(ex));
                        stats.ui_state = true;
                    }
                    FeedMsg::CoreBase { base } => {
                        self.core_base.insert(sess.id, base);
                        stats.ui_state = true;
                    }
                    FeedMsg::MarketDataChanged(markets) => {
                        if !markets.is_empty() {
                            self.market_source.mark_dirty(sess.id, &markets);
                            stats.chart_data = true;
                        }
                    }
                    FeedMsg::Orders(orders) => {
                        if let Some(core) = self.store.core_mut(sess.id) {
                            let before = core.order_lines_rev;
                            core.apply(FeedMsg::Orders(orders));
                            stats.chart_data |= core.order_lines_rev != before;
                            stats.ui_state = true;
                        }
                    }
                    FeedMsg::OrderLines(orders) => {
                        if let Some(core) = self.store.core_mut(sess.id) {
                            let before = core.order_lines_rev;
                            core.apply(FeedMsg::OrderLines(orders));
                            stats.chart_data |= core.order_lines_rev != before;
                        }
                    }
                    other => {
                        if let Some(core) = self.store.core_mut(sess.id) {
                            core.apply(other);
                            stats.ui_state = true;
                        }
                    }
                }
            }
        }
        stats
    }

    /// Debug-only stress fixture for the dev panel fill button.
    ///
    /// Production/default terminal builds deliberately do not enable MoonProto's
    /// diagnostics feature. The real hook is available only through the explicit
    /// `moonproto-diagnostics` feature, which `moon-ui-gpui/debug-tools` enables
    /// for local/manual stress runs.
    #[cfg(feature = "moonproto-diagnostics")]
    pub fn diag_fill_market_history_to_capacity(
        &self,
        core: CoreId,
        market: &str,
        now_ms: i64,
        span_ms: i64,
    ) -> bool {
        let provider = self.core_provider.get(&core).copied().unwrap_or(core);
        let Some(sess) = self.sessions.iter().find(|s| s.id == provider) else {
            log::warn!("diag history fill skipped: provider core={provider} not found");
            return false;
        };
        let Some(client) = sess.handle.client.get() else {
            log::warn!(
                "diag history fill skipped: provider core={provider} has no MoonProto client"
            );
            return false;
        };

        match client.diag_fill_market_history_to_capacity(market, now_ms, span_ms) {
            Ok(filled) => {
                log::info!(
                    "diag history fill: core={core} provider={provider} market={market} \
                     span_ms={span_ms} filled={filled}",
                );
                filled
            }
            Err(err) => {
                log::warn!(
                    "diag history fill failed: core={core} provider={provider} \
                     market={market} span_ms={span_ms}: {err:#}"
                );
                false
            }
        }
    }

    /// Same public method in production builds: the button path stays compiled,
    /// but hidden MoonProto diagnostics are not pulled into the dependency graph.
    #[cfg(not(feature = "moonproto-diagnostics"))]
    pub fn diag_fill_market_history_to_capacity(
        &self,
        core: CoreId,
        market: &str,
        now_ms: i64,
        span_ms: i64,
    ) -> bool {
        let provider = self.core_provider.get(&core).copied().unwrap_or(core);
        log::warn!(
            "diag history fill disabled: MoonProto diagnostics feature is forbidden in terminal \
             core={core} provider={provider} market={market} now_ms={now_ms} span_ms={span_ms}"
        );
        false
    }

    /// Снимок статусов подключения всех ядер (id → статус) — для бейджей в окне
    /// Настроек. Владеющая копия, чтобы не держать заём на сессию.
    pub fn status_map(&self) -> HashMap<CoreId, ConnStatus> {
        self.store.statuses().collect()
    }

    /// Сводка подключений ядер ОДНОЙ группы: ready/total + список не-Ready ядер
    /// (имя, статус). Группа = ОС-окно, поэтому каждый статус-бар показывает свою
    /// группу (3/3 + 7/7 при 10 ядрах в двух группах). Учитываются и headless-ядра
    /// группы — у них тоже есть сессия (show_window влияет лишь на наличие окна).
    pub fn conn_summary_group(&self, group: &str) -> ConnSummary {
        let mut total = 0;
        let mut ready = 0;
        let mut down = Vec::new();
        for s in self.sessions.iter().filter(|s| s.group == group) {
            total += 1;
            let st = self
                .store
                .core(s.id)
                .map(|d| d.status.clone())
                .unwrap_or(ConnStatus::Connecting);
            if st == ConnStatus::Ready {
                ready += 1;
            } else {
                down.push((s.name.clone(), st));
            }
        }
        ConnSummary { ready, total, down }
    }

    /// Сводка license state ядер ОДНОЙ группы. License приходит позже connect/init,
    /// поэтому `known` может быть меньше `total`; UI обязан показывать это честно.
    pub fn license_summary_group(&self, group: &str) -> LicenseSummary {
        let mut out = LicenseSummary::default();
        for s in self.sessions.iter().filter(|s| s.group == group) {
            out.total += 1;
            let Some(core) = self.store.core(s.id) else {
                continue;
            };
            let Some(license) = core.license else {
                continue;
            };
            out.known += 1;
            if license.paid_version {
                out.paid += 1;
            } else {
                out.free += 1;
            }
            out.moon_credits += i64::from(license.moon_credits);
            out.moon_credits_hold += i64::from(license.moon_credits_hold);
            out.moon_credits_auction += i64::from(license.moon_credits_auction);
        }
        out
    }

    /// Переподключить одно ядро: гасит старый backend-поток (дроп хэндла закрывает
    /// его каналы) и поднимает новый по текущему конфигу. Сбрасывает рыночную роль
    /// ядра, чтобы провайдер переизбрался. Неактивные ядра/группы игнорирует.
    pub fn reconnect(&mut self, id: CoreId, config: &AppConfig, reports: Option<&ReportTx>) {
        let Some(server) = config.servers.iter().find(|s| s.id == id).cloned() else {
            return;
        };
        if !(server.active && config.group(&server.group).active) {
            return;
        }
        let name = server.name.clone();
        let group = server.group.clone();
        let sig = conn_sig(&server);
        let handle = feed::spawn(
            server,
            config.chart_memory_percent,
            reports.cloned(),
            self.feed_wake.clone(),
            Some(self.market.clone()),
        );
        self.market_source.set_client(id, handle.client.clone());
        match self.sessions.iter_mut().find(|s| s.id == id) {
            Some(sess) => {
                sess.handle = handle; // дроп старого хэндла → старый поток завершится
                sess.conn_sig = sig;
                sess.name = name;
                sess.group = group;
            }
            None => self.sessions.push(CoreSession {
                id,
                name,
                group,
                conn_sig: sig,
                handle,
            }),
        }
        self.store.ensure(id);
        if let Some(core) = self.store.core_mut(id) {
            core.status = ConnStatus::Connecting;
        }
        // Сброс координации для ядра: пусть провайдер/роль переизберутся заново.
        self.core_key.remove(&id);
        self.core_base.remove(&id);
        self.core_provider.remove(&id);
        self.providers.retain(|_, prov| *prov != id);
        self.last_cmd.remove(&id);
        log::info!("reconnect: core={id}");
    }

    fn send_core_cmd(&self, core: CoreId, cmd: CoreCmd, action: &str) -> Result<()> {
        let Some(s) = self.sessions.iter().find(|s| s.id == core) else {
            return Err(anyhow!("ядро {core} недоступно для команды: {action}"));
        };
        s.handle
            .cmd_tx
            .send(cmd)
            .map_err(|_| anyhow!("канал команд ядра {core} закрыт: {action}"))
    }

    /// Действие со стратегиями ядра (из окна стратегий): единый путь команд через
    /// per-core канал. Сначала синхронизирует галки (`checks`), затем — старт/стоп
    /// отмеченных (`start_stop`). Пустое действие — no-op.
    pub fn apply_strategies(
        &self,
        core: CoreId,
        checks: Vec<(u64, bool)>,
        start_stop: Option<bool>,
    ) -> Result<()> {
        if checks.is_empty() && start_stop.is_none() {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::StrategiesAction { checks, start_stop },
            "strategies action",
        )
    }

    /// Редактирование полей стратегий ядра: на каждую стратегию свой `(id, changes)`. ВСЕ
    /// правки ядра уходят ОДНОЙ командой (полный снимок правится на стороне feed одним
    /// `sync_local_strategies`) — иначе при нескольких выбранных стратегиях одного ядра
    /// второй sync перетирал бы первый (применялось бы к одной).
    pub fn edit_strategies(
        &self,
        core: CoreId,
        edits: Vec<(u64, Vec<(String, String)>)>,
    ) -> Result<()> {
        if edits.is_empty() {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::EditStrategyFields { edits },
            "edit strategies",
        )
    }

    /// Удалить ОДНУ стратегию ядра по `id` (необратимо). Правило «только выключенные»
    /// проверяется в UI до вызова.
    pub fn delete_strategy(&self, core: CoreId, id: u64) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::DeleteStrategy { id }, "delete strategy")
    }

    /// Удалить ПАПКУ ядра целиком по пути (необратимо). Стратегии под папкой должны быть
    /// удалены/перенесены заранее (UI это гарантирует).
    pub fn delete_folder(&self, core: CoreId, path: String) -> Result<()> {
        if path.is_empty() {
            return Ok(());
        }
        self.send_core_cmd(core, CoreCmd::DeleteFolder { path }, "delete folder")
    }

    /// Создать новые стратегии ядра (создание / вставка из буфера). feed добавит их к
    /// полному набору с новыми id и одним sync. Один набор на ядро (вызывать по разу на ядро).
    pub fn create_strategies(&self, core: CoreId, specs: Vec<NewStrategySpec>) -> Result<()> {
        if specs.is_empty() {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::CreateStrategies { specs },
            "create strategies",
        )
    }

    /// Сменить папку существующих стратегий ядра (переименование папки / перенос).
    /// `moves` — `(strategy_id, новый folder_path)`. Один набор на ядро.
    pub fn move_strategies(&self, core: CoreId, moves: Vec<(u64, String)>) -> Result<()> {
        if moves.is_empty() {
            return Ok(());
        }
        self.send_core_cmd(core, CoreCmd::MoveStrategies { moves }, "move strategies")
    }

    /// Перенос актива между кошельками ОДНОГО ядра (drag&drop в окне «Активы»).
    /// `qty` в базовой монете; `from`/`to` — кошельки (Spot/Futures/Quarterly).
    pub fn transfer_asset(
        &self,
        core: CoreId,
        asset: String,
        qty: f64,
        from: WalletKind,
        to: WalletKind,
    ) -> Result<()> {
        if from == to || asset.is_empty() || !(qty > 0.0) {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::TransferAsset {
                asset,
                qty,
                from,
                to,
            },
            "transfer asset",
        )
    }

    /// Запросить свежий список transfer-активов ядра (по всем кошелькам).
    pub fn refresh_transfer_assets(&self, core: CoreId) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::RefreshTransferAssets,
            "refresh transfer assets",
        )
    }

    /// Сконвертировать «пыль» ядра в BNB (необратимо). Per-core.
    pub fn convert_dust(&self, core: CoreId) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::ConvertDust, "convert dust")
    }

    /// Поставить ордер вручную на рынке `market` ядра (ручная торговля). `short` —
    /// сторона позиции (Long/Short); `strategy_id=None` → `StratID=0` (ордер без
    /// стратегии). `price`/`size` должны быть положительными, иначе no-op.
    pub fn place_order(
        &self,
        core: CoreId,
        market: String,
        short: bool,
        price: f64,
        size: f64,
        strategy_id: Option<u64>,
    ) -> Result<()> {
        if market.is_empty() || !(price > 0.0) || !(size > 0.0) {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::PlaceOrder {
                market,
                short,
                price,
                size,
                strategy_id,
            },
            "place order",
        )
    }

    /// Переставить (move/replace) ордер ядра по `uid` на новую цену — «потянуть за
    /// линию». `new_price` должен быть положительным, иначе no-op.
    pub fn move_order(&self, core: CoreId, uid: u64, new_price: f64) -> Result<()> {
        if !(new_price > 0.0) {
            return Ok(());
        }
        self.send_core_cmd(core, CoreCmd::MoveOrder { uid, new_price }, "move order")
    }

    /// Отменить ордер ядра по `uid`.
    pub fn cancel_order(&self, core: CoreId, uid: u64) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::CancelOrder { uid }, "cancel order")
    }

    /// «Паник-селл» по рынку ядра (кнопка на чарте). `on` — вкл/выкл флаг.
    pub fn panic_sell_market(&self, core: CoreId, market: String, on: bool) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::PanicSellMarket { market, on },
            "panic sell market",
        )
    }

    /// Отменить ожидающие buy-ордера рынка ядра (кнопка «Cancel Buy»).
    pub fn cancel_market_buys(&self, core: CoreId, market: String) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::CancelMarketBuys { market },
            "cancel market buys",
        )
    }

    /// Включить/выключить стоп-флаг (SL/TS/VStop) ордера ядра по `uid` — клик по ячейке в
    /// таблице «Ордера». feed сохраняет настроенный уровень стопа при повторном включении.
    pub fn set_order_stop(
        &self,
        core: CoreId,
        uid: u64,
        kind: OrderStopKind,
        on: bool,
    ) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::SetOrderStop { uid, kind, on },
            "set order stop",
        )
    }

    /// Точечная правка `ClientSettings` ядра из тулбара (TP/SL/выбор sell-пресета). feed
    /// патчит удержанный снимок и шлёт его целиком в ядро.
    pub fn edit_client_settings(&self, core: CoreId, edit: ClientSettingsEdit) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::EditClientSettings(edit),
            "edit client settings",
        )
    }

    /// Точечная правка управления плечом ядра (фикс. плечо из тулбара).
    pub fn edit_lev_manage(&self, core: CoreId, edit: LevManageEdit) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::EditLevManage(edit), "edit lev manage")
    }

    /// Переключить hedge-mode аккаунта ядра (dual-side позиции). Реальное действие на бирже.
    pub fn set_hedge_mode(&self, core: CoreId, on: bool) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::SetHedgeMode(on), "set hedge mode")
    }

    /// Read-only доступ к аккаунтному плану (статусы/ордера/детекты/стратегии).
    /// Наружу отдаём только `&` — мутирует store исключительно сам менеджер.
    pub fn store(&self) -> &CoreStore {
        &self.store
    }

    /// Живые сессии ядер (id/имя/группа) — read-only срез для UI.
    pub fn sessions(&self) -> &[CoreSession] {
        &self.sessions
    }

    /// Базовая валюта аккаунта ядра ("USDT"/"BTC"/…), если ядро уже идентифицировано
    /// (`CoreBase`). UI берёт её для дефолтов размера ордера по базе.
    pub fn core_base(&self, core: CoreId) -> Option<&str> {
        self.core_base.get(&core).map(String::as_str)
    }

    pub fn feed_wake(&self) -> Option<FeedWakeTx> {
        self.feed_wake.clone()
    }

    /// Стакан для ядра `core` на рынке `market`: резолвим провайдера ядра и читаем
    /// только book-view. История/last-price идут через retained-history API, не отсюда.
    pub fn with_orderbook_view<R>(
        &self,
        core: CoreId,
        market: &str,
        f: impl FnOnce(Option<(&OrderBookModel, u64)>) -> R,
    ) -> R {
        self.market_source.with_orderbook_view(core, market, f)
    }

    pub fn market_source(&self) -> MarketDataSource {
        self.market_source.clone()
    }

    /// Переключить режим источника рыночных данных (рубильник из Настроек). При
    /// реальной смене сбрасывает рыночный план целиком — провайдеры, обслуживаемые
    /// рынки и данные переизберутся/перельются с нуля на следующем `set_open`
    /// (старые ядра получат свежие роли, т.к. `last_cmd` очищен).
    pub fn set_market_mode(&mut self, mode: MarketDataMode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.market_source.clear();
        self.providers.clear();
        self.core_provider.clear();
        self.wanted.clear();
        self.pending_drop.clear();
        self.last_cmd.clear();
    }
}

fn orderbook_kind_for_exchange(ex: ExchangeId) -> OrderBookKind {
    match ex.code {
        // Spot exchanges.
        3 | 5 | 7 | 8 | 10 | 12 => OrderBookKind::Spot,
        // Futures/quarterly derivatives.
        2 | 4 | 6 | 9 | 11 | 13 => OrderBookKind::Futures,
        _ => OrderBookKind::Futures,
    }
}
