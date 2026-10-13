//! Pure view-model of the Summary tab: everything the KPI band, the insights list, the cores
//! ranking and the profit chart show, computed from one loaded snapshot without any rendering.
//!
//! Keeping it here, free of `gpui` elements, lets every rule — which direction is better, which
//! fact is worth an insight, which cores fold away — be pinned by plain unit tests, and lets the
//! render code stay a thin mapping from these values to MoonUI widgets.

use std::collections::BTreeSet;
use std::rc::Rc;

use moon_core::db::analytics::{CoreSeries, Summary};
use moon_core::util::fmt::{self, DeltaSign};
use moon_ui::MoonPalette;
use rust_i18n::t;

use super::state::{CoreFilter, CoreLines};
use super::{fmt_dm_hm, fmt_signed, strat_display_ex};
use crate::analytics::tuner::plural_key;

/// Which direction of change is BETTER for a metric (plan table, section 1).
///
/// Max drawdown is stored as a magnitude, so growth there is the worse direction. Hold time has
/// no better direction at all, so the band shows it as a muted caption with no delta.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics) enum DeltaRule {
    /// Growth is better: profit, winrate, profit factor, trades, average trade.
    Up,
    /// Shrinkage is better: drawdown.
    Down,
}

/// Meaning of a change, mapped to a palette token only at render time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics) enum DeltaTone {
    /// The metric moved in its better direction.
    Better,
    /// The metric moved in its worse direction.
    Worse,
    /// No direction to judge: the change rounds to zero or the metric has none.
    Neutral,
}

impl DeltaTone {
    /// Palette token for this tone: better green, worse orange, neutral `text_soft`.
    ///
    /// `text_soft` rather than `text_muted` for neutral: a neutral delta is still a figure the
    /// user reads, and `text_muted` misses the 4.5:1 caption contrast floor on both themes.
    pub(in crate::analytics) fn color(self, p: MoonPalette) -> u32 {
        match self {
            Self::Better => p.green,
            Self::Worse => p.orange,
            Self::Neutral => p.text_soft,
        }
    }
}

impl DeltaRule {
    /// Tone of a change whose ROUNDED sign is `sign`.
    pub(in crate::analytics) fn tone(self, sign: DeltaSign) -> DeltaTone {
        match self {
            Self::Up => sign.pick(DeltaTone::Better, DeltaTone::Worse, DeltaTone::Neutral),
            Self::Down => sign.pick(DeltaTone::Worse, DeltaTone::Better, DeltaTone::Neutral),
        }
    }
}

/// How one KPI's change against the previous period is spelled (plan table, section 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics) enum DeltaFmt {
    /// Profit: `+abs (+pct%)`, previous value as `was X`.
    Money,
    /// Winrate: arrow and percentage points, previous value as `was X%`.
    Points,
    /// Profit factor: arrow and the absolute change at two places, previous as `was X`.
    Ratio,
    /// Trades: arrow and relative percent, previous count as `was N`.
    Count,
    /// Average trade: arrow and relative percent, no previous value.
    Pct,
    /// Max drawdown: `deeper/shallower by x%`, no previous value.
    Drawdown,
}

/// One rendered comparison line of a KPI.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::analytics) struct DeltaLine {
    /// The change itself, arrow included where the format has one.
    pub(in crate::analytics) text: String,
    /// Meaning of the change.
    pub(in crate::analytics) tone: DeltaTone,
    /// The previous period's value (`was X`), when the format shows it.
    pub(in crate::analytics) prev_text: Option<String>,
}

/// Percentage change of `cur` against `prev`, over `|prev|` so the sign is the change's.
///
/// Args:
///     cur: Current-period value.
///     prev: Previous-period value, when one was loaded.
///
/// Returns:
///     Change in percent, or `None` when there is no non-zero baseline.
pub(in crate::analytics) fn pct_delta(cur: f64, prev: Option<f64>) -> Option<f64> {
    let prev = prev?;
    (prev.abs() > f64::EPSILON).then(|| (cur - prev) / prev.abs() * 100.0)
}

