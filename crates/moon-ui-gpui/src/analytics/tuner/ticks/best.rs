//! Sequential Entry/Exit searches with validated answers and explicit bulk consent.

use super::state::{NowValue, SuggState, TICKS_DEFAULT_TRAIN, TicksData, TicksState};
use super::summary::{self, ProfitRow, Verdict};
use super::tape::{PendingDeal, prepare_sample};
use crate::analytics::AnalyticsView;
use crate::analytics::tuner::shared::SaveTarget;
use crate::design::{self, moon};
use gpui::*;
use moon_core::db::tuner::threshold_search::SearchHandle;
use moon_core::db::tuner::ticks::params::param_keys;
use moon_core::db::tuner::ticks::search::{
    DEFAULT_MAX_PASSES, SearchMiss, SearchParams, SearchResult, sample_floor,
    suggest_with_candidate,
};
use moon_ui::{MoonButton, MoonButtonVariant, MoonCheckbox, MoonPalette, h_flex, v_flex};
use rust_i18n::t;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

/// Match the readiness and card progress cadence.
const PROGRESS_TICK: Duration = Duration::from_millis(250);

/// A batch owns one search handle and retains results in pick order.
pub(in crate::analytics::tuner) struct BestRun {
    pub seq: u64,
    pub keys: Vec<String>,
    pub next: usize,
    pub results: Vec<BestResult>,
    pub handle: Option<SearchHandle>,
    pub checked: HashSet<String>,
    pub done: bool,
}

impl Drop for BestRun {
    /// Closing a view or invalidating its scope must stop the independent search too.
    fn drop(&mut self) {
        if let Some(handle) = &self.handle {
            handle.cancel();
        }
    }
}

/// A validated answer, never an open candidate, and the strategy values it was based on.
pub(in crate::analytics::tuner) struct BestResult {
    pub key: String,
    pub target: SaveTarget,
    pub outcome: Result<SearchResult, SearchMiss>,
    pub(super) verdict: Verdict,
    pub now: HashMap<String, NowValue>,
    pub warns: Vec<String>,
}

