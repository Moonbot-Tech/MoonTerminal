//! Live-backend: подключение к ядру MoonBot через MoonProtoBeta.
//! Единственный модуль, знающий про moonproto.
//!
//! Поток: event-driven. `MoonEventSink` будит backend thread после реального события;
//! market data остаётся в immutable read-model snapshot, сюда идёт только лёгкий сигнал.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use moonproto::state::{
    AccountEvent, MarketHistorySizing, MarketsEvent, Order, OrderBookEvent, OrderEvent,
    OrderTraceChartPoint, OrderTraceLine, SettingsEvent, TradesEvent,
};
use moonproto::{
    ClientConfig, ConnectConfig, Event, InitConfig, InitialStrategies, LifecycleEvent, MoonClient,
    MoonEventSink, StrategyFields, StrategyKind, StrategySchema, StrategySnapshot,
    TradesStreamMode, TransportMode,
};

use super::assets::{build_assets, build_transfer_assets, to_exchange_kind};
use super::report::{send_close_report, OrderIndex, OrderMeta};
use super::strategies::{
    alert_params, build_schema_model, fmt_field, fv_from_str, strat_kind_name,
};
use super::{
    ClientSettings, ClientSettingsEdit, ConnStatus, CoreCmd, CoreLogLine, DetectRow, ExchangeId,
    FeedMsg, FeedTx, LevManageEdit, LevManageState, LicenseState, MarketDirty, MarketDirtyFlags,
    OrderRow, OrderTrace, OrderTracePoint, RuntimeState, SharedMoonClient, StrategyRow,
};
use crate::config::ServerConfig;
use crate::db::ReportTx;

use crate::util::{now_unix_ms as now_ms, now_unix_ms_i64 as now_ms_i64};

/// Общий путь синка стратегий: берём ПОЛНЫЙ текущий набор, даём его `build` на правку
/// (патч полей / смена пути / добавление новых), и если что-то изменилось — шлём ОДИН
/// `sync_local_strategies` + лог. `build` возвращает число затронутых. Бамп `last_date`
/// (rollback-guard Delphi) делает сам `build` у изменённых снапшотов.
fn rebuild_sync(
    client: &MoonClient,
    server_id: u64,
    action: &str,
    build: impl FnOnce(&mut Vec<StrategySnapshot>, Option<&StrategySchema>, u64) -> usize,
) {
    if let Some(snap) = client.snapshot() {
        let strats = snap.strats();
        let schema = strats.strategy_schema();
        let now = now_ms() as u64;
        let mut full: Vec<StrategySnapshot> = strats.snapshots().cloned().collect();
        let changed = build(&mut full, schema, now);
        if changed > 0 {
            let _ = client.strategies().sync_local_strategies(full);
            log::info!("core {server_id} {action} {changed} strategies");
        }
    }
}

fn trace_point(p: OrderTraceChartPoint) -> Option<OrderTracePoint> {
    let time_ms = p.unix_millis() as f64;
    (time_ms > 1.0 && p.price.is_finite() && p.price > 0.0).then_some(OrderTracePoint {
        time_ms,
        price: p.price,
    })
}

fn moon_time_to_unix_seconds(time: moonproto::MoonTime) -> Option<i64> {
    let millis = time.unix_millis();
    (millis > 0).then_some(millis.div_euclid(1000))
}

fn moon_time_to_unix_millis_f64(time: moonproto::MoonTime) -> f64 {
    let millis = time.unix_millis();
    if millis > 0 {
        millis as f64
    } else {
        0.0
    }
}

fn license_state_from_proto(license: moonproto::KernelLicenseStateCommand) -> LicenseState {
    LicenseState {
        paid_version: license.paid_version,
        reg_id: license.reg_id,
        moon_credits: license.moon_credits,
        moon_credits_hold: license.moon_credits_hold,
        moon_credits_auction: license.moon_credits_auction,
        can_use_watcher: license.can_use_watcher,
    }
}