/// Arrow and unsigned digits of a relative percent change, classified from the rounded value.
///
/// Args:
///     delta: Percentage change, when comparable.
///
/// Returns:
///     `"▲ 168.7%"`-shaped text and its rounded sign, or `None` when not comparable.
pub(in crate::analytics) fn delta_parts(delta: Option<f64>) -> Option<(String, DeltaSign)> {
    let (text, sign) = fmt::pct(delta?, 1)?;
    Some((with_arrow(sign, text.trim_start_matches('-')), sign))
}

/// Prefix an unsigned figure with the arrow of its rounded sign; zero gets no arrow.
fn with_arrow(sign: DeltaSign, digits: &str) -> String {
    match sign {
        DeltaSign::Zero => digits.to_string(),
        _ => format!("{} {digits}", sign.pick("▲", "▼", "")),
    }
}

/// Unsigned digits of an absolute change at fixed places, with its rounded sign.
fn abs_change(diff: f64, decimals: usize) -> Option<(String, DeltaSign)> {
    let (text, sign) = fmt::signed_fixed(diff, decimals)?;
    Some((text.trim_start_matches(['+', '-']).to_string(), sign))
}

/// Build one KPI comparison line, or `None` when there is nothing to compare against.
///
/// Sign and colour come from the ROUNDED change, so a change that rounds to zero carries no
/// arrow and the neutral tone.
///
/// Args:
///     cur: Current-period value.
///     prev: Previous-period value, when loaded.
///     rule: Which direction is better for this metric.
///     kind: How the change is spelled.
///
/// Returns:
///     The comparison line, or `None` without a comparable previous value.
pub(in crate::analytics) fn delta_line(
    cur: f64,
    prev: Option<f64>,
    rule: DeltaRule,
    kind: DeltaFmt,
) -> Option<DeltaLine> {
    let prev = prev.filter(|v| v.is_finite())?;
    let was = |v: String| Some(t!("analytics.sum.was", v = v).to_string());
    // A zero baseline has no relative change, but the change itself is still news: a drawdown
    // that appeared from nothing is "deeper" by its whole size, and a count from zero reads
    // "was 0" in the rule's tone.
    if prev.abs() <= f64::EPSILON
        && matches!(kind, DeltaFmt::Count | DeltaFmt::Pct | DeltaFmt::Drawdown)
    {
        let sign = fmt::signed_fixed(cur - prev, 2)?.1;
        let text = match kind {
            DeltaFmt::Drawdown if sign == DeltaSign::Zero => return None,
            DeltaFmt::Drawdown => t!(
                "analytics.sum.dd_deeper",
                pct = format!("{}{}", fmt::compact(cur, 2), crate::analytics::pnl_suffix())
            )
            .to_string(),
            _ => t!("analytics.sum.was", v = "0").to_string(),
        };
        return Some(DeltaLine {
            text,
            tone: rule.tone(sign),
            prev_text: None,
        });
    }
    let (text, sign, prev_text) = match kind {
        DeltaFmt::Money => {
            let diff = cur - prev;
            let sign = fmt::signed_fixed(diff, 2)?.1;
            let d = fmt_signed(diff);
            let text = match pct_delta(cur, Some(prev)).and_then(|v| fmt::signed_pct(v, 1)) {
                Some((pct, _)) => t!("analytics.sum.delta_abs_pct", d = d, pct = pct).to_string(),
                None => d,
            };
            (text, sign, was(fmt_signed(prev)))
        }
        DeltaFmt::Points => {
            let (digits, sign) = abs_change(cur - prev, 1)?;
            let text = with_arrow(sign, &t!("analytics.sum.pp", v = digits));
            (text, sign, was(fmt::pct(prev, 1)?.0))
        }
        DeltaFmt::Ratio => {
            let (digits, sign) = abs_change(cur - prev, 2)?;
            (with_arrow(sign, &digits), sign, was(fmt::compact(prev, 2)))
        }
        DeltaFmt::Count => {
            let (text, sign) = delta_parts(pct_delta(cur, Some(prev)))?;
            (text, sign, was(fmt::compact(prev, 0)))
        }
        DeltaFmt::Pct => {
            let (text, sign) = delta_parts(pct_delta(cur, Some(prev)))?;
            (text, sign, None)
        }
        DeltaFmt::Drawdown => {
            let (pct, sign) = fmt::pct(pct_delta(cur, Some(prev))?, 1)?;
            let pct = pct.trim_start_matches('-').to_string();
            let text = match sign {
                DeltaSign::Positive => t!("analytics.sum.dd_deeper", pct = pct).to_string(),
                DeltaSign::Negative => t!("analytics.sum.dd_shallower", pct = pct).to_string(),
                DeltaSign::Zero => pct,
            };
            (text, sign, None)
        }
    };
    Some(DeltaLine {
        text,
        tone: rule.tone(sign),
        prev_text,
    })
}

