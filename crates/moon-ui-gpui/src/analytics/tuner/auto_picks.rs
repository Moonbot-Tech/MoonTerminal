//! The auto-pick for EVERY strategy of the list: the "Auto" column, its filter and the
//! progress line above the table.
//!
//! The pick runs by itself in the background with the auto-pick card's settings and each
//! strategy's own saved lists (`moon_core::db::tuner::coin_pick::suggest_coins_by_strategy`).
//! It reruns when the list's scope is reloaded by hand (period, mode, metric, cores), when a
//! list is saved, or when the card's settings change — not on every live report commit, which
//! would rescan the whole period every few seconds for answers that barely move.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui::*;
use moon_core::db::ReadFail;
use moon_core::db::tuner::coin_pick::{
    BatchProgress, CoinPick, NoiseReason, PickSettings, PickVerdict,
};
use rust_i18n::t;

use super::super::AnalyticsView;
use super::coins::state::{PickCfg, PickState, SuggestedLists};
use crate::analytics::refresh::CatchUpOutcome;

/// How long a burst of setting edits keeps coalescing before the batch is launched: typing
/// "10" into the minimum-trades field must cost one scan, not two.
const LAUNCH_DEBOUNCE: Duration = Duration::from_millis(400);
/// How often the progress line repaints while the batch runs.
const PROGRESS_TICK: Duration = Duration::from_millis(250);
/// Pause before a busy database is asked again.
const RETRY_AFTER: Duration = Duration::from_secs(5);

/// What one strategy's batch pick says, as the column and the filter read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics) enum AutoClass {
    /// Fresh trades confirm the suggestion; `n` coins on the suggested list.
    Useful { n: usize, white: bool },
    /// Candidates exist, but fresh trades did not confirm them: a small gain, a worse
    /// holdout, or too few fresh trades touched. `n` coins on the named list.
    Noise { n: usize, white: bool },
    /// No coin stands out.
    Nothing,
    /// Too few trades in the period to judge, or nothing to show.
    Few,
    /// Not computed yet (running, or the strategy has no trades in the period).
    Pending,
    /// The batch read failed; the progress line says why.
    Unread,
}

impl AutoClass {
    /// Classify a finished pick.
    ///
    /// Args:
    ///     pick: The strategy's pick.
    ///
    /// Returns:
    ///     The column's class. Any noise verdict with candidates (few fresh trades touched
    ///     included) counts them, so the column never hides coins the card would offer; too
    ///     few trades in the period is "few". A whitelist-only suggestion reads as the
    ///     whitelist; otherwise the blacklist is named, since it is the default search.
    pub(in crate::analytics) fn of(pick: &CoinPick) -> Self {
        let white = pick.black.is_empty() && !pick.white.is_empty();
        let n = if white {
            pick.white.len()
        } else {
            pick.black.len()
        };
        match &pick.verdict {
            PickVerdict::Useful => AutoClass::Useful { n, white },
            PickVerdict::TooFewTrades => AutoClass::Few,
            PickVerdict::Noise(_) if n > 0 => AutoClass::Noise { n, white },
            PickVerdict::Noise(NoiseReason::FewHoldout) => AutoClass::Few,
            PickVerdict::Noise(_) | PickVerdict::NothingFound => AutoClass::Nothing,
        }
    }

    /// Sort rank: findings first, confirmed ahead of noise, everything else after.
    pub(in crate::analytics) fn rank(self) -> u8 {
        match self {
            AutoClass::Useful { .. } => 0,
            AutoClass::Noise { .. } => 1,
            AutoClass::Nothing => 2,
            AutoClass::Few => 3,
            AutoClass::Pending => 4,
            AutoClass::Unread => 5,
        }
    }

    /// The candidate count inside a rank, so the bigger finding leads its group.
    pub(in crate::analytics) fn size(self) -> usize {
        match self {
            AutoClass::Useful { n, .. } | AutoClass::Noise { n, .. } => n,
            _ => 0,
        }
    }
}

/// The strategy toolbar's "Auto-pick" filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics) enum AutoFilter {
    All,
    /// Any pick with candidates, confirmed or not.
    Found,
    /// Useful only.
    Confirmed,
}

impl AutoFilter {
    /// Every filter, in menu order.
    pub(in crate::analytics) const ALL: [AutoFilter; 3] =
        [AutoFilter::All, AutoFilter::Found, AutoFilter::Confirmed];

    /// Does a strategy of this class pass?
    pub(in crate::analytics) fn keeps(self, class: AutoClass) -> bool {
        match self {
            AutoFilter::All => true,
            AutoFilter::Found => {
                matches!(class, AutoClass::Useful { .. } | AutoClass::Noise { .. })
            }
            AutoFilter::Confirmed => matches!(class, AutoClass::Useful { .. }),
        }
    }