/// Плоская проекция moonproto `ClientSettings` → терминальный снимок. Raw-поля
/// (`s_price`/`sb_num`/…) в проде `pub(crate)`, поэтому читаем ТОЛЬКО через хелперы.
fn client_settings_from_proto(c: &moonproto::ClientSettingsCommand) -> ClientSettings {
    let fixed_sell_pcts =
        std::array::from_fn(|i| c.fixed_sell_preset_percent(i + 1).unwrap_or(0.0));
    ClientSettings {
        take_profit_pct: c.effective_take_profit_percent(),
        take_profit_extended: c.x_tmode,
        fixed_sell_mode: c.fixed_sell_mode,
        stop_loss_pct: c.price_drop_level,
        trailing_drop_pct: c.trailing_drop,
        use_global_take_profit: c.use_g_take_profit,
        global_take_profit_pct: c.g_take_profit,
        panic_if_price_drop: c.panic_if_price_drop,
        emu_mode: c.emu_mode,
        buy_iceberg: c.buy_iceberg,
        sell_iceberg: c.sell_iceberg,
        sign_orders: c.sign_orders,
        use_stop_market: c.use_stop_market,
        fixed_sell_pcts,
        fixed_sell_slot: c.selected_fixed_sell_slot(),
    }
}

fn lev_manage_from_proto(l: &moonproto::LevManage) -> LevManageState {
    LevManageState {
        auto_max_order: l.auto_max_order,
        auto_lev_up: l.auto_lev_up,
        auto_isolated: l.auto_isolated,
        auto_cross: l.auto_cross,
        auto_fix_lev: l.auto_fix_lev,
        fix_lev: l.fix_lev,
        tlg_report: l.tlg_report,
        lev_control: l.lev_control.clone(),
    }
}

fn runtime_state_from_proto(s: &moonproto::RuntimeStateCommand) -> RuntimeState {
    RuntimeState {
        is_started: s.is_started,
        auto_detect_active: s.auto_detect_active,
    }
}

/// Применяет точечную правку тулбара к удержанному снимку настроек ЧЕРЕЗ хелперы команды
/// (raw-поля `s_price`/`sb_num` в проде `pub(crate)`; `price_drop_level` — pub).
fn apply_client_settings_edit(s: &mut moonproto::ClientSettingsCommand, edit: ClientSettingsEdit) {
    match edit {
        ClientSettingsEdit::TakeProfit { pct, extended } => {
            // x_tmode/«s9»: on → x_sell хранит pct/10 (видимые 100..900%); off → x_sell=pct
            // напрямую (1..100%). Ядро без флага само режет TP до 100, поэтому пишем оба поля.
            s.fixed_sell_mode = false;
            if extended {
                s.x_tmode = true;
                s.x_sell = (pct / 10.0).round().clamp(10.0, 90.0) as i32;
            } else {
                s.x_tmode = false;
                s.x_sell = pct.round().clamp(1.0, 100.0) as i32;
            }
        }
        ClientSettingsEdit::StopLossPct(pct) => s.price_drop_level = pct,
        ClientSettingsEdit::ScalpTakeProfit(pct) => s.set_scalp_take_profit_percent(pct),
        ClientSettingsEdit::SelectFixedSellSlot(slot) => {
            // Включаем fixed-sell режим — иначе effective TP остаётся на x_sell и не меняется.
            // С ним effective_take_profit_percent() = процент выбранного пресета → TP в тулбаре
            // становится равным значению S-кнопки.
            s.fixed_sell_mode = true;
            s.set_selected_fixed_sell_slot(slot);
        }
        ClientSettingsEdit::SetFixedSellPct { slot, pct } => {
            // Видимый процент = s_price · (x_tmode? 10 : 1); пишем s_price обратным пересчётом.
            let price = if s.x_tmode {
                (pct / 10.0) as f32
            } else {
                pct as f32
            };
            s.set_fixed_sell_preset_price(slot, price);
        }
    }
}

