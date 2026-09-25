//! What "Search all" will cost before it runs (LinKvo, 2026-09-25): the points it scores and,
//! at what one point costs on this sample, roughly how long — a line under the parameter grid,
//! and a question before a run longer than [`LONG_SEARCH`]. A search of both groups nests a whole
//! exit search under every entry point it scores (`moon_core::db::tuner::ticks::search`), and its
//! count is the product: minutes to hours where one group takes seconds.
//!
//! The count is the core's (`search_size`). The cost of a point is measured, never assumed, and
//! kept with what it was measured under ([`CostKey`]: the rows, the training share, the model,
//! the restarts side by side): replays of the training slice run side by side as a search runs
//! its restarts (`point_cost`), measured again whenever one of those moves, and taken from every
//! finished search — its time over the points it scored, the same quantity.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gpui::*;
use moon_ui::{MoonButton, MoonButtonVariant, MoonPalette, MoonWindowExt as _, h_flex};
use rust_i18n::t;

use super::super::super::AnalyticsView;
use super::model_cfg;
use super::tape::prepare_sample;
use super::variants::{passes_of, restarts_of};
use crate::design;
use crate::design::moon;
use moon_core::db::tuner::ticks::params::ParamGroup;
use moon_core::db::tuner::ticks::search::{SearchParams, SearchSize, point_cost, search_size};
use moon_core::db::tuner::ticks::{ModelSettings, TICK_PARAMS};

/// A search estimated to run longer than this asks before it starts.
pub(super) const LONG_SEARCH: Duration = Duration::from_secs(10 * 60);

/// How long the sample must stand still before its point cost is measured: rows join it one by
/// one while their tape is read, and each would start a measurement of its own.
const COST_DEBOUNCE: Duration = Duration::from_millis(600);

/// What a point cost is measured under: a change of any of them prices the point again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::analytics::tuner) struct CostKey {
    /// The load the rows came from (`TicksState::seq`): another scope of as many rows is
    /// another sample.
    pub load: u64,
    /// Replayable rows of the sample.
    pub n: usize,
    /// The training share, per cent: a point replays that slice.
    pub train_pct: usize,
    /// The model's settings every replay runs under.
    pub model: ModelSettings,
    /// Restarts run side by side ([`side_by_side`]).
    pub parallel: usize,
}

/// How many of a search's restarts run side by side: the machine's threads bound them.
fn side_by_side(restarts: usize) -> usize {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    restarts.clamp(1, threads)
}

/// What one search varies: whether each group is searched, and the fields it holds.
pub(super) struct Scope {
    pub vary_entry: bool,
    pub vary_exit: bool,
    pub locked: HashSet<String>,
}