/// Result group of the KPI band.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::analytics) struct KpiResult {
    /// Period profit in the active unit.
    pub(in crate::analytics) profit: f64,
    /// Unit caption beside the hero figure.
    pub(in crate::analytics) unit: String,
    /// Cumulative-profit points for the sparkline.
    pub(in crate::analytics) spark: Rc<[f32]>,
    /// Change against the previous period.
    pub(in crate::analytics) was: Option<DeltaLine>,
}

/// Quality group of the KPI band.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::analytics) struct KpiQuality {
    /// Winrate in percent.
    pub(in crate::analytics) winrate: f64,
    /// Previous winrate, for the meter's tick.
    pub(in crate::analytics) wr_prev: Option<f64>,
    /// Winrate change in percentage points.
    pub(in crate::analytics) wr_delta: Option<DeltaLine>,
    /// Profit factor.
    pub(in crate::analytics) pf: f64,
    /// Profit factor change.
    pub(in crate::analytics) pf_delta: Option<DeltaLine>,
}

/// Risk group of the KPI band.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::analytics) struct KpiRisk {
    /// Max drawdown magnitude.
    pub(in crate::analytics) max_dd: f64,
    /// Deeper/shallower line against the previous period.
    pub(in crate::analytics) dd_line: Option<DeltaLine>,
    /// Drawdown as a share of a positive profit (`N% of profit`).
    pub(in crate::analytics) dd_of_profit: Option<String>,
    /// Profit divided by drawdown, absent for a drawdown that rounds to zero.
    pub(in crate::analytics) recovery: Option<f64>,
}

/// Activity group of the KPI band.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::analytics) struct KpiActivity {
    /// Closed trades.
    pub(in crate::analytics) trades: i64,
    /// Trade-count change.
    pub(in crate::analytics) trades_delta: Option<DeltaLine>,
    /// Average trade profit.
    pub(in crate::analytics) avg: f64,
    /// Average-trade change.
    pub(in crate::analytics) avg_delta: Option<DeltaLine>,
    /// Average hold in minutes, shown neutrally beside the average trade.
    pub(in crate::analytics) hold_min: f64,
}

/// The four KPI groups of the Summary band.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::analytics) struct KpiBand {
    /// Profit with its sparkline.
    pub(in crate::analytics) result: KpiResult,
    /// Winrate and profit factor.
    pub(in crate::analytics) quality: KpiQuality,
    /// Drawdown and recovery factor.
    pub(in crate::analytics) risk: KpiRisk,
    /// Trades and average trade.
    pub(in crate::analytics) activity: KpiActivity,
}