    /// Stable, locale-independent element key.
    pub(in crate::analytics) fn key(self) -> &'static str {
        match self {
            AutoFilter::All => "all",
            AutoFilter::Found => "found",
            AutoFilter::Confirmed => "confirmed",
        }
    }

    /// The menu item's text.
    pub(in crate::analytics) fn label(self) -> String {
        match self {
            AutoFilter::All => t!("analytics.auto.filter_all").to_string(),
            AutoFilter::Found => t!("analytics.auto.filter_found").to_string(),
            AutoFilter::Confirmed => t!("analytics.auto.filter_confirmed").to_string(),
        }
    }
}

/// The batch's classes as the list's filter-and-sort memo reads them.
///
/// Compared by generation and filter only: the class map is rebuilt exactly when the generation
/// moves, so comparing ~600 entries per render would buy nothing.
#[derive(Clone, Debug)]
pub(in crate::analytics) struct AutoView {
    pub(in crate::analytics) generation: u64,
    pub(in crate::analytics) filter: AutoFilter,
    pub(in crate::analytics) classes: Arc<HashMap<String, AutoClass>>,
    /// The class of a row the batch has no pick for.
    pub(in crate::analytics) missing: AutoClass,
}

impl PartialEq for AutoView {
    fn eq(&self, other: &Self) -> bool {
        self.generation == other.generation && self.filter == other.filter
    }
}

impl Default for AutoView {
    fn default() -> Self {
        Self {
            generation: 0,
            filter: AutoFilter::All,
            classes: Arc::default(),
            missing: AutoClass::Pending,
        }
    }
}

impl AutoView {
    /// The class of one strategy-list row.
    pub(in crate::analytics) fn class(&self, key: &str) -> AutoClass {
        self.classes.get(key).copied().unwrap_or(self.missing)
    }
}

/// The face of a strategy list the auto-pick filter left empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics) enum AutoEmpty {
    /// The batch is still running: "none found" would be a guess.
    Computing,
    /// The batch answered and no strategy passed the filter.
    NoFinding,
}

/// What a batch was launched for. A different key supersedes the running batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics) struct AutoKey {
    /// Bumped on every hand-made reload of the strategy list (period, mode, metric, cores).
    pub(in crate::analytics) scope_gen: u64,
    /// Bumped after every list save.
    pub(in crate::analytics) lists_gen: u64,
    pub(in crate::analytics) cfg: PickCfg,
}

/// Where the batch stands.
pub(in crate::analytics) enum AutoRun {
    Idle,
    Running {
        progress: Arc<BatchProgress>,
    },
    Done,
    /// The read failed; `retrying` when it was a busy database and a retry is armed.
    Failed {
        fail: ReadFail,
        retrying: bool,
    },
}

/// The batch's state on the view.
pub(in crate::analytics) struct AutoPicks {
    /// Generation of the batch allowed to publish; a superseded reply is dropped.
    pub(in crate::analytics) seq: u64,
    /// Inputs of the batch launched last (or armed to launch).
    pub(in crate::analytics) launched: Option<AutoKey>,
    /// Inputs of the batch that actually started last; `launched` runs ahead of it while a
    /// launch is debounced.
    pub(in crate::analytics) ran: Option<AutoKey>,
    pub(in crate::analytics) scope_gen: u64,
    pub(in crate::analytics) lists_gen: u64,
    pub(in crate::analytics) run: AutoRun,
    /// The last finished batch, per strategy-list row key.
    pub(in crate::analytics) picks: HashMap<String, Arc<CoinPick>>,
    /// The fresh share the published picks ran with.
    pub(in crate::analytics) holdout_pct: u32,
    /// Bumped whenever `picks` or `run` changes, so the list's memo knows to re-sort.
    pub(in crate::analytics) generation: u64,
    /// `picks` classified once per batch, shared with the list's memo.
    classes: Arc<HashMap<String, AutoClass>>,
    /// The filter the user picked from the menu; `None` until then, so each axis shows its
    /// own default (see [`AutoPicks::filter`]).
    pub(in crate::analytics) chosen: Option<AutoFilter>,
    /// The (strategy, batch) whose pick the card already adopted, so "Reset" sticks.
    pub(in crate::analytics) adopted: Option<(String, u64)>,
    /// Debounce timer, progress ticker and retry timer; dropping one cancels it.
    pub(in crate::analytics) launch_task: Option<Task<()>>,
    pub(in crate::analytics) ticker: Option<Task<()>>,
}

