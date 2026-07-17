//! Live-backend: подключение к ядру Moonbot через MoonProtoBeta.
//! Единственный модуль, знающий про moonproto.
//!
//! Поток: event-driven. `MoonEventSink` будит backend thread после реального события;
//! market data остаётся в immutable read-model snapshot, сюда идёт только лёгкий сигнал.
//!
//! `run()` — главный event-цикл; команды роли вынесены в [`commands`], чистые конвертеры
//! moonproto→терминал — в [`convert`], расчёт «грязных» рынков — в [`dirty`].

mod commands;
mod convert;
mod dirty;

use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use moonproto::state::{AccountEvent, MarketHistorySizing, OrderEvent, SettingsEvent};
use moonproto::{
    ClientConfig, ConnectConfig, Event, InitConfig, InitialStrategies, LifecycleEvent, MoonClient,
    MoonEventSink, ReportEvent, ReportHistoryDepth, ReportSyncRequest, TransportMode,
};

use super::assets::{build_assets, build_transfer_assets};
use super::report::{send_close_report, OrderIndex};
use super::strategies::{
    alert_params, build_schema_model, fmt_field, schema_default_fields, strat_db_dump,
    strat_kind_name,
};
use super::{
    ConnStatus, CoreCmd, CoreLogLine, DetectRow, ExchangeId, FeedMsg, FeedTx, SharedMoonClient,
    StrategyRow,
};
use crate::config::ServerConfig;
use crate::db::{DbMsg, ReportTx};
use crate::util::{now_unix_ms as now_ms, now_unix_ms_i64 as now_ms_i64};

use commands::{drain_commands, LocalStratEdits};
use convert::{
    build_order_rows, client_settings_from_proto, lev_manage_from_proto, license_state_from_proto,
    runtime_state_from_proto, settings_event_snapshot,
};
use dirty::market_dirty_from_events;

struct ClientSlotGuard {
    slot: SharedMoonClient,
}

impl Drop for ClientSlotGuard {
    fn drop(&mut self) {
        self.slot.set(None);
    }
}

