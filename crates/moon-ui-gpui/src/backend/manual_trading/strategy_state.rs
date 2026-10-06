//! Stored manual-strategy selection and snapshot adoption.

use super::terms::{
    effective_manual_strat_state, is_manual, manual_selection_is_broken, manual_strat_seed,
    manual_strategy_id, resolve_manual_selection,
};
use super::types::SettleKey;
use crate::Backend;
use moon_core::config::ManualStratState;
use moon_core::session::CoreId;

/// Refresh sorted live IDs, reusing the buffer so steady rosters allocate nothing after warm-up.
pub(super) fn refresh_live_ids(scratch: &mut Vec<CoreId>, ids: impl Iterator<Item = CoreId>) {
    scratch.clear();
    scratch.extend(ids);
    scratch.sort_unstable();
}

impl Backend {
    /// Drop sync bookkeeping for cores that are no longer live.
    ///
    /// What this function no longer does is the point: it used to PUSH every group's exit
    /// generation into every live core on each coordination tick, which is why a TP changed in
    /// Moonbot itself sprang back within a tick, and why the compact settings channel could stay
    /// permanently busy — starving the safe-share writes queued behind it.
    ///
    /// The terminal's exits are local. They reach a core exactly where they must: the manual-order
    /// path serializes the visible generation ahead of the order it belongs to
    /// (`feed::live::client_settings`), so an order can still never go out under someone else's
    /// TP/SL, while a core left alone stays editable from its own screen.
    pub(crate) fn sync_manual_settings(&mut self) {
        refresh_live_ids(
            &mut self.live_core_scratch,
            self.session.sessions().iter().map(|session| session.id),
        );
        let live_ids = &self.live_core_scratch;
        self.group_exit_sync
            .retain(|core, _| live_ids.binary_search(core).is_ok());
        self.manual_strat_checked
            .retain(|core, _| live_ids.binary_search(core).is_ok());
        self.manual_exit_checked
            .retain(|core, _| live_ids.binary_search(core).is_ok());
        // Keyed by `(core, strategy)`, so it is pruned by the core half. A strategy that goes away
        // on a LIVE core keeps its entry, which is deliberate: the trader may switch back to a
        // rebuilt one and expect the exits they last used with it.
        self.ms_exit_local
            .retain(|(core, _), _| live_ids.binary_search(core).is_ok());
    }

    /// Set this core's manual-strategy mode, which is terminal state and stays here.
    ///
    /// Nothing is sent to the core here: the strategy travels with the order instead
    /// ([`ManualOrderTerms::strategy_id`]), so Moonbot's own manual-strategy switch is left exactly
    /// where its user put it, and two terminals on one core can sit on different strategies. The
    /// one write happens at order time and only with the mode OFF: the exit barrier switches the
    /// core's own mode off so a bare order is not handed to the strategy that switch names.
    ///
    /// Args:
    ///     core: Core whose mode is being set.
    ///     on: Whether manual-strategy mode is enabled.
    ///     id: Selected strategy, or `0` to keep the stored selection while only `on` changes.
    pub(crate) fn set_manual_strat(&mut self, core: CoreId, on: bool, id: u64) -> u64 {
        // Name resolved here, while the snapshot is at hand, and the id PINNED alongside it: this
        // is the moment the trader actually chose, and every later order must go to that same
        // strategy rather than to whatever the name resolves to at the time.
        //
        // An id that resolves to nothing keeps the stored pair rather than clearing it — that is a
        // strategy list which has not arrived, not a trader deselecting anything.
        let resolved = self.manual_strategy_name(core, id).map(str::to_string);
        let (strategy, id) = match resolved {
            Some(name) => (name, id),
            None => {
                if id != 0 {
                    log::warn!(
                        "core {} manual strategy {id} is not in the retained snapshot; keeping the \
                         previous selection",
                        moon_core::feed::core_label(core)
                    );
                }
                self.stored_manual_strat(core)
                    .map(|stored| (stored.strategy.trim().to_string(), stored.id))
                    .unwrap_or_default()
            }
        };
        // Everything this setter does not own is carried over: `mb_logic` is a separate switch in
        // the same popup, and rebuilding the state around a selection must not reset it.
        let mb_logic = self.ms_mb_logic(core);
        let next = ManualStratState {
            on,
            strategy,
            id,
            mb_logic,
        };
        // Held hotkeys repeat, and a re-selection of the same strategy is the common case; writing
        // unconditionally would raise `config_dirty` and buy a full encrypted save each repeat.
        if self.stored_manual_strat(core) == Some(&next) {
            return id;
        }
        self.update_server(core, |server| server.manual_strategy = Some(next.clone()));
        id
    }

