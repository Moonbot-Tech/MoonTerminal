//! Group (= window) properties and local manual-trading controls.

use serde::{Deserialize, Serialize};

use super::servers::default_true;

/// Default USD-equivalent order-size presets for F1-F6.
pub const DEFAULT_ORDER_SIZES_USD: [f64; 6] = [50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0];

/// Default selected order-size preset: F3.
fn default_order_size_sel() -> usize {
    2
}

/// Main take-profit representation used by the visible group controls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TakeProfitMode {
    /// Fine sub-percent scalp take profit.
    Scalp,
    /// Ordinary 1..=100 percent take profit.
    #[default]
    Normal,
    /// Extended 100..=900 percent take profit.
    Extended,
}

impl TakeProfitMode {
    /// Return the clamped and quantized visible TP percentage for an interactive edit.
    pub fn canonical_take_profit_pct(self, pct: f64) -> Option<f64> {
        if !pct.is_finite() {
            return None;
        }
        Some(match self {
            Self::Scalp => (pct * 50.0).round().clamp(0.0, 100.0) / 50.0,
            Self::Normal => pct.round().clamp(1.0, 100.0),
            Self::Extended => (pct / 10.0).round().clamp(10.0, 90.0) * 10.0,
        })
    }

    /// Return whether a persisted TP is inside the range this mode can canonicalize without clamp.
    fn accepts_persisted_take_profit_pct(self, pct: f64) -> bool {
        pct.is_finite()
            && match self {
                Self::Scalp => (0.0..=2.0).contains(&pct),
                Self::Normal => (1.0..=100.0).contains(&pct),
                Self::Extended => (100.0..=900.0).contains(&pct),
            }
    }

    /// Quantize a fixed-sell percentage to the protocol float without permitting overflow or NaN.
    pub fn canonical_fixed_sell_pct(self, pct: f64) -> Option<f64> {
        if !(pct.is_finite() && pct >= 0.0) {
            return None;
        }
        let quantized = if self == Self::Extended {
            f64::from((pct / 10.0) as f32) * 10.0
        } else {
            f64::from(pct as f32)
        };
        quantized.is_finite().then_some(quantized)
    }
}

/// Visible exit controls shared by every manual chart in one group.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GroupExitSettings {
    /// Main TP value displayed by the toolbar.
    pub take_profit_pct: f64,
    /// Encoding/range used for the main TP value.
    pub take_profit_mode: TakeProfitMode,
    /// Six visible fixed-sell percentages for S1-S6.
    pub fixed_sell_pcts: [f64; 6],
    /// Active fixed-sell slot in 1..=6, or `None` when the main TP is active.
    pub fixed_sell_slot: Option<usize>,
    /// Visible stop-loss percentage.
    pub stop_loss_pct: f32,
    /// Whether price-drop protection is enabled.
    pub stop_loss_enabled: bool,
    /// Whether a stop-market order is used.
    pub use_stop_market: bool,
}

impl Default for GroupExitSettings {
    /// Return a neutral exit generation that cannot close or protect a position by itself.
    fn default() -> Self {
        Self {
            take_profit_pct: 0.0,
            take_profit_mode: TakeProfitMode::Scalp,
            fixed_sell_pcts: [0.0; 6],
            fixed_sell_slot: None,
            stop_loss_pct: 0.0,
            stop_loss_enabled: false,
            use_stop_market: false,
        }
    }
}

impl GroupExitSettings {
    /// Clamp a finite stop-loss percentage to the protocol-supported visible range.
    pub fn canonical_stop_loss_pct(pct: f32) -> Option<f32> {
        pct.is_finite().then(|| pct.clamp(-20.0, 1.0))
    }

    /// Return whether a persisted stop loss is inside the visible protocol range without clamp.
    fn accepts_persisted_stop_loss_pct(pct: f32) -> bool {
        pct.is_finite() && (-20.0..=1.0).contains(&pct)
    }

    /// Canonicalize one complete exit generation, rejecting any corrupt persisted component.
    pub fn canonicalize(&mut self) -> bool {
        if !self
            .take_profit_mode
            .accepts_persisted_take_profit_pct(self.take_profit_pct)
            || !Self::accepts_persisted_stop_loss_pct(self.stop_loss_pct)
        {
            return false;
        }
        let Some(take_profit_pct) = self
            .take_profit_mode
            .canonical_take_profit_pct(self.take_profit_pct)
        else {
            return false;
        };
        self.take_profit_pct = take_profit_pct;
        for pct in &mut self.fixed_sell_pcts {
            let Some(canonical) = self.take_profit_mode.canonical_fixed_sell_pct(*pct) else {
                return false;
            };
            *pct = canonical;
        }
        if self
            .fixed_sell_slot
            .is_some_and(|slot| !(1..=6).contains(&slot))
        {
            return false;
        }
        let Some(stop_loss_pct) = Self::canonical_stop_loss_pct(self.stop_loss_pct) else {
            return false;
        };
        self.stop_loss_pct = stop_loss_pct;
        true
    }
}

/// Local manual-trading values persisted for one window group.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroupTradeSettings {
    /// F1-F6 order sizes expressed as USD equivalents.
    #[serde(default = "default_order_sizes_usd")]
    pub order_sizes_usd: [f64; 6],
    /// Selected F-key slot in 0..=5.
    #[serde(default = "default_order_size_sel")]
    pub order_size_sel: usize,
    /// Complete group-owned exit controls, independent from every core snapshot.
    #[serde(default)]
    pub exit: GroupExitSettings,
}

impl Default for GroupTradeSettings {
    /// Return safe USD presets and a complete neutral exit generation.
    fn default() -> Self {
        Self {
            order_sizes_usd: DEFAULT_ORDER_SIZES_USD,
            order_size_sel: default_order_size_sel(),
            exit: GroupExitSettings::default(),
        }
    }
}

impl GroupTradeSettings {
    /// Repair invalid persisted values and return whether anything changed.
    pub(crate) fn repair(&mut self) -> bool {
        let before = self.clone();
        for (index, size) in self.order_sizes_usd.iter_mut().enumerate() {
            if !(size.is_finite() && *size > 0.0) {
                *size = DEFAULT_ORDER_SIZES_USD[index];
            }
        }
        self.order_size_sel = self.order_size_sel.min(5);
        if !self.exit.canonicalize() {
            self.exit = GroupExitSettings::default();
        }
        *self != before
    }
}

/// Return the serde default for group USD-equivalent order sizes.
fn default_order_sizes_usd() -> [f64; 6] {
    DEFAULT_ORDER_SIZES_USD
}

/// Persisted properties and manual-trading state for one terminal window group.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroupConfig {
    /// Group name (key; matches `ServerConfig.group`).
    pub name: String,
    /// Whether the group is active. An inactive group's cores do not connect, and it has no window.
    #[serde(default = "default_true")]
    pub active: bool,
    /// Icon id in assets/icons (taskbar + group-window header).
    #[serde(default)]
    pub icon: u32,
    /// Manual-trading values shared by Main, docked, and detached charts in this group.
    #[serde(default)]
    pub trade: GroupTradeSettings,
}

impl GroupConfig {
    /// Create an active group with safe local manual-trading defaults.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            active: true,
            icon: 0,
            trade: GroupTradeSettings::default(),
        }
    }
}

#[cfg(test)]
mod tests;