impl Default for AutoPicks {
    fn default() -> Self {
        Self {
            seq: 0,
            launched: None,
            ran: None,
            scope_gen: 0,
            lists_gen: 0,
            run: AutoRun::Idle,
            picks: HashMap::new(),
            holdout_pct: 0,
            generation: 0,
            classes: Arc::default(),
            chosen: None,
            adopted: None,
            launch_task: None,
            ticker: None,
        }
    }
}

impl AutoPicks {
    /// The filter in force on one axis of the strategy list.
    ///
    /// Args:
    ///     coins_axis: The list is on the By-coin axis, where the Auto column lives.
    ///
    /// Returns:
    ///     The user's menu choice when there is one; otherwise "with a finding" on By-coin,
    ///     which is what the column is for, and "all" on every other axis.
    pub(in crate::analytics) fn filter(&self, coins_axis: bool) -> AutoFilter {
        self.chosen.unwrap_or(if coins_axis {
            AutoFilter::Found
        } else {
            AutoFilter::All
        })
    }

    /// What an EMPTY strategy list shows under the auto-pick filter.
    ///
    /// Args:
    ///     filter: The filter in force.
    ///
    /// Returns:
    ///     `None` when the filter is off (the generic "no match" note applies),
    ///     [`AutoEmpty::Computing`] until the batch has answered, and [`AutoEmpty::NoFinding`]
    ///     with its "show all" button once it answered (or failed) with nothing to keep.
    pub(in crate::analytics) fn empty_face(&self, filter: AutoFilter) -> Option<AutoEmpty> {
        if filter == AutoFilter::All {
            None
        } else if matches!(self.run, AutoRun::Done | AutoRun::Failed { .. }) {
            Some(AutoEmpty::NoFinding)
        } else {
            Some(AutoEmpty::Computing)
        }
    }

    /// The classes and the filter, for the list's memo and its rows.
    ///
    /// Args:
    ///     coins_axis: The list is on the By-coin axis (see [`AutoPicks::filter`]).
    pub(in crate::analytics) fn view(&self, coins_axis: bool) -> AutoView {
        AutoView {
            generation: self.generation,
            filter: self.filter(coins_axis),
            classes: self.classes.clone(),
            missing: self.view_missing(),
        }
    }

    /// The class of a row the batch holds no pick for: unread after a failure, else pending.
    pub(in crate::analytics) fn view_missing(&self) -> AutoClass {
        if matches!(self.run, AutoRun::Failed { .. }) {
            AutoClass::Unread
        } else {
            AutoClass::Pending
        }
    }

    /// The strategy list was reloaded by hand: every pick belongs to the old scope.
    pub(in crate::analytics) fn scope_changed(&mut self) {
        self.scope_gen = self.scope_gen.wrapping_add(1);
    }

    /// A list was saved: each strategy's manual baseline may have moved.
    pub(in crate::analytics) fn lists_changed(&mut self) {
        self.lists_gen = self.lists_gen.wrapping_add(1);
    }

    /// Start a batch of generation `seq + 1`, retiring whatever ran before.
    ///
    /// Returns:
    ///     The new generation and its progress handle.
    pub(in crate::analytics) fn begin(&mut self, key: AutoKey) -> (u64, Arc<BatchProgress>) {
        if let AutoRun::Running { progress } = &self.run {
            progress.cancel();
        }
        self.seq = self.seq.wrapping_add(1);
        self.launched = Some(key);
        self.ran = Some(key);
        let progress = Arc::new(BatchProgress::default());
        self.run = AutoRun::Running {
            progress: progress.clone(),
        };
        self.picks.clear();
        self.classes = Arc::default();
        self.adopted = None;
        self.generation = self.generation.wrapping_add(1);
        (self.seq, progress)
    }

    /// Publish a batch reply.
    ///
    /// Args:
    ///     seq: The generation the batch was launched under.
    ///     holdout_pct: The fresh share it ran with.
    ///     result: The background answer; `Ok(None)` is a cancelled batch.
    ///
    /// Returns:
    ///     Whether it landed (`false` for a superseded or cancelled reply) and, when it failed,
    ///     whether the failure is a busy database worth retrying.
    pub(in crate::analytics) fn land(
        &mut self,
        seq: u64,
        holdout_pct: u32,
        result: moon_core::db::ReadResult<Option<HashMap<String, CoinPick>>>,
    ) -> (bool, bool) {
        if seq != self.seq {
            return (false, false);
        }
        self.generation = self.generation.wrapping_add(1);
        match result {
            Ok(Some(picks)) => {
                self.classes = Arc::new(
                    picks
                        .iter()
                        .map(|(k, p)| (k.clone(), AutoClass::of(p)))
                        .collect(),
                );
                self.picks = picks.into_iter().map(|(k, p)| (k, Arc::new(p))).collect();
                self.holdout_pct = holdout_pct;
                self.run = AutoRun::Done;
                (true, false)
            }
            Ok(None) => {
                self.run = AutoRun::Idle;
                (false, false)
            }
            Err(fail) => {
                let retrying = CatchUpOutcome::of_read::<()>(&Err(fail.clone())).is_transient();
                self.run = AutoRun::Failed { fail, retrying };
                (true, retrying)
            }
        }
    }

