//! Terminal-local trade sounds, keyed by the same platform/DEX identity as the session roster.

use serde::{Deserialize, Serialize};

use crate::feed::ExchangeId;

impl crate::config::layout::WindowLayout {
    /// Read the trade gain without rewriting missing or hand-edited preferences.
    /// Missing values keep today's loudness; playback and the slider share the 0-100 range.
    pub fn trade_sound_volume_percent(&self) -> u8 {
        self.trade_sound_volume.unwrap_or(100).min(100) as u8
    }
}

/// Separate entry and exit sounds; an empty stem is an explicit, persisted mute for that edge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TradeSounds {
    /// Embedded sound stem played on the first observed entry fill.
    pub open: String,
    /// Embedded sound stem played on a successful completed exit.
    pub close: String,
}

impl Default for TradeSounds {
    /// Missing settings enable distinct sounds without rewriting an explicitly muted edge.
    fn default() -> Self {
        Self {
            open: "ringin".into(),
            close: "ringout".into(),
        }
    }
}

/// Stable TOML map key, independent of captions, core ordering and connection timing.
pub fn exchange_key(id: ExchangeId) -> String {
    format!("{}:{}", id.code, id.dex)
}

#[cfg(test)]
mod tests;
