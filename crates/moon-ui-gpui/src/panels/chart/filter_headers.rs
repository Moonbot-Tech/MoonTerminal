//! Route a strategy-filter header press and a blacklist-line press.

use gpui::prelude::*;
use gpui::{App, Context, CursorStyle, Div, SharedString, Stateful, Window, div, px};
use moon_ui::MoonPalette;
use rust_i18n::t;

use super::ChartPanel;
use crate::chartdx::FilterLineTarget;
use crate::controls::coin_menu::core_ref::{
    core_action_allowed, core_action_group, core_session_name,
};
use crate::design::{moon, moon_alpha};

/// Dictionary key for the tooltip of one blacklist line.
///
/// Args:
///     target: Parsed line. The coin and the strategy name are not part of the key.
///
/// Returns:
///     `chart_labels.filter.open_core_bl` or `chart_labels.filter.open_strategy_bl`.
pub(super) fn filter_line_tooltip_key(target: &FilterLineTarget) -> &'static str {
    match target {
        FilterLineTarget::GlobalBlacklist { .. } => "chart_labels.filter.open_core_bl",
        FilterLineTarget::StrategyBlacklist { .. } => "chart_labels.filter.open_strategy_bl",
    }
}

impl ChartPanel {
    /// Consume a header press before chart gestures; coordinates arrive in slot device pixels.
    pub(super) fn try_toggle_strategy_filters(
        &mut self,
        pos: (f32, f32),
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane) = self.input.pane_at(pos.0, pos.1) else {
            return false;
        };
        let Some((origin, scale, _)) = self.chart_origin_logical() else {
            return false;
        };
        let mut cfg = self.effective_labels(cx);
        let Some(row_ix) = self.chart.filter_header_at(
            pane,
            pos.0 / scale + origin.0,
            pos.1 / scale + origin.1,
            &cfg,
        ) else {
            return false;
        };
        let Some(row) = cfg
            .rows
            .get_mut(row_ix)
            .filter(|row| row.draws_strategy_filters())
        else {
            return false;
        };
        row.collapsed = !row.collapsed;
        // This relay applies the change now and lets the owning docked/detached host persist it.
        self.apply_labels_edit(cfg, cx);
        true
    }

    /// Open the blacklist a drawn filter line names.
    ///
    /// A global line posts `Backend.core_settings_goto` for the pane's own core. A strategy line
    /// opens that strategy's `CoinsBlackList` field, by id when the name is in the store and by
    /// name otherwise. Authorization uses the pane core's own workspace group. A refused core
    /// still consumes the press so it does not fall through into an order.
    ///
    /// Args:
    ///     pos: Point in the chart's slot device pixels.
    ///     window: Window that owns the chart, used as the Strategies window's owner.
    ///     cx: Panel context.
    ///
    /// Returns:
    ///     Whether a drawn blacklist line was under the point.
    pub(super) fn try_open_filter_line(
        &mut self,
        pos: (f32, f32),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane) = self.input.pane_at(pos.0, pos.1) else {
            return false;
        };
        let Some((origin, scale, _)) = self.chart_origin_logical() else {
            return false;
        };
        let cfg = self.effective_labels(cx);
        let Some(target) = self.chart.filter_line_at(
            pane,
            pos.0 / scale + origin.0,
            pos.1 / scale + origin.1,
            &cfg,
        ) else {
            return false;
        };
        let Some((core, _)) = self.chart.pane_target(pane) else {
            log::info!("filter line press skipped: pane {pane} has no core");
            return true;
        };
        let group = {
            let backend = self.backend.read(cx);
            if !core_action_allowed(backend, core) {
                log::info!("filter line press skipped: core {core} is not allowed");
                return true;
            }
            core_action_group(backend, core)
        };
        let Some(group) = group else {
            log::info!("filter line press skipped: core {core} has no group");
            return true;
        };
        match target {
            FilterLineTarget::GlobalBlacklist { coin } => {
                self.backend.update(cx, |backend, bcx| {
                    backend.core_settings_goto =
                        Some(crate::shell::CoreSettingsReveal { core, coin });
                    bcx.notify();
                });
            }
            FilterLineTarget::StrategyBlacklist { strategy, coin } => {
                let sid = {
                    let backend = self.backend.read(cx);
                    backend.session.store().core(core).and_then(|data| {
                        data.strategies
                            .iter()
                            .find(|row| row.name == strategy)
                            .map(|row| row.id)
                    })
                };
                let field = crate::strategies::RevealField {
                    key: moon_core::feed::FIELD_COINS_BLACK_LIST,
                    coin,
                };
                let owner = Some(window.window_handle());
                let display = window.display(cx).map(|display| display.id());
                if let Some(sid) = sid {
                    crate::strategies::open_goto_field(
                        self.backend.clone(),
                        core,
                        sid,
                        field,
                        Some(group),
                        owner,
                        display,
                        cx,
                    );
                } else {
                    crate::strategies::open_goto_named_field(
                        self.backend.clone(),
                        core,
                        strategy,
                        field,
                        Some(group),
                        owner,
                        display,
                        cx,
                    );
                }
            }
        }
        true
    }

    /// Pointing-hand zones over blacklist lines, with an amber tint and underline on the hot one.
    ///
    /// The chart draws these captions on the GPU and has no tooltip channel there. The zones are
    /// the same overlay the filter header uses for its cursor, plus
    /// [`crate::panels::common::text_tooltip_value`] for the sentence. They carry no click
    /// handler: the press is routed by [`Self::try_open_filter_line`].
    ///
    /// Args:
    ///     cx: Application context for the palette, the dictionary, and the core's display name.
    ///
    /// Returns:
    ///     One absolutely placed zone per drawn line. The id is what lets the zone carry a tooltip.
    pub(super) fn filter_line_cursor_zones(&self, cx: &App) -> Vec<Stateful<Div>> {
        let mut out = Vec::new();
        let Some((origin, _, zoom)) = self.chart_origin_logical() else {
            return out;
        };
        let hot = self.input.cursor.and_then(|pos| {
            let (origin, scale, _) = self.chart_origin_logical()?;
            Some((pos.0 / scale + origin.0, pos.1 / scale + origin.1))
        });
        let palette = MoonPalette::active(cx);
        for pane in 0..self.chart.pane_count() {
            let Some((core, _)) = self.chart.pane_target(pane) else {
                continue;
            };
            let core_name = {
                let backend = self.backend.read(cx);
                if !core_action_allowed(backend, core) {
                    continue;
                }
                core_session_name(backend, core)
            };
            for (ix, (rect, target)) in self.chart.filter_line_draws(pane).into_iter().enumerate() {
                let [x, y, w, h] = rect;
                if w <= 0.0 || h <= 0.0 {
                    continue;
                }
                let hot_here =
                    hot.is_some_and(|(px, py)| px >= x && px <= x + w && py >= y && py <= y + h);
                let tip = filter_line_tip(&target, &core_name);
                out.push(
                    div()
                        .id(SharedString::from(format!("chart-filter-line-{pane}-{ix}")))
                        .absolute()
                        .left(px((x - origin.0) / zoom))
                        .top(px((y - origin.1) / zoom))
                        .w(px(w / zoom))
                        .h(px(h / zoom))
                        .cursor(CursorStyle::PointingHand)
                        .when(hot_here, |zone| {
                            zone.bg(moon_alpha(palette.amber, 0.16))
                                .border_b(px(1.0))
                                .border_color(moon(palette.amber))
                        })
                        .tooltip(crate::panels::common::text_tooltip_value(tip)),
                );
            }
        }
        out
    }
}

/// The sentence shown while the pointer rests on a blacklist line.
///
/// Args:
///     target: Parsed line. A strategy line names that strategy; a global line names the core.
///     core_name: Display name of the pane's core.
///
/// Returns:
///     The localized tooltip.
fn filter_line_tip(target: &FilterLineTarget, core_name: &str) -> String {
    let key = filter_line_tooltip_key(target);
    match target {
        FilterLineTarget::GlobalBlacklist { .. } => t!(key, core = core_name).to_string(),
        FilterLineTarget::StrategyBlacklist { strategy, .. } => {
            t!(key, name = strategy).to_string()
        }
    }
}

#[cfg(test)]
mod tests;
