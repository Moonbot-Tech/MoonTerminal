//! View-free account of the displayed variant, its search verdict and changed fields.

use std::collections::HashMap;
use std::time::Duration;

use gpui::{App, Context, Window};
use moon_core::db::tuner::ticks::TICK_PARAMS;
use moon_core::db::tuner::ticks::params::ParamGroup;
use moon_core::db::tuner::ticks::search::{MIN_HOLDOUT, MIN_SEARCH_DEALS, SearchResult};
use moon_ui::MoonPalette;
use rust_i18n::t;
use std::rc::Rc;

use super::super::shared::InlineWrite;
use super::estimate::duration_text;
use super::sections::GridSection;
use super::state::{NowValue, SuggState, TicksState};
use crate::analytics::AnalyticsView;
use crate::analytics::summary::fmt_signed_unit;

mod card;
pub(super) use card::{SummaryActs, summary_card_el};

/// Immutable inline consent, captured before the user presses the final write button.
pub(in crate::analytics::tuner) struct WriteConfirm {
    pub write: InlineWrite,
    pub name: String,
    pub core: String,
    pub n: usize,
    pub holdout_worse: bool,
    pub reason: Option<String>,
    pub warns: Vec<String>,
}

/// View-free text shown inside the single inline confirmation.
pub(super) struct ConfirmModel {
    pub title: String,
    pub reason: Option<String>,
    pub bold_reason: bool,
    pub warns: Vec<String>,
}

/// The decision supported by the current search, never by a superseded variant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    Idle,
    Running {
        done: usize,
        total: usize,
        left: Option<Duration>,
        points: usize,
    },
    Cannot(String),
    Apply,
    DontApply(Why),
    Unchecked(Unchecked),
}

/// A numerical reason why the found variant should not be applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Why {
    Train,
    HoldoutWorse,
    HoldoutOpen { k: usize },
    NoGain,
}

/// Why the displayed point has no sufficient independent check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Unchecked {
    Whole,
    Small(i64),
    HandEdited,
}

/// Apply requires a strict gain on both slices and a closed, sufficient holdout.
pub(super) fn verdict_of(
    running: Option<(usize, usize, Option<Duration>, usize)>,
    too_few: Option<usize>,
    refusal: Option<&str>,
    result: Option<&SearchResult>,
    has_changes: bool,
) -> Verdict {
    if let Some((done, total, left, points)) = running {
        return Verdict::Running {
            done,
            total,
            left,
            points,
        };
    }
    if let Some(n) = too_few.filter(|_| result.is_none()) {
        return Verdict::Cannot(
            t!("analytics.ticks.sum.too_few", n = n, min = MIN_SEARCH_DEALS).to_string(),
        );
    }
    let Some(r) = result else {
        return match refusal {
            Some(reason) => Verdict::Cannot(reason.to_string()),
            None if has_changes => Verdict::Unchecked(Unchecked::HandEdited),
            None => Verdict::Idle,
        };
    };
    if r.train.profit < r.fact_train.profit {
        return Verdict::DontApply(Why::Train);
    }
    let (Some(hold), Some(fact)) = (&r.holdout, &r.fact_holdout) else {
        return Verdict::Unchecked(Unchecked::Whole);
    };
    if hold.n < MIN_HOLDOUT {
        return Verdict::Unchecked(Unchecked::Small(hold.n));
    }
    if hold.profit < fact.profit {
        return Verdict::DontApply(Why::HoldoutWorse);
    }
    if r.holdout_open > 0 {
        return Verdict::DontApply(Why::HoldoutOpen { k: r.holdout_open });
    }
    if r.holdout_loses || r.train.profit <= r.fact_train.profit || hold.profit <= fact.profit {
        return Verdict::DontApply(Why::NoGain);
    }
    Verdict::Apply
}

