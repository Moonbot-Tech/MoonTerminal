//! Typed, guarded automation bridge over the existing per-core ActiveLib connection.
//!
//! The public API returns protocol values rather than formatted UI strings. Writes enter the
//! normal feed queue and validate their preview again there, immediately before submission.

pub mod ids;
mod read;
pub mod reports;
pub(crate) mod settings;
mod strategies;

use std::collections::BTreeMap;
use std::sync::{Arc, mpsc};
use std::time::Instant;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::feed::{ConnStatus, CoreCmd};
use crate::session::{CoreId, SessionManager};
use crate::station_api::mcp::Scope;

pub use moonproto::OrderActionTicket;

/// Explicit semantic actions. No shell, arbitrary command ID, or raw MoonProto packet is accepted.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    StrategyEdit {
        #[serde(with = "ids")]
        #[schemars(with = "String")]
        id: u64,
        fields: BTreeMap<String, Value>,
        checked: Option<bool>,
        path: Option<String>,
        #[serde(default)]
        apply_to_orders: bool,
    },
    StrategyCreate {
        strategy_kind: u8,
        name: String,
        path: String,
        fields: BTreeMap<String, Value>,
    },
    StrategyDelete {
        #[serde(with = "ids")]
        #[schemars(with = "String")]
        id: u64,
    },
    Trading {
        running: bool,
    },
    Autodetect {
        enabled: bool,
    },
    SettingsPatch {
        fields: BTreeMap<String, Value>,
    },
    /// Amount is denominated in the core account balance currency; it is not base-coin quantity or margin.
    OrderPlace {
        market: String,
        short: bool,
        price: f64,
        balance_amount: f64,
        #[serde(default, with = "ids::optional")]
        #[schemars(with = "Option<String>")]
        strategy_id: Option<u64>,
        use_market_stop: bool,
        planned_sell_price: Option<f64>,
    },
    OrderCancel {
        #[serde(with = "ids")]
        #[schemars(with = "String")]
        uid: u64,
    },
    OrderMove {
        #[serde(with = "ids")]
        #[schemars(with = "String")]
        uid: u64,
        price: f64,
    },
    PositionClose {
        market: String,
        /// Cancel current market orders and use aggressive closing limits, not guaranteed market fills.
        aggressive_limit: bool,
    },
    PanicSell {
        #[serde(with = "ids")]
        #[schemars(with = "String")]
        uid: u64,
        enabled: bool,
    },
    Leverage {
        market: String,
        leverage: i32,
    },
    HedgeMode {
        enabled: bool,
    },
    /// Start/restart the market runtime, leave passive mode and start checked strategies; may trade.
    RuntimeRestart,
}

impl Action {
    /// The same typed action drives both validation and the MCP input schema.
    pub fn schema() -> Value {
        serde_json::to_value(schemars::schema_for!(Action)).expect("JSON schema is serializable")
    }
    /// Required capability is derived from the typed action, never from an AI-supplied scope.
    pub fn scope(&self) -> Scope {
        match self {
            Self::StrategyEdit { .. }
            | Self::StrategyCreate { .. }
            | Self::StrategyDelete { .. } => Scope::Strategies,
            Self::Autodetect { .. } | Self::SettingsPatch { .. } => Scope::Settings,
            _ => Scope::Trading,
        }
    }

    /// Name used in bounded audit records, excluding parameters and financial information.
    pub fn name(&self) -> &'static str {
        match self {
            Self::StrategyEdit { .. } => "strategy_edit",
            Self::StrategyCreate { .. } => "strategy_create",
            Self::StrategyDelete { .. } => "strategy_delete",
            Self::Trading { .. } => "trading",
            Self::Autodetect { .. } => "autodetect",
            Self::OrderPlace { .. } => "order_place",
            Self::OrderCancel { .. } => "order_cancel",
            Self::OrderMove { .. } => "order_move",
            Self::PositionClose { .. } => "position_close",
            Self::PanicSell { .. } => "panic_sell",
            Self::Leverage { .. } => "leverage",
            Self::HedgeMode { .. } => "hedge_mode",
            Self::RuntimeRestart => "runtime_restart",
            Self::SettingsPatch { .. } => "settings_patch",
        }
    }
}

/// A revocation check supplied by the authenticated transport, evaluated on the feed thread.
#[derive(Clone)]
pub struct Authorization(pub Arc<dyn Fn() -> bool + Send + Sync>);