    /// The toolbar's progress sentence and whether it is a warning.
    ///
    /// Args:
    ///     total: Strategies in the list, the "of M" of every sentence.
    ///
    /// Returns:
    ///     `None` before the first batch, or when the database is simply absent.
    pub(in crate::analytics) fn status(&self, total: usize) -> Option<(String, bool)> {
        match &self.run {
            AutoRun::Idle => None,
            AutoRun::Running { progress } => {
                let (done, of) = progress.counts();
                Some((
                    t!(
                        "analytics.auto.progress",
                        k = done,
                        m = if of > 0 { of } else { total }
                    )
                    .to_string(),
                    false,
                ))
            }
            AutoRun::Done => {
                let n = self
                    .classes
                    .values()
                    .filter(|c| AutoFilter::Found.keeps(**c))
                    .count();
                Some((
                    t!("analytics.auto.done", n = n, m = total).to_string(),
                    false,
                ))
            }
            AutoRun::Failed {
                fail: ReadFail::NotReady,
                ..
            } => None,
            AutoRun::Failed { retrying: true, .. } => {
                Some((t!("analytics.auto.busy").to_string(), true))
            }
            AutoRun::Failed { .. } => Some((t!("analytics.auto.failed").to_string(), true)),
        }
    }

    /// The running batch's progress as a 0..=1 fraction, for the thin bar.
    pub(in crate::analytics) fn fraction(&self) -> Option<f32> {
        match &self.run {
            AutoRun::Running { progress } => {
                let (done, of) = progress.counts();
                Some(if of == 0 {
                    0.0
                } else {
                    done as f32 / of as f32
                })
            }
            _ => None,
        }
    }
}