    /// This core's stored manual-strategy mode, or `None` while it has never had one here.
    pub(super) fn stored_manual_strat(&self, core: CoreId) -> Option<&ManualStratState> {
        self.config
            .servers
            .iter()
            .find(|server| server.id == core)?
            .manual_strategy
            .as_ref()
    }

    /// Name of the manual strategy this core is set to, when one is actually named.
    ///
    /// Answers "did the trader select something" independently of whether it currently resolves,
    /// which is what separates an unconfigured core from one pointing at a missing strategy.
    pub(crate) fn selected_manual_strategy_name(&self, core: CoreId) -> Option<&str> {
        let name = self.stored_manual_strat(core)?.strategy.trim();
        (!name.is_empty()).then_some(name)
    }

    /// Resolve a Manual-kind strategy id to its name in this core's retained snapshot.
    pub(super) fn manual_strategy_name(&self, core: CoreId, id: u64) -> Option<&str> {
        if id == 0 {
            return None;
        }
        crate::strategies::logic::row(self.session.store(), core, id)
            .filter(|row| is_manual(row))
            .map(|row| row.name.trim())
    }

    /// Seed a core's manual-strategy mode from its own snapshot, once, and only if it has none.
    ///
    /// The upgrade path: before this terminal owned the mode it lived in the core, so a trader who
    /// left Moonbot on a manual strategy must find the terminal on that same strategy after the
    /// first launch instead of silently switched off. Runs on the coordination tick because both
    /// halves it needs — the settings snapshot and a CONFIRMED strategy list to resolve the id
    /// against — arrive asynchronously and at different times.
    ///
    /// Returns whether anything was seeded, so the caller knows to repaint.
    pub(crate) fn tick_manual_strat_seed(&mut self) -> bool {
        // Never while the settings window holds a draft: `update_server` mirrors into that preview,
        // so a tick-driven pin would appear inside a row the user is mid-edit and be committed by
        // their Save. It settles on the next tick after the window closes.
        if self.preview.is_some() {
            return false;
        }
        let mut seeds: Vec<(CoreId, ManualStratState)> = Vec::new();
        let mut answered: Vec<(CoreId, SettleKey)> = Vec::new();
        for server in &self.config.servers {
            let Some(data) = self.session.store().core(server.id) else {
                continue;
            };
            // Nothing this pass can decide differently until one of the inputs it reads has moved.
            // Without this gate every unresolvable core — a deleted strategy, a core with the
            // strategy feed switched off, a dead core — re-ran the whole resolution ten times a
            // second, forever. `client_settings_stale` is part of the key because it clears WITHOUT
            // moving a revision: a reconnect whose settings equal the retained ones bumps nothing,
            // and a core marked while stale would otherwise never be examined again.
            let key = (
                data.strategies_rev,
                data.client_settings_rev,
                data.client_settings_stale,
            );
            if self.manual_strat_checked.get(&server.id) == Some(&key) {
                continue;
            }
            // Marked unconditionally, because the key holds every input this pass reads: the two
            // revisions and the staleness flag. A core it cannot answer for today gets exactly one
            // more attempt per input change, instead of the same two scans ten times a second.
            answered.push((server.id, key));
            match server.manual_strategy.as_ref() {
                // Nothing selected, and nothing adopted into it either. The core's own
                // `manual_strategy_id` is NOT read here, however tempting: Moonbot moves that field
                // by itself, and forwarding it is what put two real orders on a strategy nobody
                // chose on 2026-09-01 (see `manual_strat_state`). Adoption belongs to the `None`
                // arm below — a core this terminal has never known — and a stored state, even one
                // naming nothing, means the trader has already touched this mode here.
                //
                // The cost is narrow and visible: turning MS on before the core has reported skips
                // the one-time carry-over of its selection, and the picker — on screen precisely
                // because the mode is on — is how one gets chosen instead.
                Some(stored) if stored.strategy.trim().is_empty() => {}
                // A stored selection whose pin is missing or has gone stale. Missing is every
                // config written before the id was kept; stale is a strategy deleted and rebuilt,
                // which keeps its name and loses its number. Re-pin from the name either way —
                // leaving it would silently return this core to resolving the name before every
                // order, which is the behaviour the pin exists to end.
                Some(stored) => {
                    if resolve_manual_selection(&data.strategies, stored) == Some(stored.id) {
                        continue;
                    }
                    // Re-pinning writes a per-host id into permanent config, so it waits for the
                    // same freshness signal the first seed demands. PARTIAL cover, knowingly: the
                    // flag tracks the SETTINGS feed while this reads the strategy list, and that
                    // list carries no staleness marker at all. It closes the window around a
                    // disconnect, not the one between a reconnect's settings and its first
                    // strategy publish.
                    if data.client_settings_stale {
                        continue;
                    }
                    if let Some(id) = manual_strategy_id(&data.strategies, &stored.strategy) {
                        seeds.push((
                            server.id,
                            ManualStratState {
                                id,
                                ..stored.clone()
                            },
                        ));
                    }
                }
                None => {
                    let Some(settings) = data.client_settings.as_ref() else {
                        continue;
                    };
                    // Stale settings are a snapshot the store itself will not vouch for — after a
                    // disconnect, or after a key change that may point the feed at a DIFFERENT
                    // Moonbot, since the store keeps the previous host's settings until new ones
                    // arrive. Adopting one into permanent config is the one mistake this seed
                    // cannot undo.
                    //
                    // It covers the settings half only: the retained strategy list carries no
                    // staleness marker at all, so the NAME this resolves can still come from a
                    // previous host's list until that list is replaced. Narrow enough to live with,
                    // wide enough to write down.
                    if data.client_settings_stale {
                        continue;
                    }
                    let Some(state) = manual_strat_seed(
                        settings.use_manual_strategy,
                        settings.manual_strategy_id,
                        &data.strategies,
                    ) else {
                        continue;
                    };
                    seeds.push((server.id, state));
                }
            }
        }
        for (core, key) in answered {
            self.manual_strat_checked.insert(core, key);
        }
        let seeded = !seeds.is_empty();
        for (core, state) in seeds {
            // At info only when there is a selection to report; a fleet settling on the first
            // launch after an upgrade would otherwise put one line per core into the log for
            // having decided that nothing is selected.
            if state.strategy.is_empty() {
                log::debug!(
                    "core {} manual strategy settled: nothing selected",
                    moon_core::feed::core_label(core)
                );
            } else {
                log::info!(
                    "core {} manual strategy settled: on={} strategy={:?} id={}",
                    moon_core::feed::core_label(core),
                    state.on,
                    state.strategy,
                    state.id
                );
            }
            self.update_server(core, |server| server.manual_strategy = Some(state.clone()));
        }
        seeded
    }

