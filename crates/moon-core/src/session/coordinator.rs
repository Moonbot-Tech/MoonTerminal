//! Координатор рыночного плана: выбор ядра-провайдера на биржу, failover, расчёт
//! обслуживаемых рынков (union открытых чартов + linger) и рассылка ядрам рыночной
//! роли. Вынесено из `mod.rs`, потому что это новая логика поверх существующего
//! пер-ядерного потока. Логика живёт как методы `SessionManager` (доступ к её полям
//! из дочернего модуля разрешён).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use super::{CoreId, SessionManager};
use crate::feed::{ConnStatus, CoreCmd, ExchangeId};
use crate::market::MarketDataMode;

/// Рынок держим обслуживаемым ещё этот срок после закрытия последнего чарта — чтобы
/// быстрое переоткрытие не рвало подписку/чтение и не перевыгружало историю заново.
const UNSUB_DELAY: Duration = Duration::from_secs(5);

fn market_diag_enabled() -> bool {
    std::env::var_os("MOON_MARKET_DIAG").is_some() || std::env::var_os("MOON_RENDER_DIAG").is_some()
}

fn market_diag(msg: impl std::fmt::Display) {
    if market_diag_enabled() {
        log::info!("[market_diag] {msg}");
    }
}

impl SessionManager {
    /// Сообщает, какие рынки сейчас удерживаются открытыми (ядро → рынок).
    /// Вызывается по dirty-флагу `desired` и страховочно по редкому wall-clock fallback,
    /// но не на каждый present/render кадр. Перевыбирает провайдеров, считает
    /// обслуживаемые рынки на провайдера и шлёт ядрам рыночную роль только при изменении.
    pub fn set_open(
        &mut self,
        desired: &[(CoreId, String)],
        desired_orderbook: &[(CoreId, String)],
    ) {
        let now = Instant::now();
        self.reconcile_providers();

        // 1. Желаемые рынки на провайдера = union открытых чартов ядер этого
        //    провайдера. Принимаем СПИСОК пар (ядро может иметь несколько открытых
        //    рынков — мульти-панель), агрегируем в множество на провайдера.
        let mut desired_pm: HashMap<CoreId, HashSet<String>> = HashMap::new();
        for (core, market) in desired {
            if let Some(&p) = self.core_provider.get(core) {
                desired_pm.entry(p).or_default().insert(market.clone());
            }
        }
        // Рынки, которым нужен стакан, агрегированные на провайдера (OR по всем окнам). Подписку
        // стакана держим только на них (без linger — стакан можно дёргать быстро).
        let mut orderbook_pm: HashMap<CoreId, HashSet<String>> = HashMap::new();
        for (core, market) in desired_orderbook {
            if let Some(&p) = self.core_provider.get(core) {
                orderbook_pm.entry(p).or_default().insert(market.clone());
            }
        }

        // 2a. Новые желаемые рынки → в wanted + чистый view (провайдер перечитает
        //     retained-историю с начала); отложенный сброс отменяем.
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

        // 2b. Рынки в wanted, которых больше никто не хочет → отложенный сброс (linger).
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

        // 2c. Истёкшие отложенные сбросы → реально убрать из wanted и освободить view.
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

        // 3. Рассылаем ядрам роль. Провайдер (значение в core_provider) → (true, его
        //    рынки); остальные → (false, []). Шлём только при изменении роли.
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
            markets.sort(); // стабильный порядок для сравнения с last_cmd
                            // Стакан: подмножество markets, которым нужен стакан (без linger — снимаем сразу).
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
                    "set_open send core={id} provider={provider} markets={markets:?} \
                     orderbook={orderbook_markets:?}"
                ));
                let _ = s.handle.cmd_tx.send(CoreCmd::SetMarket {
                    provider,
                    markets,
                    orderbook_markets,
                });
            }
        }
    }

    /// Перестраивает `core_provider` (ядро → ядро-провайдер) и `providers` (биржа →
    /// провайдер). В Dedup избирает по одному здоровому Ready-ядру на биржу с
    /// удержанием текущего и failover; в PerCore каждое ядро — свой провайдер.
    fn reconcile_providers(&mut self) {
        // Снимок (id, биржа, Ready?) — без удержания заимствований self во время мутаций.
        let infos: Vec<(CoreId, Option<ExchangeId>, bool)> = self
            .sessions
            .iter()
            .map(|s| {
                let key = self.core_key.get(&s.id).copied();
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
                // Группируем ядра по бирже (только с известной идентичностью).
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
                // Удерживаем текущего провайдера, если он жив и Ready; иначе берём
                // первое Ready-ядро биржи (fallback — первое любое).
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
                // Провайдер биржи сменился → сбрасываем данные старого, его wanted и
                // last_cmd (получит свежее (false,[]) и снимет all-trades).
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
        self.core_provider = new_core_provider;
    }
}