/// Translate the tag independently of the explanatory sentence.
pub(super) fn tag(verdict: &Verdict) -> String {
    t!(match verdict {
        Verdict::Idle => "analytics.ticks.sum.tag_idle",
        Verdict::Running { .. } => "analytics.ticks.sum.tag_running",
        Verdict::Cannot(_) => "analytics.ticks.sum.tag_cannot",
        Verdict::Apply => "analytics.ticks.sum.tag_apply",
        Verdict::DontApply(_) => "analytics.ticks.sum.tag_dont",
        Verdict::Unchecked(_) => "analytics.ticks.sum.tag_unchecked",
    })
    .to_string()
}

/// Include moving entry-point work without adding an empty counter to a plain search.
pub(super) fn progress_text(done: usize, total: usize, points: usize) -> String {
    let text = t!("analytics.ticks.sum.progress", done = done, total = total).to_string();
    if points > 0 {
        format!(
            "{text} · {}",
            t!("analytics.ticks.sum.progress_points", n = points)
        )
    } else {
        text
    }
}

/// Explain the verdict using the launched sample and the tape that can still be fetched.
pub(super) fn sentence(
    verdict: &Verdict,
    train_n: usize,
    hold_n: usize,
    fetchable: usize,
) -> String {
    match verdict {
        Verdict::Idle => t!("analytics.ticks.sum.idle").to_string(),
        Verdict::Running { .. } if hold_n == 0 => {
            t!("analytics.ticks.sum.running_whole", n = train_n).to_string()
        }
        Verdict::Running { .. } => {
            t!("analytics.ticks.sum.running", n = train_n, hold = hold_n).to_string()
        }
        Verdict::Cannot(reason) => format!(
            "{reason} {}",
            t!("analytics.ticks.sum.cannot_help", n = fetchable)
        ),
        Verdict::Apply => t!("analytics.ticks.sum.apply").to_string(),
        Verdict::DontApply(Why::Train) => t!("analytics.ticks.sum.dont_train").to_string(),
        Verdict::DontApply(Why::HoldoutWorse) => t!("analytics.ticks.sum.dont_holdout").to_string(),
        Verdict::DontApply(Why::HoldoutOpen { k }) => {
            t!("analytics.ticks.sum.dont_open", n = k).to_string()
        }
        Verdict::DontApply(Why::NoGain) => t!("analytics.ticks.sum.no_gain").to_string(),
        Verdict::Unchecked(Unchecked::Whole) => t!("analytics.ticks.sum.whole").to_string(),
        Verdict::Unchecked(Unchecked::Small(n)) => {
            t!("analytics.ticks.sum.small", n = n, min = MIN_HOLDOUT).to_string()
        }
        Verdict::Unchecked(Unchecked::HandEdited) => t!("analytics.ticks.sum.hand").to_string(),
    }
}

/// One comparable search slice, with open profit kept outside its scored profit.
pub(super) struct ProfitRow {
    pub label: String,
    pub sub: String,
    pub n: i64,
    pub fact: f64,
    pub variant: f64,
    pub open_line: Option<String>,
}

/// One field whose displayed V1 value differs from the loaded strategy values.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ChangeRow {
    pub key: String,
    pub section: String,
    pub old: String,
    pub new: String,
}

/// Owned content for the summary card and for a layout probe without an Analytics entity.
pub(super) struct SummaryModel {
    pub title: String,
    pub scope_label: String,
    pub verdict: Verdict,
    pub tag: String,
    pub sentence: String,
    pub train: Option<ProfitRow>,
    pub holdout: Option<ProfitRow>,
    pub changed_header: String,
    pub changes: Vec<ChangeRow>,
    pub warnings: Vec<String>,
    pub details: Vec<(String, String)>,
    pub details_open: bool,
    pub write_enabled: bool,
    pub copy_visible: bool,
    pub can_reset: bool,
    pub confirm: Option<ConfirmModel>,
    pub write_error: Option<String>,
}

