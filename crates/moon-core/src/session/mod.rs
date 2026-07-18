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

mod commands;
mod lifecycle;

pub use store::{BalanceState, CoreId, CoreStore};

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use moonproto::state::OrderBookKind;

use crate::config::ServerConfig;
use crate::feed::{ConnStatus, ExchangeId, FeedHandle, FeedWakeTx};
use crate::market::{MarketDataMode, MarketDataSource, SharedMarketStore};

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
    /// Retained market data changed. Visible charts pull this from `gpu_canvas.frame()`;
    /// this flag must not trigger account/order overlay sync.
    pub market_data: bool,
    /// Account/order overlays changed. These still live in `CoreStore` and need a narrow
    /// sync into visible chart userdata.
    pub order_lines_data: bool,
    /// Slow GPUI chrome/account state changed and the Backend entity should be notified.
    pub ui_state: bool,
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