impl std::fmt::Debug for Authorization {
    /// Debug logs must never include captured credentials.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Authorization")
    }
}

/// One authorized preview plus its reply channel; cloneable because CoreCmd is cloneable.
#[derive(Clone, Debug)]
pub struct Submission {
    pub action: Action,
    pub expected: Value,
    pub expires: Instant,
    pub authorization: Authorization,
    pub reply: mpsc::SyncSender<std::result::Result<Receipt, String>>,
}

/// A feed receipt can carry a nonblocking runtime admission ticket without waiting on the feed.
#[derive(Debug)]
pub struct Receipt {
    pub value: Value,
    pub admission: Option<moonproto::OrderActionTicket>,
}

impl From<Value> for Receipt {
    /// Most semantic actions report queue admission immediately and have no runtime ticket.
    fn from(value: Value) -> Self {
        Self {
            value,
            admission: None,
        }
    }
}

impl Submission {
    /// Check time and current grant just before touching the live client's command queue.
    pub(crate) fn admitted(&self) -> Result<()> {
        ensure!(Instant::now() < self.expires, "preview expired");
        ensure!((self.authorization.0)(), "authorization expired or revoked");
        Ok(())
    }
}

impl SessionManager {
    /// Exact schema and dynamic picklists of this core, not its market-data provider.
    pub fn automation_schema(
        &self,
        core: CoreId,
        kind: Option<u8>,
        field: Option<&str>,
        offset: usize,
    ) -> Result<Value> {
        let client = self.automation_client(core)?;
        strategies::schema(&client, kind, field, offset)
    }

    /// Core-authoritative strategy values and any outstanding local edit are separately labelled.
    pub fn automation_strategies(
        &self,
        core: CoreId,
        id: Option<u64>,
        offset: usize,
    ) -> Result<Value> {
        let client = self.automation_client(core)?;
        strategies::read(&client, id, offset)
    }

    /// Match an immutable resolution to the submitted revision, never to the strategy ID alone.
    pub fn automation_strategy_resolution(&self, core: CoreId, sent: &Value) -> Option<Value> {
        let data = self.store().core(core)?;
        let id = ids::parse(&sent["id"]).ok()?;
        let version = i32::try_from(sent["version"].as_i64()?).ok()?;
        let date = sent["last_date"].as_u64()?;
        let note = data
            .strategy_edit_notes_since(0)
            .find(|note| note.id == id && note.sent_revision == (version, date))?;
        let status = match note.result {
            crate::feed::StrategyEditResult::Confirmed => "confirmed",
            crate::feed::StrategyEditResult::Adjusted => "adjusted",
            crate::feed::StrategyEditResult::Superseded => "superseded",
        };
        Some(
            json!({"status":status,"execution_confirmed":status=="confirmed","strategy_id":id.to_string(),
            "sent":sent,"saved":strategies::row(&note.actual),"observed_at_ms":note.at_ms}),
        )
    }

    /// Prepare from one live snapshot; `expected` is opaque to clients and retained by the caller.
    pub fn automation_preview(&self, core: CoreId, action: &Action) -> Result<Value> {
        let client = self.automation_client(core)?;
        preview(&client, action)
    }

    /// Return a receipt channel without blocking the SessionManager owner on the feed.
    pub fn automation_submit(&self, core: CoreId, submission: Submission) -> Result<()> {
        self.automation_client(core)?;
        self.send_core_cmd(
            core,
            CoreCmd::Automation(Box::new(submission)),
            "automation",
        )
    }

    /// Toggle this client's desired log delivery; other terminal connections are unaffected.
    pub fn automation_logs(&self, core: CoreId, enabled: bool) -> Result<()> {
        let client = self
            .market_source()
            .core_client(core)
            .context("core client unavailable")?;
        client
            .settings()
            .set_logs_subscription(enabled)
            .context("queue log subscription")
    }

    /// Never preview or admit a mutation against retained data from a disconnected core.
    fn automation_client(&self, core: CoreId) -> Result<Arc<moonproto::MoonClient>> {
        ensure!(
            self.store()
                .core(core)
                .is_some_and(|c| c.status == ConnStatus::Ready),
            "core is not connected"
        );
        self.market_source()
            .core_client(core)
            .context("core client unavailable")
    }
}

