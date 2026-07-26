//! Retains and reconciles one core's complete market-data assignment across connections.

use moonproto::{MoonClient, TradesStreamMode};

/// One complete desired or applied market-data assignment.
///
/// Account-only assignments carry empty market lists so a provider-to-account transition removes
/// every order-book subscription together with the all-trades stream.
#[derive(Clone, Debug, PartialEq, Eq)]
struct MarketPlan {
    provider: bool,
    markets: Vec<String>,
    orderbook_markets: Vec<String>,
}

impl MarketPlan {
    /// Normalizes one coordinator assignment into the effective plan.
    fn new(provider: bool, markets: Vec<String>, orderbook_markets: Vec<String>) -> Self {
        if provider {
            Self {
                provider,
                markets,
                orderbook_markets,
            }
        } else {
            Self {
                provider,
                markets: Vec::new(),
                orderbook_markets: Vec::new(),
            }
        }
    }
}

/// Desired market assignment retained across connection attempts.
///
/// `desired=None` preserves the unassigned startup state, making the first account-only assignment
/// actionable. `applied` is connection-local and resets before every `live::run`, so a failed Init
/// cannot consume the coordinator's only complete assignment.
#[derive(Default)]
pub(in crate::feed) struct MarketRoleState {
    desired: Option<MarketPlan>,
    applied: Option<MarketPlan>,
    operational: bool,
}

impl MarketRoleState {
    /// Resets connection-local application state while retaining the coordinator's desired plan.
    pub(super) fn begin_connection(&mut self) {
        self.applied = None;
        self.operational = false;
    }

    /// Records a complete desired plan and returns whether it changed.
    pub(super) fn update(
        &mut self,
        provider: bool,
        markets: Vec<String>,
        orderbook_markets: Vec<String>,
    ) -> bool {
        let plan = MarketPlan::new(provider, markets, orderbook_markets);
        if self.desired.as_ref() == Some(&plan) {
            return false;
        }
        self.desired = Some(plan);
        true
    }

    /// Marks the current MoonProto domain operational and applies any pending complete plan.
    pub(super) fn set_operational(&mut self, client: &MoonClient, server_id: u64) {
        self.operational = true;
        self.apply_if_needed(client, server_id);
    }

    /// Invalidates connection-local application until MoonProto reports an operational domain.
    pub(super) fn set_non_operational(&mut self) {
        self.operational = false;
        self.applied = None;
    }

    /// Returns whether this core currently owns exchange market data.
    pub(super) fn is_provider(&self) -> bool {
        self.desired.as_ref().is_some_and(|plan| plan.provider)
    }

    /// Returns the markets served by the desired provider plan.
    pub(super) fn wanted(&self) -> &[String] {
        self.desired
            .as_ref()
            .map(|plan| plan.markets.as_slice())
            .unwrap_or_default()
    }

    /// Applies the desired trade and order-book subscriptions once per connection or plan change.
    pub(super) fn apply_if_needed(&mut self, client: &MoonClient, server_id: u64) {
        if !self.needs_apply() {
            return;
        }
        let desired = self
            .desired
            .as_ref()
            .expect("needs_apply requires a desired market plan");
        let applied = self.applied.as_ref();

        if applied.is_none_or(|current| current.provider != desired.provider) {
            apply_market_role(client, server_id, desired.provider);
        }

        let diag_on = std::env::var_os("MOON_MARKET_DIAG").is_some()
            || std::env::var_os("MOON_RENDER_DIAG").is_some();
        let applied_orderbooks = applied
            .map(|current| current.orderbook_markets.as_slice())
            .unwrap_or_default();
        for market in &desired.orderbook_markets {
            if !applied_orderbooks.iter().any(|current| current == market) {
                match client.streams().subscribe_orderbook(market.clone()) {
                    Ok(()) => {
                        if diag_on {
                            log::info!(
                                "[market_diag] core {server_id} subscribe_orderbook({market})"
                            );
                        }
                    }
                    Err(error) => {
                        log::warn!("core {server_id} subscribe_orderbook({market}) failed: {error}")
                    }
                }
            }
        }
        for market in applied_orderbooks {
            if !desired
                .orderbook_markets
                .iter()
                .any(|current| current == market)
            {
                match client.streams().unsubscribe_orderbook(market.clone()) {
                    Ok(()) => {
                        if diag_on {
                            log::info!(
                                "[market_diag] core {server_id} unsubscribe_orderbook({market})"
                            );
                        }
                    }
                    Err(error) => log::warn!(
                        "core {server_id} unsubscribe_orderbook({market}) failed: {error}"
                    ),
                }
            }
        }
        self.applied = Some(desired.clone());
    }

    /// Returns whether an operational connection has unapplied desired state.
    fn needs_apply(&self) -> bool {
        self.operational && self.desired.is_some() && self.desired != self.applied
    }
}

/// Applies one market-provider role to MoonProto.
///
/// Reapplying `account-only` after `Ready` or a hard reconnect is intentional: it makes the remote
/// core stop an unsolicited all-trades stream even when the original role command arrived before
/// MoonProto's domain layer was ready.
fn apply_market_role(client: &MoonClient, server_id: u64, provider: bool) {
    if provider {
        let _ = client
            .streams()
            .subscribe_all_trades(TradesStreamMode::TradesOnly);
        log::info!("core {server_id} -> market provider (all-trades)");
    } else {
        let _ = client.streams().unsubscribe_all_trades();
        log::info!("core {server_id} -> account-only");
    }
}

#[cfg(test)]
mod tests;
