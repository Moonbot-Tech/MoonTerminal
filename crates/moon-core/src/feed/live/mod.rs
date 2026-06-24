//! Live-backend: подключение к ядру MoonBot через MoonProtoBeta.
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
    MoonEventSink, TransportMode,
};

use super::assets::{build_assets, build_transfer_assets};
use super::report::{send_close_report, OrderIndex};
use super::strategies::{alert_params, build_schema_model, fmt_field, strat_kind_name};
use super::{
    ConnStatus, CoreCmd, CoreLogLine, DetectRow, ExchangeId, FeedMsg, FeedTx, SharedMoonClient,
    StrategyRow,
};
use crate::config::ServerConfig;
use crate::db::ReportTx;
use crate::util::{now_unix_ms as now_ms, now_unix_ms_i64 as now_ms_i64};

use commands::drain_commands;
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
        .ok_or_else(|| anyhow::anyhow!("не удалось разобрать ключ MoonBot (server.key)"))?;

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
    // Курсор transfer-активов: шлём только при смене revision (request/response).
    let mut last_transfer_rev: u64 = u64::MAX;
    // Курсоры выгрузки стратегий: revision схемы и сигнатура состава/checked —
    // шлём только при изменениях (поля стратегий тяжёлые, гонять каждую секунду незачем).
    let mut last_schema_rev: u64 = u64::MAX;
    let mut last_strat_sig: u64 = u64::MAX;
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
        if drain_commands(
            cmd_rx,
            &client,
            server,
            &mut is_provider,
            &mut wanted,
            &mut wanted_orderbook,
            &mut force_market_sample,
        ) {
            return Ok(());
        }

        // Биржа ядра (из server_info после BaseCheck) — координатору для группировки
        // и выбора провайдера. Шлём один раз, как только идентичность известна.
        if !identity_sent {
            if let Some(info) = client.server_info() {
                if let Some(code) = info.exchange_code {
                    let _ = tx.send(FeedMsg::Identity(ExchangeId(code.stable_id())));
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
        let license_state = settings_event_snapshot(
            &events,
            &client,
            |ev| matches!(ev, &Event::Settings(SettingsEvent::KernelLicenseStateUpdated)),
            |state| state.settings().kernel_license_state.map(license_state_from_proto),
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
            |state| state.settings().lev_manage.as_ref().map(lev_manage_from_proto),
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
        if server.feed.detects || (server.feed.reports && reports.is_some()) || want_log {
            let mut detects: Vec<DetectRow> = Vec::new();
            let mut logs: Vec<CoreLogLine> = Vec::new();
            // Снимок для полей стратегии-источника детекта (SoundAlert/KeepAlert).
            let detect_snap = server.feed.detects.then(|| client.snapshot()).flatten();
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
                    Event::Detect(d) if server.feed.detects => {
                        let params = detect_snap
                            .as_ref()
                            .and_then(|s| s.strats().snapshot(d.strategy_id))
                            .map(alert_params)
                            .unwrap_or_default();
                        detect_seq += 1;
                        detects.push(DetectRow {
                            seq: detect_seq,
                            market: d.market_name.clone(),
                            time_ms: now_ms(),
                            sound_alert: params.sound_alert,
                            keep_alert_secs: params.keep_alert_secs,
                            add_to_chart: params.add_to_chart,
                            keep_in_chart_secs: params.keep_in_chart_secs,
                        });
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
                let order_rows =
                    build_order_rows(&snap, &events, server.feed.reports, &mut orders_index);
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
                }
            }
        }

        // Активы ядра (окно «Активы»): по domain event, не чаще ~1 Гц. Цены живут от
        // рынка, поэтому снимок шлём целиком каждую секунду (UI гейтит перерисовку
        // секундным ведром по assets_rev). Transfer-активы — лишь при смене revision.
        if had_domain_event && last_assets.elapsed() >= Duration::from_secs(1) {
            last_assets = Instant::now();
            if let Some(snap) = client.snapshot() {
                // Базовая валюта аккаунта (USDT/BTC/…) — нужна для корректного пересчёта
                // `btc_balance_*` (исторически в базовой валюте) в USDT.
                let base = client
                    .server_info()
                    .and_then(|i| i.base_currency_name)
                    .unwrap_or_default();
                let assets = build_assets(snap.markets(), snap.balances(), &base);
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
