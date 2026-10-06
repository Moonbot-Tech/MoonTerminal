//! Core and group ownership of manual-trading settings.

use super::types::{ManualSource, seed_on_enable};
use crate::Backend;
use moon_core::config::{GroupExitSettings, GroupTradeSettings};
use moon_core::session::CoreId;

impl Backend {
    /// Whether `core` keeps its OWN manual-trading generation instead of sharing its group's.
    ///
    /// The flag alone is not enough: a core must also HAVE a generation for the answer to be yes.
    /// `set_core_own_trade` seeds one before it sets the flag and `config::reconcile` seeds any
    /// file that predates the seeding rule, so the two agree in practice — but a hand-edited file
    /// can still carry the flag with no set, and there this must answer the same as the display
    /// resolver ([`Self::core_trade_settings`]), or the toolbar would show the group's numbers with
    /// the switch off while a click silently forked a per-core set.
    pub(crate) fn core_own_trade(&self, core: CoreId) -> bool {
        self.core_trade_settings(core).is_some()
    }

    /// Return `core`'s own manual-trading generation, or `None` while it shares its group's.
    ///
    /// A core with the switch ON but no stored generation cannot happen through
    /// [`Self::set_core_own_trade`], which seeds one before it flips the flag; this still answers
    /// `None` for a hand-edited config so every reader falls back to the group rather than to
    /// invented numbers.
    pub(super) fn core_trade_settings(&self, core: CoreId) -> Option<&GroupTradeSettings> {
        self.config
            .servers
            .iter()
            .find(|server| server.id == core)
            .filter(|server| server.own_trade_config)
            .and_then(|server| server.trade.as_ref())
    }

    /// Set whether `core` keeps its own manual-trading generation, mirroring the live edit into an
    /// open Settings preview exactly like [`Self::update_group_trade`] (contract:
    /// `docs/ARCHITECTURE.md`'s preview-mirror rule, this never skips it).
    ///
    /// Turning the switch ON seeds the core's generation FROM THE GROUP the first time only: the
    /// numbers on screen must not move at the moment of the flip. A core that already has one
    /// keeps it, so toggling off and on again restores exactly what that core had — the reason the
    /// generation survives an off state at all.
    pub(crate) fn set_core_own_trade(&mut self, core: CoreId, on: bool) {
        let seed = on
            .then(|| {
                self.config
                    .servers
                    .iter()
                    .find(|s| s.id == core)
                    .and_then(|s| {
                        seed_on_enable(s.trade.as_ref(), &self.group_trade_settings(&s.group))
                    })
            })
            .flatten();
        self.update_server(core, |server| {
            server.own_trade_config = on;
            if let Some(seed) = seed.clone() {
                server.trade = Some(seed);
            }
        });
    }

    /// Apply one mutation to `core`'s server row in BOTH the live config and an open Settings
    /// preview — the server-row twin of [`Self::update_group_trade`], and the one place that
    /// mirroring is written for them.
    ///
    /// Without the preview half a per-core edit made while Settings is open is discarded the moment
    /// the user presses Save, which replaces the live config from the preview wholesale.
    pub(super) fn update_server(
        &mut self,
        core: CoreId,
        update: impl Fn(&mut moon_core::config::ServerConfig),
    ) {
        for servers in [
            Some(&mut self.config.servers),
            self.preview.as_mut().map(|preview| &mut preview.servers),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(server) = servers.iter_mut().find(|s| s.id == core) {
                update(server);
            }
        }
        self.config_dirty = true;
    }

    /// Return one group's complete manual-trading generation, or the neutral standard when the
    /// group row does not exist yet.
    pub(super) fn group_trade_settings(&self, group: &str) -> GroupTradeSettings {
        self.config
            .group_ref(group)
            .map(|group| group.trade.clone())
            .unwrap_or_default()
    }