pub fn run(
    server: &ServerConfig,
    chart_memory_percent: u16,
    tx: &FeedTx,
    cmd_rx: &Receiver<CoreCmd>,
    wake_tx: &Sender<()>,
    wake_rx: &Receiver<()>,
    reports: Option<&ReportTx>,
    client_slot: SharedMoonClient,
) -> anyhow::Result<()> {
    let _ = tx.send(FeedMsg::Status(ConnStatus::Connecting));

    // 1. Ключ -> мастер/мак ключи + предложенная сеть.
    let info = moonproto::parse_key_info(server.key.expose())
        .ok_or_else(|| anyhow::anyhow!("не удалось разобрать ключ Moonbot (server.key)"))?;

    // 2. Endpoint берётся из ключа (host/port/transport зашиты в нём; отдельных
    //    полей в конфиге больше нет).
    let net = info.network.as_ref();
    let host: String = net
        .and_then(|n| n.address)
        .map(|a| a.to_string())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let port: u16 = net.map(|n| n.port).filter(|p| *p != 0).unwrap_or(3000);
    let transport = net.map(|n| n.transport_mode).unwrap_or(TransportMode::V0);
    log::info!("live connect {host}:{port} market={}", server.market);

    let client_cfg = ClientConfig::new(host, port, info.keys.master_key, info.keys.mac_key)
        .with_transport_mode(transport)
        .with_market_history(MarketHistorySizing::auto_with_budget_percent(
            chart_memory_percent,
        ));

    // 3. Init БЕЗ рыночных подписок. Рыночную роль ядра задаёт координатор командой
    //    SetMarket после того, как узнает биржу ядра (Identity) и изберёт провайдера:
    //    только ОДНО ядро на биржу делает subscribe_all_trades, остальные шлют лишь
    //    аккаунт. Так трейды биржи тянутся 1 раз, а не с каждого из 200 ядер.
    //    initial_strategies ОБЯЗАТЕЛЬНО — иначе init зависает после Connected.
    let init = InitConfig {
        initial_strategies: Some(InitialStrategies::new(0, Vec::new())),
        ..Default::default()
    };

    // connect (не blocking) + connect_timeout, чтобы зависший шаг init пришёл
    // как ConnectFailed с причиной, а не молчал.
    let event_wake_tx = wake_tx.clone();
    let (event_sink, event_queue) = MoonEventSink::queue_with_waker(move || {
        let _ = event_wake_tx.send(());
    });
    let client = Arc::new(MoonClient::connect_with_sink(
        client_cfg,
        ConnectConfig::new(init).with_connect_timeout(Duration::from_secs(15)),
        event_sink,
    )?);
    client_slot.set(Some(client.clone()));
    let _client_slot_guard = ClientSlotGuard {
        slot: client_slot.clone(),
    };

    // Typed-реплика БД отчётов: заявляем catch-up сразу (лежит intent'ом — lib отправит
    // после connect и САМА повторит после hard-reconnect). Курсор = max(newRecID)+1
    // локальной реплики; 0 = реплика пуста → fresh со ВСЕЙ удержанной историей ядра
    // (заменяет легаси-таблицу). Догонка идёт СТРАНИЦАМИ: следующая не запрашивается,
    // пока writer не закоммитил и не ack'нул текущую (backpressure by design).
    if server.feed.reports {
        if let Some(sink) = reports {
            let from = sink.next_cursor(server.uid);
            let req = if from > 0 {
                ReportSyncRequest::resume(from)
            } else {
                ReportSyncRequest::fresh(ReportHistoryDepth::All)
            };
            match client.reports().sync(req) {
                Ok(_) => log::info!("отчёты: core={} sync запрошен (from_rec_id={from})", server.uid),
                Err(e) => log::warn!("отчёты: core={} sync не запустился: {e:?}", server.uid),
            }
            // Открытые строки могли закрыться/удалиться в оффлайне НИЖЕ курсора —
            // регистрируем их проверку (lib держит набор и повторяет на hard-reconnect;
            // результаты придут обычными RowUpsert/RowDelete).
            let open = sink.open_rows(server.uid);
            if !open.is_empty() {
                if let Err(e) = client.reports().check_open_rows(&open) {
                    log::warn!("отчёты: core={} check_open_rows не ушёл: {e:?}", server.uid);
                }
            }
        }
    }

    // Рыночная роль ядра (задаётся координатором командой SetMarket).
    // is_provider — ретейним ли ВСЕ трейды биржи (subscribe_all_trades).
    // wanted — рынки, которые активно обслуживаем (подписки + snapshot source).
    let mut is_provider = false;
    let mut wanted: Vec<String> = Vec::new();
    // Рынки, на стакан которых подписаны (подмножество wanted; вкл стакан хотя бы в одном окне).
    let mut wanted_orderbook: Vec<String> = Vec::new();
    let mut identity_sent = false;
    let mut last_orders = Instant::now();
    let mut orders_table_pending = false;
    let mut last_strats = Instant::now();
    // Активы (окно «Активы»): тот же ~1 Гц тик, что у ордеров/стратегий.
    let mut last_assets = Instant::now();
    // Троттл активного запроса баланса у ЯДРА после филлов (см. блок в цикле).
    let mut last_balance_refresh = Instant::now();
    let mut last_wallet_refresh = Instant::now();
    // Окно логирования Balance-событий после нашего refresh (диагностика фантомов «Активов»).
    let mut balance_refresh_log_until: Option<Instant> = None;
    // Курсор transfer-активов: шлём только при смене revision (request/response).
    let mut last_transfer_rev: u64 = u64::MAX;
    // Курсоры выгрузки стратегий: revision схемы и сигнатура состава/checked —
    // шлём только при изменениях (поля стратегий тяжёлые, гонять каждую секунду незачем).
    let mut last_schema_rev: u64 = u64::MAX;
    let mut last_strat_sig: u64 = u64::MAX;
    // strat_db: дефолты полей схемы по видам (нормализация дампов — сервер не шлёт
    // поля, равные дефолту), кольцо наших правок (origin=local) и флаг первого
    // набора после (ре)коннекта (origin неизвестен — правки могли пройти оффлайн).
    let mut strat_schema_defaults: std::collections::HashMap<u8, Vec<(String, moonproto::FieldValue)>> =
        std::collections::HashMap::new();
    let mut local_strat_edits = LocalStratEdits::new();
    let mut strat_db_initial = true;
    // Монотонный per-core номер детекта — курсор ингеста в ленту детектов UI.
    let mut detect_seq: u64 = 0;
    // Полные данные ордеров для close-report'ов (uid/db_id) — см. feed::report.
    let mut orders_index = OrderIndex::default();
    // Файловый писатель серверного лога этого ядра (logs/<дата>_<ядро>.log) с дневной
    // ротацией. Пишем на ПОТОКЕ ФИДА (не на UI), т.к. лога много — UI не должен ждать
    // диск. В UI уходит лишь in-memory копия для живого просмотра/поиска.
    let mut log_writer = crate::applog::DatedWriter::new(&server.name);
    let mut events = Vec::new();
    let mut lifecycle_events = Vec::new();
    let mut force_market_sample = false;

    loop {
        // Команды роли от координатора (полное желаемое состояние, не дельта).
        // Закрытие канала = координатор ушёл → отключаемся.
        let mut orders_mutated = false;
        if drain_commands(
            cmd_rx,
            &client,
            server,
            &mut is_provider,
            &mut wanted,
            &mut wanted_orderbook,
            &mut force_market_sample,
            &mut orders_mutated,
            &mut local_strat_edits,
        ) {
            return Ok(());
        }
        // ОПТИМИСТИЧНАЯ отрисовка ордеров: команда (тогл стопа/drag/отмена/постановка) уже
        // применена к ЛОКАЛЬНОЙ модели (рантайм применяет до отправки пакета) — отдаём
        // строки НЕМЕДЛЕННО, мимо event-гейта и 250мс-троттла. Линии/подписи/таблица
        // реагируют на клик мгновенно; эхо ядра придёт следом и подтвердит/поправит.
        if orders_mutated && server.feed.orders {
            if let Some(snap) = client.snapshot() {
                let order_rows = build_order_rows(
                    server.id,
                    &snap,
                    &[],
                    server.feed.reports,
                    &mut orders_index,
                );
                last_orders = Instant::now();
                orders_table_pending = false;
                if tx.send(FeedMsg::Orders(order_rows)).is_err() {
                    break;
                }
            }
        }

        // Биржа ядра (из server_info после BaseCheck) — координатору для группировки
        // и выбора провайдера. Шлём один раз, как только идентичность известна.
        if !identity_sent {
            if let Some(info) = client.server_info() {
                if let Some(code) = info.exchange_code {
                    // dex_name: непустое только для Hyperliquid HIP-3 фьючей. Входит в
                    // идентичность, чтобы ядра разных dex НЕ дедуплились на одного
                    // провайдера с неполным списком рынков (см. ExchangeId).
                    let dex = info.dex_name.as_deref().unwrap_or("");
                    let id = ExchangeId::with_dex(code.stable_id(), dex);
                    log::info!(
                        "core {} identity: exchange_code={} dex_name={:?} -> {:?}",
                        server.id,
                        code.stable_id(),
                        dex,
                        id
                    );
                    let _ = tx.send(FeedMsg::Identity(id));
                    // Базовая валюта аккаунта — для дефолтов размера ордера в UI (BTC vs USDT).
                    let base = info.base_currency_name.unwrap_or_default();
                    if !base.is_empty() {
                        let _ = tx.send(FeedMsg::CoreBase { base });
                    }
                    identity_sent = true;
                }
            }
        }

        // Lifecycle -> статус (стадии и ошибки видны прямо в бейдже).
        // ConnectFailed — ТЕРМИНАЛЬНЫЙ отказ начального connect/init: фоновый рантайм
        // moonproto при нём делает break и больше НЕ реконнектится (авто-реконнект у
        // него только для потери линка ПОСЛЕ успешного коннекта). При этом сам
        // MoonClient::connect неблокирующий и уже вернул Ok, так что без явного выхода
        // мы бы крутились вечно со статусом Failed и app-level реконнект (feed/mod.rs)
        // не запустился бы. Поэтому ловим ConnectFailed и возвращаем Err → внешний
        // цикл пересоздаст клиент с backoff. (Ровно баг «5/7, авто-реконнекта нет».)
        let mut connect_failed: Option<String> = None;
        lifecycle_events.clear();
        event_queue.drain_lifecycle_events_into(&mut lifecycle_events);
        for ev in lifecycle_events.drain(..) {
            log::info!("lifecycle: {ev:?}");
            let request_license_state = match &ev {
                LifecycleEvent::Ready => true,
                LifecycleEvent::Connected { fresh } => !*fresh,
                _ => false,
            };
            let st = match ev {
                LifecycleEvent::Connecting => ConnStatus::Stage("connecting…".into()),
                LifecycleEvent::Connected { fresh } => {
                    // fresh=true → дальше идёт одноразовый init, ждём Ready.
                    // fresh=false → реконнект: moonproto НЕ повторяет init и НЕ шлёт
                    // Ready снова, но подписки/индексы уже восстановлены и клиент
                    // операционен — иначе статус навсегда застрял бы на «reconnected»
                    // (0/N), хотя данные идут. Поэтому реконнект = сразу Ready.
                    if fresh {
                        ConnStatus::Stage("connected, init…".into())
                    } else {
                        ConnStatus::Ready
                    }
                }
                LifecycleEvent::InitStepCompleted { step, .. } => {
                    ConnStatus::Stage(format!("init: {step}"))
                }
                LifecycleEvent::Ready => ConnStatus::Ready,
                LifecycleEvent::Reconnecting => ConnStatus::Stage("reconnecting…".into()),
                LifecycleEvent::ServerRestart => ConnStatus::Stage("server restart…".into()),
                LifecycleEvent::ConnectFailed { error } => {
                    let msg = error.to_string();
                    connect_failed = Some(msg.clone());
                    ConnStatus::Failed(msg)
                }
                LifecycleEvent::BindFailed {
                    consecutive_failures,
                } => ConnStatus::Failed(format!(
                    "UDP bind failed x{consecutive_failures} (VPN/firewall/порты?)"
                )),
                LifecycleEvent::Disconnected => ConnStatus::Disconnected,
            };
            let _ = tx.send(FeedMsg::Status(st));
            if request_license_state {
                if let Err(error) = client.settings().request_kernel_license_state() {
                    log::warn!(
                        "core {} request kernel license state failed: {error}",
                        server.id
                    );
                }
                // Полный снимок ClientSettings (TP/SL/sell/…). LevManage/RuntimeState ядро
                // присылает само после connect; здесь дёргаем только settings-refresh.
                if let Err(error) = client.settings().refresh() {
                    log::warn!("core {} request client settings failed: {error}", server.id);
                }
                // Hedge-mode аккаунта (для тоггла в тулбаре).
                if let Err(error) = client.account().refresh_hedge_mode() {
                    log::warn!("core {} request hedge mode failed: {error}", server.id);
                }
                // Chart-алерты авторитетны на ядре: после init/reconnect просим полный
                // снапшот (без него локальный набор может отстать от сервера).
                if server.feed.alerts {
                    if let Err(error) = client.chart_alerts().request_snapshot() {
                        log::warn!(
                            "core {} request chart alerts snapshot failed: {error}",
                            server.id
                        );
                    }
                }
            }
        }
        // Терминальный отказ старта → наружу как Err: пусть app-level цикл пересоздаст
        // клиент (moonproto сам этот рантайм уже не оживит).
        if let Some(e) = connect_failed {
            return Err(anyhow::anyhow!("{e}"));
        }

        // Дренируем доменные события из MoonEventSink. Тики/стакан/ордера берём из
        // snapshot только после реального события, а не постоянным 8мс polling.
        events.clear();
        event_queue.drain_events_into(&mut events);
        let had_domain_event = !events.is_empty();
        let has_order_line_event = events.iter().any(|ev| {
            matches!(
                ev,
                Event::Order(
                    OrderEvent::Created(_)
                        | OrderEvent::Updated(_)
                        | OrderEvent::Removed(_)
                        | OrderEvent::TracePoint { .. }
                        | OrderEvent::CorridorChanged(_)
                        | OrderEvent::VStopChanged(_)
                        | OrderEvent::StopsChanged(_)
                        | OrderEvent::Snapshot
                )
            )
        });
        let has_orders_table_event = events.iter().any(|ev| {
            matches!(
                ev,
                Event::Order(
                    OrderEvent::Created(_)
                        | OrderEvent::Updated(_)
                        | OrderEvent::Removed(_)
                        | OrderEvent::Snapshot
                        | OrderEvent::CorridorChanged(_)
                        | OrderEvent::VStopChanged(_)
                        | OrderEvent::StopsChanged(_)
                )
            )
        });
        // Филл/снятие ордера меняет позицию и баланс, но ядро НЕ всегда пушит свежий
        // баланс (проданный токен зависает фантомом в «Активах» до реконнекта — снимок
        // протухает). Активно просим у ЯДРА свежий снимок: `Command::Balance` идёт к ядру
        // (Delphi `SendBalanceCmd`), НЕ на биржу — дёшево. Троттл 3с, чтобы серия филлов
        // не спамила. Ответ придёт Balance-событием на следующей итерации и обновит снимок.
        if has_orders_table_event && last_balance_refresh.elapsed() >= Duration::from_secs(3) {
            last_balance_refresh = Instant::now();
            match client.balances().refresh() {
                // Диагностика фантомов «Активов»: фиксируем сам факт запроса и (ниже, в цикле
                // событий) ЧЕМ ядро ответило — полным снимком (SnapshotApplied: обнуляет
                // пропавшие монеты) или инкрементом (IncrementalApplied: обнулённую монету
                // НЕ сотрёт — дырка на стороне ядра).
                Ok(()) => {
                    balance_refresh_log_until = Some(Instant::now() + Duration::from_secs(5));
                    log::info!("core {} balance refresh requested (orders event)", server.id);
                }
                Err(error) => {
                    log::warn!("core {} balance refresh request failed: {error}", server.id)
                }
            }
        }
        // У кошельковых спот-бирж (Bitget, Hyperliquid-спот) купленные монеты живут ТОЛЬКО
        // в transfer_assets — per-market балансов ядро не шлёт вовсе. Без переопроса
        // свежекупленная монета не появится в «Активах» до ручного клика по ядру.
        // Спрашиваем один Spot-кошелёк (не все три) и реже балансов: этот запрос ядро
        // форвардит на биржу (CheckAssets в логе ядра бывает и в таймаут).
        if has_orders_table_event && last_wallet_refresh.elapsed() >= Duration::from_secs(10) {
            last_wallet_refresh = Instant::now();
            if let Err(error) = client
                .balances()
                .refresh_transfer_assets_kind(moonproto::ExchangeKind::Spot)
            {
                log::warn!(
                    "core {} spot wallet refresh request failed: {error}",
                    server.id
                );
            }
        }
        // Судьба bulk-снимка 5м-свечей (авто-запрос moonproto после subscribe_all_trades):
        // от него живут свечные величины retained-истории (H.vol/72h скринера). Провал
        // иначе полностью беззвучен (никто событие не слушал). Учти известный баг
        // moonproto (зарепорчен авторам): снимок может молча выброситься уже ПОСЛЕ
        // Ready из-за таймзоны сервера.
        // Результаты Engine-действий (плечо/hedge/cancel-all/перенос/…) — в UI тостами.
        // Приходят и при обрыве (`success=false`), так что «не дошло» тоже видно.
        let mut engine_actions: Vec<crate::feed::EngineActionResult> = Vec::new();
        // Chart-алерты (фигуры с галкой Alert): авторитетный набор ядра.
        let mut chart_alerts: Vec<crate::feed::ChartAlertUpdate> = Vec::new();
        for ev in &events {
            match ev {
                Event::ChartAlert(ev) if server.feed.alerts => {
                    // Этап реверса blob (TChartObject.Save): полный hex в лог — алерты
                    // создаются руками и их единицы, объём лога не проблема.
                    match ev {
                        moonproto::ChartAlertEvent::Upserted(obj) => {
                            log::info!(
                                "core {} chart alert upserted: {} uid={} blob[{}]={}",
                                server.id,
                                obj.market_name,
                                obj.obj_uid,
                                obj.blob.len(),
                                hex_dump(&obj.blob)
                            );
                            chart_alerts.push(crate::feed::ChartAlertUpdate::Upserted(
                                crate::feed::ChartAlertRow {
                                    market: obj.market_name.clone(),
                                    obj_uid: obj.obj_uid,
                                    blob: obj.blob.clone(),
                                },
                            ));
                        }
                        moonproto::ChartAlertEvent::Deleted {
                            market_name,
                            obj_uid,
                        } => {
                            log::info!(
                                "core {} chart alert deleted: {} uid={}",
                                server.id,
                                market_name,
                                obj_uid
                            );
                            chart_alerts.push(crate::feed::ChartAlertUpdate::Deleted {
                                market: market_name.clone(),
                                obj_uid: *obj_uid,
                            });
                        }
                    }
                }
                Event::EngineAction(e) => {
                    if !e.success {
                        log::warn!(
                            "core {} engine action failed: {:?} code={} msg={}",
                            server.id,
                            e.kind,
                            e.error_code,
                            e.error_msg
                        );
                    }
                    engine_actions.push(convert::engine_action_result(e));
                }
                Event::CandlesSnapshot(moonproto::state::CandlesSnapshotEvent::Ready {
                    summary,
                    ..
                }) => {
                    log::debug!(
                        "core {} candles snapshot ready: markets {}/{} candles {}/{}",
                        server.id,
                        summary.retained_markets,
                        summary.received_markets,
                        summary.retained_candles,
                        summary.received_candles
                    );
                }
                Event::CandlesSnapshot(moonproto::state::CandlesSnapshotEvent::Failed {
                    error,
                    ..
                }) => {
                    log::warn!("core {} candles snapshot failed: {error}", server.id);
                }
                // Отказ CoinCard-запроса (deep history чарта): раньше падал в `_ => {}`
                // МОЛЧА — свечи «не приезжали» без единого следа в логе.
                Event::CoinCardCandles(moonproto::state::CoinCardCandlesEvent::UpdateFailed {
                    market,
                    kind,
                    error,
                    ..
                }) => {
                    log::warn!(
                        "core {} coin-card {market} {kind:?} failed: {error}",
                        server.id
                    );
                }
                // Окно диагностики после нашего balance-refresh (фантомы «Активов»): вид
                // ответа решает судьбу зависшей монеты — Snapshot обнуляет пропавшие,
                // Incremental нет. Вне окна не логируем (балансы пушатся постоянно).
                Event::Balance(bev) => {
                    if balance_refresh_log_until.is_some_and(|t| Instant::now() < t) {
                        log::info!("core {} balance event after refresh: {bev:?}", server.id);
                    }
                }
                _ => {}
            }
        }
        if !engine_actions.is_empty() && tx.send(FeedMsg::EngineActions(engine_actions)).is_err() {
            break;
        }
        if !chart_alerts.is_empty() && tx.send(FeedMsg::ChartAlerts(chart_alerts)).is_err() {
            break;
        }
        let license_state = settings_event_snapshot(
            &events,
            &client,
            |ev| {
                matches!(
                    ev,
                    &Event::Settings(SettingsEvent::KernelLicenseStateUpdated)
                )
            },
            |state| {
                state
                    .settings()
                    .kernel_license_state
                    .map(license_state_from_proto)
            },
        );
        if let Some(license) = license_state {
            if tx.send(FeedMsg::License(license)).is_err() {
                break;
            }
        }
        // ClientSettings/LevManage/RuntimeState — снимки настроек ядра. Каждый тянем из
        // snapshot ТОЛЬКО когда пришло его событие (а не каждый тик), как и license выше.
        let client_settings = settings_event_snapshot(
            &events,
            &client,
            |ev| matches!(ev, &Event::Settings(SettingsEvent::ClientSettingsUpdated)),
            |state| {
                state
                    .settings()
                    .client_settings
                    .as_ref()
                    .map(client_settings_from_proto)
            },
        );
        if let Some(settings) = client_settings {
            if tx.send(FeedMsg::ClientSettings(settings)).is_err() {
                break;
            }
        }
        let lev_manage = settings_event_snapshot(
            &events,
            &client,
            |ev| matches!(ev, &Event::Settings(SettingsEvent::LevManageUpdated)),
            |state| {
                state
                    .settings()
                    .lev_manage
                    .as_ref()
                    .map(lev_manage_from_proto)
            },
        );
        if let Some(lev) = lev_manage {
            if tx.send(FeedMsg::LevManage(lev)).is_err() {
                break;
            }
        }
        let runtime_state = settings_event_snapshot(
            &events,
            &client,
            |ev| matches!(ev, &Event::Settings(SettingsEvent::RuntimeStateUpdated)),
            |state| {
                state
                    .settings()
                    .runtime_state
                    .as_ref()
                    .map(runtime_state_from_proto)
            },
        );
        if let Some(state) = runtime_state {
            if tx.send(FeedMsg::RuntimeState(state)).is_err() {
                break;
            }
        }
        // Hedge-mode: значение приходит прямо в событии (Engine API ответ).
        let hedge_mode = events.iter().find_map(|ev| match ev {
            Event::Account(AccountEvent::HedgeModeUpdated { hedge_mode, .. }) => Some(*hedge_mode),
            _ => None,
        });
        if let Some(on) = hedge_mode {
            if tx.send(FeedMsg::HedgeMode(on)).is_err() {
                break;
            }
        }
        let dirty_markets = if is_provider && !wanted.is_empty() {
            market_dirty_from_events(&events, &wanted, force_market_sample)
        } else {
            Vec::new()
        };
        let want_log = server.feed.log;
        // detect-diag: один раз за процесс — состояние серверных флагов фида. Если
        // `feed.detects=false`, ветка `Event::Detect` ниже вообще не работает → корень
        // «нет детектов» виден сразу, без догадок. (env MOON_DETECT_DIAG, off by default.)
        {
            use std::sync::OnceLock;
            static FLAGS_ONCE: OnceLock<()> = OnceLock::new();
            if crate::detect_diag::enabled() && FLAGS_ONCE.set(()).is_ok() {
                crate::detect_diag::line(&format!(
                    "[live] flags: feed.detects={} feed.reports={} feed.log={}",
                    server.feed.detects, server.feed.reports, want_log
                ));
            }
        }
        // Алерт-фаеры (`DETECT_KIND_ALERT`) приходят Event::Detect: чтобы они работали
        // при включённых АЛЕРТАХ даже без общего потока детектов — заходим и по feed.alerts.
        let want_detects = server.feed.detects || server.feed.alerts;
        if want_detects || (server.feed.reports && reports.is_some()) || want_log {
            let mut detects: Vec<DetectRow> = Vec::new();
            let mut logs: Vec<CoreLogLine> = Vec::new();
            // Снимок для полей стратегии-источника детекта (SoundAlert/KeepAlert/звук).
            let detect_snap = want_detects.then(|| client.snapshot()).flatten();
            // Схема стратегий — для фолбэка на default_value: поля, равные дефолту
            // схемы, сервер не шлёт (в т.ч. звук/SoundAlert).
            let detect_schema = detect_snap.as_ref().and_then(|s| s.strats().strategy_schema());
            for ev in &events {
                match ev {
                    Event::ServerLog(l) if want_log => {
                        let ms = l.unix_millis();
                        let recv_ms = now_ms_i64();
                        // На диск — сразу (буферизованно); время бьём на дату+часы.
                        let (date, hms) = crate::applog::split_unix_ms(ms);
                        log_writer.write(&date, &hms, "INFO", "", &l.msg);
                        logs.push(CoreLogLine {
                            time_ms: ms,
                            recv_ms,
                            msg: l.msg.clone(),
                        });
                    }
                    Event::Detect(d)
                        if server.feed.detects || (server.feed.alerts && d.is_alert_fire()) =>
                    {
                        let strat = detect_snap
                            .as_ref()
                            .and_then(|s| s.strats().snapshot(d.strategy_id));
                        let params = strat
                            .map(|st| alert_params(st, detect_schema))
                            .unwrap_or_default();
                        crate::detect_diag::line(&format!(
                            "[feed] detect market={} strat_id={} strat_found={} sound_alert={} sound={:?} is_alert={}",
                            d.market_name,
                            d.strategy_id,
                            strat.is_some(),
                            params.sound_alert,
                            params.sound_name,
                            d.is_alert_fire(),
                        ));
                        detect_seq += 1;
                        detects.push(DetectRow {
                            seq: detect_seq,
                            market: d.market_name.clone(),
                            time_ms: now_ms(),
                            sound_alert: params.sound_alert,
                            keep_alert_secs: params.keep_alert_secs,
                            add_to_chart: params.add_to_chart,
                            keep_in_chart_secs: params.keep_in_chart_secs,
                            sound_name: params.sound_name,
                            is_alert: d.is_alert_fire(),
                            // Вид стратегии-источника для бейджа типа детекта; без снимка
                            // стратегии срабатывание алерта помечаем видом Alerts (22).
                            kind: strat
                                .map(|st| st.kind().ordinal())
                                .unwrap_or(if d.is_alert_fire() { 22 } else { 0 }),
                            is_short: strat.map(|st| st.is_short()).unwrap_or(false),
                        });
                    }
                    // Typed-реплика БД отчётов: схема/строки/завершение catch-up →
                    // SQLite-writer'у (он один владеет соединением на запись).
                    Event::Report(rev) if server.feed.reports => {
                        if let Some(sink) = reports {
                            match rev {
                                ReportEvent::Schema(s) => sink.send(DbMsg::Schema {
                                    core_uid: server.uid,
                                    schema: s.clone(),
                                }),
                                ReportEvent::RowUpsert(row) => sink.send(DbMsg::Upsert {
                                    core_uid: server.uid,
                                    core_name: server.name.clone(),
                                    row: row.clone(),
                                }),
                                ReportEvent::RowDelete { rec_id } => sink.send(DbMsg::Delete {
                                    core_uid: server.uid,
                                    rec_id: *rec_id,
                                }),
                                ReportEvent::SyncStarted { request, .. } => log::info!(
                                    "отчёты: core={} sync начат (from_rec_id={})",
                                    server.uid,
                                    request.from_rec_id,
                                ),
                                // Страница catch-up: writer применит транзакцией и сам
                                // ack'нет (`page_applied`) после коммита — до этого lib
                                // следующую страницу не запросит. `database_recreated`
                                // writer тоже обрабатывает (wipe), lib рестартует сама.
                                ReportEvent::SyncPage(page) => sink.send(DbMsg::Page {
                                    core_uid: server.uid,
                                    core_name: server.name.clone(),
                                    page: page.clone(),
                                    ack: client.reports(),
                                }),
                                ReportEvent::SyncComplete(done) => {
                                    sink.send(DbMsg::SyncComplete {
                                        core_uid: server.uid,
                                        done: done.clone(),
                                    });
                                }
                                ReportEvent::OpenRowsCheckStarted { rec_ids } => log::info!(
                                    "отчёты: core={} проверка открытых строк начата ({} шт)",
                                    server.uid,
                                    rec_ids.len(),
                                ),
                                ReportEvent::OpenRowsCheckComplete { rec_ids } => log::info!(
                                    "отчёты: core={} проверка открытых строк завершена ({} шт)",
                                    server.uid,
                                    rec_ids.len(),
                                ),
                                ReportEvent::SchemaRejected { reason } => log::error!(
                                    "отчёты: core={} схема отвергнута: {reason}",
                                    server.uid,
                                ),
                            }
                        }
                    }
                    Event::ClosedSellOrderReport(r) if server.feed.reports => {
                        if let Some(tx_db) = reports {
                            // db_id → uid → полные данные (uid стабилен с открытия).
                            // Если db_id ещё не успели замапить — сканируем ТЕКУЩИЙ
                            // снапшот: ордер часто ещё в модели с присвоенным db_id,
                            // а его полные данные уже есть в индексе по uid.
                            let m = orders_index.by_dbid(r.db_id as i32).or_else(|| {
                                client
                                    .snapshot()
                                    .and_then(|snap| {
                                        snap.orders()
                                            .iter()
                                            .find(|o| o.db_id as i64 == r.db_id)
                                            .map(|o| o.uid)
                                    })
                                    .and_then(|uid| orders_index.by_uid(uid))
                            });
                            send_close_report(tx_db, server, r.db_id, r.sql.clone(), m);
                        }
                    }
                    // Диагностика (фича moonproto-diagnostics, по умолчанию ВЫКЛ):
                    // пакеты, отвергнутые парсером/валидацией lib, в обычной сборке
                    // исчезают бесследно — так завис report-sync BB1 (страница
                    // отвергалась молча, курсор стоял сутками). Для страницы
                    // catch-up (CmdId=39) декодируем заголовок: видно, ЧТО прислал
                    // сервер (row_count/last/max_rec_id) и почему не прошло.
                    #[cfg(feature = "moonproto-diagnostics")]
                    Event::ParseFailed { cmd, len, payload } => {
                        let mut extra = String::new();
                        if payload.first() == Some(&39) && payload.len() >= 37 {
                            let i = |a: usize| {
                                i64::from_le_bytes(payload[a..a + 8].try_into().unwrap())
                            };
                            let ru = u64::from_le_bytes(payload[11..19].try_into().unwrap());
                            let rc = u16::from_le_bytes(payload[35..37].try_into().unwrap());
                            extra = format!(
                                " sync-page: request_uid={ru} last_rec_id={} max_rec_id={} row_count={rc}",
                                i(19),
                                i(27),
                            );
                        }
                        log::warn!(
                            "отчёты(diag): core={} пакет отвергнут: cmd={cmd:?} len={len}{extra}",
                            server.uid,
                        );
                    }
                    _ => {}
                }
            }
            if !logs.is_empty() {
                log_writer.flush(); // один флаш на пачку (не на строку) — диск не узкое место
                if tx.send(FeedMsg::ServerLog(logs)).is_err() {
                    break;
                }
            }
            // detect-diag: сколько Event::Detect реально надренажено и сколько из них с
            // AddToChart>0. raw>0 но with_chart=0 → стратегия без AddToChart (вкладки и не
            // будет — это не баг чарта). raw=0 при flags.detects=true → сервер не шлёт детекты.
            if server.feed.detects && !detects.is_empty() {
                let raw = detects.len();
                let with_chart = detects.iter().filter(|d| d.add_to_chart > 0).count();
                crate::detect_diag::line(&format!(
                    "[live] drained detects raw={raw} add_to_chart>0={with_chart}"
                ));
            }
            if !detects.is_empty() && tx.send(FeedMsg::Detects(detects)).is_err() {
                break;
            }
        }

        // Снимок дёшев (Arc-clone), но читаем его по реальному domain event.
        // Таблицу ордеров в UI троттлим до ~4 Гц, а chart/order-line store
        // обновляем сразу на OrderEvent. Иначе короткий terminal status
        // (Cancel/Fail с deferred-removal=0) можно проспать между двумя table ticks.
        if server.feed.orders && has_orders_table_event && !orders_table_pending {
            orders_table_pending = true;
        }
        if (had_domain_event && (server.feed.orders || server.feed.reports))
            || (server.feed.orders && orders_table_pending)
        {
            let orders_due = last_orders.elapsed() >= Duration::from_millis(250);
            let orders_table_due = server.feed.orders && orders_table_pending && orders_due;
            let order_lines_due = server.feed.orders && has_order_line_event && !orders_table_due;
            if orders_table_due || order_lines_due || server.feed.reports {
                let Some(snap) = client.snapshot() else {
                    continue;
                };
                let order_rows = build_order_rows(
                    server.id,
                    &snap,
                    &events,
                    server.feed.reports,
                    &mut orders_index,
                );
                if orders_table_due {
                    last_orders = Instant::now();
                    orders_table_pending = false;
                    if tx.send(FeedMsg::Orders(order_rows)).is_err() {
                        break;
                    }
                } else if order_lines_due && tx.send(FeedMsg::OrderLines(order_rows)).is_err() {
                    break;
                }
            }
        }

        // Стратегии ядра (для окна стратегий): проверяем по domain event, не чаще
        // ~1 Гц, и шлём только при изменениях.
        if had_domain_event
            && server.feed.strategies
            && last_strats.elapsed() >= Duration::from_secs(1)
        {
            last_strats = Instant::now();
            if let Some(snap) = client.snapshot() {
                let strats = snap.strats();

                // Схема (секции/поля по видам) — при смене revision.
                let sr = strats.strategy_schema_revision();
                if sr != last_schema_rev {
                    last_schema_rev = sr;
                    if let Some(schema) = strats.strategy_schema() {
                        // Дефолты полей для нормализации дампов strat_db (см. ниже).
                        strat_schema_defaults = schema_default_fields(schema);
                        if tx
                            .send(FeedMsg::StrategySchema(build_schema_model(schema)))
                            .is_err()
                        {
                            break;
                        }
                    }
                }

                // Состав/значения — при смене сигнатуры (id/ver/last_date/checked).
                let mut sig = 0u64;
                for s in strats.snapshots() {
                    sig = sig
                        .wrapping_mul(1099511628211)
                        .wrapping_add(s.strategy_id)
                        .wrapping_add((s.strategy_ver as u32 as u64).wrapping_shl(1))
                        .wrapping_add(s.last_date)
                        .wrapping_add(s.checked as u64);
                }
                if sig != last_strat_sig {
                    last_strat_sig = sig;
                    // Колонка Strat таблицы ордеров резолвит strat_id → тип через этот же
                    // реестр (`build_order_row`). Реестр наполняется ПОЗЖЕ ордеров, а сама
                    // таблица пересобирается только на order-событии → до него видны сырые
                    // strat_id. Смена состава стратегий = повод пере-резолвить имена: взводим
                    // orders_table_pending, чтобы таблица пересобралась в пределах ~250мс даже
                    // без нового order-события (order_wait ниже разбудит цикл по таймеру).
                    if server.feed.orders {
                        orders_table_pending = true;
                    }
                    let strategies: Vec<StrategyRow> = strats
                        .snapshots()
                        .map(|s| {
                            let name = s
                                .strategy_name()
                                .filter(|n| !n.is_empty())
                                .map(str::to_string)
                                .unwrap_or_else(|| format!("strat {}", s.strategy_id));
                            let fields = s
                                .fields
                                .iter()
                                .map(|(n, v)| (n.to_string(), fmt_field(v)))
                                .collect();
                            StrategyRow {
                                id: s.strategy_id,
                                name,
                                kind: strat_kind_name(s.kind().ordinal()).to_string(),
                                kind_ordinal: s.kind().ordinal(),
                                folder_path: s.path.to_string(),
                                checked: s.checked,
                                is_short: s.is_short(),
                                fields,
                            }
                        })
                        .collect();
                    if tx.send(FeedMsg::Strategies(strategies)).is_err() {
                        break;
                    }

                    // strat_db: тот же снапшот — в локальную БД стратегий (head +
                    // версии по контент-диффу; эхо/косметика дедупятся у writer'а).
                    // Ждём схему: без материализации её дефолтов дампы неполны и
                    // при её приходе появились бы фантомные версии.
                    if !strat_schema_defaults.is_empty() {
                        if let Some(sink) = crate::strat_db::sink() {
                            local_strat_edits.prune();
                            let dumps: Vec<crate::strat_db::StratDump> = strats
                                .snapshots()
                                .map(|s| {
                                    strat_db_dump(
                                        s,
                                        &strat_schema_defaults,
                                        local_strat_edits.is_local(s.strategy_id),
                                    )
                                })
                                .collect();
                            sink.send(crate::strat_db::StratMsg::FullSet {
                                core_uid: server.uid,
                                core_name: server.name.clone(),
                                initial: strat_db_initial,
                                strategies: dumps,
                            });
                            strat_db_initial = false;
                        }
                    }
                }
            }
        }

        // Активы ядра (окно «Активы»): по domain event, не чаще ~1 Гц при живом окне.
        // Без окна снапшот всё равно нужен (баланс шапки, метрика плеча читают
        // global/leverage), но полный ребилд по всем рынкам ×N ядер каждую секунду —
        // лишний фон; сбавляем до 1 раза в 5 с. Цены живут от рынка, поэтому снимок
        // шлём целиком (UI гейтит перерисовку секундным ведром по assets_rev).
        // Transfer-активы — лишь при смене revision.
        let assets_every = if crate::feed::assets_view_active() {
            Duration::from_secs(1)
        } else {
            Duration::from_secs(5)
        };
        if had_domain_event && last_assets.elapsed() >= assets_every {
            last_assets = Instant::now();
            if let Some(snap) = client.snapshot() {
                // Базовая валюта аккаунта (USDT/BTC/…) — нужна для корректного пересчёта
                // `btc_balance_*` (исторически в базовой валюте) в USDT. Оттуда же —
                // фьючность ядра (маска BaseCheck): фьюч-активы UI режет до позиций.
                let info = client.server_info();
                let base = info
                    .as_ref()
                    .and_then(|i| i.base_currency_name.clone())
                    .unwrap_or_default();
                let futures_account = info
                    .as_ref()
                    .is_some_and(|i| i.supports(moonproto::ExchangeTypeMask::FUTURES));
                let assets =
                    build_assets(snap.markets(), snap.balances(), &base, futures_account);
                if tx.send(FeedMsg::Assets(assets)).is_err() {
                    break;
                }
            }
        }

        // Transfer-активы: проверяем КАЖДУЮ итерацию (а не в 1 Гц/domain-event блоке) — чтобы
        // ответ на `refresh_transfer_assets` (клик по ядру в окне «Активы») доходил до UI
        // сразу, даже если у ядра нет потока рыночных событий.
        if let Some(snap) = client.snapshot() {
            let tr = snap.transfer_assets();
            let rev = tr.revision();
            if rev != last_transfer_rev {
                last_transfer_rev = rev;
                let msg = build_transfer_assets(snap.markets(), tr);
                if tx.send(FeedMsg::TransferAssets(msg)).is_err() {
                    break;
                }
            }
        }

        // Рыночные данные НЕ переливаем здесь. Feed только сигналит, что у provider
        // появился свежий read-model snapshot; видимый chart сам подтянет нужные рынки.
        if !dirty_markets.is_empty() {
            if tx.send(FeedMsg::MarketDataChanged(dirty_markets)).is_err() {
                let _ = client.disconnect();
                return Ok(());
            }
        }
        force_market_sample = false;

        let order_wait = if server.feed.orders && orders_table_pending {
            let elapsed = last_orders.elapsed();
            Some(Duration::from_millis(250).saturating_sub(elapsed))
        } else {
            None
        };
        let wake_result = match order_wait {
            Some(timeout) => wake_rx.recv_timeout(timeout).map_err(|err| match err {
                std::sync::mpsc::RecvTimeoutError::Timeout => None,
                std::sync::mpsc::RecvTimeoutError::Disconnected => Some(()),
            }),
            None => wake_rx.recv().map_err(|_| Some(())),
        };
        match wake_result {
            Ok(()) => while wake_rx.try_recv().is_ok() {},
            Err(None) => {}
            Err(Some(())) => {
                let _ = client.disconnect();
                return Ok(());
            }
        }
    }

    let _ = client.disconnect();
    Ok(())
}

/// Hex-строка байт (без разделителей) — дамп blob chart-алертов для реверса
/// формата `TChartObject.Save()` (см. docs-internal, этап 0 алертов).
fn hex_dump(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}
