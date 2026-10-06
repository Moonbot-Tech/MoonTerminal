//! Strategy-sourced manual exits and hook selection.

use super::terms::{
    exit_source, hook_of, is_hook, resolve_manual_selection, signed_stop_pct, strat_field_value,
};
use super::types::{FIELD_USE_HOOK_STRATEGY, MsExitOverlay};
use crate::Backend;
use moon_core::config::GroupExitSettings;
use moon_core::feed::ClientSettingsEdit;
use moon_core::session::CoreId;

impl Backend {
    /// Seed the manual-strategy exit OVERLAY from a strategy's own values.
    ///
    /// An overlay rather than a write: while MS is on, the take profit and stop on screen belong to
    /// the SELECTED STRATEGY, and the group's (or the core's own) saved values must survive
    /// untouched underneath. Switch to another chart and those saved values are what show; switch
    /// MS off and they come back here too.
    ///
    /// Keyed by `(core, strategy)`, so returning to a strategy restores what was last used with it
    /// rather than re-reading the strategy over the trader's own adjustment.
    pub(crate) fn seed_exit_from_strategy(&mut self, core: CoreId, strategy_id: u64) -> bool {
        if strategy_id == 0 {
            return false;
        }
        // The strategy the CORE will read these from, which is the selected one unless it defers to
        // a MoonHook. `None` means a hook is named but its row is not here, so nothing on screen can
        // be made to agree with the order: leave the saved generation showing rather than display
        // the selected strategy's own numbers, which that order will not use.
        let Some(source) = self.exit_source_strategy(core, strategy_id) else {
            return false;
        };
        // Already holding this source's values, adjustments included — nothing to read. A DIFFERENT
        // source is a different exit set altogether (the hook was changed or cleared), and that one
        // is re-read over whatever is here.
        if self
            .ms_exit_local
            .get(&(core, strategy_id))
            .map(|ms| ms.source)
            == Some(source)
        {
            return false;
        }
        let Some((stop_on, stop_pct, sell_pct)) = self.strategy_exit(core, source) else {
            return false;
        };
        // Through the same clamp every other writer of this number uses, or a strategy carrying a
        // stop outside the protocol range would be displayed and priced at a value that silently
        // snaps to the boundary the first time the SL popup is touched.
        let stop_pct = GroupExitSettings::canonical_stop_loss_pct(signed_stop_pct(Some(stop_pct)))
            .unwrap_or(0.0);
        // The take profit comes from the SELECTED strategy or from nowhere. The stop above may be
        // the hook's — a hook has one — but a hook has no `SellPrice`, so a hooked strategy leaves
        // this unseeded and the trader's own take profit stays on screen and on the order.
        //
        // A sell distance is a percentage forward from entry; anything non-finite or negative is
        // not one, and `planned_sell_price` would turn it into a target below the buy.
        let take_profit_pct = (source == strategy_id).then(|| {
            if sell_pct.is_finite() && sell_pct >= 0.0 {
                sell_pct
            } else {
                0.0
            }
        });
        self.ms_exit_local.insert(
            (core, strategy_id),
            MsExitOverlay {
                source,
                stop_on,
                stop_pct,
                take_profit_pct,
            },
        );
        true
    }

    /// Seed the exit overlay for a core whose manual-strategy mode is ALREADY on.
    ///
    /// The overlay is process-lifetime and used to be filled only by the click that selected a
    /// strategy. After a restart there is no click: the toolbar showed the saved generation and the
    /// first order carried it, while the header named a strategy whose values were never loaded.
    /// This closes that window as soon as the strategy list makes the selection resolvable.
    ///
    /// Returns whether anything was seeded, so the caller knows to repaint.
    pub(crate) fn tick_manual_exit_seed(&mut self) -> bool {
        let mut pending: Vec<(CoreId, u64)> = Vec::new();
        let mut checked: Vec<(CoreId, (u64, u64))> = Vec::new();
        for server in &self.config.servers {
            // Cheapest tests first: only a core whose mode is on and which names a strategy can
            // need an overlay, and those are a handful even on a 200-core desk. Everything after
            // this point costs a store lookup and a scan of that core's strategy list.
            let Some(stored) = server.manual_strategy.as_ref() else {
                continue;
            };
            if !stored.on || stored.strategy.trim().is_empty() {
                continue;
            }
            let Some(data) = self.session.store().core(server.id) else {
                continue;
            };
            // Both revisions this seed reads: the strategy list it resolves the selection against,
            // and the schema without which a field left at its default cannot be told from one that
            // has not arrived. Until one of them moves the answer cannot change, and re-deriving it
            // ten times a second is the cost this gate exists to remove.
            let key = (data.strategies_rev, data.schema_rev);
            if self.manual_exit_checked.get(&server.id) == Some(&key) {
                continue;
            }
            checked.push((server.id, key));
            let Some(id) = resolve_manual_selection(&data.strategies, stored) else {
                continue;
            };
            // No "already has an overlay" test here: an entry seeded off a source that no longer
            // applies — the strategy was pointed at another hook — has to be re-read, and only
            // `seed_exit_from_strategy` knows which source produced it. It answers cheaply.
            pending.push((server.id, id));
        }
        for (core, key) in checked {
            self.manual_exit_checked.insert(core, key);
        }
        let mut seeded = false;
        for (core, id) in pending {
            // Reports what it STORED, not what was attempted: a strategy whose fields have not
            // arrived seeds nothing, and treating that as work done would repaint at 10 Hz forever.
            seeded |= self.seed_exit_from_strategy(core, id);
        }
        seeded
    }

