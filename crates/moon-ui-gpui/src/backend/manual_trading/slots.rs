//! Per-core manual strategy slots and sell-price overrides.

use super::terms::effective_ignore_sell;
use super::types::IgnoreSellLocal;
use crate::Backend;
use moon_core::config::{DEFAULT_ORDER_SIZES_USD, MANUAL_STRAT_SLOTS, StratSlot};
use moon_core::feed::FieldMask;
use moon_core::session::CoreId;
use std::time::Instant;

impl Backend {
    /// Return the six USD-equivalent presets and selected slot for one window group.
    pub(crate) fn manual_order_size_state(&self, group: &str) -> ([f64; 6], usize) {
        self.config
            .group_ref(group)
            .map_or((DEFAULT_ORDER_SIZES_USD, 2), |group| {
                (
                    group.trade.order_sizes_usd,
                    group
                        .trade
                        .order_size_sel
                        .min(group.trade.order_sizes_usd.len() - 1),
                )
            })
    }

    /// Resolve `core`'s ten manual-strategy quick-select slots: what each button fires and what it
    /// says.
    ///
    /// The terminal's own slots win when it has them; otherwise the core's `manual_strats_names`
    /// stand in, so a terminal that has never assigned a button shows exactly what Moonbot shows.
    /// `None` means neither source exists yet — the core has not reported its config and nothing
    /// local was ever assigned — and the caller must draw no buttons rather than ten empty ones.
    pub(crate) fn strat_slots(&self, core: CoreId) -> Option<Vec<StratSlot>> {
        if let Some(local) = self.core_strat_slots(core) {
            return Some(local.to_vec());
        }
        Some(self.core_slots_from_config(core)?.to_vec())
    }

    /// Build the slot array the CORE describes: its names, and its own per-slot visibility.
    ///
    /// `use_buttons` is folded into every slot's `show` rather than kept beside it, because from
    /// here on visibility is one boolean per slot — the core's master switch has no counterpart in
    /// the terminal's own slots, and carrying it separately would leave two different answers to
    /// "is this button drawn".
    pub(super) fn core_slots_from_config(
        &self,
        core: CoreId,
    ) -> Option<[StratSlot; MANUAL_STRAT_SLOTS]> {
        let manual = &self
            .session
            .store()
            .core(core)?
            .core_config
            .as_ref()?
            .manual;
        Some(std::array::from_fn(|i| StratSlot {
            strategy: manual.strat_names[i].trim().to_string(),
            show: manual.strat_buttons.use_buttons && manual.strat_buttons.show_button[i],
        }))
    }

    /// Whether ANY slot arrangement exists for `core` — its own or the core's.
    ///
    /// The hotkey path asks this before falling back to its pre-slot ordinal reading: with a slot
    /// table present, the slot is the authority and an empty one fires nothing; with none at all,
    /// the ordinal is the only thing left to go on.
    pub(crate) fn core_owns_strat_trade_slots(&self, core: CoreId) -> bool {
        self.strat_slots(core).is_some()
    }

    /// Show or hide one quick-select slot, taking this core's slots local in the process.
    pub(crate) fn set_strat_slot_show(&mut self, core: CoreId, ix: usize, show: bool) {
        self.update_strat_slot(core, ix, |slot| slot.show = show);
    }

    /// Replace this core's local slots with what the CORE currently describes — Moonbot's own
    /// names and per-button visibility.
    ///
    /// The popup's explicit "pull from the core" action, and the only way back to the core's
    /// arrangement once a slot has been assigned here. Captions are dropped with the rest: they
    /// name the core's strategies again, which is exactly what pulling asks for.
    pub(crate) fn pull_strat_slots_from_core(&mut self, core: CoreId) -> bool {
        let Some(slots) = self.core_slots_from_config(core) else {
            return false;
        };
        self.update_server(core, |server| server.strat_slots = Some(slots.clone()));
        true
    }