/// Validate action arguments and return the before/after facts that must still hold at apply time.
pub(crate) fn preview(client: &moonproto::MoonClient, action: &Action) -> Result<Value> {
    match action {
        Action::SettingsPatch { fields } => {
            settings::preview(&client.settings().build_shared_config()?, fields)
        }
        Action::StrategyEdit { .. }
        | Action::StrategyCreate { .. }
        | Action::StrategyDelete { .. } => strategies::preview(client, action),
        Action::OrderPlace {
            market,
            price,
            balance_amount,
            strategy_id,
            planned_sell_price,
            ..
        } => {
            ensure!(!market.is_empty() && market.len() <= 64, "invalid market");
            ensure!(
                price.is_finite()
                    && *price > 0.0
                    && balance_amount.is_finite()
                    && *balance_amount > 0.0,
                "positive finite price and balance amount required"
            );
            ensure!(
                planned_sell_price.is_none_or(|p| p.is_finite() && p > 0.0),
                "invalid planned sell price"
            );
            let snapshot = client.snapshot().context("snapshot unavailable")?;
            let info = snapshot.server_info();
            let core_emulator = snapshot
                .settings()
                .client_settings
                .as_ref()
                .context(
                    "core trading settings unavailable; cannot determine real/emulated execution",
                )?
                .emu_mode;
            let exchange = info.exchange_code.context("exchange unavailable")?;
            let inverse = exchange == moonproto::ExchangeCode::QBinance;
            let handle = snapshot
                .markets()
                .get(market)
                .context("market not found; use its exact core name")?;
            ensure!(
                snapshot.markets().indexes_synchronized(),
                "market catalog is not synchronized"
            );
            let limits = market_limits(&handle);
            ensure!(
                limits["trading"].as_bool() == Some(true),
                "market is not open for trading"
            );
            let currency = if inverse {
                limits["market_currency"]
                    .as_str()
                    .context("settlement currency unavailable")?
            } else {
                info.base_currency_name
                    .as_deref()
                    .context("balance currency unavailable")?
            };
            ensure!(!currency.is_empty(), "balance currency unavailable");
            let strategy = strategy_id
                .map(|id| {
                    ensure!(
                        snapshot.strategy_edit(id).is_none(),
                        "selected strategy has an unresolved edit"
                    );
                    snapshot
                        .strategy_snapshot(id)
                        .context("selected strategy does not exist")
                })
                .transpose()?;
            let configured_size =
                strategy
                    .and_then(|s| s.fields.get("OrderSize"))
                    .and_then(|value| match value {
                        moonproto::FieldValue::Double(value) => Some(*value),
                        moonproto::FieldValue::Single(value) => Some(f64::from(*value)),
                        _ => None,
                    });
            let strategy_emulator = strategy
                .map(|strategy| match strategy.fields.get("EmulatorMode") {
                    Some(moonproto::FieldValue::Bool(enabled)) => Ok(*enabled),
                    _ => anyhow::bail!("selected strategy emulator mode unavailable"),
                })
                .transpose()?;
            let overridden = strategy.is_some_and(|s| s.kind() != moonproto::StrategyKind::MANUAL)
                && configured_size.is_some_and(|value| value > 0.0);
            let effective = if overridden {
                configured_size.context("strategy OrderSize unavailable")?
            } else {
                *balance_amount
            };
            ensure!(
                effective.is_finite() && effective > 0.0,
                "invalid effective order size"
            );
            let quantity = if inverse {
                let contract_size = limits["contract_size"]
                    .as_f64()
                    .context("contract size unavailable")?;
                ensure!(
                    contract_size.is_finite() && contract_size > 0.0,
                    "contract size unavailable"
                );
                effective * price / contract_size
            } else {
                effective / price
            };
            ensure!(
                quantity.is_finite() && quantity > 0.0,
                "order quantity cannot be represented"
            );
            check_price(*price, &limits)?;
            Ok(
                json!({"action":action,"market_limits":limits,"balance_currency":currency,"inverse":inverse,
                "initial_order_balance_amount":effective,"size_source":if overridden { "strategy.OrderSize" } else { "command.balance_amount" },
                "core_emulator":core_emulator,"strategy_emulator":strategy_emulator,
                "strategy":strategy.map(strategies::row),"initial_order_quantity_estimate":quantity,
                "total_balance_amount":null,
                "multiplicity":"A selected strategy can create multiple orders and scale their size (OrdersCount, OrderSizeStep and grid settings). This estimate covers the initial order, not total exposure; read the strategy and MoonDoc before applying.",
                "quantity_unit":if inverse { "contracts" } else { "base_asset" },
                "size_semantics":"Linear futures amount is not margin and is not multiplied by leverage. The core applies exchange-specific rounding, minimums, available balance and position limits.",
                "delivery":"submission is not exchange execution","manual_strategy":"absent strategy_id requests no strategy; older cores may attach Manual and change size/emulator behavior. Use an explicit strategy_id when exact strategy behavior is required."}),
            )
        }
        Action::OrderCancel { uid }
        | Action::OrderMove { uid, .. }
        | Action::PanicSell { uid, .. } => {
            let snapshot = client.snapshot().context("snapshot unavailable")?;
            let order = snapshot
                .orders()
                .get(*uid)
                .context("order does not exist")?;
            ensure!(
                !order.job_is_done
                    && !order.pending_cancel
                    && !order.bulk_replace_buy
                    && !order.bulk_replace_sell,
                "order is finished or changing; read its next state first"
            );
            let limits = if let Action::OrderMove { price, .. } = action {
                let market = snapshot
                    .markets()
                    .get(&order.market_name)
                    .context("order market unavailable")?;
                let limits = market_limits(&market);
                check_price(*price, &limits)?;
                limits
            } else {
                Value::Null
            };
            if let Action::PanicSell { enabled, .. } = action {
                ensure!(
                    order.status == moonproto::OrderWorkerStatus::SellSet,
                    "Panic Sell requires a SELL phase order"
                );
                ensure!(
                    order.panic_sell != *enabled,
                    "Panic Sell already has the requested value; no command needed"
                );
            }
            Ok(
                json!({"action":action,"before":read::order(order),"market_limits":limits,
                "concurrency":"The current order phase is rechecked before queueing. MoonProto does not provide a server-side compare-and-swap.",
                "effect":match action {
                    Action::PanicSell { .. } => "Set this order's Panic Sell flag; preserve other orders and stop settings.",
                    Action::OrderMove { .. } => "Move the observed pending trigger or current exchange leg to the requested price.",
                    _ => "Cancel the observed pending order or current exchange leg; this is not position liquidation."
                }}),
            )
        }
        Action::PositionClose { market, .. } | Action::Leverage { market, .. } => {
            let snapshot = client.snapshot().context("snapshot unavailable")?;
            let handle = snapshot
                .markets()
                .get(market)
                .context("market does not exist")?;
            let position = handle.balance_position();
            let closing_emulator = if matches!(action, Action::PositionClose { .. }) {
                Some(snapshot.settings().client_settings.as_ref()
                    .context("core trading settings unavailable; cannot determine real/emulated execution")?.emu_mode)
            } else {
                None
            };
            if let Action::Leverage { leverage, .. } = action {
                ensure!(
                    *leverage > 0,
                    "leverage must be positive; the exchange enforces its market limit"
                );
            }
            let mut orders: Vec<_> = snapshot
                .orders()
                .iter()
                .filter(|order| order.market_name == *market && !order.job_is_done)
                .collect();
            orders.sort_by_key(|order| order.uid);
            Ok(
                json!({"action":action,"before":{"position":read::position(&position),"core_emulator":closing_emulator,
                "leverage":position.leverage_x,"hedge_mode":snapshot.account().hedge_mode(),"orders":orders.into_iter().map(read::order).collect::<Vec<_>>()},
                "effect":if matches!(action, Action::PositionClose { .. }) {
                    "Cancel existing market orders and create closing LIMIT orders for both hedge legs. aggressive_limit requests crossing prices (currently about 4%); ordinary mode uses about 2%. Neither promises an immediate market fill."
                } else { "Request the market leverage setting; the exchange validates and applies its limits." },
                "concurrency":"Positions and orders may change after dispatch. Inspect the resulting exchange state; never blindly retry."}),
            )
        }
        Action::Trading { .. }
        | Action::Autodetect { .. }
        | Action::RuntimeRestart
        | Action::HedgeMode { .. } => {
            let snapshot = client.snapshot().context("snapshot unavailable")?;
            Ok(
                json!({"action":action,"before":{"strategies_running":snapshot.strats().strategies_running(),
                "runtime_started":snapshot.settings().runtime_state.map(|state| state.is_started),
                "autodetect":snapshot.settings().runtime_state.map(|state| state.auto_detect_active),"hedge_mode":snapshot.account().hedge_mode()},
                "effect":match action {
                    Action::RuntimeRestart => "Start/restart the market runtime, leave passive mode and start checked strategies. This can start trading.",
                    Action::Trading { .. } => "Change the global strategy engine state; this is not position liquidation.",
                    Action::Autodetect { .. } => "Change the market autodetection flag.",
                    _ => "Change the account hedge-mode setting; exchange constraints still apply."
                }}),
            )
        }
    }
}