impl AnalyticsView {
    /// The scope of a search of `only`, or of every ticked field: every ticked field of the
    /// groups the gate lets through, or the one field with every other held. `Err` carries the
    /// locale key of why it cannot start — `None` for a field the grid does not know.
    pub(super) fn ticks_search_scope(
        &self,
        only: Option<&'static str>,
    ) -> Result<Scope, Option<&'static str>> {
        let (vary_entry, vary_exit, locked) = match only {
            None => (
                self.ticks_group_searchable(ParamGroup::Entry),
                self.ticks_group_searchable(ParamGroup::Exit),
                self.ticks.locked.clone(),
            ),
            Some(key) => {
                let field = TICK_PARAMS.iter().find(|f| f.key == key).ok_or(None)?;
                if !model_cfg::current().entry_method.reads(key) {
                    return Err(Some("analytics.ticks.sugg_not_read"));
                }
                if !self.ticks_group_searchable(field.group) {
                    return Err(Some("analytics.ticks.sugg_gated"));
                }
                let locked = TICK_PARAMS
                    .iter()
                    .map(|f| f.key)
                    .filter(|k| *k != key)
                    .map(str::to_string)
                    .collect();
                (
                    field.group == ParamGroup::Entry,
                    field.group == ParamGroup::Exit,
                    locked,
                )
            }
        };
        if !(vary_entry || vary_exit) {
            return Err(Some("analytics.ticks.sugg_nothing"));
        }
        Ok(Scope {
            vary_entry,
            vary_exit,
            locked,
        })
    }

    /// The size of the search `only` names over the grids as they stand, or `None` when it
    /// cannot start or the scope holds more than one kind.
    pub(super) fn ticks_search_size(&self, only: Option<&'static str>) -> Option<SearchSize> {
        let scope = self.ticks_search_scope(only).ok()?;
        let kind = self.ticks.data.data()?.single_kind()?.to_string();
        let (grids, _) = self.ticks_search_grids();
        let (held, defaults) = (HashMap::new(), HashMap::new());
        Some(search_size(&SearchParams {
            held: &held,
            defaults: &defaults,
            kind: &kind,
            vary_entry: scope.vary_entry,
            vary_exit: scope.vary_exit,
            locked: &scope.locked,
            grids: &grids,
            restarts: restarts_of(&self.ticks.iters),
            min_n: None,
            seed: None,
            train_frac: 1.0,
            max_passes: passes_of(&self.ticks.passes),
            model: model_cfg::current(),
            keep_corridor: self.ticks.keep_corridor,
        }))
    }

    /// What a point cost measured now would be measured under — the rows, the training share,
    /// the model's settings and the restarts run side by side — or `None` before a sample.
    pub(super) fn ticks_cost_key(&self) -> Option<CostKey> {
        let n = self.ticks.data.data()?.replayable().count();
        (n > 0).then(|| CostKey {
            load: self.ticks.seq,
            n,
            train_pct: self.ticks.train_pct,
            model: model_cfg::current(),
            parallel: side_by_side(restarts_of(&self.ticks.iters)),
        })
    }

    /// Roughly how long a search of `size` runs, when a point's cost was measured under the
    /// settings as they stand.
    pub(super) fn ticks_search_time(&self, size: &SearchSize) -> Option<Duration> {
        let key = self.ticks_cost_key()?;
        let (at, cost) = self.ticks.point_cost?;
        (at == key).then(|| size.time(cost))
    }

    /// The line under the parameter grid: what "Search all" scores and roughly how long it
    /// takes, amber past [`LONG_SEARCH`]. Nothing when no search could start.
    pub(super) fn ticks_estimate_row(&self, p: MoonPalette, cx: &App) -> Option<AnyElement> {
        let size = self.ticks_search_size(None)?;
        let time = self.ticks_search_time(&size);
        let time_text = time.map_or_else(
            || t!("analytics.ticks.est_time_pending").to_string(),
            duration_text,
        );
        let text = if size.nested() {
            t!(
                "analytics.ticks.est_nested",
                points = count_text(size.points),
                entry = count_text(size.entry_points),
                time = time_text
            )
        } else {
            t!(
                "analytics.ticks.est_line",
                points = count_text(size.points),
                time = time_text
            )
        }
        .to_string();
        let long = time.is_some_and(|t| t > LONG_SEARCH);
        Some(
            div()
                .id("an-ticks-estimate")
                .w_full()
                .flex_none()
                .px(design::ui_px(cx, 12.0))
                .py(design::ui_px(cx, 4.0))
                .border_t_1()
                .border_color(moon(p.border))
                // Wrapped, left-aligned, two lines at most: the nested line does not fit one
                // (LinKvo, 2026-09-25). A tail past them ends in an ellipsis — the time leads the
                // line, so it is never the part cut — and the tooltip carries the whole line.
                .whitespace_normal()
                .text_left()
                .line_clamp(2)
                .text_ellipsis()
                .text_size(design::t_caption(cx))
                .font_family(design::ui_font())
                .text_color(moon(if long { p.amber } else { p.text_muted }))
                .tooltip(crate::panels::common::text_tooltip(format!(
                    "{text}\n\n{}",
                    t!("analytics.ticks.est_tip")
                )))
                .child(text)
                .into_any_element(),
        )
    }

    /// Measure what a point costs under the settings as they stand, once they stop moving —
    /// asked by every paint of the grid, so a change of the rows, the training share, the model
    /// or the restarts is priced without each of them saying so. A cost already measured or being
    /// measured under them, a sample still reading its tape, and a running search — which answers
    /// the cost itself, and would slow the measurement — are left alone.
    pub(super) fn ticks_measure_cost(&mut self, cx: &mut Context<Self>) {
        if self.ticks.tape_reading
            || matches!(self.ticks.sugg, super::state::SuggState::Running { .. })
        {
            return;
        }
        let Some(key) = self.ticks_cost_key() else {
            return;
        };
        if self.ticks.point_cost.is_some_and(|(at, _)| at == key)
            || self.ticks.cost_pending == Some(key)
        {
            return;
        }
        self.ticks.cost_pending = Some(key);
        self.ticks.cost_task = Some(cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(COST_DEBOUNCE).await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| this.run_ticks_cost(key, cx));
            });
        }));
    }

    /// Time the replay of the sample's training slice, off the UI thread. Settings that moved
    /// meanwhile, or a search started meanwhile — the two would have shared the pool — drop the
    /// answer; the next paint asks again.
    fn run_ticks_cost(&mut self, key: CostKey, cx: &mut Context<Self>) {
        let searching =
            |this: &Self| matches!(this.ticks.sugg, super::state::SuggState::Running { .. });
        let kind = self
            .ticks
            .data
            .data()
            .and_then(|d| d.single_kind().map(String::from));
        let (Some(kind), false, Some(true)) = (
            kind,
            searching(self),
            self.ticks_cost_key().map(|now| now == key),
        ) else {
            self.ticks.cost_pending = None;
            return;
        };
        let pending = self.prepared_deals();
        let defaults = self.filter_defaults(cx);
        let train_frac = super::super::filter::state::train_frac(key.train_pct);
        let seq = self.ticks.sugg_seq;
        self.spawn_db(
            false,
            cx,
            move || {
                let deals = prepare_sample(pending);
                point_cost(
                    &deals,
                    &defaults,
                    &kind,
                    key.model,
                    train_frac,
                    key.parallel,
                )
            },
            move |this, cost, cx| {
                if this.ticks.cost_pending == Some(key) {
                    this.ticks.cost_pending = None;
                }
                let still = this.ticks_cost_key() == Some(key)
                    && this.ticks.sugg_seq == seq
                    && !searching(this);
                if still && cost > Duration::ZERO {
                    this.ticks.point_cost = Some((key, cost));
                }
                cx.notify();
            },
        );
    }

    /// Take a finished search's time over the points it scored as the point cost under the
    /// settings it started with: it ran the restarts side by side, as the next search will. A
    /// search of a few points is left out — the replays it runs besides its points (the sample's
    /// filter, the base, the holdout) would weigh on the figure.
    pub(super) fn ticks_take_search_cost(
        &mut self,
        key: Option<CostKey>,
        elapsed: Duration,
        scored: usize,
    ) {
        /// Points a search must score before its time says what one costs.
        const MIN_SCORED: usize = 200;
        if let (Some(key), true) = (key, scored >= MIN_SCORED) {
            self.ticks.point_cost = Some((key, elapsed / scored.min(u32::MAX as usize) as u32));
        }
    }

    /// Ask before a search estimated past [`LONG_SEARCH`] — or a nested one whose point cost is
    /// not measured yet — how long it will take; answers whether it asked, the search then waits
    /// for Run.
    pub(super) fn ticks_confirm_long_search(
        &mut self,
        only: Option<&'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(size) = self.ticks_search_size(only) else {
            return false;
        };
        let time = self.ticks_search_time(&size);
        let body = match time {
            Some(time) if time > LONG_SEARCH => t!(
                "analytics.ticks.long_body",
                time = duration_text(time),
                points = count_text(size.points)
            ),
            None if size.nested() => t!(
                "analytics.ticks.long_body_unknown",
                points = count_text(size.points)
            ),
            _ => return false,
        }
        .to_string();
        let view = cx.entity();
        window.open_unique_moon_dialog("an-ticks-long-dialog", cx, move |dialog, _window, cx| {
            let p = MoonPalette::active(cx);
            let body = body.clone();
            let go = view.clone();
            dialog
                .w(design::font_w_px(cx, 420.0))
                .close_button(false)
                .overlay(true)
                .overlay_closable(true)
                .bg(moon(p.shell_high))
                .border_color(moon(p.border))
                .rounded(design::r_container(cx))
                .text_color(moon(p.text))
                .header(
                    div()
                        .w_full()
                        .py_2()
                        .border_b_1()
                        .border_color(moon(p.border))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t!("analytics.ticks.long_title").to_string()),
                )
                .content(move |content, _window, cx| {
                    content.child(
                        div()
                            .w_full()
                            .font_family(design::ui_font())
                            .text_size(design::t_body(cx))
                            .child(body.clone()),
                    )
                })
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap(design::ui_px(cx, 8.0))
                        .font_family(design::ui_font())
                        .child(
                            MoonButton::new("an-ticks-long-cancel")
                                .variant(MoonButtonVariant::Ghost)
                                .label(t!("dialogs.cancel").to_string())
                                .on_click(|_, window, cx| window.close_dialog(cx))
                                .render(),
                        )
                        .child(
                            MoonButton::new("an-ticks-long-go")
                                .variant(MoonButtonVariant::Blue)
                                .label(t!("analytics.ticks.long_go").to_string())
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    go.update(cx, |this, cx| {
                                        this.ticks_search_confirmed(only, window, cx)
                                    });
                                })
                                .render(),
                        ),
                )
        });
        true
    }
}

/// A count with its thousands apart: `4 456 380`.
fn count_text(value: f64) -> String {
    let digits = format!("{:.0}", value.max(0.0));
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push('\u{202f}');
        }
        out.push(c);
    }
    out
}

/// A duration as a person reads it: seconds under a minute, minutes under an hour, else hours
/// and minutes.
pub(super) fn duration_text(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs < 60 {
        t!("analytics.ticks.dur_s", s = secs.max(1)).to_string()
    } else if secs < 3600 {
        t!("analytics.ticks.dur_m", m = secs.div_ceil(60)).to_string()
    } else {
        t!(
            "analytics.ticks.dur_hm",
            h = secs / 3600,
            m = (secs % 3600) / 60
        )
        .to_string()
    }
}

#[cfg(test)]
mod tests;