/// Render tape-end profit separately so it never appears to be part of the closed tally.
fn open_line(n: usize, profit: f64) -> Option<String> {
    (n > 0).then(|| {
        t!(
            "analytics.ticks.sum.open_line",
            n = n,
            profit = fmt_signed_unit(profit)
        )
        .to_string()
    })
}

/// Remove no-op edits and resolve section labels from descriptors or the loaded schema grid.
fn change_rows(
    changes: &[(String, String)],
    now: &HashMap<String, NowValue>,
    grid: &[GridSection],
    human_labels: bool,
) -> Vec<ChangeRow> {
    changes
        .iter()
        .filter_map(|(key, value)| {
            let old = match now.get(key) {
                Some(NowValue::Same(old)) if old == value => return None,
                Some(NowValue::Same(old)) => old.clone(),
                Some(NowValue::Differs) => t!("analytics.ticks.sum.varies").to_string(),
                None => String::from("—"),
            };
            let section = TICK_PARAMS
                .iter()
                .find(|p| p.key == key)
                .map(|p| p.section)
                .or_else(|| {
                    grid.iter()
                        .find(|s| s.rows.iter().any(|r| &r.key == key))
                        .map(|s| s.section)
                });
            Some(ChangeRow {
                key: key.clone(),
                section: section
                    .map(|s| {
                        crate::strategies::sections::section_display_title(
                            s.schema_title(),
                            human_labels,
                        )
                    })
                    .unwrap_or_default(),
                old,
                new: value.clone(),
            })
        })
        .collect()
}

