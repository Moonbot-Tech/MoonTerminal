//! Snapshot trust accessors for retained core state.

use super::*;

impl CoreData {
    /// Best available trust classification for this core's `assets.global` USD figures.
    ///
    /// `Unpriced` outranks `Stale`: an unpriced figure has no number to show at all, so its
    /// freshness is moot. Staleness needs BOTH inputs — `assets_stale` covers the reconnect
    /// window (status returns to `Ready` before the new snapshot lands), while the `status`
    /// check covers a snapshot that arrived before the link ever reached `Ready`. The generation
    /// ambiguity documented on [`Self::assets_stale`] prevents this from proving freshness.
    pub fn balance_state(&self) -> BalanceState {
        if self.assets_rev == 0 {
            BalanceState::Awaiting
        } else if !self.assets.global.usd_rate_known {
            BalanceState::Unpriced
        } else if self.assets_stale || !matches!(self.status, ConnStatus::Ready) {
            BalanceState::Stale
        } else {
            BalanceState::Live
        }
    }

    /// Best available trust classification for this core's full safe-share configuration
    /// projection (`core_config`), mirroring [`Self::balance_state`]'s shape.
    pub fn core_config_state(&self) -> CoreConfigState {
        if self.core_config.is_none() {
            CoreConfigState::Awaiting
        } else if self.core_config_stale || !matches!(self.status, ConnStatus::Ready) {
            CoreConfigState::Stale
        } else {
            CoreConfigState::Live
        }
    }

    /// The projected page while — and only while — [`Self::core_config_state`] rates it `Live`:
    /// the one reading a surface may seed from, compare, or send.
    ///
    /// `Live` implies the page is present, so this is the classification and its consumer in one
    /// place rather than a `Live` check followed by a second `is_none` test at every call site.
    pub fn live_core_config(&self) -> Option<&CoreConfig> {
        match self.core_config_state() {
            CoreConfigState::Live => self.core_config.as_ref(),
            CoreConfigState::Awaiting | CoreConfigState::Stale => None,
        }
    }

    /// Best available trust classification for this core's compact client-settings snapshot
    /// (`client_settings`), mirroring [`Self::balance_state`]'s shape.
    pub fn client_settings_state(&self) -> CoreConfigState {
        if self.client_settings.is_none() {
            CoreConfigState::Awaiting
        } else if self.client_settings_stale || !matches!(self.status, ConnStatus::Ready) {
            CoreConfigState::Stale
        } else {
            CoreConfigState::Live
        }
    }
}
