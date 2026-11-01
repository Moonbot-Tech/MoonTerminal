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
use super::estimate::{duration_text, remaining_time};
use super::sections::GridSection;
use super::state::{NowValue, SuggState, TicksState};
use crate::analytics::AnalyticsView;
use crate::analytics::summary::fmt_signed_unit;

mod card;
pub(super) use card::{SummaryActs, parameters_header, summary_card_el};

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
        /// Stop was pressed and the run is winding down to its best so far: no time left to
        /// show, and no second Stop to press.
        stopping: bool,
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
    HoldoutOpen {
        k: usize,
    },
    /// Held edits excluded deals that close under the current strategy settings.
    HeldOpen {
        n: usize,
    },
    NoGain,
}

/// Why the displayed point has no sufficient independent check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Unchecked {
    Whole,
    Small(i64),
    HandEdited,
}

/// Apply requires strict gains, a closed sufficient holdout, and no sample loss from held edits.
pub(super) fn verdict_of(
    running: Option<(usize, usize, Option<Duration>, usize, bool)>,
    too_few: Option<usize>,
    refusal: Option<&str>,
    result: Option<&SearchResult>,
    has_changes: bool,
) -> Verdict {
    if let Some((done, total, left, points, stopping)) = running {
        return Verdict::Running {
            done,
            total,
            left,
            points,
            stopping,
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
    if r.stats.left_open_by_held > 0 {
        return Verdict::DontApply(Why::HeldOpen {
            n: r.stats.left_open_by_held,
        });
    }
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

/// Refuse automatic advice when the scored variant closes too few deals.
pub(super) fn sample_verdict(verdict: Verdict, too_few: Option<usize>) -> Verdict {
    if verdict == Verdict::Apply
        && let Some(n) = too_few
    {
        Verdict::Cannot(
            t!("analytics.ticks.var_too_few", n = n, min = MIN_SEARCH_DEALS).to_string(),
        )
    } else {
        verdict
    }
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
        Verdict::DontApply(Why::HeldOpen { n }) => {
            t!("analytics.ticks.sum.dont_held_open", n = n).to_string()
        }
        Verdict::DontApply(Why::NoGain) => t!("analytics.ticks.sum.no_gain").to_string(),
        Verdict::Unchecked(Unchecked::Whole) => t!("analytics.ticks.sum.whole").to_string(),
        Verdict::Unchecked(Unchecked::Small(n)) => {
            t!("analytics.ticks.sum.small", n = n, min = MIN_HOLDOUT).to_string()
        }
        Verdict::Unchecked(Unchecked::HandEdited) => t!("analytics.ticks.sum.hand").to_string(),
    }
}

/// The card's verdict for a strategy no search has run on yet, from its readiness alone —
/// what the card says the moment that strategy's table has loaded, through the card's own
/// [`verdict_of`] and [`sentence`].
///
/// Args:
///     ready: The strategy's sample, covered and fetchable rows.
///
/// Returns:
///     The card's full "cannot tune" sentence while it would say so; `None` when the card
///     would offer the search (its idle state).
pub(in crate::analytics::tuner) fn readiness_refusal(
    ready: &super::readiness::Readiness,
) -> Option<String> {
    let verdict = verdict_of(None, ready.too_few(), None, None, false);
    matches!(verdict, Verdict::Cannot(_)).then(|| sentence(&verdict, 0, 0, ready.fetchable))
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
    /// The selected strategy's tape is still being read: the title row says so, and the
    /// verdict below is the last one this scope showed rather than a judgement of its sample.
    pub loading: bool,
    pub verdict: Verdict,
    pub tag: String,
    pub sentence: String,
    pub train: Option<ProfitRow>,
    pub holdout: Option<ProfitRow>,
    /// Full Fact tally on the identical holdout, kept for the shared muted baseline line.
    pub fact_holdout: Option<moon_core::db::metrics::Tally>,
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

/// Remove no-op edits and show an explicit absence for unset or blank strategy values.
/// Section labels come from descriptors or the loaded schema grid.
pub(super) fn change_rows(
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
                Some(NowValue::Same(old)) if old.trim().is_empty() => String::from("—"),
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
    /// Build advice from the displayed point, deferring sample refusal until its tape answers.
    pub(super) fn from_state(state: &TicksState, scope_label: String, human_labels: bool) -> Self {
        let running = match &state.sugg {
            SuggState::Idle => None,
            SuggState::Running {
                handle,
                total,
                started,
                estimate,
                parallel,
                stopping,
                ..
            } => Some((
                handle.completed(),
                *total,
                (!*stopping)
                    .then(|| {
                        remaining_time(
                            handle.completed(),
                            *total,
                            started.elapsed(),
                            *estimate,
                            *parallel,
                        )
                    })
                    .flatten(),
                handle.points(),
                *stopping,
            )),
        };
        let result = state.current_result();
        // Until the tape answers the sample is not known, and a refusal of it would be false;
        // the scope's last verdict stands meanwhile, so the card keeps its shape on every read.
        // Its sentence stands with it: rebuilt from the unread rows' counts it would contradict
        // the verdict it explains.
        // A reload before its tape stage counts too: the previous rows it holds meanwhile are
        // retired, every one "missing", and a verdict taken then reads "0 fit deals". A first
        // load holds no rows, so it refuses nothing.
        let loading = running.is_none()
            && (state.tape_reading
                || matches!(
                    state.data,
                    crate::load_state::LoadState::Loading { stale: Some(_) }
                ));
        let (verdict, shown_sentence) = if loading {
            state
                .shown_verdict
                .borrow()
                .as_ref()
                .filter(|(scope, _, _)| *scope == scope_label)
                .map_or((Verdict::Idle, None), |(_, verdict, sentence)| {
                    (verdict.clone(), Some(sentence.clone()))
                })
        } else {
            let verdict = sample_verdict(
                verdict_of(
                    running,
                    state.search_too_few(),
                    state.sugg_refusal.as_deref(),
                    result,
                    state.has_changes(),
                ),
                state.variant_too_few(),
            );
            (verdict, None)
        };
        let data = state.data.data();
        let (fit, hold) = match state.sugg {
            SuggState::Running { sample, .. } => sample,
            SuggState::Idle => (0, 0),
        };
        let shown_sentence = shown_sentence.unwrap_or_else(|| {
            sentence(&verdict, fit, hold, state.population().readiness.fetchable)
        });
        // A running search is never stood in for: it ends before the next read, and its
        // progress and Stop would outlive it.
        if !loading && !matches!(verdict, Verdict::Running { .. }) {
            *state.shown_verdict.borrow_mut() =
                Some((scope_label.clone(), verdict.clone(), shown_sentence.clone()));
        }
        let train = result.map(|r| ProfitRow {
            label: t!("analytics.ticks.sum.train").to_string(),
            sub: if r.purged > 0 {
                t!(
                    "analytics.ticks.sum.train_sub_purged",
                    n = r.fact_train.n,
                    p = r.purged
                )
                .to_string()
            } else {
                t!("analytics.ticks.sum.train_sub", n = r.fact_train.n).to_string()
            },
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
            loading,
            tag: tag(&verdict),
            sentence: shown_sentence,
            verdict,
            train,
            holdout,
            fact_holdout: result.and_then(|r| r.fact_holdout.clone()),
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

impl AnalyticsView {
    /// Add existing model and target warnings to the view-free summary without write-time checks.
    pub(super) fn ticks_summary_model(&self, cx: &App) -> SummaryModel {
        let human = crate::strategies::settings::human_labels(&self.backend.read(cx).layout);
        let mut model = SummaryModel::from_state(&self.ticks, self.scope_label(), human);
        model.write_enabled = !self.selected_targets().is_empty()
            && self.ticks.has_changes()
            && self.axis_write_ready(super::super::shared::TunerKind::Ticks);
        model.copy_visible = !self.is_multi();
        model.write_error = self.write_error.clone();
        if let Some(data) = self.ticks.data.data() {
            for group in [ParamGroup::Entry, ParamGroup::Exit] {
                model.warnings.extend(
                    super::grid::group_note(
                        &self.ticks,
                        group,
                        data,
                        self.ticks.population().fit,
                        MoonPalette::active(cx),
                    )
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
    pub(super) fn ticks_parameters_parts(
        &self,
        host_w: gpui::Pixels,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> (gpui::AnyElement, gpui::AnyElement) {
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
        let header = parameters_header(&model, &acts, cx);
        (header, summary_card_el(&model, &acts, arrow, p, cx))
    }

    /// Prepare inline consent and warning checks once, without opening another dialog.
    pub(super) fn ticks_begin_write(&mut self, cx: &mut Context<Self>) {
        if !self.axis_write_ready(super::super::shared::TunerKind::Ticks) {
            return;
        }
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
