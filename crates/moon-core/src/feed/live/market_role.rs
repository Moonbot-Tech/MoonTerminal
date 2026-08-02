//! Retains and reconciles one core's complete market-data assignment across MoonClient instances.

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

/// Desired market assignment retained across application-level client replacements.
///
/// `desired=None` preserves the unassigned startup state, making the first account-only assignment
/// actionable. `applied` belongs to the current MoonClient and resets only when `live::run` creates
/// a replacement client. MoonProto retains the applied intent across its own reconnects and runtime
/// restarts.
#[derive(Default)]
pub(in crate::feed) struct MarketRoleState {
    desired: Option<MarketPlan>,
    applied: Option<MarketPlan>,
}

impl MarketRoleState {
    /// Starts a replacement MoonClient while retaining the coordinator's desired plan.
    pub(super) fn begin_client(&mut self) {
        self.applied = None;
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

    /// Applies the desired subscriptions once per MoonClient or plan change.
    ///
    /// MoonProto accepts subscription intent before Ready and owns its restoration across internal
    /// reconnects and runtime restarts.
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
                                "[market_diag] core {} subscribe_orderbook({market})",
                                crate::feed::core_label(server_id)
                            );
                        }
                    }
                    Err(error) => {
                        log::warn!(
                            "core {} subscribe_orderbook({market}) failed: {error}",
                            crate::feed::core_label(server_id)
                        )
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
                                "[market_diag] core {} unsubscribe_orderbook({market})",
                                crate::feed::core_label(server_id)
                            );
                        }
                    }
                    Err(error) => log::warn!(
                        "core {} unsubscribe_orderbook({market}) failed: {error}",
                        crate::feed::core_label(server_id)
                    ),
                }
            }
        }
        self.applied = Some(desired.clone());
    }

    /// Returns whether the current MoonClient has unapplied desired state.
    fn needs_apply(&self) -> bool {
        self.desired.is_some() && self.desired != self.applied
    }
}

/// Applies one market-provider role to MoonProto.
///
/// MoonProto defers a pre-Ready command and retains the explicit provider or account-only intent
/// across reconnects, so an unchanged role is not resent on lifecycle events.
fn apply_market_role(client: &MoonClient, server_id: u64, provider: bool) {
    if provider {
        let _ = client
            .streams()
            .subscribe_all_trades(TradesStreamMode::TradesOnly);
        log::info!(
            "core {} -> market provider (all-trades)",
            crate::feed::core_label(server_id)
        );
    } else {
        let _ = client.streams().unsubscribe_all_trades();
        log::info!(
            "core {} -> account-only",
            crate::feed::core_label(server_id)
        );
    }
}

#[cfg(test)]
mod tests;
