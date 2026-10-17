//! Search-button timing and long-run confirmation, calibrated by actual restart throughput.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use gpui::*;
use moon_ui::{MoonButton, MoonButtonVariant, MoonPalette, MoonWindowExt as _, h_flex};
use rust_i18n::t;

use super::super::super::AnalyticsView;
use super::model_cfg;
use super::variants::{passes_of, restarts_of};
use crate::design;
use crate::design::moon;
use moon_core::db::tuner::ticks::params::ParamGroup;
use moon_core::db::tuner::ticks::search::{SearchParams, SearchSize, full_replays, search_size};
use moon_core::db::tuner::ticks::{ModelSettings, TICK_PARAMS};

/// A search estimated to run longer than this asks before it starts.
pub(super) const LONG_SEARCH: Duration = Duration::from_secs(10 * 60);

/// Inputs that normalize a measured restart to another sample and search scope.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::analytics::tuner) struct CostKey {
    /// Training deals multiplied by estimated replay work per restart.
    work: f64,
    /// Conservative startup seconds per restart, before any observation exists.
    fallback: f64,
    /// The model's settings every replay runs under.
    pub model: ModelSettings,
    /// Restarts run side by side ([`side_by_side`]).
    pub parallel: usize,
}

/// Last running or finished search in this process, retained across panel and sample reloads.
static LAST_THROUGHPUT: Mutex<Option<(CostKey, Duration)>> = Mutex::new(None);

impl CostKey {
    /// Normalize scope work without relying on a guessed cost of a baseline replay.
    fn new(
        n: usize,
        train_pct: usize,
        starts: usize,
        size: &SearchSize,
        model: ModelSettings,
    ) -> Self {
        let starts = starts.max(1) as f64;
        Self {
            work: n as f64 * train_pct as f64 / 100.0
                * full_replays(size.points, size.points - size.fresh)
                / starts,
            // The reported 293-deal, 500-start run took 13 minutes under CPU contention.
            // Round up to 15 minutes and scale nested searches by their entry work as well.
            fallback: 900.0 / 500.0 * n as f64 / 293.0
                * (size.entry_fields + size.exit_fields).max(27) as f64
                / 27.0
                * (size.entry_points / starts).max(1.0),
            model,
            parallel: side_by_side(starts as usize),
        }
    }

    /// Estimate a new run from measured seconds per restart, adjusting sample and scope work.
    fn estimate(self, starts: usize, observed: Option<(Self, Duration)>) -> Duration {
        let seconds = observed
            .filter(|(key, _)| key.model == self.model && key.work > 0.0)
            .map_or(self.fallback, |(key, per_start)| {
                per_start.as_secs_f64() * self.work / key.work * key.parallel as f64
                    / self.parallel as f64
            });
        Duration::from_secs_f64((seconds * starts as f64).clamp(0.0, u32::MAX as f64))
    }
}

/// Shared restart-throughput measurement used both by startup prediction and running ETA.
pub(super) fn observed_start_cost(done: usize, elapsed: Duration) -> Option<Duration> {
    (done > 0 && !elapsed.is_zero()).then(|| elapsed.div_f64(done as f64))
}

/// Publish only completed restart work, using the same elapsed clock as the running ETA.
pub(super) fn take_search_cost(key: Option<CostKey>, done: usize, elapsed: Duration) {
    if let (Some(key), Some(cost)) = (key, observed_start_cost(done, elapsed)) {
        *LAST_THROUGHPUT.lock().unwrap_or_else(|e| e.into_inner()) = Some((key, cost));
    }
}

/// Update remaining time using the same measured restart cost as the search button.
pub(super) fn remaining_time(
    done: usize,
    total: usize,
    elapsed: Duration,
    estimate: Option<Duration>,
) -> Option<Duration> {
    let remaining = total.checked_sub(done).filter(|&n| n > 0)?;
    if let Some(cost) = observed_start_cost(done, elapsed) {
        Some(cost.mul_f64(remaining as f64))
    } else {
        estimate
            .and_then(|d| d.checked_sub(elapsed))
            .filter(|d| !d.is_zero())
    }
}