    /// Apply one mutation to `core`'s own manual-trading generation, in both the live config and an
    /// open Settings preview — the per-core twin of [`Self::update_group_trade`].
    ///
    /// Seeds from the group when the core somehow has the switch on with no stored generation, so a
    /// hand-edited config cannot make an edit vanish.
    pub(super) fn update_core_trade(
        &mut self,
        core: CoreId,
        update: impl Fn(&mut GroupTradeSettings),
    ) {
        let group = self
            .config
            .servers
            .iter()
            .find(|s| s.id == core)
            .map(|s| s.group.clone());
        let seed = group.map(|group| self.group_trade_settings(&group));
        self.update_server(core, |server| {
            let trade = server
                .trade
                .get_or_insert_with(|| seed.clone().unwrap_or_default());
            update(trade);
        });
    }

    /// Resolve the core whose OWN manual-trading generation a write must land in, because the
    /// group's active trading core keeps one. `None` means the write proceeds group-local exactly
    /// as before. This is the ONE choke point every group-local writer
    /// (`set_order_size_sel`, `set_order_size_value`, `edit_group_exit`) checks, so a hotkey, a
    /// strip click, a Settings-panel input, and a metric popup cannot reach three different
    /// conclusions about which core (if any) a write must go to.
    ///
    /// Gates on [`Self::active_trade_core`] rather than the hover-aware chart display core the
    /// toolbar renders from: the toolbar's own strips additionally go non-interactive against the
    /// hover-aware core at render time (see [`Self::manual_display_matches_write`]), so the two
    /// together close the gap even in the narrow case where hovering a different chart in the
    /// same group briefly disagrees with this fallback.
    pub(crate) fn manual_write_core(&self, group: &str) -> Option<CoreId> {
        self.active_trade_core(group)
            .filter(|&core| self.core_own_trade(core))
    }

    /// Whether a manual-trading control seeded from `display_core` would write to the source it
    /// just showed.
    ///
    /// The toolbar's displayed core (`toolbar::effective_chart_display_core`) is hover-aware,
    /// while every write targets [`Self::manual_write_core`], which is not: hovering a chart whose
    /// core differs from the group's active trading core — with either or both opted into the
    /// per-core route — can make the two disagree while the strip is still rendered as live. A
    /// control this answers `false` for must go non-interactive rather than mutate a source other
    /// than the one on screen (goal A2 FIX-3): a disabled control with a reason beats a live
    /// control that silently writes elsewhere.
    ///
    /// Args:
    ///     group: Window group whose manual-trading controls are being gated.
    ///     display_core: The hover-aware core the toolbar is currently showing values from.
    ///
    /// Returns:
    ///     `true` when the displayed source and the write target agree — both group-local, or the
    ///     same core.
    pub(crate) fn manual_display_matches_write(
        &self,
        group: &str,
        display_core: Option<CoreId>,
    ) -> bool {
        let display_target = display_core.filter(|&core| self.core_own_trade(core));
        display_target == self.manual_write_core(group)
    }

    /// One resolver for the toolbar, hotkeys, and every metric popup: the effective F1-F6 sizes,
    /// selected slot, and their source, so no caller reaches a different conclusion than another.
    ///
    /// `core` is the chart display core, or `None` with no chart addressed. `GroupLocal` is
    /// returned only when the opt-in is off (or `core` is `None`); once it is on the source is
    /// always `Core`, using a neutral placeholder while genuinely `Awaiting` rather than silently
    /// reverting to group-local numbers that would look like the checkbox is off.
    pub(crate) fn effective_order_size_state(
        &self,
        group: &str,
        core: Option<CoreId>,
    ) -> ([f64; 6], usize, ManualSource) {
        if let Some(trade) = core.and_then(|core| self.core_trade_settings(core)) {
            return (
                trade.order_sizes_usd,
                trade.order_size_sel.min(trade.order_sizes_usd.len() - 1),
                ManualSource::CoreOwn,
            );
        }
        let (sizes, sel) = self.manual_order_size_state(group);
        (sizes, sel, ManualSource::GroupLocal)
    }