impl KpiBand {
    /// Build the band from a snapshot and an explicit sparkline.
    ///
    /// Args:
    ///     d: Loaded Summary.
    ///     spark: Cumulative-profit points.
    ///     compare: Whether comparison lines are shown at all.
    ///
    /// Returns:
    ///     The band, comparisons hidden when `compare` is off or `d.prev` is absent.
    pub(in crate::analytics) fn new(d: &Summary, spark: Rc<[f32]>, compare: bool) -> Self {
        let cur = &d.cur;
        let prev = d.prev.as_ref().filter(|_| compare);
        let line = |v: f64, pv: Option<f64>, rule, kind| delta_line(v, pv, rule, kind);
        let dd_of_profit = (cur.profit > 0.0)
            .then(|| fmt::pct(cur.max_dd / cur.profit * 100.0, 0))
            .flatten()
            .map(|(pct, _)| t!("analytics.sum.dd_of_profit", pct = pct).to_string());
        Self {
            result: KpiResult {
                profit: cur.profit,
                unit: crate::analytics::pnl_unit_label().to_string(),
                spark,
                was: line(
                    cur.profit,
                    prev.map(|p| p.profit),
                    DeltaRule::Up,
                    DeltaFmt::Money,
                ),
            },
            quality: KpiQuality {
                winrate: cur.winrate(),
                wr_prev: prev.map(|p| p.winrate()),
                wr_delta: line(
                    cur.winrate(),
                    prev.map(|p| p.winrate()),
                    DeltaRule::Up,
                    DeltaFmt::Points,
                ),
                pf: cur.pf,
                pf_delta: line(cur.pf, prev.map(|p| p.pf), DeltaRule::Up, DeltaFmt::Ratio),
            },
            risk: KpiRisk {
                max_dd: cur.max_dd,
                dd_line: line(
                    cur.max_dd,
                    prev.map(|p| p.max_dd),
                    DeltaRule::Down,
                    DeltaFmt::Drawdown,
                ),
                dd_of_profit,
                recovery: cur.recovery_factor(),
            },
            activity: KpiActivity {
                trades: cur.n,
                trades_delta: line(
                    cur.n as f64,
                    prev.map(|p| p.n as f64),
                    DeltaRule::Up,
                    DeltaFmt::Count,
                ),
                avg: cur.avg,
                avg_delta: line(cur.avg, prev.map(|p| p.avg), DeltaRule::Up, DeltaFmt::Pct),
                hold_min: cur.avg_dur_min,
            },
        }
    }
}

/// Category tag of one insight, in the list's fixed order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics) enum InsightTag {
    /// The most profitable strategy.
    Leader,
    /// The coin that lost the most.
    Minus,
    /// The worst day of a daily grid.
    Day,
    /// The most profitable hour of the day.
    Time,
    /// The profit-factor verdict.
    Quality,
}

/// Meaning of an insight's value, mapped to a palette token at render time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics) enum InsightTone {
    /// A gain.
    Gain,
    /// A loss.
    Loss,
    /// Borderline: profit factor between 1.0 and 1.3.
    Edge,
}

/// One row of the "What matters" list.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::analytics) struct Insight {
    /// Category tag.
    pub(in crate::analytics) tag: InsightTag,
    /// What the fact is about: a strategy, coin, day, hour or the profit factor.
    pub(in crate::analytics) what: String,
    /// The headline value.
    pub(in crate::analytics) value: String,
    /// Meaning of the value.
    pub(in crate::analytics) value_tone: InsightTone,
    /// The explaining line, when the fact has one on this grid.
    pub(in crate::analytics) why: Option<String>,
}

/// Share of the day one hour covers, in percent; the best-hour insight compares against it.
const HOUR_SHARE_PCT: f64 = 100.0 / 24.0;

/// Seconds in one daily bucket; only this grid has day insights and day counts.
const DAY_SECS: i64 = 86_400;