/// How many of a search's restarts run side by side: the search pool's threads bound them,
/// not every CPU of the machine.
fn side_by_side(restarts: usize) -> usize {
    side_by_side_on(
        restarts,
        moon_core::db::tuner::threshold_search::search_threads(),
    )
}

/// [`side_by_side`] on a pool of `threads` threads.
fn side_by_side_on(restarts: usize, threads: usize) -> usize {
    restarts.clamp(1, threads.max(1))
}

/// What one search varies: whether each group is searched, and the fields it holds.
pub(super) struct Scope {
    pub vary_entry: bool,
    pub vary_exit: bool,
    pub locked: HashSet<String>,
}

impl AnalyticsView {
    /// The scope of a search of `only`, or of every ticked field: every ticked field of the
    /// searchable groups, or the one field with every other held. `Err` carries the
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
                    // Two causes, two answers: a kind whose entry is taken from the fact, or a
                    // group the model reproduced no trade of.
                    let no_entry_model = field.group == ParamGroup::Entry
                        && self.ticks.data.data().is_some_and(|d| !d.entry_modelled());
                    return Err(Some(if no_entry_model {
                        "analytics.ticks.sugg_no_entry_model"
                    } else {
                        "analytics.ticks.sugg_unanswered"
                    }));
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
            risk: self.ticks.risk_limits(),
            screen_entry: self.ticks.screen_entry,
        }))
    }

    /// Capture the launched workload so later UI edits cannot re-label its throughput.
    pub(super) fn ticks_cost_key(&self, size: &SearchSize, starts: usize) -> Option<CostKey> {
        let n = self.ticks.data.data()?.replayable().count();
        (n > 0).then(|| CostKey::new(n, self.ticks.train_pct, starts, size, model_cfg::current()))
    }

    /// Price a run from the last process observation, or a conservative first-run default.
    pub(super) fn ticks_search_time(&self, size: &SearchSize) -> Option<Duration> {
        let starts = restarts_of(&self.ticks.iters);
        let key = self.ticks_cost_key(size, starts)?;
        let observed = *LAST_THROUGHPUT.lock().unwrap_or_else(|e| e.into_inner());
        Some(key.estimate(starts, observed))
    }

    /// Format the shared throughput prediction for the button using the existing rounding.
    pub(super) fn ticks_estimate_time_text(&self) -> Option<String> {
        self.ticks_search_time(&self.ticks_search_size(None)?)
            .map(duration_text)
    }

    /// Keep the point-count breakdown on hover instead of a separate clipped estimate row.
    pub(super) fn ticks_estimate_tip(&self) -> Option<String> {
        let size = self.ticks_search_size(None)?;
        let time = self.ticks_search_time(&size).map_or_else(
            || t!("analytics.ticks.est_time_pending").to_string(),
            duration_text,
        );
        let text = if size.nested() {
            t!(
                "analytics.ticks.est_nested",
                points = count_text(size.points),
                entry = count_text(size.entry_points),
                time = time
            )
        } else {
            t!(
                "analytics.ticks.est_line",
                points = count_text(size.points),
                time = time
            )
        };
        Some(format!("{text}\n\n{}", t!("analytics.ticks.est_tip")))
    }

    /// Ask before a search estimated past [`LONG_SEARCH`] — or a nested one without a sample —
    /// how long it will take; answers whether it asked, the search then waits
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

/// Include the predicted duration once when a searchable sample exists.
pub(super) fn pick_all_label(time: Option<&str>) -> String {
    match time {
        Some(time) => t!("analytics.ticks.pick.all_est", time = time).to_string(),
        None => t!("analytics.ticks.pick.all").to_string(),
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
pub(in crate::analytics::tuner) fn duration_text(duration: Duration) -> String {
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
