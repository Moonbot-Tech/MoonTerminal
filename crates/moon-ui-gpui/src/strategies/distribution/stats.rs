//! Report figures behind the "WL distribution" tab: each coin's closed-trade profit, and the
//! trades of the coin the user clicked.
//!
//! WHOSE trades: every strategy that carries the NAME of a selected one, on the cores drawn as
//! rows. A coin moves between those cores as the list is redistributed, and its record follows it
//! because every row is read at once. Other cores of the same exchange are NOT included even when
//! they run a strategy of the same name: they are another distribution group, and its trades are
//! not this group's record (decided on 02.10 after an F2 trade showed up under BinF1-6).
//!
//! Both reads run off the UI thread and are keyed by a signature of what they ask — the strategy
//! scope, the period, the report and valuation generations — so a render only starts one when
//! that signature moved and none is in flight. A read that finished under an older signature is
//! still shown until its replacement lands, so a new trade does not blank the chips.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::atomic::Ordering;

use gpui::*;
use moon_core::db::analytics::{PreviousPeriodBasis, Query, coin_groups};
use moon_core::db::{
    ProfitMetric, QuoteCurrency, QuoteScope, ReportFilter, ReportStrategyKey, ReportTable,
    RowScope, SideFilter,
};
use moon_core::session::CoreId;
use moon_core::symbol::coin_match_key;

use crate::analytics::period::Period;
use crate::strategies::StrategiesView;

/// Most trades the bottom table lists for one coin, newest first.
pub(super) const TRADES_LIMIT: usize = 500;

/// One coin's closed trades over the scope and period.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct CoinStat {
    pub(super) trades: i64,
    pub(super) wins: i64,
    pub(super) profit: f64,
    /// The unit `profit` is in; `None` when the folded rows disagree or name none.
    pub(super) currency: Option<QuoteCurrency>,
}

/// One read's figures: per chip key, and the report spellings that folded into each key.
#[derive(Debug, Default)]
pub(super) struct Folded {
    pub(super) stats: HashMap<String, CoinStat>,
    /// The coin as the reports name it — `BTC`, `btc_0626` — per chip key. The trades table asks
    /// for exactly these, so its rows are the ones the chip's figures were summed from.
    pub(super) spellings: HashMap<String, Vec<String>>,
}

/// Which strategies, on which cores, the figures cover.
#[derive(Clone, Debug, Default, PartialEq)]
struct Scope {
    cores: Vec<u64>,
    keys: Vec<ReportStrategyKey>,
    /// Strategy name per `(core, strategy id)`, for the trades table: the report rows carry the id
    /// only, and the live store is where its name is.
    names: HashMap<(u64, i64), String>,
}

/// One background read and what it last produced.
struct Load<T> {
    /// Signature of the read started last; a different one starts the next.
    asked: Option<u64>,
    /// Advances per read, so a superseded answer is dropped.
    seq: u64,
    inflight: bool,
    data: Option<T>,
    error: Option<String>,
}

impl<T> Load<T> {
    /// Drop the result and any read in flight. The sequence ADVANCES rather than restarting, so
    /// an answer still on its way cannot match the read that replaces it.
    fn clear(&mut self) {
        self.asked = None;
        self.seq = self.seq.wrapping_add(1);
        self.inflight = false;
        self.data = None;
        self.error = None;
    }
}

impl<T> Default for Load<T> {
    fn default() -> Self {
        Self {
            asked: None,
            seq: 0,
            inflight: false,
            data: None,
            error: None,
        }
    }
}

/// The figures' state on the Strategies view.
pub(in crate::strategies) struct StatsState {
    pub(super) period: Period,
    /// Chips ordered by profit instead of by name.
    pub(super) by_profit: bool,
    /// The chip order runs the other way: names Z to A, or worst result first.
    pub(super) reversed: bool,
    /// The coin whose trades the bottom table shows.
    pub(super) coin: Option<String>,
    /// The row cores and the names the figures belong to; a change drops them.
    owner: Option<u64>,
    scope: Option<(u64, Rc<Scope>)>,
    profit: Load<Rc<Folded>>,
    trades: Load<Rc<ReportTable>>,
}

