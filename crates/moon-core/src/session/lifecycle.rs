//! Жизненный цикл сессий: старт/reconcile/reconnect, дренаж каналов ядер и сводки
//! (подключения/лицензии) для статус-баров.

use std::collections::{HashMap, HashSet};

use crate::config::{AppConfig, ServerConfig};
use crate::db::ReportTx;
use crate::feed::{self, ConnStatus, EngineActionResult, FeedHandle, FeedMsg, FeedWakeTx};
use crate::market::{MarketDataMode, MarketDataSource, MarketStore};

use super::{
    conn_sig, orderbook_kind_for_exchange, ConnSummary, CoreId, CoreSession, CoreStore,
    DrainStats, LicenseSummary, SessionManager,
};

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
        // Локальный kline-кэш (klines.sqlite в db_dir): глубина свечей переживает
        // рестарты и «один ТФ на ядро». Опционален — без него чарты живут как раньше.
        market_source.init_kline_cache(crate::config::paths::klines_db_path());
        let mut mgr = Self {
            sessions: Vec::new(),
            feed_wake,
            store: CoreStore::default(),
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
        };
        for s in config
            .servers
            .iter()
            .filter(|s| s.active && config.group(&s.group).active)
        {
            mgr.spawn_core(s, conn_sig(s), config.chart_memory_percent, reports);
        }
        if mgr.sessions.is_empty() {
            log::warn!("нет серверов в конфиге — добавь ядра в Настройках");
        }
        mgr
    }

    /// Общий блок подъёма feed-потока ядра: `feed::spawn` + регистрация market-клиента.
    fn spawn_feed(&self, server: ServerConfig, mem: u16, reports: Option<&ReportTx>) -> FeedHandle {
        let id = server.id;
        let handle = feed::spawn(
            server,
            mem,
            reports.cloned(),
            self.feed_wake.clone(),
            Some(self.market.clone()),
        );
        self.market_source.set_client(id, handle.client.clone());
        handle
    }

    /// Сброс рыночной координации ядра: ключ/база/роль провайдера и last_cmd —
    /// пусть переизберутся заново (общее для respawn/reconnect/drop).
    fn clear_core_coordination(&mut self, id: CoreId) {
        self.core_key.remove(&id);
        self.core_base.remove(&id);
        self.core_provider.remove(&id);
        self.providers.retain(|_, prov| *prov != id);
        self.last_cmd.remove(&id);
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
                        // Сменились connection-поля → пере-поднять feed-поток ядра.
                        self.respawn_session(
                            s.clone(),
                            sig,
                            mem,
                            reports,
                            "reconnect (config changed)",
                        );
                    }
                }
            }
        }
    }

    /// Поднять feed-поток нового ядра и зарегистрировать его.
    fn spawn_core(&mut self, s: &ServerConfig, sig: u64, mem: u16, reports: Option<&ReportTx>) {
        let id = s.id;
        self.store.ensure(id);
        let handle = self.spawn_feed(s.clone(), mem, reports);
        self.sessions.push(CoreSession {
            id,
            name: s.name.clone(),
            group: s.group.clone(),
            conn_sig: sig,
            handle,
        });
        log::info!("session up: core={id}");
    }

    /// Пере-поднять feed-поток ядра: заменяет хэндл (дроп старого завершает его поток;
    /// если сессии не было — регистрирует новую), ставит Connecting и сбрасывает
    /// координацию под переизбор провайдера. Общее тело respawn-по-конфигу и reconnect.
    fn respawn_session(
        &mut self,
        server: ServerConfig,
        sig: u64,
        mem: u16,
        reports: Option<&ReportTx>,
        why: &str,
    ) {
        let id = server.id;
        let name = server.name.clone();
        let group = server.group.clone();
        let handle = self.spawn_feed(server, mem, reports);
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
        self.clear_core_coordination(id);
        log::info!("{why}: core={id}");
    }

    /// Погасить ядро (сервер убран/деактивирован): дроп сессии завершает поток, чистим
    /// аккаунтные данные, рыночного клиента и всю координацию.
    fn drop_core(&mut self, id: CoreId) {
        self.sessions.retain(|s| s.id != id); // дроп FeedHandle → поток завершится
        self.store.remove(id);
        self.market_source.remove_client(id);
        self.clear_core_coordination(id);
        self.wanted.remove(&id);
        self.pending_drop.retain(|(core, _), _| *core != id);
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
                            stats.market_data = true;
                        }
                    }
                    FeedMsg::Orders(orders) => {
                        if let Some(core) = self.store.core_mut(sess.id) {
                            let before = core.order_lines_rev;
                            core.apply(FeedMsg::Orders(orders));
                            stats.order_lines_data |= core.order_lines_rev != before;
                            stats.ui_state = true;
                        }
                    }
                    FeedMsg::OrderLines(orders) => {
                        if let Some(core) = self.store.core_mut(sess.id) {
                            let before = core.order_lines_rev;
                            core.apply(FeedMsg::OrderLines(orders));
                            let changed = core.order_lines_rev != before;
                            stats.order_lines_data |= changed;
                            stats.ui_state |= changed;
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
    /// Забирает накопленные результаты Engine-действий всех ядер: (имя ядра, результат).
    /// Очереди дренируются — зовёт только Shell АКТИВНОГО окна, чтобы каждый тост
    /// показался ровно один раз (активно максимум одно ОС-окно).
    pub fn take_engine_action_toasts(&mut self) -> Vec<(String, EngineActionResult)> {
        let mut out = Vec::new();
        for sess in &self.sessions {
            if let Some(core) = self.store.core_mut(sess.id) {
                for r in core.take_engine_actions() {
                    out.push((sess.name.clone(), r));
                }
            }
        }
        out
    }

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
        let sig = conn_sig(&server);
        self.respawn_session(server, sig, config.chart_memory_percent, reports, "reconnect");
    }
}
