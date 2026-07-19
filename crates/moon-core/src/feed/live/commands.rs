//! Дренаж команд роли от координатора (полное желаемое состояние, не дельта): рыночная роль,
//! стратегии (sync/checked/start-stop), ручная торговля, активы и правки настроек.

use std::sync::mpsc::{Receiver, TryRecvError};

use moonproto::{
    MoonClient, StrategyFields, StrategyKind, StrategySchema, StrategySnapshot, TradesStreamMode,
};

use super::convert::{apply_client_settings_edit, apply_lev_manage_edit};
use crate::config::ServerConfig;
use crate::feed::assets::to_exchange_kind;
use crate::feed::strategies::{fv_from_str, strat_kind_name};
use crate::feed::{order_edit, trade, CoreCmd};
use crate::util::now_unix_ms as now_ms;

/// Значение поля `SignalType` → ordinal вида стратегии (`StrategyKind`). В Moonbot тип
/// (вид) стратегии И ЕСТЬ её SignalType, но snapshot хранит вид отдельным байтом `kind`
/// (не полем), поэтому правка поля сама по себе вид не меняет — мапим строку в ordinal,
/// чтобы синхронно пересобрать снапшот. Сначала по именам видов из схемы ядра (авторитет),
/// затем фолбэк на наш хардкод имён. Нет совпадения → None (вид не трогаем).
fn signaltype_to_kind_ordinal(schema: Option<&StrategySchema>, value: &str) -> Option<u8> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    if let Some(s) = schema {
        if let Some(k) = s.kinds.iter().find(|k| k.name.eq_ignore_ascii_case(v)) {
            return Some(k.ordinal());
        }
    }
    (0u8..=23).find(|o| strat_kind_name(*o).eq_ignore_ascii_case(v))
}

/// Кольцо недавних ЛОКАЛЬНЫХ правок стратегий — для пометки `origin=local` у
/// версий strat_db (эхо сервера придёт снапшотом через feed-цикл). `wildcard` —
/// команды без известного id (создание: id назначается внутри rebuild_sync).
/// TTL короткий: эхо приходит за секунды, протухшее — чужая правка.
pub(super) struct LocalStratEdits {
    ids: std::collections::HashMap<u64, std::time::Instant>,
    wildcard: Option<std::time::Instant>,
}

const LOCAL_EDIT_TTL: std::time::Duration = std::time::Duration::from_secs(30);

impl LocalStratEdits {
    pub(super) fn new() -> Self {
        Self {
            ids: std::collections::HashMap::new(),
            wildcard: None,
        }
    }

    fn mark(&mut self, id: u64) {
        self.ids.insert(id, std::time::Instant::now());
    }

    fn mark_all(&mut self) {
        self.wildcard = Some(std::time::Instant::now());
    }

    /// Правка этого id (или любая) отправлена нами недавно?
    pub(super) fn is_local(&self, id: u64) -> bool {
        let fresh = |t: &std::time::Instant| t.elapsed() < LOCAL_EDIT_TTL;
        self.ids.get(&id).map(fresh).unwrap_or(false)
            || self.wildcard.as_ref().map(fresh).unwrap_or(false)
    }

    pub(super) fn prune(&mut self) {
        self.ids.retain(|_, t| t.elapsed() < LOCAL_EDIT_TTL);
        if self
            .wildcard
            .map(|t| t.elapsed() >= LOCAL_EDIT_TTL)
            .unwrap_or(false)
        {
            self.wildcard = None;
        }
    }
}

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