impl Default for StatsState {
    fn default() -> Self {
        Self {
            period: Period::All,
            by_profit: false,
            reversed: false,
            coin: None,
            owner: None,
            scope: None,
            profit: Load::default(),
            trades: Load::default(),
        }
    }
}

impl StatsState {
    /// Each coin's figures, as last read; empty until the first read lands.
    pub(super) fn coin_stats(&self) -> Option<&HashMap<String, CoinStat>> {
        self.profit.data.as_deref().map(|f| &f.stats)
    }

    pub(super) fn profit_error(&self) -> Option<&str> {
        self.profit.error.as_deref()
    }

    /// The live name of a strategy the figures cover.
    pub(super) fn strategy_name(&self, core: u64, id: i64) -> Option<&str> {
        let (_, scope) = self.scope.as_ref()?;
        scope.names.get(&(core, id)).map(String::as_str)
    }

    pub(super) fn trades(&self) -> Option<&Rc<ReportTable>> {
        self.trades.data.as_ref()
    }

    pub(super) fn trades_error(&self) -> Option<&str> {
        self.trades.error.as_deref()
    }

    /// Forget every figure: the board now holds other row cores or other strategy names.
    fn reset(&mut self) {
        self.scope = None;
        self.profit.clear();
        self.trades.clear();
        self.coin = None;
    }
}

/// Fold the per-coin groups of one read onto list match keys, the identity the chips use.
///
/// Args:
///     groups: `(raw report coin, closed trades, wins, profit, unit)` per group.
///
/// Returns:
///     Figures per match key; a key whose groups disagree about the unit keeps none, and its
///     profit is then not comparable with anything (see [`CoinStat::comparable_profit`]).
pub(super) fn fold_coins(
    groups: impl IntoIterator<Item = (String, i64, i64, f64, Option<QuoteCurrency>)>,
) -> Folded {
    let mut folded = Folded::default();
    for (coin, trades, wins, profit, currency) in groups {
        let key = coin_match_key(&coin);
        folded.spellings.entry(key.clone()).or_default().push(coin);
        let out = &mut folded.stats;
        match out.get_mut(&key) {
            None => {
                out.insert(
                    key,
                    CoinStat {
                        trades,
                        wins,
                        profit,
                        currency,
                    },
                );
            }
            Some(stat) => {
                stat.trades += trades;
                stat.wins += wins;
                stat.profit += profit;
                if stat.currency != currency {
                    stat.currency = None;
                }
            }
        }
    }
    folded
}

impl CoinStat {
    /// The profit, when it can be compared with another coin's: a closed trade exists, the sum
    /// is finite, and it is in one known unit. A chip colours and sorts by this and nothing else.
    pub(super) fn comparable_profit(&self) -> Option<f64> {
        (self.trades > 0 && self.currency.is_some() && self.profit.is_finite())
            .then_some(self.profit)
    }
}