impl SummaryModel {
    /// Build numerical rows and localised explanations exclusively from the displayed point.
    pub(super) fn from_state(state: &TicksState, scope_label: String, human_labels: bool) -> Self {
        let running = match &state.sugg {
            SuggState::Idle => None,
            SuggState::Running {
                handle,
                total,
                started,
                estimate,
                ..
            } => Some((
                handle.completed(),
                *total,
                remaining_time(handle.completed(), *total, started.elapsed(), *estimate),
                handle.points(),
            )),
        };
        let result = state.current_result();
        let mut verdict = verdict_of(
            running,
            state.search_too_few(),
            state.sugg_refusal.as_deref(),
            result,
            state.has_changes(),
        );
        if verdict == Verdict::Apply
            && let Some(n) = state.variant_too_few()
        {
            verdict = Verdict::Cannot(
                t!("analytics.ticks.var_too_few", n = n, min = MIN_SEARCH_DEALS).to_string(),
            );
        }
        let data = state.data.data();
        let (fit, hold) = match state.sugg {
            SuggState::Running { sample, .. } => sample,
            SuggState::Idle => (0, 0),
        };
        let train = result.map(|r| ProfitRow {
            label: t!("analytics.ticks.sum.train").to_string(),
            sub: t!("analytics.ticks.sum.train_sub", n = r.fact_train.n).to_string(),
            n: r.fact_train.n,
            fact: r.fact_train.profit,
            variant: r.train.profit,
            open_line: None,
        });
        let holdout = result.and_then(|r| {
            let (hold, fact) = (r.holdout.as_ref()?, r.fact_holdout.as_ref()?);
            Some(ProfitRow {
                label: t!("analytics.ticks.sum.hold").to_string(),
                sub: t!("analytics.ticks.sum.hold_sub", n = fact.n).to_string(),
                n: fact.n,
                fact: fact.profit,
                variant: hold.profit,
                open_line: open_line(r.holdout_open, r.holdout_open_profit),
            })
        });
        let changes = data
            .map(|d| change_rows(&state.variant_changes(), &d.now, &d.grid, human_labels))
            .unwrap_or_default();
        let mut warnings: Vec<String> = state.sugg_note.iter().cloned().collect();
        warnings.extend(open_line(state.var_open, state.var_open_profit));
        if let Some(n) = state.variant_too_few() {
            warnings
                .push(t!("analytics.ticks.var_too_few", n = n, min = MIN_SEARCH_DEALS).to_string());
        }
        if state.var_untraded > 0 {
            warnings.push(t!("analytics.ticks.var_untraded", n = state.var_untraded).to_string());
        }
        if let Some(r) = result
            && r.stats.left_open > 0
        {
            warnings
                .push(t!("analytics.ticks.search_base_open", n = r.stats.left_open).to_string());
        }
        let mut details = Vec::new();
        if let Some(r) = result {
            let stats = &r.stats;
            for (key, value) in [
                ("analytics.ticks.sum.d_starts", stats.restarts),
                ("analytics.ticks.sum.d_distinct", stats.distinct),
                ("analytics.ticks.sum.d_best", stats.best_restart),
                ("analytics.ticks.sum.d_refused", stats.refused),
                ("analytics.ticks.sum.d_evals", stats.evaluations),
                ("analytics.ticks.sum.d_entry_points", stats.entry_points),
                ("analytics.ticks.sum.d_left_open", stats.left_open),
            ] {
                details.push((t!(key).to_string(), value.to_string()));
            }
            let passes = if stats.converged {
                t!("analytics.ticks.stats_converged", n = stats.passes)
            } else {
                t!("analytics.ticks.stats_cut", n = stats.passes)
            };
            details.push((
                t!("analytics.ticks.sum.d_passes").to_string(),
                passes.to_string(),
            ));
            if let Some(elapsed) = state.last_search_elapsed {
                details.push((
                    t!("analytics.ticks.sum.d_time").to_string(),
                    duration_text(elapsed),
                ));
            }
        }
        let bold_reason = matches!(verdict, Verdict::DontApply(_));
        Self {
            title: t!("analytics.ticks.sum.title").to_string(),
            scope_label,
            tag: tag(&verdict),
            sentence: sentence(
                &verdict,
                fit,
                hold,
                data.map_or(0, |d| d.fetchable().count()),
            ),
            verdict,
            train,
            holdout,
            changed_header: t!("analytics.ticks.sum.changed", n = changes.len()).to_string(),
            changes,
            warnings,
            details,
            details_open: state.details_open,
            write_enabled: false,
            copy_visible: false,
            can_reset: state.has_changes(),
            confirm: state.write_confirm.as_ref().map(|confirm| {
                let title = if confirm.write.targets.len() > 1 {
                    t!(
                        "analytics.ticks.sum.confirm_bulk",
                        n = confirm.n,
                        targets = confirm.write.targets.len()
                    )
                    .to_string()
                } else {
                    t!(
                        "analytics.ticks.sum.confirm",
                        n = confirm.n,
                        name = confirm.name.clone(),
                        core = confirm.core.clone()
                    )
                    .to_string()
                };
                ConfirmModel {
                    title,
                    bold_reason,
                    reason: if confirm.holdout_worse {
                        Some(t!("analytics.ticks.sum.confirm_worse").to_string())
                    } else {
                        confirm.reason.clone()
                    },
                    warns: confirm.warns.clone(),
                }
            }),
            write_error: None,
        }
    }
}

/// Update remaining time from observed restart throughput once any restart has completed.
/// Before that, an exhausted startup estimate is unknown rather than a permanent one second.
fn remaining_time(
    done: usize,
    total: usize,
    elapsed: Duration,
    estimate: Option<Duration>,
) -> Option<Duration> {
    let remaining = total.checked_sub(done).filter(|&n| n > 0)?;
    if done > 0 {
        Some(elapsed.mul_f64(remaining as f64 / done as f64))
    } else {
        estimate
            .and_then(|d| d.checked_sub(elapsed))
            .filter(|d| !d.is_zero())
    }
}