    /// Return this core's effective manual-strategy state as `(enabled, id)`.
    ///
    /// The STORED terminal state is the only source. A core the terminal has not adopted yet reads
    /// as off rather than borrowing the core's own `use_manual_strategy`: Moonbot moves that field
    /// itself, and forwarding it put two real orders on a strategy nobody selected. Adoption is one
    /// coordination tick away (`tick_manual_strat_seed`).
    ///
    /// A confirmed snapshot with no Manual-kind strategy makes the state effectively disabled while
    /// preserving the selection; pending strategy data retains the raw state so TP/SL stay
    /// fail-safe. The id comes from `resolve_manual_selection`, so a pinned strategy keeps being
    /// found across a rename and the stored name takes over once the pin names nothing.
    ///
    /// Args:
    ///     core: Core whose effective manual-strategy state is requested.
    ///
    /// Returns:
    ///     Effective enabled state and resolved selected id.
    pub(crate) fn manual_strat_state(&self, core: CoreId) -> (bool, u64) {
        let core_data = self.session.store().core(core);
        // The STORED selection, and nothing else. Reading the core's live `manual_strategy_id` here
        // as a stand-in until the seed runs is what put two real orders on the wrong strategy on
        // 2026-09-01: Moonbot moves that field itself, so the terminal was faithfully forwarding a
        // choice that changed without anybody touching this screen. A core the terminal has not
        // adopted yet simply has the mode off here; `tick_manual_strat_seed` adopts it within a
        // tick of the core reporting enough to adopt.
        let raw = self
            .stored_manual_strat(core)
            .map(|stored| {
                (
                    stored.on,
                    core_data
                        .and_then(|data| resolve_manual_selection(&data.strategies, stored))
                        .unwrap_or(0),
                )
            })
            .unwrap_or((false, 0));
        core_data
            .map(|data| effective_manual_strat_state(raw, &data.strategies))
            .unwrap_or(raw)
    }