/// Build the insights in fixed order: leader, minus, day, time, quality; a missing fact is omitted.
///
/// Args:
///     d: Loaded Summary.
///     zone: Display zone used to name the worst day.
///
/// Returns:
///     At most five insights; empty only for an empty period.
pub(in crate::analytics) fn insights(d: &Summary, zone: chrono_tz::Tz) -> Vec<Insight> {
    let cur = &d.cur;
    let mut out = Vec::with_capacity(5);
    let share = |part: f64, whole: f64| fmt::pct(part / whole * 100.0, 0).map(|(s, _)| s);

    if let Some(best) = d.strategies.first().filter(|s| s.profit > 0.0)
        && cur.profit > 0.0
    {
        let id = best.key.rsplit_once('@').map_or("", |(id, _)| id);
        let label = strat_display_ex(&best.name, id, best.name_is_id, best.alive);
        out.push(Insight {
            tag: InsightTag::Leader,
            what: t!("analytics.sum.ins.what.strategy", name = label.text).to_string(),
            value: fmt_signed(best.profit),
            value_tone: InsightTone::Gain,
            why: share(best.profit, cur.profit).map(|pct| {
                t!(
                    plural_key("analytics.sum.ins.why.leader", best.n as usize),
                    pct = pct,
                    winrate = fmt::pct(best.winrate(), 1).map(|v| v.0).unwrap_or_default(),
                    n = best.n
                )
                .to_string()
            }),
        });
    }

    if let Some(worst) = d.coins.last().filter(|c| c.profit < 0.0) {
        out.push(Insight {
            tag: InsightTag::Minus,
            what: t!("analytics.sum.ins.what.coin", name = worst.name).to_string(),
            value: fmt_signed(worst.profit),
            value_tone: InsightTone::Loss,
            why: Some(
                t!(
                    plural_key("analytics.sum.ins.why.minus", worst.n as usize),
                    n = worst.n,
                    winrate = fmt::pct(worst.winrate(), 1)
                        .map(|v| v.0)
                        .unwrap_or_default(),
                    worst = fmt_signed(worst.worst)
                )
                .to_string(),
            ),
        });
    }

    let daily = d.bucket_secs == DAY_SECS;
    if daily
        && cur.max_dd > 0.0
        && let Some(day) = d
            .days
            .iter()
            .filter(|day| day.profit < 0.0)
            .min_by(|a, b| a.profit.total_cmp(&b.profit))
    {
        let name: String = fmt_dm_hm(day.start, zone).chars().take(5).collect();
        out.push(Insight {
            tag: InsightTag::Day,
            what: t!("analytics.sum.ins.what.worst_day", name = name).to_string(),
            value: fmt_signed(day.profit),
            value_tone: InsightTone::Loss,
            why: fmt::pct((day.profit.abs() / cur.max_dd * 100.0).min(100.0), 0)
                .map(|(pct, _)| t!("analytics.sum.ins.why.day", pct = pct).to_string()),
        });
    }

    if let Some((hour, profit, trades)) = d.best_hour.filter(|h| h.1 > 0.0)
        && cur.profit > 0.0
    {
        let name = format!("{hour:02}:00–{:02}:00", (hour + 1) % 24);
        out.push(Insight {
            tag: InsightTag::Time,
            what: t!("analytics.sum.ins.what.best_hour", name = name).to_string(),
            value: fmt_signed(profit),
            value_tone: InsightTone::Gain,
            why: share(profit, cur.profit).map(|pct| {
                t!(
                    plural_key("analytics.sum.ins.why.hour", trades as usize),
                    n = trades,
                    pct = pct,
                    time_pct = fmt::pct(HOUR_SHARE_PCT, 0)
                        .map(|(s, _)| s)
                        .unwrap_or_default()
                )
                .to_string()
            }),
        });
    }

    if cur.n > 0 {
        let (verdict, tone) = if cur.pf >= 2.0 {
            (t!("analytics.ins.pf_great"), InsightTone::Gain)
        } else if cur.pf >= 1.3 {
            (t!("analytics.ins.pf_good"), InsightTone::Gain)
        } else if cur.pf >= 1.0 {
            (t!("analytics.ins.pf_edge"), InsightTone::Edge)
        } else {
            (t!("analytics.ins.pf_bad"), InsightTone::Loss)
        };
        // Day counts mean days only on a daily grid; hourly and weekly grids get no why-line
        // rather than calling hours or weeks "days".
        let why = daily.then(|| {
            let days = |base: &str, n: usize| t!(plural_key(base, n), n = n).to_string();
            t!(
                "analytics.sum.ins.why.quality",
                plus = days(
                    "analytics.sum.ins.why.quality_plus",
                    d.days.iter().filter(|day| day.profit > 0.0).count()
                ),
                minus = days(
                    "analytics.sum.ins.why.quality_minus",
                    d.days.iter().filter(|day| day.profit < 0.0).count()
                )
            )
            .to_string()
        });
        out.push(Insight {
            tag: InsightTag::Quality,
            what: t!(
                "analytics.sum.ins.what.pf",
                v = super::kpi::ratio_text(cur.pf)
            )
            .to_string(),
            value: verdict.to_string(),
            value_tone: tone,
            why,
        });
    }
    out
}