fn apply_lev_manage_edit(l: &mut moonproto::LevManage, edit: LevManageEdit) {
    match edit {
        LevManageEdit::FixLev(n) => {
            l.auto_fix_lev = true;
            l.fix_lev = n;
        }
    }
}

fn push_dirty(
    dirty: &mut HashMap<String, MarketDirtyFlags>,
    market: impl Into<String>,
    flags: MarketDirtyFlags,
) {
    dirty
        .entry(market.into())
        .and_modify(|existing| *existing = existing.union(flags))
        .or_insert(flags);
}

fn push_wanted_dirty(
    dirty: &mut HashMap<String, MarketDirtyFlags>,
    wanted: &[String],
    flags: MarketDirtyFlags,
) {
    for market in wanted {
        push_dirty(dirty, market.clone(), flags);
    }
}

fn market_dirty_from_events(
    events: &[Event],
    wanted: &[String],
    force_sample: bool,
) -> Vec<MarketDirty> {
    let mut dirty = HashMap::<String, MarketDirtyFlags>::new();
    if force_sample {
        push_wanted_dirty(&mut dirty, wanted, MarketDirtyFlags::ALL);
    }

    for event in events {
        match event {
            Event::OrderBook(OrderBookEvent::Apply {
                market_name: Some(market),
                ..
            }) => {
                push_dirty(&mut dirty, market.to_string(), MarketDirtyFlags::ORDERBOOK);
            }
            Event::Trade(TradesEvent::Applied { .. }) => {
                // MoonProto keeps TradesEvent intentionally small and does not
                // expose market names here. The terminal still narrows the wake
                // to provider-wanted markets instead of waking all charts on
                // every domain event.
                push_wanted_dirty(&mut dirty, wanted, MarketDirtyFlags::HISTORY);
            }
            Event::Markets(MarketsEvent::PricesUpdated { .. }) => {
                push_wanted_dirty(&mut dirty, wanted, MarketDirtyFlags::HISTORY);
            }
            Event::Markets(
                MarketsEvent::MarketsListReplaced { .. }
                | MarketsEvent::NewMarketsAdded { .. }
                | MarketsEvent::IndexesUpdated { .. },
            ) => {
                push_wanted_dirty(&mut dirty, wanted, MarketDirtyFlags::MARKET_META);
            }
            _ => {}
        }
    }

    dirty
        .into_iter()
        .map(|(market, flags)| MarketDirty::new(market, flags))
        .collect()
}

fn order_trace(line: &OrderTraceLine) -> Option<OrderTrace> {
    let points: Vec<OrderTracePoint> = line
        .points
        .iter()
        .copied()
        .filter_map(trace_point)
        .collect();
    if points.is_empty() {
        return None;
    }
    Some(OrderTrace {
        points,
        tmp_point: line.tmp_point.and_then(trace_point),
        stop_price: line.stop_price.filter(|p| p.is_finite() && *p > 0.0),
    })
}