    /// Resolve the group's F1-F6 USD-equivalent presets through the SAME source
    /// [`Self::set_order_size_value`] / [`Self::set_order_size_sel`] will write to.
    ///
    /// Every wheel-step or inline-editor seed that feeds one of those writes reads THIS, never
    /// [`Self::manual_order_size_state`] directly: a relative edit (Ctrl+wheel) must be computed
    /// against the value about to be overwritten, not against the group's generation while the
    /// write lands on the core's (goal A2 FIX-1).
    ///
    /// Args:
    ///     group: Window group whose write-target presets are requested.
    ///
    /// Returns:
    ///     The six presets and selected slot from [`Self::manual_write_core`]'s source, or the
    ///     group-local generation while that source is `None`.
    pub(crate) fn write_aligned_order_sizes(&self, group: &str) -> ([f64; 6], usize) {
        let (sizes, sel, _source) =
            self.effective_order_size_state(group, self.manual_write_core(group));
        (sizes, sel)
    }

    /// Exit twin of [`Self::effective_order_size_state`]: the core's retained
    /// `ClientSettings::group_exit_settings()` when the opt-in is on and available, else the
    /// group-local exit generation. Reuses existing machinery entirely — no new projection.
    pub(crate) fn effective_group_exit(
        &self,
        group: &str,
        core: Option<CoreId>,
    ) -> (GroupExitSettings, ManualSource) {
        if let Some(trade) = core.and_then(|core| self.core_trade_settings(core)) {
            return (trade.exit, ManualSource::CoreOwn);
        }
        (self.group_exit_settings(group), ManualSource::GroupLocal)
    }

    /// Resolve the group's complete exit generation through the SAME source
    /// [`Self::edit_group_exit`] will write to.
    ///
    /// Every TP/SL/S-slot reader that feeds a subsequent [`Self::edit_group_exit`] write — popup
    /// seeding, the Extended-TP toggle, the stop-market checkbox, S-slot wheel and inline editing
    /// — reads THIS, never [`Self::group_exit_settings`] directly, so it can never be computed
    /// against a different generation than the one about to be overwritten (goal A2 FIX-2).
    ///
    /// Args:
    ///     group: Window group whose write-target exit generation is requested.
    ///
    /// Returns:
    ///     The exit generation from [`Self::manual_write_core`]'s source, or the group-local
    ///     generation while that source is `None`.
    pub(crate) fn write_aligned_group_exit(&self, group: &str) -> GroupExitSettings {
        self.effective_group_exit(group, self.manual_write_core(group))
            .0
    }

    /// Select an F1-F6 USD-equivalent preset for one group, or for the group's active core when
    /// that core keeps its own generation: the choke point re-targets the write onto the core's
    /// own set rather than the group's.
    pub(crate) fn set_order_size_sel(&mut self, group: &str, ix: usize) {
        if ix >= 6 {
            return;
        }
        if let Some(core) = self.manual_write_core(group) {
            self.update_core_trade(core, |trade| trade.order_size_sel = ix);
            return;
        }
        self.update_group_trade(group, |trade| trade.order_size_sel = ix);
    }

    /// Resolve the USD-equivalent order size for a real order about to reach `core`, refusing
    /// rather than guessing when the core route is on but has not yet reported a real value.
    ///
    /// Unlike [`Self::effective_order_size_state`] (which renders a neutral placeholder while
    /// `Awaiting`, correct for a toolbar that must draw six cells regardless), a placeholder here
    /// would size a real order from `DEFAULT_ORDER_SIZES_USD` while the user believes it is sized
    /// from their configured core. `None` therefore means "do not place this order", not "show 0".
    pub(super) fn effective_order_size_usd_for_order(
        &self,
        group: &str,
        core: CoreId,
    ) -> Option<f64> {
        let (sizes, sel, _source) = self.effective_order_size_state(group, Some(core));
        Some(sizes[sel])
    }

    /// Exit twin of [`Self::effective_order_size_usd_for_order`]: the core's own retained exit
    /// generation when the core route is on, refusing rather than sending a blank/default exit
    /// when the core has never reported `ClientSettings`.
    pub(super) fn effective_group_exit_for_order(
        &self,
        group: &str,
        core: CoreId,
    ) -> Option<GroupExitSettings> {
        Some(self.effective_group_exit(group, Some(core)).0)
    }
}
