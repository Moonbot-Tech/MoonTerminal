//! Market-plan coordinator: in `Dedup` mode it elects one provider core per exchange and handles
//! failover, while in `PerCore` mode every core is its own provider. It computes served markets
//! from the union of open charts plus linger and distributes market roles to cores. This logic
//! extends the existing per-core feed and lives as `SessionManager` methods so the child module
//! can access the manager's fields.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use super::{CoreId, SessionManager};
use crate::feed::{ConnStatus, CoreCmd, ExchangeId};
use crate::market::MarketDataMode;

/// Keep serving a market for this long after its last chart closes so a quick reopen does not
/// interrupt subscriptions or reads and reload history from scratch.
const UNSUB_DELAY: Duration = Duration::from_secs(5);

/// Whether the market channel is on (`channels.markets`, or `MOON_MARKET_DIAG`/`MOON_RENDER_DIAG`).
fn market_diag_enabled() -> bool {
    crate::diagnostics::markets()
}

fn market_diag(msg: impl std::fmt::Display) {
    if market_diag_enabled() {
        log::info!("[market_diag] {msg}");
    }
}

impl SessionManager {
    /// Reconcile the markets currently held open as `(core, market)` pairs.
    /// This runs when desired market or order-book state is dirty and as an infrequent wall-clock
    /// fallback, not on every present or render frame. It re-elects providers, computes served
    /// markets per provider, and sends each core a market role only when that role changes.
    pub fn set_open(
        &mut self,
        desired: &[(CoreId, String)],
        desired_orderbook: &[(CoreId, String)],
    ) {
        let now = Instant::now();
        self.reconcile_providers();
        self.sweep_coin_naming();

        // 1. A provider's desired markets are the union of open charts for cores assigned to that
        //    provider. The input is a list of pairs because one core may have several open markets
        //    across multiple panels; aggregate them into one set per provider.
        let mut desired_pm: HashMap<CoreId, HashSet<String>> = HashMap::new();
        for (core, market) in desired {
            if let Some(&p) = self.core_provider.get(core) {
                desired_pm.entry(p).or_default().insert(market.clone());
            }
        }
        // Aggregate markets requiring an order book per provider across all windows. Retain
        // order-book subscriptions only for this set, with no linger, so demand changes take effect
        // immediately.
        let mut orderbook_pm: HashMap<CoreId, HashSet<String>> = HashMap::new();
        for (core, market) in desired_orderbook {
            if let Some(&p) = self.core_provider.get(core) {
                orderbook_pm.entry(p).or_default().insert(market.clone());
            }
        }

        // 2a. Add newly desired markets to `wanted` and reset their views so the provider rereads
        //     retained history from the beginning. Cancel any pending drop.
        for (p, mkts) in &desired_pm {
            let w = self.wanted.entry(*p).or_default();
            for m in mkts {
                self.pending_drop.remove(&(*p, m.clone()));
                if w.insert(m.clone()) {
                    market_diag(format!("set_open reset provider={p} market={m}"));
                    self.market_source.reset_market(*p, m);
                }
            }
        }

        // 2b. Schedule a delayed drop for markets in `wanted` that no consumer still wants.
        let mut to_schedule: Vec<(CoreId, String)> = Vec::new();
        for (p, w) in &self.wanted {
            for m in w {
                let still = desired_pm.get(p).is_some_and(|s| s.contains(m));
                if !still {
                    to_schedule.push((*p, m.clone()));
                }
            }
        }
        for key in to_schedule {
            self.pending_drop.entry(key).or_insert(now + UNSUB_DELAY);
        }

        // 2c. Remove expired delayed drops from `wanted` and release their views.
        let expired: Vec<(CoreId, String)> = self
            .pending_drop
            .iter()
            .filter(|(_, &t)| now >= t)
            .map(|(k, _)| k.clone())
            .collect();
        for (p, m) in expired {
            self.pending_drop.remove(&(p, m.clone()));
            if let Some(w) = self.wanted.get_mut(&p) {
                w.remove(&m);
            }
            self.market_source.drop_market(p, &m);
        }

        // 3. Distribute roles to cores. A provider named in `core_provider` receives `(true, its
        //    markets)`; all other cores receive `(false, [])`. Send only changed roles.
        let provider_cores: HashSet<CoreId> = self.core_provider.values().copied().collect();
        let mut cmds: Vec<(CoreId, bool, Vec<String>, Vec<String>)> = Vec::new();
        for sess in &self.sessions {
            let id = sess.id;
            let is_prov = provider_cores.contains(&id);
            let mut markets: Vec<String> = if is_prov {
                self.wanted
                    .get(&id)
                    .map(|s| s.iter().cloned().collect())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            markets.sort(); // Stable ordering allows direct comparison with `last_cmd`.
                            // Order books are the subset of markets with current demand; removal is immediate.
            let mut orderbook_markets: Vec<String> = if is_prov {
                let obk = orderbook_pm.get(&id);
                markets
                    .iter()
                    .filter(|m| obk.is_some_and(|s| s.contains(*m)))
                    .cloned()
                    .collect()
            } else {
                Vec::new()
            };
            orderbook_markets.sort();
            if self.last_cmd.get(&id)
                != Some(&(is_prov, markets.clone(), orderbook_markets.clone()))
            {
                self.last_cmd
                    .insert(id, (is_prov, markets.clone(), orderbook_markets.clone()));
                cmds.push((id, is_prov, markets, orderbook_markets));
            }
        }
        for (id, provider, markets, orderbook_markets) in cmds {
            if let Some(s) = self.sessions.iter().find(|s| s.id == id) {
                market_diag(format!(
                    // `provider` here is the is-provider FLAG, not another core's id.
                    "set_open send core={} provider={provider} markets={markets:?} \
                     orderbook={orderbook_markets:?}",
                    crate::feed::core_label(id)
                ));
                let _ = s.handle.cmd_tx.send(CoreCmd::SetMarket {
                    provider,
                    markets,
                    orderbook_markets,
                });
            }
        }
    }

    /// Write each core's spelling of the coins the naming channel follows.
    ///
    /// Here rather than in the market source because the line needs the SERVER's name and its
    /// venue, which the source does not hold — and here rather than on a timer of its own because
    /// this tick already runs whenever the market state moves, and the channel costs one atomic
    /// load while it is off.
    fn sweep_coin_naming(&self) {
        if !crate::coin_naming::enabled() {
            return;
        }
        for sess in &self.sessions {
            // The directory's caption where it knows the platform, the core's own where it does
            // not: an unnamed venue must still identify its row.
            let venue = self
                .core_venue
                .get(&sess.id)
                .map(|venue| match venue.resolved() {
                    Some(v) => v.brand.display().to_string(),
                    None => venue.reported.clone(),
                })
                .unwrap_or_default();
            self.market_source
                .dump_coin_naming(sess.id, &sess.name, &venue);
        }
    }

    /// Rebuild `core_provider` (core to provider core) and `providers` (exchange to provider).
    /// `Dedup` prefers one Ready core per exchange, retaining the current Ready provider when
    /// possible and falling back to the exchange's first available core when none are Ready. In
    /// `PerCore`, every core is its own provider.
    fn reconcile_providers(&mut self) {
        // Snapshot `(id, exchange, Ready?)` so later mutations do not retain borrows of `self`.
        let infos: Vec<(CoreId, Option<ExchangeId>, bool)> = self
            .sessions
            .iter()
            .map(|s| {
                let key = self.core_venue.get(&s.id).map(|venue| venue.id);
                let ready = self
                    .store
                    .core(s.id)
                    .map(|d| matches!(d.status, ConnStatus::Ready))
                    .unwrap_or(false);
                (s.id, key, ready)
            })
            .collect();

        let mut new_core_provider: HashMap<CoreId, CoreId> = HashMap::new();

        match self.mode {
            MarketDataMode::PerCore => {
                for (id, _, _) in &infos {
                    new_core_provider.insert(*id, *id);
                }
                self.providers.clear();
            }
            MarketDataMode::Dedup => {
                // Group cores by exchange, excluding cores whose exchange identity is unknown.
                let mut by_key: HashMap<ExchangeId, Vec<CoreId>> = HashMap::new();
                for (id, key, _) in &infos {
                    if let Some(k) = key {
                        by_key.entry(*k).or_default().push(*id);
                    }
                }
                let ready_of = |id: CoreId| {
                    infos
                        .iter()
                        .find(|(i, _, _)| *i == id)
                        .map(|(_, _, r)| *r)
                        .unwrap_or(false)
                };
                // Keep the current provider when it is present and Ready; otherwise choose the
                // exchange's first Ready core, falling back to its first core of any status.
                let mut elected: HashMap<ExchangeId, CoreId> = HashMap::new();
                for (k, cores) in &by_key {
                    let cur = self.providers.get(k).copied();
                    let keep = cur.filter(|c| cores.contains(c) && ready_of(*c));
                    let chosen = keep
                        .or_else(|| cores.iter().copied().find(|c| ready_of(*c)))
                        .or_else(|| cores.first().copied());
                    if let Some(p) = chosen {
                        elected.insert(*k, p);
                        for &c in cores {
                            new_core_provider.insert(c, p);
                        }
                    }
                }
                // When an exchange changes provider, clear the old provider's data, `wanted`, and
                // `last_cmd`. It will receive a fresh `(false, [])` role and unsubscribe all trades.
                for (k, &p) in &elected {
                    if self.providers.get(k).copied() != Some(p) {
                        if let Some(old) = self.providers.get(k).copied() {
                            self.market_source.drop_provider(old);
                            self.wanted.remove(&old);
                            self.last_cmd.remove(&old);
                            self.pending_drop.retain(|(pp, _), _| *pp != old);
                        }
                    }
                }
                self.providers = elected;
            }
        }

        self.market_source.set_provider_map(&new_core_provider);
        // Stable provider exchange identities key the local kline cache, allowing cores on one
        // exchange to share cached data and preserving that cache across provider elections.
        let provider_exchange: HashMap<CoreId, ExchangeId> = new_core_provider
            .values()
            .filter_map(|p| self.core_venue.get(p).map(|venue| (*p, venue.id)))
            .collect();
        self.market_source
            .set_provider_exchanges(&provider_exchange);
        // Every core's venue, not just the elected providers': the arbitrage column keeps a pane's
        // own exchange out of its own column, and a pane sits on a core whether or not that core
        // serves prices.
        self.market_source.set_core_venues(&self.core_venue);
        self.core_provider = new_core_provider;
    }
}