/// How concentrated the period's gains are across cores.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::analytics) struct Concentration {
    /// The best core's share of all gains, in percent.
    pub(in crate::analytics) leader_pct: f64,
    /// The five best cores' share of all gains, in percent.
    pub(in crate::analytics) top5_pct: f64,
    /// Fewest best cores whose gains reach half of all gains.
    pub(in crate::analytics) half_k: usize,
    /// Cores in the period.
    pub(in crate::analytics) n: usize,
    /// Losing cores' losses as a share of all gains, in percent.
    pub(in crate::analytics) eaten_pct: f64,
}

impl Concentration {
    /// Measure concentration over core totals; `None` when no core gained.
    ///
    /// Args:
    ///     cores: Per-core series of the period.
    ///
    /// Returns:
    ///     The concentration figures, or `None` when the sum of positive totals is not above zero.
    pub(in crate::analytics) fn of(cores: &[CoreSeries]) -> Option<Self> {
        let mut gains: Vec<f64> = cores.iter().map(|c| c.total).filter(|t| *t > 0.0).collect();
        gains.sort_by(|a, b| b.total_cmp(a));
        let sum: f64 = gains.iter().sum();
        if sum <= 0.0 {
            return None;
        }
        let losses: f64 = cores.iter().map(|c| c.total).filter(|t| *t < 0.0).sum();
        let mut acc = 0.0;
        let half_k = gains
            .iter()
            .position(|g| {
                acc += g;
                acc >= sum / 2.0
            })
            .map_or(gains.len(), |i| i + 1);
        Some(Self {
            leader_pct: gains[0] / sum * 100.0,
            top5_pct: gains.iter().take(5).sum::<f64>() / sum * 100.0,
            half_k,
            n: cores.len(),
            eaten_pct: losses.abs() / sum * 100.0,
        })
    }
}

/// One row of the cores ranking.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::analytics) enum CoreRowItem {
    /// A core, by index into the snapshot's `core_days`.
    Core(usize),
    /// The folded middle of a long list.
    Fold {
        /// Cores hidden behind the fold.
        hidden: usize,
        /// Totals of the first and last hidden cores.
        between: (f64, f64),
    },
}

/// Rows shown above the fold of a folded list.
const FOLD_TOP: usize = 8;
/// Rows shown below the fold of a folded list.
const FOLD_BOTTOM: usize = 5;

/// Whether an unfiltered list of `len` cores is long enough to fold its middle.
pub(in crate::analytics) fn folds(len: usize) -> bool {
    len > FOLD_TOP + FOLD_BOTTOM
}

/// Order the ranking rows for a filter, folding the middle of a long unfiltered list.
///
/// Args:
///     cores: Per-core series of the period.
///     filter: Which cores to list.
///     unfolded: Whether the user asked to see every core.
///
/// Returns:
///     Rows by total descending (losers ascending under `Neg`); with `All`, folded and more
///     than 13 cores: the top 8, one fold row, the bottom 5.
pub(in crate::analytics) fn core_rows(
    cores: &[CoreSeries],
    filter: CoreFilter,
    unfolded: bool,
) -> Vec<CoreRowItem> {
    let mut idx: Vec<usize> = (0..cores.len())
        .filter(|&i| match filter {
            CoreFilter::All => true,
            CoreFilter::Pos => cores[i].total > 0.0,
            CoreFilter::Neg => cores[i].total < 0.0,
        })
        .collect();
    match filter {
        CoreFilter::Neg => idx.sort_by(|&a, &b| cores[a].total.total_cmp(&cores[b].total)),
        _ => idx.sort_by(|&a, &b| cores[b].total.total_cmp(&cores[a].total)),
    }
    let fold = filter == CoreFilter::All && !unfolded && folds(idx.len());
    if !fold {
        return idx.into_iter().map(CoreRowItem::Core).collect();
    }
    let tail = idx.len() - FOLD_BOTTOM;
    let mut rows: Vec<CoreRowItem> = idx[..FOLD_TOP]
        .iter()
        .map(|&i| CoreRowItem::Core(i))
        .collect();
    rows.push(CoreRowItem::Fold {
        hidden: tail - FOLD_TOP,
        between: (cores[idx[FOLD_TOP]].total, cores[idx[tail - 1]].total),
    });
    rows.extend(idx[tail..].iter().map(|&i| CoreRowItem::Core(i)));
    rows
}