impl StrategiesView {
    /// Start whichever report read the board now needs, and forget figures that describe another
    /// board.
    ///
    /// Args:
    ///     row_cores: The cores drawn as rows.
    ///     names: Names of the strategies drawn as rows.
    ///     cx: View context used to read the session and spawn the reads.
    pub(super) fn ensure_distribution_stats(
        &mut self,
        row_cores: &[CoreId],
        names: &BTreeSet<String>,
        cx: &mut Context<Self>,
    ) {
        let mut cores = row_cores.to_vec();
        cores.sort_unstable();
        // A different set of cores or names is a different question: the old figures must not
        // stand in for it while the new ones load.
        let mut h = DefaultHasher::new();
        cores.hash(&mut h);
        names.hash(&mut h);
        let owner = h.finish();
        if self.dist.stats.owner != Some(owner) {
            self.dist.stats.reset();
            self.dist.stats.owner = Some(owner);
            self.dist.trades_table = None;
            self.dist.trades_rows = None;
        }

        // Everything the reads need from the session, gathered under one borrow that ends before
        // the reads are spawned.
        let (scope_sig, scope, generation, valuation, core_names) = {
            let backend = self.backend.read(cx);
            let store = backend.session.store();

            // The scope moves only when a row core changes its strategy list: that is what
            // `strategies_rev` counts, so the walk below runs only then.
            let mut h = DefaultHasher::new();
            names.hash(&mut h);
            for core in &cores {
                core.hash(&mut h);
                store.core(*core).map(|cd| cd.strategies_rev).hash(&mut h);
            }
            let scope_sig = h.finish();
            let scope = match &self.dist.stats.scope {
                Some((sig, scope)) if *sig == scope_sig => scope.clone(),
                _ => {
                    let mut keys = Vec::new();
                    let mut key_names = HashMap::new();
                    for core in &cores {
                        let Some(cd) = store.core(*core) else {
                            continue;
                        };
                        for r in cd.strategies.iter().filter(|r| names.contains(&r.name)) {
                            let key = ReportStrategyKey {
                                core_uid: *core,
                                strategy_id: r.id as i64,
                            };
                            key_names.insert((key.core_uid, key.strategy_id), r.name.clone());
                            keys.push(key);
                        }
                    }
                    let scope = Rc::new(Scope {
                        cores: cores.clone(),
                        keys,
                        names: key_names,
                    });
                    self.dist.stats.scope = Some((scope_sig, scope.clone()));
                    scope
                }
            };
            if scope.keys.is_empty() {
                // The names no longer match any strategy on the row cores: nothing to read, and
                // whatever was read before describes strategies that are gone.
                let stats = &mut self.dist.stats;
                if stats.profit.data.is_some() || stats.profit.asked.is_some() {
                    stats.profit.clear();
                    stats.trades.clear();
                    stats.coin = None;
                }
                return;
            }

            let generation = backend
                .reports
                .as_ref()
                .map(|r| r.generation.load(Ordering::Relaxed))
                .unwrap_or(0)
                .wrapping_add(
                    backend
                        .valuation
                        .as_ref()
                        .map(|v| v.generation.load(Ordering::Relaxed))
                        .unwrap_or(0),
                );
            (
                scope_sig,
                scope,
                generation,
                backend.valuation_mode(),
                backend.report_core_names(),
            )
        };
        let zone = self.display_zone;
        let period = self.dist.stats.period;
        let mut h = DefaultHasher::new();
        scope_sig.hash(&mut h);
        period.id().hash(&mut h);
        // The civil day too, so "today" and the rolling presets are re-read on the first repaint
        // after midnight even when no trade has landed to bump a generation. Nothing wakes an
        // idle window AT midnight; the stale day lasts until the next repaint.
        let now = moon_core::util::now_unix_ms_i64() / 1000;
        moon_core::util::display_time::date(now, zone).hash(&mut h);
        zone.name().hash(&mut h);
        generation.hash(&mut h);
        valuation.hash(&mut h);
        let read_sig = h.finish();

        let axis = moon_core::db::ReportAxis::from_measured(Default::default(), zone);
        let (from, to) = period.range(zone);

        let profit = &mut self.dist.stats.profit;
        if profit.asked != Some(read_sig) && !profit.inflight {
            profit.asked = Some(read_sig);
            profit.seq = profit.seq.wrapping_add(1);
            profit.inflight = true;
            let seq = profit.seq;
            let query = Query {
                axis: axis.clone(),
                previous_period_basis: PreviousPeriodBasis::Civil,
                from,
                to,
                cores: scope.cores.clone(),
                side: SideFilter::All,
                emulator: Some(false),
                strategies: scope
                    .keys
                    .iter()
                    .map(|k| (k.strategy_id, Some(k.core_uid)))
                    .collect(),
                strategy_name_mask: String::new(),
                metric: ProfitMetric::Quote,
                valuation,
                // One unit for every row core, so the chips compare by colour.
                prefer_usdt: true,
                core_names: core_names.clone(),
            };
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_spawn(async move { coin_groups(&query) })
                    .await;
                cx.update(|cx| {
                    let _ = this.update(cx, |this, cx| {
                        let load = &mut this.dist.stats.profit;
                        if load.seq != seq {
                            return;
                        }
                        load.inflight = false;
                        match result {
                            Ok(groups) => {
                                load.error = None;
                                load.data =
                                    Some(Rc::new(fold_coins(groups.into_iter().map(|g| {
                                        let currency = match g.quote {
                                            QuoteScope::Single(c) => Some(c),
                                            _ => None,
                                        };
                                        (g.key, g.n, g.wins, g.profit, currency)
                                    }))));
                            }
                            Err(e) => load.error = Some(e.to_string()),
                        }
                        cx.notify();
                    });
                });
            })
            .detach();
        }

        let Some(coin) = self.dist.stats.coin.clone() else {
            return;
        };
        // Ask for the spellings the chip's figures were summed from, so the table lists the same
        // trades; before the figures land, the key itself.
        let spellings = self
            .dist
            .stats
            .profit
            .data
            .as_ref()
            .and_then(|f| f.spellings.get(&coin).cloned())
            .unwrap_or_else(|| vec![coin.clone()]);
        let mut h = DefaultHasher::new();
        read_sig.hash(&mut h);
        spellings.hash(&mut h);
        let trades_sig = h.finish();
        let trades = &mut self.dist.stats.trades;
        if trades.asked == Some(trades_sig) || trades.inflight {
            return;
        }
        trades.asked = Some(trades_sig);
        trades.seq = trades.seq.wrapping_add(1);
        trades.inflight = true;
        let seq = trades.seq;
        // `to` is exclusive on the Analytics side and inclusive in a report filter.
        let filter = ReportFilter {
            core_uids: scope.cores.clone(),
            date_from: (from >= 0).then_some(from),
            date_to: Some(to - 1),
            exact_coins: Some(spellings),
            side: SideFilter::All,
            emulator: Some(false),
            rows: RowScope::Closed,
            axis,
            strategies: Some(scope.keys.clone()),
            valuation,
            core_names,
            ..ReportFilter::default()
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let conn = moon_core::db::open_reader()?;
                    let _rates = moon_core::db::valuation::pin_current_rates();
                    let snap = moon_core::db::read_snapshot(&conn)?;
                    moon_core::db::query_reports(&snap, &filter, "closedate", true, TRADES_LIMIT)
                })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    let load = &mut this.dist.stats.trades;
                    if load.seq != seq {
                        return;
                    }
                    load.inflight = false;
                    match result {
                        Ok(table) => {
                            load.error = None;
                            load.data = Some(Rc::new(table));
                        }
                        Err(e) => load.error = Some(e.to_string()),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Show the trades of one coin, keeping them when it is the coin already shown: the right
    /// click that opens a chip's menu must not hide them (a second LEFT click does, through
    /// `toggle_distribution_coin`; so does "×").
    pub(super) fn show_distribution_coin(&mut self, coin: &str, cx: &mut Context<Self>) {
        if self.dist.stats.coin.as_deref() != Some(coin) {
            self.toggle_distribution_coin(coin, cx);
        }
    }

    /// Show, or hide again, the trades of one coin.
    pub(super) fn toggle_distribution_coin(&mut self, coin: &str, cx: &mut Context<Self>) {
        let dist = &mut self.dist;
        if dist.stats.coin.as_deref() == Some(coin) {
            dist.stats.coin = None;
            // A coin opened again starts from the newest trades, not from the last sort.
            dist.trades_table = None;
            dist.trades_rows = None;
        } else {
            dist.stats.coin = Some(coin.to_string());
            // The previous coin's rows must not sit under the new coin's heading while it loads,
            // nor its scroll position carry over to them.
            dist.stats.trades.clear();
            dist.trades_table = None;
        }
        cx.notify();
    }
}