impl BestResult {
    /// Whether the independent holdout check permits this answer to be written.
    pub(in crate::analytics::tuner) fn is_apply(&self) -> bool {
        self.verdict == Verdict::Apply && !self.changes().is_empty()
    }
    /// Use the single-card verdict over the validated result only.
    fn new(
        key: String,
        target: SaveTarget,
        outcome: Result<SearchResult, SearchMiss>,
        now: HashMap<String, NowValue>,
    ) -> Self {
        let too_few = outcome.as_ref().ok().and_then(|r| {
            let n = (r.fact_train.n + r.fact_holdout.as_ref().map_or(0, |h| h.n)).max(0) as usize;
            sample_floor(n).is_err().then_some(n)
        });
        let verdict = summary::sample_verdict(
            summary::verdict_of(None, None, None, outcome.as_ref().ok(), false),
            too_few,
        );
        Self {
            key,
            target,
            outcome,
            verdict,
            now,
            warns: Vec::new(),
        }
    }
    /// Remove no-op values without a later old-value database read.
    fn changes(&self) -> Vec<(String, String)> {
        self.outcome
            .as_ref()
            .ok()
            .map(|r| {
                summary::change_rows(&r.values, &self.now, &[], false)
                    .into_iter()
                    .map(|r| (r.key, r.new))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Discard the open candidate and enforce the sample floor on the validated answer's closed sample.
fn validated_answer(
    suggested: moon_core::db::tuner::ticks::search::Suggested,
    deals: usize,
) -> Result<SearchResult, SearchMiss> {
    suggested
        .result
        .and_then(|found| sample_floor(deals.saturating_sub(found.stats.left_open)).map(|()| found))
}

/// Skip replay after cancellation and discard work cancelled before it can be published.
fn while_active<T>(handle: &SearchHandle, work: impl FnOnce() -> T) -> Option<T> {
    if handle.is_cancelled() {
        return None;
    }
    let result = work();
    (!handle.is_cancelled()).then_some(result)
}

/// Aligned strategy edits and captured old values in pick order.
pub(in crate::analytics::tuner) type BestPayload = (
    Vec<SaveTarget>,
    Vec<Vec<(String, String)>>,
    Vec<Vec<Option<String>>>,
);

/// Carry per-strategy warnings into consent for exactly the writable checked answers.
pub(in crate::analytics::tuner) fn bulk_warns(
    results: &[BestResult],
    checked: &HashSet<String>,
) -> Vec<String> {
    results
        .iter()
        .filter(|r| r.is_apply() && checked.contains(&r.key))
        .flat_map(|r| r.warns.clone())
        .collect()
}

/// Produce aligned edits and targets from checked Apply answers, preserving pick order.
pub(in crate::analytics::tuner) fn bulk_payload(
    results: &[BestResult],
    checked: &HashSet<String>,
) -> BestPayload {
    let mut targets = Vec::new();
    let mut edits = Vec::new();
    let mut olds = Vec::new();
    for result in results
        .iter()
        .filter(|r| r.is_apply() && checked.contains(&r.key))
    {
        let changes = result.changes();
        olds.push(
            changes
                .iter()
                .map(|(key, _)| match result.now.get(key) {
                    Some(NowValue::Same(value)) => Some(value.clone()),
                    _ => None,
                })
                .collect(),
        );
        targets.push(result.target.clone());
        edits.push(changes);
    }
    (targets, edits, olds)
}

impl BestRun {
    /// Reject stale or out-of-order landings before advancing to another strategy.
    fn land(&mut self, seq: u64, result: BestResult) -> bool {
        if self.seq != seq || self.done || self.keys.get(self.next) != Some(&result.key) {
            return false;
        }
        if result.is_apply() {
            self.checked.insert(result.key.clone());
        }
        self.results.push(result);
        self.next += 1;
        self.handle = None;
        self.done = self.next == self.keys.len();
        true
    }
}

impl TicksState {
    /// Both card and batch entry points share the same exclusive search ownership gate.
    pub(super) fn search_available(&self) -> bool {
        !self.best_running && !matches!(self.sugg, SuggState::Running { .. })
    }
    /// Retire the producer before a late background result can land into another scope.
    pub(in crate::analytics) fn cancel_best(&mut self) {
        self.best_seq = self.best_seq.wrapping_add(1);
        self.best = None;
        self.best_running = false;
    }
}

impl AnalyticsView {
    /// Start a batch only when neither search producer owns the shared lane.
    pub(in crate::analytics::tuner) fn start_best_run(
        &mut self,
        keys: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        if keys.is_empty()
            || !self.ticks.search_available()
            || matches!(
                self.ready.run,
                crate::analytics::tuner::ready::ReadyRun::Running { .. }
            )
            || keys.iter().any(|key| {
                !crate::analytics::tuner::list::best_key_in_scope(key, self.action_core_ids())
            })
        {
            return;
        }
        self.ticks.cancel_best();
        let seq = self.ticks.best_seq;
        self.ticks.best = Some(BestRun {
            seq,
            keys,
            next: 0,
            results: Vec::new(),
            handle: None,
            checked: HashSet::new(),
            done: false,
        });
        self.ticks.best_running = true;
        self.best_next(seq, cx);
        // The timer survives reads but only a live search needs progress repaints.
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            loop {
                executor.timer(PROGRESS_TICK).await;
                let mut active = false;
                cx.update(|cx| {
                    let _ = this.update(cx, |view, cx| {
                        active = view
                            .ticks
                            .best
                            .as_ref()
                            .is_some_and(|r| r.seq == seq && !r.done);
                        if active && view.ticks.best.as_ref().is_some_and(|r| r.handle.is_some()) {
                            cx.notify();
                        }
                    });
                });
                if !active {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// Retire batch landings and cancel the search handle, then re-arm readiness.
    pub(in crate::analytics::tuner) fn close_best_run(&mut self, cx: &mut Context<Self>) {
        if self.ticks.best.is_none() {
            return;
        }
        self.ticks.cancel_best();
        self.ensure_ready_pass(cx);
        cx.notify();
    }

    /// Read the next strategy independently of the table selection.
    fn best_next(&mut self, seq: u64, cx: &mut Context<Self>) {
        let Some(run) = self.ticks.best.as_ref().filter(|r| r.seq == seq && !r.done) else {
            return;
        };
        let key = run.keys[run.next].clone();
        let Some((sid, core)) = parse_key(&key) else {
            self.close_best_run(cx);
            return;
        };
        let mut query = self.tuner_query_all();
        query.strategies = vec![(sid, Some(core))];
        self.spawn_db(
            false,
            cx,
            move || moon_core::db::tuner::ticks::read_deals(&query),
            move |view, read, cx| {
                if !view
                    .ticks
                    .best
                    .as_ref()
                    .is_some_and(|r| r.seq == seq && !r.done)
                {
                    return;
                }
                match read {
                    Ok(read) => view.best_search(seq, key, read.deals, cx),
                    Err(_) => view.close_best_run(cx),
                }
            },
        );
    }

    /// Replay held tape while the batch is active, then search with the card's default settings.
    fn best_search(
        &mut self,
        seq: u64,
        key: String,
        deals: Vec<moon_core::db::tuner::ticks::Deal>,
        cx: &mut Context<Self>,
    ) {
        let addresses = self.resolve_addresses(&deals, cx);
        let Some((sid, core)) = parse_key(&key) else {
            self.close_best_run(cx);
            return;
        };
        let name = self
            .strategy_data
            .data()
            .and_then(|d| d.strategies.iter().find(|s| s.key == key))
            .map(|s| s.name.clone())
            .unwrap_or_else(|| key.clone());
        let target = SaveTarget {
            sid,
            core: Some(core),
            name,
        };
        let targets = deals
            .into_iter()
            .filter_map(|d| {
                let address = addresses
                    .get(&(d.core_uid, d.coin.clone()))
                    .cloned()
                    .flatten()?;
                Some((d, address))
            })
            .collect();
        let defaults = self.filter_defaults(cx);
        let model = super::model_cfg::current();
        let klines = self.backend.read(cx).session.market_source().kline_cache();
        let handle = SearchHandle::new();
        if let Some(run) = self.ticks.best.as_mut().filter(|r| r.seq == seq) {
            run.handle = Some(handle.clone());
        } else {
            return;
        }
        let human = crate::strategies::settings::human_labels(&self.backend.read(cx).layout);
        self.spawn_db(
            false,
            cx,
            move || {
                if handle.is_cancelled() {
                    return None;
                }
                let fetched = super::fetch_batch(
                    targets,
                    moon_core::market::trade_replay::long_position_ms(),
                );
                let rows = while_active(&handle, || {
                    super::replay_fetched(
                        fetched,
                        &defaults,
                        model,
                        klines.as_ref(),
                        chrono::Utc::now().timestamp_millis(),
                        None,
                        Some(&handle),
                    )
                })?;
                let mut kinds: Vec<String> = rows.iter().map(|r| r.deal.kind.clone()).collect();
                kinds.sort();
                kinds.dedup();
                let mut data = TicksData {
                    rows,
                    kinds,
                    ..Default::default()
                };
                data.retain_within_cap();
                let mut keys = param_keys();
                for key in moon_core::db::tuner::ticks::unmodelled::watched_keys() {
                    if !keys.contains(&key) {
                        keys.push(key);
                    }
                }
                let Some(own) =
                    moon_core::db::tuner::strategy_current_values_opt(sid, Some(core), &keys)
                else {
                    let mut result =
                        BestResult::new(key, target, Err(SearchMiss::Nothing), HashMap::new());
                    result.verdict =
                        Verdict::Cannot(t!("analytics.ticks.best.read_failed").to_string());
                    return while_active(&handle, || result);
                };
                let warns = super::unmodelled::strategy_warns(
                    &target.name,
                    &own,
                    &defaults,
                    &moon_core::feed::strategy_deps::FieldDeps::load(),
                    human,
                );
                let own = Arc::new(own);
                let now = keys
                    .iter()
                    .filter_map(|k| {
                        own.get(k)
                            .cloned()
                            .or_else(|| {
                                defaults.get(&k.to_ascii_lowercase()).map(|n| n.to_string())
                            })
                            .map(|v| (k.clone(), NowValue::Same(v)))
                    })
                    .collect();
                let kind = data.single_kind().map(str::to_string);
                let mut pending: Vec<_> = data
                    .replayable()
                    .filter_map(|r| {
                        Some(PendingDeal {
                            deal: r.deal.clone(),
                            tape: r.ticks.clone()?,
                            entry_line: r.entry_line.clone(),
                            trail_ms: r.held.map_or(0, |(_, t)| t),
                            own: own.clone(),
                        })
                    })
                    .collect();
                pending.sort_by_key(|d| d.deal.close_ms);
                let outcome = sample_floor(pending.len()).and_then(|()| {
                    let kind = kind.as_deref().ok_or(SearchMiss::Nothing)?;
                    let grids = super::ranges::auto_grids_for(kind, &own, &defaults);
                    let held = HashMap::new();
                    let locked = HashSet::new();
                    let params = SearchParams {
                        held: &held,
                        defaults: &defaults,
                        kind,
                        vary_entry: data.entry_modelled(),
                        vary_exit: true,
                        locked: &locked,
                        grids: &grids,
                        restarts: super::variants::DEFAULT_RESTARTS,
                        min_n: None,
                        seed: None,
                        train_frac: TICKS_DEFAULT_TRAIN as f64 / 100.0,
                        max_passes: DEFAULT_MAX_PASSES,
                        model,
                        keep_corridor: true,
                        risk: Default::default(),
                        screen_entry: true,
                    };
                    let sample = prepare_sample(pending);
                    // The open candidate is discarded at the search boundary.
                    validated_answer(
                        suggest_with_candidate(&sample, &params, &handle),
                        sample.len(),
                    )
                });
                let mut result = BestResult::new(key, target, outcome, now);
                result.warns = warns;
                while_active(&handle, || result)
            },
            move |view, result, cx| {
                let Some(result) = result else {
                    return;
                };
                if !view
                    .ticks
                    .best
                    .as_mut()
                    .is_some_and(|r| r.land(seq, result))
                {
                    return;
                }
                view.ticks.best_running = view.ticks.best.as_ref().is_some_and(|r| !r.done);
                if view.ticks.best_running {
                    view.best_next(seq, cx);
                } else {
                    view.ensure_ready_pass(cx);
                }
                cx.notify();
            },
        );
    }

    /// Keep the final write action outside the scroll body so it is always reachable.
    pub(in crate::analytics::tuner) fn best_pane_el(
        &self,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(run) = &self.ticks.best else {
            return div().into_any_element();
        };
        let mut body = v_flex().w_full().gap(design::ui_px(cx, 8.0));
        for result in &run.results {
            let core = result
                .target
                .core
                .map(|uid| self.core_names.resolve(uid, &uid.to_string()).to_string())
                .unwrap_or_default();
            body = body.child(result_card(
                result,
                run.checked.contains(&result.key),
                Some(cx.entity().downgrade()),
                &core,
                p,
                cx,
            ));
        }
        let (targets, _, _) = bulk_payload(&run.results, &run.checked);
        let cores = self.best_core_list(&targets);
        let current = run
            .keys
            .get(run.next)
            .map(|key| {
                self.strategy_data
                    .data()
                    .and_then(|d| d.strategies.iter().find(|s| &s.key == key))
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| key.clone())
            })
            .unwrap_or_default();
        let completed = run.handle.as_ref().map_or(0, SearchHandle::completed);
        let text = t!(
            "analytics.ticks.best.progress",
            k = if run.done { run.next } else { run.next + 1 },
            n = run.keys.len(),
            name = current
        )
        .to_string();
        let fraction = (run.next as f32
            + completed as f32 / super::variants::DEFAULT_RESTARTS as f32)
            / run.keys.len().max(1) as f32;
        let header = crate::analytics::tuner::list::progress_row(
            text,
            false,
            (!run.done).then_some(fraction),
            false,
            p,
            cx,
        );
        let footer = footer_el(
            targets.len(),
            run.done,
            &cores,
            Some(cx.entity().downgrade()),
            p,
            cx,
        );
        shell_el(header, body.into_any_element(), footer)
    }
}

/// Pin consent outside the scrolling answers using the same frame in production and layout probes.
fn shell_el(header: AnyElement, body: AnyElement, footer: AnyElement) -> AnyElement {
    v_flex()
        .debug_selector(|| "an-best-pane".into())
        .size_full()
        .min_h_0()
        .child(header)
        .child(
            div()
                .w_full()
                .flex_none()
                .child(t!("analytics.ticks.best.found").to_string()),
        )
        .child(
            div()
                .id("an-best-scroll")
                .debug_selector(|| "an-best-scroll".into())
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(body),
        )
        .child(footer)
        .into_any_element()
}

/// Name the concrete core destinations and offer only the existing confirmation dialog.
fn footer_el(
    count: usize,
    done: bool,
    cores: &str,
    view: Option<WeakEntity<AnalyticsView>>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let save = view.clone();
    v_flex()
        .id("an-best-footer")
        .debug_selector(|| "an-best-footer".into())
        .w_full()
        .flex_none()
        .gap(design::ui_px(cx, 8.0))
        .p(design::ui_px(cx, 10.0))
        .text_size(design::t_caption(cx))
        .text_color(moon(p.text_soft))
        .child(
            div()
                .w_full()
                .font_weight(FontWeight::SEMIBOLD)
                .child(format!("2 · {}", t!("analytics.ticks.best.write"))),
        )
        .children((count > 0).then(|| {
            div()
                .w_full()
                .child(t!("analytics.ticks.best.warning", cores = cores).to_string())
        }))
        .child(
            MoonButton::new("an-best-save")
                .label(format!("{} · {count}", t!("analytics.ticks.best.write")))
                .disabled(count == 0 || !done)
                .variant(MoonButtonVariant::Blue)
                .on_click(move |_, _, cx| {
                    if let Some(view) = &save {
                        let _ = view.update(cx, |v, cx| v.open_best_save(cx));
                    }
                })
                .render(),
        )
        .child(
            MoonButton::new("an-best-close")
                .label(t!("analytics.ticks.best.close").to_string())
                .variant(MoonButtonVariant::Ghost)
                .on_click(move |_, _, cx| {
                    if let Some(view) = &view {
                        let _ = view.update(cx, |v, cx| v.close_best_run(cx));
                    }
                })
                .render(),
        )
        .into_any_element()
}

/// Decode the concrete per-core list identity without admitting a fan-out write target.
fn parse_key(key: &str) -> Option<(i64, u64)> {
    let (s, c) = key.split_once('@')?;
    Some((s.parse().ok()?, c.parse().ok()?))
}

/// Render a validated answer's changes and independent training and held-out profits.
fn result_card(
    result: &BestResult,
    checked: bool,
    view: Option<WeakEntity<AnalyticsView>>,
    core: &str,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let tag = if result.outcome.is_err() {
        t!("analytics.ticks.best.nothing").to_string()
    } else {
        summary::tag(&result.verdict)
    };
    let mut head = h_flex().w_full().gap(design::ui_px(cx, 6.0)).items_start();
    if result.is_apply() {
        let key = result.key.clone();
        head = head.child(
            MoonCheckbox::new(SharedString::from(format!("an-best-check-{key}")))
                .checked(checked)
                .on_change(move |on: &bool, _, cx| {
                    let Some(view) = &view else {
                        return;
                    };
                    let _ = view.update(cx, |v, cx| {
                        if let Some(run) = &mut v.ticks.best {
                            if *on {
                                run.checked.insert(key.clone());
                            } else {
                                run.checked.remove(&key);
                            }
                        }
                        v.tuner.dialog_seq = v.tuner.dialog_seq.wrapping_add(1);
                        cx.notify();
                    });
                }),
        );
    }
    let badge = if result.is_apply() {
        moon_ui::MoonTag::positive()
    } else if result.outcome.is_err() {
        moon_ui::MoonTag::new()
    } else {
        moon_ui::MoonTag::warning()
    };
    head = head
        .flex_wrap()
        .child(div().flex_1().min_w_0().child(result.target.name.clone()))
        .child(div().text_color(moon(p.text_muted)).child(core.to_string()))
        .child(badge.mono(false).child(tag));
    let mut card = v_flex()
        .w_full()
        .p(design::ui_px(cx, 10.0))
        .gap(design::ui_px(cx, 6.0))
        .bg(moon(p.panel))
        .border_1()
        .border_color(moon(p.border))
        .rounded(design::ui_px(cx, 6.0))
        .font_family(design::ui_font())
        .text_size(design::t_caption(cx))
        .text_color(moon(p.text_soft))
        .child(head);
    if let Verdict::Cannot(reason) = &result.verdict {
        card = card.child(div().w_full().child(reason.clone()));
    }
    for warning in &result.warns {
        card = card.child(
            div()
                .w_full()
                .text_color(moon(p.amber))
                .child(warning.clone()),
        );
    }
    if let Ok(answer) = &result.outcome {
        for row in summary::change_rows(&answer.values, &result.now, &[], false) {
            card = card.child(
                div()
                    .w_full()
                    .child(format!("{}: {} → {}", row.key, row.old, row.new)),
            );
        }
        let mut profits = vec![ProfitRow {
            label: t!("analytics.ticks.sum.train").to_string(),
            sub: String::new(),
            n: answer.train.n,
            fact: answer.fact_train.profit,
            variant: answer.train.profit,
            open_line: None,
        }];
        if let (Some(h), Some(f)) = (&answer.holdout, &answer.fact_holdout) {
            profits.push(ProfitRow {
                label: t!("analytics.ticks.sum.hold").to_string(),
                sub: String::new(),
                n: h.n,
                fact: f.profit,
                variant: h.profit,
                open_line: None,
            });
        }
        let effect: f64 = profits.iter().map(|r| r.variant - r.fact).sum();
        card = card.child(
            div()
                .text_color(moon(if effect > 0.0 {
                    p.green
                } else if effect < 0.0 {
                    p.red
                } else {
                    p.text_muted
                }))
                .child(format!(
                    "Δ {}: {}",
                    t!("analytics.ticks.sum.profit_head"),
                    crate::analytics::summary::fmt_signed_unit(effect)
                )),
        );
        for row in profits {
            card = card.child(div().w_full().child(format!(
                "{} ({}): {} → {}",
                row.label,
                row.n,
                crate::analytics::summary::fmt_signed_unit(row.fact),
                crate::analytics::summary::fmt_signed_unit(row.variant)
            )));
        }
    }
    card.into_any_element()
}

/// Explain the three separate steps before a strategy or batch is opened.
pub(in crate::analytics::tuner) fn empty_pane_el(
    no_deals: bool,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    v_flex()
        .debug_selector(|| "an-best-empty".into())
        .size_full()
        .min_w_0()
        .p(design::ui_px(cx, 16.0))
        .gap(design::ui_px(cx, 12.0))
        .bg(moon(p.panel))
        .text_color(moon(p.text_soft))
        .font_family(design::ui_font())
        .text_size(design::t_body(cx))
        .child(
            div()
                .w_full()
                .child(t!("analytics.ticks.best.empty").to_string()),
        )
        .children(no_deals.then(|| {
            div()
                .w_full()
                .child(t!("analytics.ticks.best.no_deals").to_string())
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests;