/// Дренаж команд роли от координатора (полное желаемое состояние, не дельта).
/// Мутирует рыночную роль ядра (`is_provider`/`wanted`/`wanted_orderbook`) и
/// взводит `force_market_sample`. Возвращает `true`, если канал команд закрыт
/// (координатор ушёл) → ядро должно отключиться и выйти из `run`.
pub(super) fn drain_commands(
    cmd_rx: &Receiver<CoreCmd>,
    client: &MoonClient,
    server: &ServerConfig,
    is_provider: &mut bool,
    wanted: &mut Vec<String>,
    wanted_orderbook: &mut Vec<String>,
    force_market_sample: &mut bool,
    orders_mutated: &mut bool,
    local_strat_edits: &mut LocalStratEdits,
) -> bool {
    loop {
        match cmd_rx.try_recv() {
            Ok(CoreCmd::SetMarket {
                provider,
                markets,
                orderbook_markets,
            }) => {
                // Переход провайдерства: вкл → ретейним все трейды биржи; выкл →
                // снимаем подписку. Курсоры чтения market history живут у потребителя.
                if provider != *is_provider {
                    if provider {
                        let _ = client
                            .streams()
                            .subscribe_all_trades(TradesStreamMode::TradesOnly);
                        log::info!("core {} → market provider (all-trades)", server.id);
                    } else {
                        let _ = client.streams().unsubscribe_all_trades();
                        log::info!("core {} → account-only", server.id);
                    }
                    *is_provider = provider;
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
                for m in wanted_orderbook.iter() {
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
                *wanted = markets;
                *wanted_orderbook = orderbook_markets;
                *force_market_sample = true;
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
                for (id, _) in &edits {
                    local_strat_edits.mark(*id);
                }
                // `sync_local_strategies` СИНХРОНИТ ВЕСЬ локальный набор (moonproto делает
                // replace_with_snapshots). Патчим ВСЕ указанные в `edits` за один проход → один
                // sync (раздельные команды на стратегии одного ядра перетёрли бы друг друга).
                rebuild_sync(client, server.id, "edit", |full, schema, now| {
                    let mut edited = 0usize;
                    for sc in full.iter_mut() {
                        let Some((_, changes)) = edits.iter().find(|(id, _)| *id == sc.strategy_id)
                        else {
                            continue;
                        };
                        for (name, val) in changes {
                            let existing = sc.fields.get(name).cloned();
                            let stype = schema.and_then(|s| s.field(name)).map(|f| f.type_id);
                            sc.fields
                                .insert(name.as_str(), fv_from_str(existing.as_ref(), stype, val));
                        }
                        // Смена SignalType = смена вида стратегии: snapshot держит вид
                        // отдельным (pub(crate)) байтом, а не полем, поэтому пересобираем
                        // снапшот с новым `kind` (иначе бейдж вида в дереве остаётся старым).
                        if let Some((_, sig)) = changes
                            .iter()
                            .find(|(n, _)| n.eq_ignore_ascii_case("SignalType"))
                        {
                            if let Some(ord) = signaltype_to_kind_ordinal(schema, sig) {
                                if ord != sc.kind().ordinal() {
                                    *sc = StrategySnapshot::new(
                                        sc.strategy_id,
                                        sc.strategy_ver,
                                        sc.last_date,
                                        sc.checked,
                                        StrategyKind::from_ordinal(ord),
                                        sc.path.clone(),
                                        sc.fields.clone(),
                                    );
                                }
                            }
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
                // id новых назначаются внутри rebuild_sync (max+1) — метим «любую» правку.
                local_strat_edits.mark_all();
                // К полному набору добавляем новые снапшоты. id = max+1 ЦЕЛЕВОГО ядра
                // (безопасно для межъядерной вставки). Поля — из строк по типу схемы
                // (как fv_from_str при правках), existing=None.
                rebuild_sync(client, server.id, "create", |full, schema, now| {
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
            Ok(CoreCmd::RestoreStrategy {
                id,
                kind_ordinal,
                folder_path,
                fields,
            }) => {
                local_strat_edits.mark(id);
                rebuild_sync(client, server.id, "restore", |full, schema, now| {
                    // Уже живая (двойной клик по меню/эхо) — не дублируем.
                    if full.iter().any(|s| s.strategy_id == id) {
                        return 0;
                    }
                    let mut f = StrategyFields::new();
                    for (name, val) in &fields {
                        let stype = schema.and_then(|s| s.field(name)).map(|x| x.type_id);
                        f.insert(name.as_str(), fv_from_str(None, stype, val));
                    }
                    full.push(StrategySnapshot::new(
                        id,
                        0,
                        now,
                        false, // восстановленная всегда ВЫКЛЮЧЕНА — включать осознанно
                        StrategyKind::from_ordinal(kind_ordinal),
                        folder_path.clone(),
                        f,
                    ));
                    1
                });
            }
            Ok(CoreCmd::MoveStrategies { moves }) => {
                // Смена `path` у указанных стратегий + bump last_date → один sync.
                rebuild_sync(client, server.id, "move", |full, _schema, now| {
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
                // Заодно свежий баланс-снимок: ручной «пинок» против фантомов «Активов»
                // (зависшая проданная монета) — клик по ядру в окне и есть просьба
                // пользователя перечитать остатки. Дёшево (запрос к ядру, не к бирже).
                if let Err(error) = client.balances().refresh() {
                    log::warn!("core {} balance refresh request failed: {error}", server.id);
                } else {
                    log::info!(
                        "core {} balance refresh requested (assets click)",
                        server.id
                    );
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
            Ok(CoreCmd::ChartAlertUpsert {
                market,
                obj_uid,
                blob,
            }) => {
                if let Err(error) = client.chart_alerts().upsert(market.clone(), obj_uid, blob) {
                    log::warn!(
                        "core {} chart alert upsert {market} uid={obj_uid} failed: {error}",
                        server.id
                    );
                } else {
                    log::info!(
                        "core {} chart alert upsert {market} uid={obj_uid}",
                        server.id
                    );
                }
            }
            Ok(CoreCmd::ChartAlertDelete { market, obj_uid }) => {
                if let Err(error) = client.chart_alerts().delete(market.clone(), obj_uid) {
                    log::warn!(
                        "core {} chart alert delete {market} uid={obj_uid} failed: {error}",
                        server.id
                    );
                } else {
                    log::info!(
                        "core {} chart alert delete {market} uid={obj_uid}",
                        server.id
                    );
                }
            }
            // Ордер-мутирующие команды: рантайм moonproto применяет их к ЛОКАЛЬНОЙ модели
            // ордеров сразу (до отправки пакета) → помечаем `orders_mutated`, чтобы цикл
            // фида НЕМЕДЛЕННО отдал свежие строки (оптимистичная отрисовка линий/таблицы),
            // не дожидаясь эха ядра и 250мс-троттла.
            Ok(CoreCmd::PlaceOrder {
                market,
                short,
                price,
                size,
                strategy_id,
            }) => {
                // Селл/стоп новому ордеру ставит САМО ЯДРО из ClientSettings (ROE). Терминал не
                // переставляет — показываем то, что прислало ядро.
                trade::place_order(client, server.id, market, short, price, size, strategy_id);
                *orders_mutated = true;
            }
            Ok(CoreCmd::MoveOrder { uid, new_price }) => {
                trade::move_order(client, server.id, uid, new_price);
                *orders_mutated = true;
            }
            Ok(CoreCmd::CancelOrder { uid }) => {
                trade::cancel_order(client, server.id, uid);
                *orders_mutated = true;
            }
            Ok(CoreCmd::SetOrderStop { uid, kind, on }) => {
                trade::set_order_stop(client, server.id, uid, kind, on);
                *orders_mutated = true;
            }
            Ok(CoreCmd::MoveOrderStopPrice { uid, kind, price }) => {
                trade::move_order_stop_price(client, server.id, uid, kind, price);
                *orders_mutated = true;
            }
            Ok(CoreCmd::UpdateOrderStopsForm { uid, form }) => {
                order_edit::update_order_stops_form(client, server.id, uid, form);
                *orders_mutated = true;
            }
            Ok(CoreCmd::PanicSellMarket { market, on }) => {
                trade::panic_sell_market(client, server.id, market, on);
                *orders_mutated = true;
            }
            Ok(CoreCmd::TurnOrderPanicSell { uid, on }) => {
                trade::turn_order_panic_sell(client, server.id, uid, on);
                *orders_mutated = true;
            }
            Ok(CoreCmd::MarketSellPosition { market }) => {
                trade::market_sell_position(client, server.id, market);
            }
            Ok(CoreCmd::MarketSellToken { market, size }) => {
                trade::market_sell_token(client, server.id, market, size);
            }
            Ok(CoreCmd::CancelMarketBuys { market }) => {
                trade::cancel_market_buys(client, server.id, &market);
            }
            Ok(CoreCmd::JoinSells { market, short }) => {
                trade::join_sells(client, server.id, market, short);
            }
            Ok(CoreCmd::SplitOrder { market, parts }) => {
                trade::split_order(client, server.id, market, parts);
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
                            log::warn!("core {} send client settings failed: {error}", server.id);
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
            Ok(CoreCmd::SetLeverage { market, leverage }) => {
                // РЕАЛЬНОЕ действие на бирже (Engine API). Тикет игнорируем — новое плечо
                // придёт balance-пушем рынка (leverage_x) и обновит карту плеч в ассетах.
                match client.account().set_leverage(&market, leverage) {
                    Ok(_ticket) => {
                        log::info!("core {} set leverage {market} -> {leverage}x", server.id)
                    }
                    Err(error) => log::warn!(
                        "core {} set leverage {market} -> {leverage}x failed: {error}",
                        server.id
                    ),
                }
            }
            Ok(CoreCmd::RestartNow) => {
                // Старт/рестарт рантайма; итог придёт событием RuntimeStateUpdated → стор.
                if let Err(error) = client.settings().restart_now() {
                    log::warn!("core {} restart_now failed: {error}", server.id);
                } else {
                    log::info!("core {} restart_now sent", server.id);
                }
            }
            Ok(CoreCmd::ResetProfit(kind)) => {
                let proto_kind = match kind {
                    crate::feed::ResetProfitKind::Session => {
                        moonproto::ResetProfitKind::CurrentProfit
                    }
                    crate::feed::ResetProfitKind::All => moonproto::ResetProfitKind::AllProfit,
                };
                if let Err(error) = client.settings().reset_profit(proto_kind) {
                    log::warn!("core {} reset_profit({kind:?}) failed: {error}", server.id);
                } else {
                    log::info!("core {} reset_profit({kind:?}) sent", server.id);
                }
            }
            Ok(CoreCmd::CancelAllOrders) => {
                // РЕАЛЬНОЕ действие на бирже. Тикет игнорируем — итог придёт снимком ордеров.
                match client.account().cancel_all_orders() {
                    Ok(_ticket) => log::info!("core {} cancel_all_orders sent", server.id),
                    Err(error) => {
                        log::warn!("core {} cancel_all_orders failed: {error}", server.id)
                    }
                }
            }
            Ok(CoreCmd::SetBlacklist { on, text }) => {
                // Патчим удержанный снимок настроек (флаг + текст ЧС) и шлём целиком.
                match client
                    .snapshot()
                    .and_then(|s| s.settings().client_settings.clone())
                {
                    Some(mut settings) => {
                        settings.use_coins_black_list = on;
                        settings.coins_black_list_text = text;
                        if let Err(error) = client.settings().send(settings) {
                            log::warn!("core {} set blacklist failed: {error}", server.id);
                        }
                    }
                    None => log::warn!("core {} set blacklist ignored: no snapshot yet", server.id),
                }
            }
            Ok(CoreCmd::SetExcludeBlacklistedDelta(on)) => {
                if let Err(error) = client
                    .settings()
                    .set_exclude_blacklisted_markets_from_exchange_delta(on)
                {
                    log::warn!(
                        "core {} set exclude blacklisted delta failed: {error}",
                        server.id
                    );
                }
            }
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                let _ = client.disconnect();
                return true;
            }
        }
    }
}
