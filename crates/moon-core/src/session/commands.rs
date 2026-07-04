//! Команды ядрам (стратегии/торговля/Engine-действия) через per-core канал `CoreCmd`
//! + read-only аксессоры менеджера (store/sessions/market_source/…).

use anyhow::{anyhow, Result};

use crate::data::OrderBookModel;
use crate::feed::{
    ClientSettingsEdit, CoreCmd, FeedWakeTx, LevManageEdit, NewStrategySpec, OrderLinePriceKind,
    OrderStopKind, ResetProfitKind, WalletKind,
};
use crate::market::{MarketDataMode, MarketDataSource};

use super::{CoreId, CoreSession, CoreStore, SessionManager};

impl SessionManager {
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

    /// Заармить/обновить chart-алерт (фигура с галкой «Alert») на рынке ядра.
    /// `blob` — сериализованный `TChartObject`; `obj_uid` — стабильный id фигуры.
    pub fn chart_alert_upsert(
        &self,
        core: CoreId,
        market: String,
        obj_uid: u64,
        blob: Vec<u8>,
    ) -> Result<()> {
        if market.is_empty() || obj_uid == 0 || blob.is_empty() {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::ChartAlertUpsert {
                market,
                obj_uid,
                blob,
            },
            "chart alert upsert",
        )
    }

    /// Разоружить/удалить chart-алерт по `obj_uid`.
    pub fn chart_alert_delete(&self, core: CoreId, market: String, obj_uid: u64) -> Result<()> {
        if market.is_empty() || obj_uid == 0 {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::ChartAlertDelete { market, obj_uid },
            "chart alert delete",
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

    /// Закрыть позицию рынка ядра ПО МАРКЕТУ (кнопка «Market sell» у строки с позицией).
    pub fn market_sell_position(&self, core: CoreId, market: String) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::MarketSellPosition { market },
            "market sell position",
        )
    }

    /// Продать спот-токен рынка ядра ПО МАРКЕТУ (кнопка «Market sell» у строки-холдинга).
    /// `size` — количество в базовой монете (обычно полный остаток).
    pub fn market_sell_token(&self, core: CoreId, market: String, size: f64) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::MarketSellToken { market, size },
            "market sell token",
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

    /// «Join all sells» (ПКМ по линии sell): объединить sell-ордера рынка по стороне `short`.
    pub fn join_sells(&self, core: CoreId, market: String, short: bool) -> Result<()> {
        if market.is_empty() {
            return Ok(());
        }
        self.send_core_cmd(core, CoreCmd::JoinSells { market, short }, "join sells")
    }

    /// «Split order» (ПКМ по линии sell): разбить выбранный sell-ордер рынка на `parts` частей.
    pub fn split_order(&self, core: CoreId, market: String, parts: i32) -> Result<()> {
        if market.is_empty() || parts < 2 {
            return Ok(());
        }
        self.send_core_cmd(core, CoreCmd::SplitOrder { market, parts }, "split order")
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

    /// Передвинуть цену стоп/тейк-линии ордера ядра по `uid` (перетаскивание линии на чарте)
    /// на абсолютную `price`. SL/TS ставятся ФИКСИРОВАННЫМ стопом по цене, take-profit —
    /// абсолютной ценой; остальные стопы ордера сохраняются. `price` должен быть положительным.
    pub fn move_order_stop_price(
        &self,
        core: CoreId,
        uid: u64,
        kind: OrderLinePriceKind,
        price: f64,
    ) -> Result<()> {
        if !(price > 0.0) {
            return Ok(());
        }
        self.send_core_cmd(
            core,
            CoreCmd::MoveOrderStopPrice { uid, kind, price },
            "move order stop price",
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

    /// Установить плечо рынка ядра (Engine API). Реальное действие на бирже.
    pub fn set_leverage(&self, core: CoreId, market: String, leverage: i32) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::SetLeverage { market, leverage },
            "set leverage",
        )
    }

    /// Старт/рестарт рантайма ядра (попап настроек ядра).
    pub fn restart_now(&self, core: CoreId) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::RestartNow, "restart now")
    }

    /// Сброс счётчика прибыли ядра (сессия / всё время).
    pub fn reset_profit(&self, core: CoreId, kind: ResetProfitKind) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::ResetProfit(kind), "reset profit")
    }

    /// Отменить все ордера ядра (реальное биржевое действие).
    pub fn cancel_all_orders(&self, core: CoreId) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::CancelAllOrders, "cancel all orders")
    }

    /// Чёрный список монет ядра: вкл/выкл + текст списка.
    pub fn set_blacklist(&self, core: CoreId, on: bool, text: String) -> Result<()> {
        self.send_core_cmd(core, CoreCmd::SetBlacklist { on, text }, "set blacklist")
    }

    /// Исключать монеты ЧС из рыночной дельты (локально в Active Lib ядра).
    pub fn set_exclude_blacklisted_delta(&self, core: CoreId, on: bool) -> Result<()> {
        self.send_core_cmd(
            core,
            CoreCmd::SetExcludeBlacklistedDelta(on),
            "set exclude blacklisted delta",
        )
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