    /// The exit the toolbar must show and the order must use, while MS owns it.
    ///
    /// `None` whenever the saved generation is the one in force: MS off, no strategy selected, or
    /// no overlay seeded for it yet.
    pub(crate) fn manual_exit_overlay(&self, core: CoreId) -> Option<MsExitOverlay> {
        let strategy_id = self.manual_strat_active(core)?;
        self.ms_exit_local.get(&(core, strategy_id)).copied()
    }

    /// Apply an exit edit to the manual-strategy overlay instead of the saved generation.
    ///
    /// Returns whether the edit was absorbed here; `false` leaves it to the ordinary group/core
    /// write, which is what happens with MS off.
    pub(super) fn edit_manual_exit_overlay(
        &mut self,
        group: &str,
        edit: ClientSettingsEdit,
    ) -> bool {
        let Some(core) = self.active_trade_core(group) else {
            return false;
        };
        let Some(strategy_id) = self.manual_strat_active(core) else {
            return false;
        };
        // Moonbot's own rule, when this core follows it: the stop belongs to the strategy and is
        // read-only here. ABSORBED rather than passed on — letting it fall through would write the
        // saved generation instead, moving a number that is not the one on screen. The toolbar
        // disables the same controls from `manual_stop_locked`, so this catches only what arrives by
        // another route: a hotkey, or a popup opened before the mode came on.
        if self.manual_stop_locked(core)
            && matches!(
                edit,
                ClientSettingsEdit::PanicIfPriceDrop(_) | ClientSettingsEdit::StopLossPct(_)
            )
        {
            log::debug!(
                "core {}: the stop belongs to the manual strategy, edit ignored",
                moon_core::feed::core_label(core)
            );
            return true;
        }
        // Seed first: an edit landing before the coordination tick would otherwise create the entry
        // itself, and the freshness guard would then block the strategy's own values from ever being
        // read into it. Idempotent, so this is a no-op once seeded.
        self.seed_exit_from_strategy(core, strategy_id);
        // Where the seed DECLINES — a hook owns the exits, or the schema has not arrived — there is
        // no overlay to edit and none is invented here: the toolbar is showing the saved generation,
        // so the edit belongs to it. Creating one would latch a set whose contents depend on whether
        // the trader clicked before or after the strategy's fields arrived, and the `contains_key`
        // guard would then block the strategy's own values forever.
        let Some(mut current) = self.ms_exit_local.get(&(core, strategy_id)).copied() else {
            return false;
        };
        match edit {
            ClientSettingsEdit::PanicIfPriceDrop(on) => current.stop_on = on,
            ClientSettingsEdit::StopLossPct(pct) => {
                let Some(pct) = GroupExitSettings::canonical_stop_loss_pct(pct) else {
                    return true;
                };
                current.stop_pct = pct;
            }
            ClientSettingsEdit::TakeProfit { pct, .. }
            | ClientSettingsEdit::ScalpTakeProfit(pct) => {
                if !(pct.is_finite() && pct >= 0.0) {
                    return true;
                }
                current.take_profit_pct = Some(pct);
            }
            // A fixed-sell preset is a take profit too, and the engaged one is what the order
            // sells at: keeping it out would leave the readout and the order disagreeing.
            ClientSettingsEdit::SelectFixedSellSlot(slot) if (1..=6).contains(&slot) => {
                current.take_profit_pct =
                    Some(self.write_aligned_group_exit(group).fixed_sell_pcts[slot - 1]);
            }
            _ => return false,
        }
        self.ms_exit_local.insert((core, strategy_id), current);
        true
    }

