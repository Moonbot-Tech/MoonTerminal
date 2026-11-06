//! Automation reads distinguish core intent, exchange facts and incomplete valuations.

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::session::SessionManager;

impl SessionManager {
    /// Return every open position, preserving both hedge legs even when their net size is zero.
    pub fn automation_positions(&self, core: u64, offset: usize) -> Result<Value> {
        let client = self.automation_client(core)?;
        let snapshot = client.snapshot().context("snapshot unavailable")?;
        let positions: Vec<_> = snapshot
            .markets()
            .iter()
            .filter_map(|market| {
                let position = market.balance_position();
                (position.pos_size != 0.0
                    || position.long_pos_size != 0.0
                    || position.short_pos_size != 0.0)
                    .then(|| (market.name().to_owned(), position))
            })
            .collect();
        let rows: Vec<_> = positions
            .iter()
            .skip(offset)
            .take(100)
            .map(|(market, value)| json!({"market":market,"position":position(value)}))
            .collect();
        Ok(
            json!({"hedge_mode":snapshot.account().hedge_mode(),"rows":rows,
            "next_offset":offset+rows.len(),"has_more":positions.len()>offset+rows.len(),
            "read_at_ms":crate::util::now_unix_ms_i64(),"units":"Native per-market position quantities; inverse contracts are not base-asset quantities."}),
        )
    }

    /// Read a market in one handle guard: live market handles are not frozen by snapshot revision.
    pub fn automation_market(&self, core: u64, name: &str) -> Result<Value> {
        let client = self.automation_client(core)?;
        let snapshot = client.snapshot().context("snapshot unavailable")?;
        let market = snapshot
            .markets()
            .get(name)
            .context("market not found; use its exact core name")?;
        Ok(market.with(|m| json!({"market":name,"bid":m.price.bid,"ask":m.price.ask,"midpoint":m.price.p_last,
            "mark_price":m.price.mark_price,"mark_price_known":m.price.mark_price_found,
            "tick_size":m.tick_size(),"quantity_step":m.step_size(),"min_quantity":m.min_qty(),"max_quantity":m.max_qty(),
            "min_notional":m.min_notional(),"max_notional":m.max_notional(),"min_price":m.min_price(),"max_price":m.max_price(),
            "contract_size":m.contract_size(),"funding_rate":m.price.funding_rate,"read_at_ms":crate::util::now_unix_ms_i64()})))
    }

    /// Exchange order values are separate from optimistic target prices in the core's order model.
    pub fn automation_orders(&self, core: u64, offset: usize) -> Result<Value> {
        let client = self.automation_client(core)?;
        let snapshot = client.snapshot().context("snapshot unavailable")?;
        let rows: Vec<_> = snapshot
            .orders()
            .iter()
            .filter(|order| !order.job_is_done)
            .skip(offset)
            .take(100)
            .map(order)
            .collect();
        Ok(
            json!({"orders":rows,"next_offset":offset+rows.len(),"read_at_ms":crate::util::now_unix_ms_i64(),
            "correlation":"Exchange fields and subsequent core snapshots are evidence; target prices can be local optimistic edits."}),
        )
    }
}

/// Share the same factual order representation between reads and mutation preconditions.
pub(super) fn order(order: &moonproto::state::Order) -> Value {
    json!({"uid":order.uid.to_string(),"local_incarnation":order.guard().incarnation().to_string(),"market":order.market_name,"status":order.status.name(),"strategy_id":order.strat_id.to_string(),
        "short":order.is_short,"emulator":order.emulator_mode,"job_is_done":order.job_is_done,
        "target_buy_price":order.buy_price,"target_sell_price":order.sell_price,"buy_size":order.buy_size,
        "pending_trigger_price":order.pending_buy_cond_price,
        "panic_sell":order.panic_sell,"panic_sell_auto":order.panic_sell_auto,
        "pending_cancel":order.pending_cancel,"exchange_buy":exchange_order(&order.buy_order),"exchange_sell":exchange_order(&order.sell_order)})
}

/// Keep the exchange's one-way fields and both hedge legs distinct; never choose a dominant leg.
pub(super) fn position(value: &moonproto::state::MarketBalancePosition) -> Value {
    json!({"size":value.pos_size,"entry":value.pos_price,"direction":format!("{:?}",value.pos_dir),
        "liquidation_price":value.liq_price,"leverage":value.leverage_x,
        "long":{"size":value.long_pos_size,"entry":value.long_pos_price,"liquidation_price":value.long_liq_price},
        "short":{"size":value.short_pos_size,"entry":value.short_pos_price,"liquidation_price":value.short_liq_price}})
}

/// Preserve exchange-confirmed quantities and lifecycle flags without internal protocol ordinals.
fn exchange_order(order: &moonproto::ExchangeOrder) -> Value {
    json!({"exchange_id":order.int_id.to_string(),"actual_price":order.actual_price,"mean_price":order.mean_price,"quantity":order.quantity,
        "quantity_remaining":order.quantity_remaining,"opened":order.is_opened(),"closed":order.is_closed(),"canceled":order.canceled()})
}

#[cfg(test)]
mod tests;