/// Stable market constraints belong in a preview; changing quotes do not invalidate its consent.
fn market_limits(handle: &moonproto::state::MarketHandle) -> Value {
    handle.with(|market| json!({"tick_size":market.tick_size(),"quantity_step":market.step_size(),"min_quantity":market.min_qty(),
        "max_quantity":market.max_qty(),"min_notional":market.min_notional(),"max_notional":market.max_notional(),
        "min_price":market.min_price(),"max_price":market.max_price(),"contract_size":market.contract_size(),
        "market_currency":market.market_currency_canonic,"trading":market.status_trading}))
}

/// Reject a price outside advertised positive bounds without inventing exchange-specific rounding.
fn check_price(price: f64, limits: &Value) -> Result<()> {
    ensure!(
        price.is_finite() && price > 0.0,
        "positive finite price required"
    );
    if let Some(min) = limits["min_price"].as_f64().filter(|min| *min > 0.0) {
        ensure!(price >= min, "price below market minimum");
    }
    if let Some(max) = limits["max_price"].as_f64().filter(|max| *max > 0.0) {
        ensure!(price <= max, "price above market maximum");
    }
    Ok(())
}

/// Submit non-strategy actions through typed ActiveLib methods; success is only queue admission.
pub(crate) fn execute(
    client: &moonproto::MoonClient,
    action: &Action,
    expected: &Value,
) -> Result<Receipt> {
    use moonproto::{ClosePositionParams, NewOrderParams, OrderSide};
    match action {
        Action::Trading { running } => {
            if *running {
                client.strategies().start()?;
            } else {
                client.strategies().stop()?;
            }
        }
        Action::Autodetect { enabled } => client.settings().set_auto_detect_active(*enabled)?,
        Action::OrderPlace {
            market,
            short,
            price,
            balance_amount,
            strategy_id,
            use_market_stop,
            planned_sell_price,
        } => {
            let side = if *short {
                OrderSide::Short
            } else {
                OrderSide::Long
            };
            let mut params = NewOrderParams::new(market.clone(), side, *price, *balance_amount)
                .with_market_stop(*use_market_stop);
            if let Some(id) = strategy_id {
                params = params.with_strategy_id(*id);
            }
            if let Some(price) = planned_sell_price {
                params = params.with_planned_sell_price(*price);
            }
            let ticket = client.trade().new_order(params)?;
            return Ok(json!({"status":"submitted","client_order_id":ticket.client_order_id.to_string(),"execution_confirmed":false,
                "correlation":"The core does not echo this client ID in its order stream. Do not infer a server order UID or retry automatically."}).into());
        }
        Action::OrderCancel { uid }
        | Action::OrderMove { uid, .. }
        | Action::PanicSell { uid, .. } => {
            let snapshot = client.snapshot().context("order snapshot unavailable")?;
            let order = snapshot
                .orders()
                .get(*uid)
                .context("order no longer exists")?;
            ensure!(
                read::order(order) == expected["before"],
                "order changed since preview"
            );
            let guard = order.guard();
            let ticket = match action {
                Action::OrderMove { price, .. } => client.orders().move_guarded(guard, *price)?,
                Action::PanicSell { enabled, .. } => {
                    client.orders().panic_guarded(guard, *enabled)?
                }
                _ => client.orders().cancel_guarded(guard)?,
            };
            return Ok(Receipt {
                value: json!({"status":"queued_in_runtime","execution_confirmed":false}),
                admission: Some(ticket),
            });
        }
        Action::PositionClose {
            market,
            aggressive_limit,
        } => {
            let params = if *aggressive_limit {
                ClosePositionParams::market_order(market.clone())
            } else {
                ClosePositionParams::limit_orders(market.clone())
            };
            client.trade().close_position(params)?;
        }
        Action::Leverage { market, leverage } => {
            client.account().set_leverage(market, *leverage)?;
        }
        Action::HedgeMode { enabled } => {
            client.account().set_hedge_mode(*enabled)?;
        }
        Action::RuntimeRestart => client.settings().restart_now()?,
        _ => anyhow::bail!("strategy action requires the strategy synchronization owner"),
    }
    Ok(json!({"status":"submitted","execution_confirmed":false}).into())
}

pub(crate) use strategies::apply as apply_strategy;