    /// One strategy's `UseStopLoss`, `StopLoss` and `SellPrice` as the core would apply them.
    ///
    /// `None` only when the answer is not KNOWABLE yet: no such strategy, or no schema. The schema
    /// is what makes an absent field readable at all — the server omits every value equal to its
    /// default and the schema carries the non-zero ones, so without it "absent" cannot be told from
    /// "at its default". WITH it, an unresolved field IS the default: `UseStopLoss=No`,
    /// `StopLoss=0`, `SellPrice=0`. That is why the tuple carries plain values, not options: every
    /// element is a real answer, and a caller that cannot get one gets `None` for the whole thing.
    pub(super) fn strategy_exit(&self, core: CoreId, strategy_id: u64) -> Option<(bool, f64, f64)> {
        let data = self.session.store().core(core)?;
        let row = data.strategies.iter().find(|s| s.id == strategy_id)?;
        let schema = data.schema.as_ref()?;
        let field = |name: &str| strat_field_value(row, Some(schema), name);
        Some((
            field("UseStopLoss").is_some_and(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "yes" | "true" | "1" | "on"
                )
            }),
            field("StopLoss")
                .and_then(|v| v.trim().parse::<f64>().ok())
                .unwrap_or(0.0),
            field("SellPrice")
                .and_then(|v| v.trim().parse::<f64>().ok())
                .unwrap_or(0.0),
        ))
    }

    /// The MoonHook strategy this one defers its exits to, as the CORE currently holds it.
    ///
    /// Empty means none. Moonbot substitutes the hook at order time — its log says `Manual strategy
    /// X turned into Hook Y` — and the order's exits then come from the hook, not from the selected
    /// strategy (`Using (strategy <Y>) Sell Price`). Only the STOP is readable from here: a hook
    /// carries `UseStopLoss`/`StopLoss` like any strategy, but its sell lives in
    /// `HookSellLevel`/`HookSellFixed` rather than the `SellPrice` every other kind uses — see
    /// [`MsExitOverlay::take_profit_pct`].
    ///
    /// Read from `UseHookStrategy`, whose picklist moonproto builds from the local MoonHook
    /// strategies with an empty first item.
    pub(crate) fn strategy_hook(&self, core: CoreId, strategy_id: u64) -> String {
        let Some(data) = self.session.store().core(core) else {
            return String::new();
        };
        let Some(row) = data.strategies.iter().find(|s| s.id == strategy_id) else {
            return String::new();
        };
        hook_of(row, data.schema.as_ref())
    }

    /// [`Self::strategy_hook`] with this terminal's own unconfirmed edit on top.
    ///
    /// The control that sent the edit has to keep showing what was chosen: a core takes a moment to
    /// echo a strategy back, and a dropdown that snaps to the old value in between reads as a click
    /// that did nothing. The confirmed value returns the moment the echo lands.
    pub(crate) fn strategy_hook_shown(&self, core: CoreId, strategy_id: u64) -> String {
        let open = self
            .session
            .store()
            .core(core)
            .and_then(|data| data.strategy_edit(strategy_id));
        match open {
            // An open edit carries the strategy's DESIRED state whole, and a field equal to its
            // default is omitted from it — so an absent hook there means "no hook", not "this edit
            // says nothing about it". Falling back to the confirmed value instead would leave a
            // just-cleared hook on screen until the core echoed.
            Some(edit) => edit
                .fields
                .iter()
                .find(|(name, _)| name == FIELD_USE_HOOK_STRATEGY)
                .map(|(_, value)| value.trim().to_string())
                .unwrap_or_default(),
            None => self.strategy_hook(core, strategy_id),
        }
    }

    /// The MoonHook strategy carrying this name, when this core has one.
    ///
    /// Through [`exit_source`], the same resolver the exits go through, so a control that reveals
    /// "the hook this strategy uses" and the order that reads its stop can never mean two different
    /// strategies. The `0` is the unused no-hook fallback of that function: with a non-empty name
    /// it only ever answers the hook's own id.
    pub(crate) fn hook_strategy_id(&self, core: CoreId, hook: &str) -> Option<u64> {
        let hook = hook.trim();
        if hook.is_empty() {
            return None;
        }
        exit_source(&self.session.store().core(core)?.strategies, hook, 0)
    }

    /// Names of this core's MoonHook strategies, in snapshot order — the picker's options.
    ///
    /// A strategy the core never named is skipped. `StrategyRow::name` is a DISPLAY name and
    /// substitutes `strat <id>` for those (`feed::strategies::strat_display_name`), which is not a
    /// name the core's own `UseHookStrategy` picklist carries — writing it would set a hook nothing
    /// on the core resolves. Duplicates go too: the field addresses a hook by name, so two rows
    /// sharing one are a choice this picker cannot make on the trader's behalf.
    pub(crate) fn hook_strategy_names(&self, core: CoreId) -> Vec<String> {
        let Some(data) = self.session.store().core(core) else {
            return Vec::new();
        };
        let mut names: Vec<String> = Vec::new();
        for row in data.strategies.iter().filter(|row| is_hook(row)) {
            let name = row.name.trim();
            // The placeholder `strat <id>` spelling `feed::strategies::strat_display_name` puts on
            // an unnamed strategy, tested without allocating one to compare against.
            let placeholder = name
                .strip_prefix("strat ")
                .is_some_and(|rest| rest.parse::<u64>() == Ok(row.id));
            if name.is_empty() || placeholder {
                continue;
            }
            let name = name.to_string();
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names
    }

    /// Point one strategy at a MoonHook strategy, or clear it with an empty `hook`.
    ///
    /// This writes to the CORE's own strategy, exactly as the Strategies panel's field editor does
    /// and through the same command — it is the same setting, reachable from the popup that already
    /// names the strategy. Nothing local is touched: the exit overlay re-seeds itself off the new
    /// source once the core echoes the strategy back ([`MsExitOverlay::source`]), so a write the
    /// core refuses leaves the screen on the values still in force.
    ///
    /// Returns whether the command was queued.
    pub(crate) fn set_strategy_hook(
        &mut self,
        core: CoreId,
        strategy_id: u64,
        hook: String,
    ) -> bool {
        let label = if hook.trim().is_empty() {
            rust_i18n::t!("header.ms_hook_none").to_string()
        } else {
            hook.trim().to_string()
        };
        let edits = vec![(
            strategy_id,
            vec![(FIELD_USE_HOOK_STRATEGY.to_string(), hook)],
        )];
        match self.session.edit_strategies(core, edits) {
            Ok(()) => {
                // The popup this is clicked from has no window of its own, so an outcome other than
                // a clean confirmation can only be reported through the shell's toast queue — the
                // same route the coin menu's field edit uses. Quiet on the way out: the picker
                // already shows what was chosen, and a toast per pick would be noise.
                self.watch_strategy_edit_quiet(core, strategy_id, label);
                true
            }
            Err(error) => {
                log::warn!(
                    "core {} strategy {strategy_id}: setting the hook failed: {error}",
                    moon_core::feed::core_label(core)
                );
                false
            }
        }
    }

    /// Whether this strategy's KIND exposes `field` in the schema the core sent.
    ///
    /// A field outside the kind's schema is dropped by the serializer, so the edit would be a
    /// silent no-op — and with no schema at all the whole batch is refused before it is staged.
    /// Both are "do not offer this control", which is why an absent schema answers `false`. The
    /// coin menu gates its own field edit the same way (`controls::coin_menu`).
    pub(crate) fn strategy_has_field(&self, core: CoreId, strategy_id: u64, field: &str) -> bool {
        let Some(data) = self.session.store().core(core) else {
            return false;
        };
        let Some(row) = data.strategies.iter().find(|s| s.id == strategy_id) else {
            return false;
        };
        let Some(schema) = data.schema.as_ref() else {
            return false;
        };
        schema
            .kinds
            .iter()
            .find(|kind| kind.ordinal == row.kind_ordinal)
            .is_some_and(|kind| {
                kind.sections
                    .iter()
                    .any(|section| section.fields.iter().any(|f| f.name == field))
            })
    }

    /// The strategy whose exits a manual order on `strategy_id` will actually carry.
    ///
    /// `Some(strategy_id)` with no hook set; the hook's own id when one is set and present in the
    /// snapshot; `None` when a hook is NAMED but its row is not here — the one case where this
    /// terminal cannot say what the order's exits will be, and must not guess.
    pub(super) fn exit_source_strategy(&self, core: CoreId, strategy_id: u64) -> Option<u64> {
        let hook = self.strategy_hook(core, strategy_id);
        let strategies = self
            .session
            .store()
            .core(core)
            .map(|data| data.strategies.as_slice())
            .unwrap_or_default();
        exit_source(strategies, &hook, strategy_id)
    }
}