fn build_order_row(
    snap: &moonproto::MoonStateSnapshot,
    o: &Order,
    remember_reports: bool,
    orders_index: &mut OrderIndex,
) -> OrderRow {
    // Полные данные ордера по uid (есть с открытия); db_id→uid —
    // когда db_id появился (перед закрытием). close-SQL этих полей
    // не несёт.
    if remember_reports {
        orders_index.remember(
            o.uid,
            OrderMeta {
                coin: o.market_name.clone(),
                isshort: o.is_short,
                buyprice: o.buy_price,
                sellprice: o.sell_price,
                quantity: o.buy_order.quantity,
                spentbtc: o.buy_order.spent_btc,
                gainedbtc: o.sell_order.total_btc,
                lev: o.buy_order.leverage as i64,
                strategyid: o.strat_id as i64,
                taskid: o.uid as i64,
                exorderid: (o.buy_order.int_id != 0).then(|| o.buy_order.int_id.to_string()),
                emulator: o.emulator_mode,
                buydate: moon_time_to_unix_seconds(o.buy_order.open_time()),
                sellsetdate: moon_time_to_unix_seconds(o.sell_order.create_time()),
                closedate: moon_time_to_unix_seconds(o.sell_order.close_time()),
            },
        );
        if o.db_id != 0 {
            orders_index.map_dbid(o.db_id, o.uid);
        }
    }

    let strat = match snap.strats().snapshot(o.strat_id) {
        Some(s) => strat_kind_name(s.kind().ordinal()).to_string(),
        None => o.strat_id.to_string(),
    };
    let leg = if o.is_short {
        &o.sell_order
    } else {
        &o.buy_order
    };
    let fill_pct = if leg.quantity > 0.0 {
        ((leg.quantity - leg.quantity_remaining) / leg.quantity * 100.0) as f32
    } else {
        0.0
    };
    let pick = |qb: f64, q: f64, qr: f64| {
        if qb != 0.0 {
            qb
        } else if q != 0.0 {
            q
        } else {
            qr
        }
    };
    let bs = pick(
        o.buy_order.quantity_base,
        o.buy_order.quantity,
        o.buy_order.quantity_remaining,
    );
    let ss = pick(
        o.sell_order.quantity_base,
        o.sell_order.quantity,
        o.sell_order.quantity_remaining,
    );
    let size = if bs.abs() >= ss.abs() { bs } else { ss };

    let mkt = snap.markets().price(&o.market_name);
    let last = mkt.as_ref().map(|p| p.p_last as f32).unwrap_or(0.0);
    let entry = o.buy_price;
    let valid_entry = entry.is_finite() && entry > 0.0;
    let fin = |v: f64| (v.is_finite() && v > 0.0).then_some(v);
    let pct_stop = |level: f64| {
        if o.is_short {
            entry * (1.0 + level / 100.0)
        } else {
            entry * (1.0 - level / 100.0)
        }
    };
    let stop_loss = if o.stops.stop_loss_enabled() {
        if o.stops.stop_loss_fixed() {
            fin(o.stops.stop_loss_level())
        } else if valid_entry {
            fin(pct_stop(o.stops.stop_loss_level()))
        } else {
            None
        }
    } else {
        None
    };
    let trailing = if o.stops.trailing_enabled() {
        if o.stops.trailing_fixed() {
            fin(o.stops.trailing_level())
        } else if valid_entry {
            fin(pct_stop(o.stops.trailing_level()))
        } else {
            None
        }
    } else {
        None
    };
    let take_profit = o
        .stops
        .take_profit_enabled()
        .then(|| fin(o.stops.take_profit()))
        .flatten();
    let vstop = o.vstop_on.then(|| fin(o.vstop_level)).flatten();
    let pending_cond = o.pending_buy_cond_price.and_then(fin);
    let liq = snap.markets().get(&o.market_name).and_then(|h| {
        let bp = h.balance_position();
        let v = if o.is_short {
            bp.short_liq_price
        } else {
            bp.long_liq_price
        };
        fin(v).or_else(|| fin(bp.liq_price))
    });
    let pending = o.pending_buy_cond_price.is_some();
    let filled = fill_pct > 0.0;
    let create_time_ms = moon_time_to_unix_millis_f64(o.buy_order.create_time());
    OrderRow {
        market: o.market_name.clone(),
        is_short: o.is_short,
        size,
        sl_on: o.stops.stop_loss_enabled(),
        ts_on: o.stops.trailing_enabled(),
        vstop_on: o.vstop_on,
        buy_price: o.buy_price,
        sell_price: o.sell_price,
        create_time_ms,
        price: last,
        fill_pct,
        strat,
        uid: o.uid,
        emulator: o.emulator_mode,
        job_is_done: o.job_is_done,
        pending,
        filled,
        stop_loss,
        trailing,
        take_profit,
        vstop,
        pending_cond,
        liq,
        panic_sell: o.panic_sell,
        is_moon_shot: o.is_moon_shot,
        corridor_price_down: o.corridor_price_down,
        corridor_price_up: o.corridor_price_up,
        buy_trace: o.buy_trace_line.as_ref().and_then(order_trace),
        sell_trace: o.sell_trace_line.as_ref().and_then(order_trace),
    }
}

