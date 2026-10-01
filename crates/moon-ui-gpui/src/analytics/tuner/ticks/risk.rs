//! The search's risk limits in the settings popover: how much deeper a drawdown and how much
//! lower a win rate than the fact on the training deals the answer may have
//! (`search::RiskLimits`). Each box holds a per cent; an empty one is the search's default.

use gpui::*;
use moon_ui::{MoonInput, MoonPalette};
use rust_i18n::t;

use super::super::super::AnalyticsView;
use super::cfg::popup_row;
use super::state::TicksState;
use crate::design;
use moon_core::db::tuner::ticks::search::{DEFAULT_WORSE_PCT, RiskLimits};

impl TicksState {
    /// The limits the search runs under, out of the two boxes.
    pub(in crate::analytics::tuner) fn risk_limits(&self) -> RiskLimits {
        RiskLimits {
            drawdown_pct: Some(worse_pct(&self.dd_worse_pct)),
            winrate_pct: Some(worse_pct(&self.wr_worse_pct)),
        }
    }
}

/// One box's per cent: the typed number, else [`DEFAULT_WORSE_PCT`].
fn worse_pct(text: &str) -> f64 {
    typed_pct(text).unwrap_or(DEFAULT_WORSE_PCT)
}

/// The per cent a box holds, as the layout keeps it — a decimal kept whole — or `None` for an
/// empty, unreadable or negative one.
pub(super) fn typed_pct(text: &str) -> Option<f64> {
    text.trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
}

impl AnalyticsView {
    /// The two rows of the limits, for the search section of the settings popover.
    pub(super) fn ticks_risk_rows(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> [AnyElement; 2] {
        let placeholder = DEFAULT_WORSE_PCT.to_string();
        let dd = self.ticks_text_input(
            "x-cfg-dd-worse",
            self.ticks.dd_worse_pct.clone(),
            placeholder.clone(),
            |this, value| this.ticks.dd_worse_pct = value,
            window,
            cx,
        );
        let wr = self.ticks_text_input(
            "x-cfg-wr-worse",
            self.ticks.wr_worse_pct.clone(),
            placeholder,
            |this, value| this.ticks.wr_worse_pct = value,
            window,
            cx,
        );
        let row = |id: &'static str, label: &str, tip: &str, state: &Entity<_>| {
            popup_row(
                t!(label).to_string(),
                Some(t!(tip, default = DEFAULT_WORSE_PCT).to_string()),
                div()
                    .w(design::font_w_px(cx, 76.0))
                    .flex_none()
                    .font_family(design::mono())
                    .child(
                        MoonInput::new(SharedString::from(id))
                            .state(state)
                            .size(design::INPUT_SIZE),
                    )
                    .into_any_element(),
                p,
                cx,
            )
        };
        [
            row(
                "tun-cfg-dd-worse-x",
                "analytics.ticks.cfg_dd_worse",
                "analytics.ticks.cfg_dd_worse_tip",
                &dd,
            ),
            row(
                "tun-cfg-wr-worse-x",
                "analytics.ticks.cfg_wr_worse",
                "analytics.ticks.cfg_wr_worse_tip",
                &wr,
            ),
        ]
    }
}

#[cfg(test)]
mod tests;