/// One shared scale for the ranking's diverging bars, independent of the active filter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::analytics) struct DivergingScale {
    /// Position of the zero axis across the bar track, 0..=1.
    pub(in crate::analytics) zero_frac: f32,
    /// Largest positive total.
    pub(in crate::analytics) max_pos: f64,
    /// Largest negative total, as a magnitude.
    pub(in crate::analytics) max_neg: f64,
}

impl DivergingScale {
    /// Scale over every core, so filtering never rescales the bars.
    ///
    /// Args:
    ///     cores: Per-core series of the period.
    ///
    /// Returns:
    ///     Zero sits at `max_neg / (max_pos + max_neg)`; at 0 when nothing moved.
    pub(in crate::analytics) fn of(cores: &[CoreSeries]) -> Self {
        let fin = |v: f64| if v.is_finite() { v } else { 0.0 };
        let max_pos = cores.iter().map(|c| fin(c.total)).fold(0.0, f64::max);
        let max_neg = cores.iter().map(|c| -fin(c.total)).fold(0.0, f64::max);
        let span = max_pos + max_neg;
        Self {
            zero_frac: if span > 0.0 {
                (max_neg / span) as f32
            } else {
                0.0
            },
            max_pos,
            max_neg,
        }
    }

    /// Bar of one total as `(start, width)` fractions of the track.
    pub(in crate::analytics) fn bar(&self, total: f64) -> (f32, f32) {
        let span = self.max_pos + self.max_neg;
        if span <= 0.0 || !total.is_finite() {
            return (self.zero_frac, 0.0);
        }
        let w = (total.abs() / span) as f32;
        if total >= 0.0 {
            (self.zero_frac, w)
        } else {
            (self.zero_frac - w, w)
        }
    }
}

/// Indices of the cores the chart draws a line for.
///
/// Args:
///     cores: Per-core series of the period.
///     lines: Chart line mode.
///     picked: Uids picked in the ranking.
///
/// Returns:
///     None: nothing; Top5: the five largest totals, best first; Selected: picked cores in
///     snapshot order.
pub(in crate::analytics) fn shown_cores(
    cores: &[CoreSeries],
    lines: CoreLines,
    picked: &BTreeSet<u64>,
) -> Vec<usize> {
    match lines {
        CoreLines::None => Vec::new(),
        CoreLines::Top5 => {
            let mut idx: Vec<usize> = (0..cores.len()).collect();
            idx.sort_by(|&a, &b| cores[b].total.total_cmp(&cores[a].total));
            idx.truncate(5);
            idx
        }
        CoreLines::Selected => (0..cores.len())
            .filter(|&i| picked.contains(&cores[i].uid))
            .collect(),
    }
}

/// Whether the chart asks the user to pick cores: Selected mode with no line actually drawn,
/// which includes picks of cores this snapshot does not have.
///
/// Args:
///     lines: Chart line mode.
///     shown: Indices of the drawn cores from [`shown_cores`].
///
/// Returns:
///     `true` when the hint shows.
pub(in crate::analytics) fn selection_hint_due(lines: CoreLines, shown: &[usize]) -> bool {
    lines == CoreLines::Selected && shown.is_empty()
}

/// Vertical range of the profit chart over the total and the cores actually shown.
///
/// Args:
///     total: Cumulative total curve.
///     curves: Cumulative curve of every core, indexed like `core_days`.
///     shown: Indices of the drawn cores.
///
/// Returns:
///     `(vmin, vmax)` with zero always inside and a non-zero upper bound.
pub(in crate::analytics) fn chart_range(
    total: &[f32],
    curves: &[Rc<[f32]>],
    shown: &[usize],
) -> (f32, f32) {
    let all = total.iter().chain(
        shown
            .iter()
            .filter_map(|&i| curves.get(i))
            .flat_map(|c| c.iter()),
    );
    let (lo, hi) = all
        .copied()
        .filter(|v| v.is_finite())
        .fold((0.0f32, 0.0f32), |(lo, hi), v| (lo.min(v), hi.max(v)));
    (lo, hi.max(1e-6))
}

#[cfg(test)]
mod tests;