fn build_order_rows(
    snap: &moonproto::MoonStateSnapshot,
    events: &[Event],
    remember_reports: bool,
    orders_index: &mut OrderIndex,
) -> Vec<OrderRow> {
    let mut order_rows = Vec::new();
    for o in snap.orders().iter() {
        order_rows.push(build_order_row(snap, o, remember_reports, orders_index));
    }

    // Snapshot is the live view. Terminal statuses can be removed from that view
    // before the app drains the event queue, so MoonProto carries an Arc<Order>
    // on Created/Updated/Removed. Overlay those captured rows onto the full live
    // snapshot: OrderLineStore still receives a complete seen-set, while closed
    // rows cannot vanish into BackstopMissing.
    for ev in events {
        let Event::Order(order_event) = ev else {
            continue;
        };
        let Some(order) = order_event.order() else {
            continue;
        };
        let row = build_order_row(snap, order, remember_reports, orders_index);
        if let Some(existing) = order_rows.iter_mut().find(|r| r.uid == row.uid) {
            *existing = row;
        } else {
            order_rows.push(row);
        }
    }

    order_rows
}

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
        loop {
            match cmd_rx.try_recv() {
                Ok(CoreCmd::SetMarket {
                    provider,
                    markets,
                    orderbook_markets,
                }) => {
                    // Переход провайдерства: вкл → ретейним все трейды биржи; выкл →
                    // снимаем подписку. Курсоры чтения market history живут у потребителя.
                    if provider != is_provider {
                        if provider {
                            let _ = client
                                .streams()
                                .subscribe_all_trades(TradesStreamMode::TradesOnly);
                            log::info!("core {} → market provider (all-trades)", server.id);
                        } else {
                            let _ = client.streams().unsubscribe_all_trades();
                            log::info!("core {} → account-only", server.id);
                        }
                        is_provider = provider;
                    }
                    // Не провайдер не обслуживает рынки (стакан/чтение) вообще.
                    let markets = if provider { markets } else { Vec::new() };
                    // Стакан подписываем ТОЛЬКО для рынков, которым он нужен (orderbook_markets ⊆
                    // markets). Рынок без стакана читается (трейды/история), но стакан не качаем.
                    let orderbook_markets = if provider {
                        orderbook_markets
                    } else {
                        Vec::new()
                    };
                    let diag_on = || {
                        std::env::var_os("MOON_MARKET_DIAG").is_some()
                            || std::env::var_os("MOON_RENDER_DIAG").is_some()
                    };
                    // Диф подписки стакана: новым из orderbook_markets — subscribe, ушедшим — unsubscribe.
                    for m in &orderbook_markets {
                        if !wanted_orderbook.iter().any(|w| w == m) {
                            match client.streams().subscribe_orderbook(m.clone()) {
                                Ok(()) => {
                                    if diag_on() {
                                        log::info!(
                                            "[market_diag] core {} subscribe_orderbook({m})",
                                            server.id
                                        );
                                    }
                                }
                                Err(error) => log::warn!(
                                    "core {} subscribe_orderbook({m}) failed: {error}",
                                    server.id
                                ),
                            }
                        }
                    }
                    for m in &wanted_orderbook {
                        if !orderbook_markets.iter().any(|x| x == m) {
                            match client.streams().unsubscribe_orderbook(m.clone()) {
                                Ok(()) => {
                                    if diag_on() {
                                        log::info!(
                                            "[market_diag] core {} unsubscribe_orderbook({m})",
                                            server.id
                                        );
                                    }
                                }
                                Err(error) => log::warn!(
                                    "core {} unsubscribe_orderbook({m}) failed: {error}",
                                    server.id
                                ),
                            }
                        }
                    }
                    wanted = markets;
                    wanted_orderbook = orderbook_markets;
                    force_market_sample = true;
                }
                Ok(CoreCmd::StrategiesAction { checks, start_stop }) => {
                    // 1. Синхронизация галок: правим локальный checked у изменённых и
                    //    шлём серверу дельту (CheckedSync).
                    for (id, checked) in &checks {
                        if let Err(error) = client.strategies().set_checked(*id, *checked) {
                            log::warn!(
                                "core {} set strategy {id} checked={checked} failed: {error}",
                                server.id
                            );
                        }
                    }
                    if !checks.is_empty() {
                        if let Err(error) = client.strategies().send_checked_delta() {
                            log::warn!("core {} send checked delta failed: {error}", server.id);
                        }
                    }
                    // 2. Старт/стоп отмеченных (отдельная команда движка).
                    match start_stop {
                        Some(true) => {
                            if let Err(error) = client.strategies().start() {
                                log::warn!("core {} start strategies failed: {error}", server.id);
                            }
                        }
                        Some(false) => {
                            if let Err(error) = client.strategies().stop() {
                                log::warn!("core {} stop strategies failed: {error}", server.id);
                            }
                        }
                        None => {}
                    }
                    log::info!(
                        "core {} strategies action: checks={} start_stop={:?}",
                        server.id,
                        checks.len(),
                        start_stop
                    );
                }
                Ok(CoreCmd::EditStrategyFields { edits }) => {
                    // `sync_local_strategies` СИНХРОНИТ ВЕСЬ локальный набор (moonproto делает
                    // replace_with_snapshots). Патчим ВСЕ указанные в `edits` за один проход → один
                    // sync (раздельные команды на стратегии одного ядра перетёрли бы друг друга).
                    rebuild_sync(&client, server.id, "edit", |full, schema, now| {
                        let mut edited = 0usize;
                        for sc in full.iter_mut() {
                            let Some((_, changes)) =
                                edits.iter().find(|(id, _)| *id == sc.strategy_id)
                            else {
                                continue;
                            };
                            for (name, val) in changes {
                                let existing = sc.fields.get(name).cloned();
                                let stype = schema.and_then(|s| s.field(name)).map(|f| f.type_id);
                                sc.fields.insert(
                                    name.as_str(),
                                    fv_from_str(existing.as_ref(), stype, val),
                                );
                            }
                            sc.last_date = now.max(sc.last_date + 1);
                            edited += 1;
                        }
                        edited
                    });
                }
                Ok(CoreCmd::DeleteStrategy { id }) => {
                    // `TStratDelete(strategy_id=id, folder_path="")` — удалить одну стратегию.
                    // Правило «только выключенные» проверено в UI до отправки.
                    if let Err(error) = client.strategies().delete(id, "") {
                        log::warn!("core {} delete strategy {id} failed: {error}", server.id);
                    }
                    log::info!("core {} delete strategy {id}", server.id);
                }
                Ok(CoreCmd::DeleteFolder { path }) => {
                    // `TStratDelete(strategy_id=0, folder_path=path)` — удалить папку целиком.
                    if let Err(error) = client.strategies().delete(0, path.as_str()) {
                        log::warn!("core {} delete folder {path} failed: {error}", server.id);
                    }
                    log::info!("core {} delete folder {path}", server.id);
                }
                Ok(CoreCmd::CreateStrategies { specs }) => {
                    // К полному набору добавляем новые снапшоты. id = max+1 ЦЕЛЕВОГО ядра
                    // (безопасно для межъядерной вставки). Поля — из строк по типу схемы
                    // (как fv_from_str при правках), existing=None.
                    rebuild_sync(&client, server.id, "create", |full, schema, now| {
                        let mut next_id = full.iter().map(|s| s.strategy_id).max().unwrap_or(0) + 1;
                        for spec in &specs {
                            let id = next_id;
                            next_id += 1;
                            let mut fields = StrategyFields::new();
                            for (name, val) in &spec.fields {
                                let stype = schema.and_then(|s| s.field(name)).map(|f| f.type_id);
                                fields.insert(name.as_str(), fv_from_str(None, stype, val));
                            }
                            full.push(StrategySnapshot::new(
                                id,
                                0,
                                now,
                                false,
                                StrategyKind::from_ordinal(spec.kind_ordinal),
                                spec.folder_path.clone(),
                                fields,
                            ));
                        }
                        specs.len()
                    });
                }
                Ok(CoreCmd::MoveStrategies { moves }) => {
                    // Смена `path` у указанных стратегий + bump last_date → один sync.
                    rebuild_sync(&client, server.id, "move", |full, _schema, now| {
                        let mut changed = 0usize;
                        for sc in full.iter_mut() {
                            if let Some((_, new_path)) =
                                moves.iter().find(|(id, _)| *id == sc.strategy_id)
                            {
                                sc.path = new_path.as_str().into();
                                sc.last_date = now.max(sc.last_date + 1);
                                changed += 1;
                            }
                        }
                        changed
                    });
                }
                Ok(CoreCmd::TransferAsset {
                    asset,
                    qty,
                    from,
                    to,
                }) => {
                    // Перенос строго в пределах ЭТОГО ядра (клиент конкретного ядра).
                    if let Err(error) = client.balances().transfer_asset(
                        &asset,
                        qty,
                        to_exchange_kind(from),
                        to_exchange_kind(to),
                    ) {
                        log::warn!(
                            "core {} transfer {qty} {asset} {from:?}->{to:?} failed: {error}",
                            server.id
                        );
                    }
                    // После переноса просим свежий список — UI увидит новые остатки.
                    if let Err(error) = client.balances().refresh_transfer_assets() {
                        log::warn!("core {} refresh transfer assets failed: {error}", server.id);
                    }
                    log::info!("core {} transfer {qty} {asset} {from:?}->{to:?}", server.id);
                }
                Ok(CoreCmd::RefreshTransferAssets) => {
                    if let Err(error) = client.balances().refresh_transfer_assets() {
                        log::warn!("core {} refresh transfer assets failed: {error}", server.id);
                    }
                }
                Ok(CoreCmd::ConvertDust) => {
                    // Конверсия мелких остатков в BNB (Engine API), необратимо.
                    if let Err(error) = client.balances().convert_dust_bnb() {
                        log::warn!("core {} convert dust failed: {error}", server.id);
                    }
                    if let Err(error) = client.balances().refresh_transfer_assets() {
                        log::warn!("core {} refresh transfer assets failed: {error}", server.id);
                    }
                    log::info!("core {} convert dust", server.id);
                }
                Ok(CoreCmd::PlaceOrder {
                    market,
                    short,
                    price,
                    size,
                    strategy_id,
                }) => {
                    super::trade::place_order(
                        &client,
                        server.id,
                        market,
                        short,
                        price,
                        size,
                        strategy_id,
                    );
                }
                Ok(CoreCmd::MoveOrder { uid, new_price }) => {
                    super::trade::move_order(&client, server.id, uid, new_price);
                }
                Ok(CoreCmd::CancelOrder { uid }) => {
                    super::trade::cancel_order(&client, server.id, uid);
                }
                Ok(CoreCmd::EditClientSettings(edit)) => {
                    // Правим УДЕРЖАННЫЙ снимок (moonproto хранит последний в SettingsState),
                    // сохраняя tail/blob'ы, и шлём его целиком. Нет снимка → нечего слать.
                    match client
                        .snapshot()
                        .and_then(|s| s.settings().client_settings.clone())
                    {
                        Some(mut settings) => {
                            apply_client_settings_edit(&mut settings, edit);
                            if let Err(error) = client.settings().send(settings) {
                                log::warn!(
                                    "core {} send client settings failed: {error}",
                                    server.id
                                );
                            } else {
                                log::info!("core {} client settings edit {edit:?} sent", server.id);
                            }
                        }
                        None => log::warn!(
                            "core {} edit client settings ignored: no snapshot yet",
                            server.id
                        ),
                    }
                }
                Ok(CoreCmd::EditLevManage(edit)) => {
                    match client
                        .snapshot()
                        .and_then(|s| s.settings().lev_manage.clone())
                    {
                        Some(mut lev) => {
                            apply_lev_manage_edit(&mut lev, edit);
                            if let Err(error) = client.settings().manage_leverage(&lev) {
                                log::warn!("core {} manage leverage failed: {error}", server.id);
                            } else {
                                log::info!("core {} lev edit {edit:?} sent", server.id);
                            }
                        }
                        None => log::warn!(
                            "core {} edit lev manage ignored: no snapshot yet",
                            server.id
                        ),
                    }
                }
                Ok(CoreCmd::SetHedgeMode(on)) => {
                    // РЕАЛЬНОЕ действие на бирже (Engine API). Тикет игнорируем — итог придёт
                    // событием HedgeModeUpdated, которое обновит стор.
                    match client.account().set_hedge_mode(on) {
                        Ok(_ticket) => log::info!("core {} set hedge mode -> {on}", server.id),
                        Err(error) => {
                            log::warn!("core {} set hedge mode -> {on} failed: {error}", server.id)
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let _ = client.disconnect();
                    return Ok(());
                }
            }
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
        let license_state = if events.iter().any(|ev| {
            matches!(
                ev,
                &Event::Settings(SettingsEvent::KernelLicenseStateUpdated)
            )
        }) {
            client
                .snapshot()
                .and_then(|state| state.settings().kernel_license_state)
                .map(license_state_from_proto)
        } else {
            None
        };
        if let Some(license) = license_state {
            if tx.send(FeedMsg::License(license)).is_err() {
                break;
            }
        }
        // ClientSettings/LevManage/RuntimeState — снимки настроек ядра. Каждый тянем из
        // snapshot ТОЛЬКО когда пришло его событие (а не каждый тик), как и license выше.
        let client_settings = if events
            .iter()
            .any(|ev| matches!(ev, &Event::Settings(SettingsEvent::ClientSettingsUpdated)))
        {
            client.snapshot().and_then(|state| {
                state
                    .settings()
                    .client_settings
                    .as_ref()
                    .map(client_settings_from_proto)
            })
        } else {
            None
        };
        if let Some(settings) = client_settings {
            if tx.send(FeedMsg::ClientSettings(settings)).is_err() {
                break;
            }
        }
        let lev_manage = if events
            .iter()
            .any(|ev| matches!(ev, &Event::Settings(SettingsEvent::LevManageUpdated)))
        {
            client.snapshot().and_then(|state| {
                state
                    .settings()
                    .lev_manage
                    .as_ref()
                    .map(lev_manage_from_proto)
            })
        } else {
            None
        };
        if let Some(lev) = lev_manage {
            if tx.send(FeedMsg::LevManage(lev)).is_err() {
                break;
            }
        }
        let runtime_state = if events
            .iter()
            .any(|ev| matches!(ev, &Event::Settings(SettingsEvent::RuntimeStateUpdated)))
        {
            client.snapshot().and_then(|state| {
                state
                    .settings()
                    .runtime_state
                    .as_ref()
                    .map(runtime_state_from_proto)
            })
        } else {
            None
        };
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