/// The cell's hover text: the card's verdict sentence plus the suggested coins.
///
/// Args:
///     pick: The strategy's pick.
///     holdout_pct: The fresh share it ran with.
///
/// Returns:
///     One or two sentences for the tooltip.
pub(in crate::analytics) fn cell_tip(pick: &CoinPick, holdout_pct: u32) -> String {
    let mut text = String::new();
    // A grey chip names coins that look like a finding: say up front that they are not one.
    if !matches!(pick.verdict, PickVerdict::Useful)
        && (!pick.black.is_empty() || !pick.white.is_empty())
    {
        text.push_str(&t!("analytics.auto.tip_unconfirmed"));
        text.push(' ');
    }
    text.push_str(&super::coins::auto::verdict_line(pick, holdout_pct));
    let coins = |list: &[moon_core::db::tuner::coin_pick::CoinSuggestion]| {
        list.iter()
            .map(|c| c.token.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    if !pick.black.is_empty() {
        text.push(' ');
        text.push_str(&t!("analytics.auto.tip_bl", coins = coins(&pick.black)));
    }
    if !pick.white.is_empty() {
        text.push(' ');
        text.push_str(&t!("analytics.auto.tip_wl", coins = coins(&pick.white)));
    }
    text
}

impl AnalyticsView {
    /// The inputs a batch for the current view would run with.
    fn auto_key(&self) -> AutoKey {
        AutoKey {
            scope_gen: self.auto_picks.scope_gen,
            lists_gen: self.auto_picks.lists_gen,
            cfg: self.coins.pick_cfg,
        }
    }

    /// Arm a batch when the list is loaded and its inputs moved since the last launch.
    ///
    /// Called on every render of the strategy list; cheap when nothing moved. The launch is
    /// debounced, so a burst of setting edits costs one scan.
    pub(in crate::analytics::tuner) fn ensure_auto_picks(&mut self, cx: &mut Context<Self>) {
        let ready = self.strategy_data.data().is_some()
            && !matches!(
                self.strategy_data,
                crate::analytics::profit_load::ProfitLoadState::Loading
            );
        let key = self.auto_key();
        if !ready || self.auto_picks.launched == Some(key) {
            return;
        }
        // Marked as launched now so the next render does not re-arm the timer.
        self.auto_picks.launched = Some(key);
        self.auto_picks.launch_task = Some(cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(LAUNCH_DEBOUNCE).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    // A later edit re-armed the timer with its own key; this one is retired.
                    if this.auto_picks.launched == Some(key) {
                        this.run_auto_picks(key, cx);
                    }
                });
            });
        }));
    }

    /// Run the batch now, in the background, and publish it bound to its generation.
    fn run_auto_picks(&mut self, key: AutoKey, cx: &mut Context<Self>) {
        let settings = PickSettings {
            min_trades: key.cfg.min_trades,
            holdout_pct: key.cfg.holdout_pct,
            black: key.cfg.black,
            white: key.cfg.white,
        };
        let q = self.tuner_query_all();
        let (seq, progress) = self.auto_picks.begin(key);
        let worker = progress.clone();
        self.spawn_latest_db(
            &[crate::analytics::bg::ReadLane::CoinPickAll],
            false,
            cx,
            move || {
                moon_core::db::tuner::coin_pick::suggest_coins_by_strategy(&q, settings, &worker)
            },
            move |this, result, cx| {
                let (landed, retry) = this.auto_picks.land(seq, settings.holdout_pct, result);
                this.auto_picks.ticker = None;
                // A finished batch changes what the list filters and sorts by.
                this.strat_visible = None;
                if landed && retry {
                    this.arm_auto_retry(key, cx);
                }
                cx.notify();
            },
        );
        // Repaint the progress line while the batch runs; the ticker ends with it.
        self.auto_picks.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                let executor = cx.update(|cx| cx.background_executor().clone());
                executor.timer(PROGRESS_TICK).await;
                let running = cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        cx.notify();
                        this.auto_picks.seq == seq
                            && matches!(this.auto_picks.run, AutoRun::Running { .. })
                    })
                    .unwrap_or(false)
                });
                if !running {
                    break;
                }
            }
        }));
        self.strat_visible = None;
        cx.notify();
    }

    /// Ask a busy database again after a pause, unless the inputs moved meanwhile (the new
    /// inputs then launch on their own).
    fn arm_auto_retry(&mut self, key: AutoKey, cx: &mut Context<Self>) {
        self.auto_picks.launch_task = Some(cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(RETRY_AFTER).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.auto_picks.launched == Some(key)
                        && matches!(this.auto_picks.run, AutoRun::Failed { .. })
                    {
                        this.run_auto_picks(key, cx);
                    }
                });
            });
        }));
    }

    /// Show the selected strategy's batch pick in the auto-pick card as if "Pick" had been
    /// pressed, once per strategy and batch.
    ///
    /// Only when the card would compute the same thing: one strategy selected, its lists
    /// loaded and unedited, the card idle, and the batch run with the card's current settings.
    /// "Reset" leaves the card idle afterwards, because the adoption is remembered.
    ///
    /// Args:
    ///     loaded: The card's coin statistics and saved lists have landed.
    ///     cx: View context, for the Auto-column rescan the suggestion feeds.
    pub(in crate::analytics::tuner) fn adopt_auto_pick(
        &mut self,
        loaded: bool,
        cx: &mut Context<Self>,
    ) {
        let Some((key, _)) = self.sel_strategy.clone() else {
            return;
        };
        let batch = self.auto_picks.seq;
        if !loaded
            || self.is_multi()
            || self.coins.pick != PickState::Idle
            || self.coins.has_changes()
            || !matches!(self.auto_picks.run, AutoRun::Done)
            // The whole launch key: a batch from an older scope or older lists is not this
            // strategy's answer any more, even while its replacement is still debounced.
            || self.auto_picks.ran != Some(self.auto_key())
            || self.auto_picks.adopted.as_ref() == Some(&(key.clone(), batch))
        {
            return;
        }
        let Some(pick) = self.auto_picks.picks.get(&key).cloned() else {
            return;
        };
        self.auto_picks.adopted = Some((key, batch));
        let has_suggestion = !pick.black.is_empty() || !pick.white.is_empty();
        if has_suggestion {
            self.coins.suggestion = Some(SuggestedLists {
                black: pick.black.iter().map(|c| c.token.clone()).collect(),
                white: pick.white.iter().map(|c| c.token.clone()).collect(),
            });
            self.coins.pick_rev = self.coins.pick_rev.wrapping_add(1);
        }
        self.coins.pick = PickState::Done {
            pick,
            lists_rev: self.coins.lists_rev,
            report_gen: self.coins.report_gen,
            holdout_pct: self.auto_picks.holdout_pct,
        };
        if has_suggestion {
            self.arm_coin_kpi(cx);
        }
    }
}

#[cfg(test)]
mod tests;