    /// Whether this core is configured to receive its strategy list at all.
    ///
    /// `FeedFlags::strategies` is a client-side filter: with it off the terminal never stores the
    /// core's strategies, so nothing that depends on resolving one against them can be answered.
    pub(super) fn core_receives_strategies(&self, core: CoreId) -> bool {
        self.config
            .servers
            .iter()
            .find(|server| server.id == core)
            .is_some_and(|server| server.feed.strategies)
    }

    /// The manual selection this core carries that currently resolves to NOTHING, described for a
    /// log — a strategy this core cannot provide, and the one state an order is refused on.
    ///
    /// A core with no Manual strategies at all is not counted here to be broken — it reads as
    /// mode-off — and an order on it is an ordinary manual order; `manual_order_terms` warns
    /// separately when such a core still has its own switch on.
    pub(crate) fn manual_strat_unresolved(&self, core: CoreId) -> Option<&str> {
        // A core whose strategy feed is switched off never receives a list, so its selection can
        // never resolve. Refusing every order forever, with the whole MS cluster hidden for the
        // same reason, would leave nothing on screen able to clear the state — the trader would
        // have to edit the config by hand. Their own flag says they accept working without it.
        if !self.core_receives_strategies(core) {
            return None;
        }
        let stored = self.stored_manual_strat(core)?;
        // A core with no store entry at all knows LESS than one with an empty strategy list, which
        // the rule below already refuses on — so it resolves nothing and is judged the same way.
        let strategies = self
            .session
            .store()
            .core(core)
            .map(|data| data.strategies.as_slice())
            .unwrap_or_default();
        manual_selection_is_broken(strategies, stored).then(|| stored.strategy.trim())
    }

    /// The manual strategy the next order would actually be placed on, if any.
    ///
    /// The question every consumer outside the header itself is really asking. `manual_strat_state`
    /// answers what the SWITCH is set to, which stays true with nothing selected and while a stored
    /// selection has not resolved; in both of those the core would receive an ordinary order, so a
    /// toolbar that locked TP and S on the strength of the switch alone would be disabling the very
    /// controls whose values ride along with it.
    pub(crate) fn manual_strat_active(&self, core: CoreId) -> Option<u64> {
        let (on, id) = self.manual_strat_state(core);
        (on && id != 0).then_some(id)
    }
}