impl AnalyticsView {
    /// Add existing model and target warnings to the view-free summary without write-time checks.
    pub(super) fn ticks_summary_model(&self, cx: &App) -> SummaryModel {
        let human = crate::strategies::settings::human_labels(&self.backend.read(cx).layout);
        let mut model = SummaryModel::from_state(&self.ticks, self.scope_label(), human);
        model.write_enabled = !self.selected_targets().is_empty() && self.ticks.has_changes();
        model.copy_visible = !self.is_multi();
        model.write_error = self.write_error.clone();
        if let Some(data) = self.ticks.data.data() {
            for group in [ParamGroup::Entry, ParamGroup::Exit] {
                model.warnings.extend(
                    self.ticks_group_note(group, data, MoonPalette::active(cx))
                        .map(|(note, _)| note),
                );
            }
        }
        model
            .warnings
            .push(t!("analytics.ticks.assumptions").to_string());
        model
            .warnings
            .extend(self.ticks_unmodelled_warns(&self.selected_targets(), cx));
        model
    }

    /// Bind the view-free card to updates without reading this entity inside its own update.
    pub(super) fn ticks_summary_card(
        &self,
        host_w: gpui::Pixels,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        let model = self.ticks_summary_model(cx);
        let status = match &model.verdict {
            Verdict::Running {
                done,
                total,
                points,
                ..
            } => format!(
                "{} {}",
                model.sentence,
                progress_text(*done, *total, *points)
            ),
            _ => model.sentence.clone(),
        };
        super::variants::probe_painted(&status);
        let view = cx.entity();
        let act = |f: fn(&mut AnalyticsView, &mut Context<AnalyticsView>)| {
            let view = view.clone();
            Rc::new(move |_: &mut Window, app: &mut App| view.update(app, f))
                as Rc<dyn Fn(&mut Window, &mut App)>
        };
        let copy = view.clone();
        let acts = SummaryActs {
            write: act(|this, cx| this.ticks_begin_write(cx)),
            confirm: act(|this, cx| this.ticks_confirm_write(cx)),
            cancel: act(|this, cx| {
                this.ticks.write_confirm = None;
                cx.notify();
            }),
            copy: Rc::new(move |window, app| {
                copy.update(app, |this, cx| this.ticks_open_copy_dialog(window, cx))
            }),
            reset: act(|this, cx| this.ticks_clear_variant(cx)),
            stop: act(|this, cx| this.ticks_stop_suggest(cx)),
            details: act(|this, cx| {
                this.ticks.details_open = !this.ticks.details_open;
                cx.notify();
            }),
        };
        let arrow = super::grid::ranges_fit(host_w - crate::design::ui_px(cx, 2.0), cx);
        summary_card_el(&model, &acts, arrow, p, cx)
    }

    /// Prepare inline consent and warning checks once, without opening another dialog.
    pub(super) fn ticks_begin_write(&mut self, cx: &mut Context<Self>) {
        let targets = self.selected_targets();
        let changes = self.ticks.variant_changes();
        let Some(target) = targets.first() else {
            return;
        };
        if changes.is_empty() {
            return;
        }
        let name = crate::analytics::summary::strat_display(&target.name);
        let core = target
            .core
            .map(|uid| self.core_names.resolve(uid, &uid.to_string()).to_string())
            .unwrap_or_else(|| t!("analytics.cores_all").to_string());
        let model = self.ticks_summary_model(cx);
        let holdout_worse = matches!(model.verdict, Verdict::DontApply(Why::HoldoutWorse));
        let reason = (!matches!(model.verdict, Verdict::Apply)).then_some(model.sentence);
        let warns = self.ticks_write_warns(&targets, &changes, cx);
        let n = changes.len();
        let write = self.begin_inline_write(targets, changes, cx);
        self.ticks.write_confirm = Some(WriteConfirm {
            write,
            name,
            core,
            n,
            holdout_worse,
            reason,
            warns,
        });
        cx.notify();
    }

    /// Consume the one consent, dropping it without a write when authority or V1 changed.
    pub(super) fn ticks_confirm_write(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.ticks.write_confirm.take() else {
            return;
        };
        self.commit_inline_write(confirm.write, cx);
    }
}

#[cfg(test)]
mod tests;