    /// Set the core's "ignore a manual strategy's own sell price" flag.
    ///
    /// The one manual-block field that is NOT local: it changes what the core does with the toolbar
    /// TP and S slots while a manual strategy is active, so it travels as a narrow shared-config
    /// write and is confirmed by the core's echo like every other core-owned setting.
    pub(crate) fn set_ignore_strat_sell_price(&mut self, core: CoreId, on: bool) {
        let Some(mut cfg) = self
            .session
            .store()
            .core(core)
            .and_then(|data| data.core_config.clone())
        else {
            log::warn!("ignore strat sell price: core={core} has not reported its configuration");
            return;
        };
        cfg.manual.ignore_strat_sell_price = on;
        if let Err(error) = self.session.edit_core_config(
            core,
            cfg,
            FieldMask::EMPTY.with_ignore_strat_sell_price(),
        ) {
            log::warn!("ignore strat sell price failed: core={core}: {error:#}");
            return;
        }
        // Optimistic, for the same reason the manual-strategy toggle and Panic Sell keep one: this
        // value travels on the SLOW channel (a whole safe-share packet, behind the compact-settings
        // gate, with an echo and three attempts), so a checkbox rendered from the core's own value
        // alone does not move when clicked and reads as broken. The override is time-boxed rather
        // than permanent — if the core never takes the value, the truth has to come back on its
        // own.
        log::info!("core {core} ignore strat sell price -> {on} (queued)");
        self.ignore_sell_local.insert(
            core,
            IgnoreSellLocal {
                want: on,
                at: Instant::now(),
            },
        );
    }

    /// The core's "ignore a manual strategy's own sell price" flag as the UI must show it: a fresh
    /// local request while one is in flight, the core's own value otherwise.
    ///
    /// `None` before the core has reported its configuration at all — there is nothing to show and
    /// nothing to write onto.
    pub(crate) fn ignore_strat_sell_price(&self, core: CoreId) -> Option<bool> {
        let core_value = self
            .session
            .store()
            .core(core)?
            .core_config
            .as_ref()?
            .manual
            .ignore_strat_sell_price;
        Some(effective_ignore_sell(
            self.ignore_sell_local
                .get(&core)
                .map(|local| (local.want, local.at.elapsed())),
            core_value,
        ))
    }

    /// This core's OWN slots, or `None` while it still follows the core's.
    pub(super) fn core_strat_slots(
        &self,
        core: CoreId,
    ) -> Option<&[StratSlot; MANUAL_STRAT_SLOTS]> {
        self.config
            .servers
            .iter()
            .find(|server| server.id == core)
            .and_then(|server| server.strat_slots.as_ref())
    }

    /// Assign the strategy one slot fires, taking this core's slots local in the process.
    ///
    /// Args:
    ///     core: Core whose slot is being assigned.
    ///     ix: Zero-based slot.
    ///     strategy: Manual-kind strategy name, or empty to clear the slot.
    pub(crate) fn set_strat_slot_strategy(&mut self, core: CoreId, ix: usize, strategy: String) {
        self.update_strat_slot(core, ix, |slot| slot.strategy = strategy.clone());
    }

    /// Apply one mutation to a slot, seeding this core's whole slot array from whatever it was
    /// SHOWING first.
    ///
    /// Seeding from the shown values rather than from empties is what keeps the other nine buttons
    /// where they were the moment the first one is assigned; without it, taking a core local would
    /// blank every button that came from `manual_strats_names`.
    pub(super) fn update_strat_slot(
        &mut self,
        core: CoreId,
        ix: usize,
        update: impl Fn(&mut StratSlot),
    ) {
        if ix >= MANUAL_STRAT_SLOTS {
            return;
        }
        let seed: [StratSlot; MANUAL_STRAT_SLOTS] = self
            .strat_slots(core)
            .map(|slots| std::array::from_fn(|i| slots.get(i).cloned().unwrap_or_default()))
            .unwrap_or_default();
        self.update_server(core, |server| {
            let slots = server.strat_slots.get_or_insert_with(|| seed.clone());
            update(&mut slots[ix]);
        });
    }
}
