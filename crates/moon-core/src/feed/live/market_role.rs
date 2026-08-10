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
    pending_history: Vec<String>,
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
        let Some(desired) = self.desired.clone() else {
            return;
        };
        let applied = self.applied.clone();
        let needs_apply = self.needs_apply();

        if desired.provider {
            self.pending_history
                .retain(|market| desired.markets.iter().any(|wanted| wanted == market));
        } else {
            self.pending_history.clear();
        }
        if !needs_apply && self.pending_history.is_empty() {
            return;
        }

        let diag_on = std::env::var_os("MOON_MARKET_DIAG").is_some()
            || std::env::var_os("MOON_RENDER_DIAG").is_some();

        let newly_wanted_markets: Vec<String> = desired
            .markets
            .iter()
            .filter(|market| {
                applied.as_ref().is_none_or(|current| {
                    !current.markets.iter().any(|existing| existing == *market)
                })
            })
            .cloned()
            .collect();
        for market in newly_wanted_markets {
            self.queue_pending_history(market);
        }

        if needs_apply {
            let provider_changed = applied
                .as_ref()
                .is_none_or(|current| current.provider != desired.provider);
            let markets_changed = applied
                .as_ref()
                .is_none_or(|current| current.markets.as_slice() != desired.markets.as_slice());
            if provider_changed || (desired.provider && markets_changed) {
                apply_market_role(client, server_id, &desired);
            }

            let applied_orderbooks = applied
                .as_ref()
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

        if desired.provider {
            self.retry_pending_history(client, server_id, diag_on);
        }
    }

    /// Returns whether the current MoonClient has unapplied desired state.
    fn needs_apply(&self) -> bool {
        self.desired.is_some() && self.desired != self.applied
    }

    fn queue_pending_history(&mut self, market: String) {
        if !self
            .pending_history
            .iter()
            .any(|pending| pending == &market)
        {
            self.pending_history.push(market);
        }
    }

    fn retry_pending_history(&mut self, client: &MoonClient, server_id: u64, diag_on: bool) {
        let mut still_pending = Vec::new();
        for market in self.pending_history.drain(..) {
            match client.history().request_chart(market.clone()) {
                Ok(_) => {
                    if diag_on {
                        log::info!(
                            "[market_diag] core {} request_chart_history({market})",
                            crate::feed::core_label(server_id)
                        );
                    }
                }
                Err(error) => {
                    if diag_on {
                        log::info!(
                            "[market_diag] core {} request_chart_history({market}) deferred: {error}",
                            crate::feed::core_label(server_id)
                        );
                    }
                    still_pending.push(market);
                }
            }
        }
        self.pending_history = still_pending;
    }
}

/// Applies one market-provider role to MoonProto.
///
/// MoonProto defers a pre-Ready command and retains the explicit provider or account-only intent
/// across reconnects, so an unchanged role is not resent on lifecycle events.
fn apply_market_role(client: &MoonClient, server_id: u64, desired: &MarketPlan) {
    if desired.provider {
        if desired.markets.is_empty() {
            let _ = client.streams().unsubscribe_all_trades();
            log::info!(
                "core {} -> market provider (trades scope: no open markets)",
                crate::feed::core_label(server_id)
            );
        } else {
            let _ = client
                .streams()
                .subscribe_trades_for(TradesStreamMode::TradesOnly, desired.markets.clone());
            log::info!(
                "core {} -> market provider (trades scope: {} markets)",
                crate::feed::core_label(server_id),
                desired.markets.len()
            );
        }
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
