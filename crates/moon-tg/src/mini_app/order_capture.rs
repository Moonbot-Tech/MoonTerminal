//! Revision-keyed, trace-free order inputs shared by Mini App background readers.

use std::collections::HashMap;
use std::sync::Arc;

use moon_core::feed::OrderRow;
use moon_core::session::CoreStore;

/// One core's immutable table revision; projections and identities are built by the executor.
#[derive(Clone)]
pub(super) struct CoreOrders {
    pub core: u64,
    pub name: String,
    pub exchange: String,
    pub rows: Arc<Vec<OrderRow>>,
}

/// Only the latest capture per configured core is retained; background jobs own older handles.
#[derive(Default)]
pub(crate) struct OrderCaptures {
    epoch: u64,
    cores: HashMap<u64, (u64, Arc<Vec<OrderRow>>)>,
}

impl OrderCaptures {
    /// Copy a changed core once, or clone its existing handle without walking its orders.
    /// The store epoch invalidates counters reused after a core is removed and restored.
    pub(super) fn capture(
        &mut self,
        store: &CoreStore,
        listed: &[(u64, String, String)],
    ) -> Vec<CoreOrders> {
        if self.epoch != store.time_offset_epoch() {
            self.cores.clear();
            self.epoch = store.time_offset_epoch();
        }
        self.cores.retain(|core, _| store.core(*core).is_some());
        listed
            .iter()
            .filter_map(|(core, name, exchange)| {
                let data = store.core(*core)?;
                let cached = self.cores.entry(*core).or_insert_with(|| {
                    (
                        data.orders_table_rev,
                        Arc::new(data.orders.iter().map(capture_row).collect()),
                    )
                });
                if cached.0 != data.orders_table_rev {
                    cached.1 = Arc::new(data.orders.iter().map(capture_row).collect());
                    cached.0 = data.orders_table_rev;
                }
                Some(CoreOrders {
                    core: *core,
                    name: name.clone(),
                    exchange: exchange.clone(),
                    rows: Arc::clone(&cached.1),
                })
            })
            .collect()
    }
}

/// Capture every input consumed by Orders, closing identity and coin overlays, without traces.
fn capture_row(row: &OrderRow) -> OrderRow {
    OrderRow {
        uid: row.uid,
        coin: row.coin.clone(),
        market: row.market.clone(),
        market_display: row.market_display.clone(),
        quote: row.quote.clone(),
        strat_name: row.strat_name.clone(),
        buy_price: row.buy_price,
        price: row.price,
        size: row.size,
        remaining_size: row.remaining_size,
        fill_pct: row.fill_pct,
        filled: row.filled,
        pending: row.pending,
        is_short: row.is_short,
        emulator: row.emulator,
        job_is_done: row.job_is_done,
        entry_fill_time_ms: row.entry_fill_time_ms,
        take_profit: row.take_profit,
        stop_loss: row.stop_loss,
        sl_on: false,
        ts_on: false,
        vstop_on: false,
        sl_fixed: false,
        ts_fixed: false,
        vstop_fixed: false,
        vstop_level: 0.0,
        vstop_vol: 0.0,
        sell_price: 0.0,
        create_time_ms: 0.0,
        sell_create_time_ms: 0.0,
        strat: String::new(),
        strat_id: 0,
        status: String::new(),
        trailing: None,
        vstop: None,
        pending_cond: None,
        liq: None,
        panic_sell: false,
        is_moon_shot: false,
        corridor_price_down: 0.0,
        corridor_price_up: 0.0,
        buy_trace: None,
        sell_trace: None,
    }
}

#[cfg(test)]
mod tests;
