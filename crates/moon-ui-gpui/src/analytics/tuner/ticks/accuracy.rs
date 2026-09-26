//! How far a search's answer can be trusted (LinKvo, 2026-09-25): under the parameter grid, once
//! a search has run, the share of the tape the model reproduces — "entry 85.5 %, exit 54.4 %" —
//! said plainly, because the search learns on the reproduced trades only and a reader who sees
//! its plan should know how much of the strategy's history stands behind it.
//!
//! The honest base is every trade of the scope whose tape the terminal holds: a trade the model
//! misses (✗) and one it cannot judge at all (·, a rule it has no model of) both count against it
//! — that is the part of the history the answer does not speak for. The ✓ shares of the KPI
//! caption answer a narrower question (of the trades the model judged, how many it matched) and
//! stay as they are. What the model assumes where the data is silent — no order book, one latency
//! for every core, the verdict's tolerances — goes into the tooltip beside the counts, with the
//! numbers the model runs under.

use gpui::*;
use moon_core::db::tuner::ticks::{ModelSettings, Verdict};
use moon_ui::MoonPalette;
use rust_i18n::t;

use super::super::super::AnalyticsView;
use crate::design;
use crate::design::moon;

#[cfg(test)]
mod tests;

/// One group's verdicts over the trades with tape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::analytics::tuner) struct GroupCount {
    /// Reproduced within the verdict's tolerances.
    pub hits: usize,
    /// Judged and missed.
    pub misses: usize,
    /// Not judged: a rule the model has no model of, or a close the model's line cannot be
    /// compared with.
    pub unjudged: usize,
}

impl GroupCount {
    fn add(&mut self, verdict: Option<bool>) {
        match verdict {
            Some(true) => self.hits += 1,
            Some(false) => self.misses += 1,
            None => self.unjudged += 1,
        }
    }

    /// Reproduced trades over every trade with tape, per cent; `None` with none.
    pub fn pct(&self) -> Option<f64> {
        let n = self.hits + self.misses + self.unjudged;
        (n > 0).then(|| self.hits as f64 / n as f64 * 100.0)
    }
}

/// The model's accuracy over the scope's trades with tape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::analytics::tuner) struct Accuracy {
    pub entry: GroupCount,
    pub exit: GroupCount,
}

impl Accuracy {
    /// Count the verdicts of the trades with tape. A trade with tape and no verdict yet is
    /// unjudged in both groups — the tape is there, the model has not answered for it.
    pub fn of<'a>(verdicts: impl IntoIterator<Item = Option<&'a Verdict>>) -> Self {
        let mut out = Self::default();
        for verdict in verdicts {
            out.entry.add(verdict.and_then(|v| v.entry));
            out.exit.add(verdict.and_then(|v| v.exit));
        }
        out
    }

    /// Trades with tape the counts are over.
    pub fn n(&self) -> usize {
        self.exit.hits + self.exit.misses + self.exit.unjudged
    }
}

/// A share as the line prints it: one decimal, or a dash with nothing to count.
fn pct_text(pct: Option<f64>) -> String {
    let Some(pct) = pct else {
        return "—".to_string();
    };
    match moon_core::util::fmt::round_to(pct, 1) {
        Some(pct) => format!("{pct:.1} %"),
        None => "—".to_string(),
    }
}

/// The tooltip: what each share counts, then what the model assumes, with its numbers.
fn tooltip(acc: &Accuracy, entry_modelled: bool, without_tape: usize, fit: usize) -> String {
    let model = super::model_cfg::current();
    let group = |name: String, count: &GroupCount| {
        t!(
            "analytics.ticks.acc_tip_group",
            name = name,
            hits = count.hits,
            misses = count.misses,
            unjudged = count.unjudged
        )
        .to_string()
    };
    let mut lines = vec![t!("analytics.ticks.acc_tip_base", n = acc.n()).to_string()];
    if entry_modelled {
        lines.push(group(
            t!("analytics.ticks.group_entry").to_string(),
            &acc.entry,
        ));
    }
    lines.push(group(
        t!("analytics.ticks.group_exit").to_string(),
        &acc.exit,
    ));
    lines.push(t!("analytics.ticks.acc_tip_fit", fit = fit).to_string());
    if without_tape > 0 {
        lines.push(t!("analytics.ticks.acc_tip_no_tape", n = without_tape).to_string());
    }
    lines.push(String::new());
    lines.push(assumptions(&model));
    lines.join("\n")
}

/// What the model assumes where the data is silent, with the numbers it runs under.
fn assumptions(model: &ModelSettings) -> String {
    t!(
        "analytics.ticks.acc_tip_assumptions",
        latency = format!("{:.0}", model.latency_ms),
        price = format!("{}", model.price_pct),
        time = format!("{:.1}", model.point_time_ms as f64 / 1000.0),
        ticker = format!("{:.1}", model.ticker_period_ms as f64 / 1000.0)
    )
    .to_string()
}

impl AnalyticsView {
    /// The accuracy line under the parameter grid, once a search has run: the model's entry and
    /// exit shares over the scope's trades with tape. Nothing before the first search, while
    /// one runs, and with no trade with tape to count.
    pub(super) fn ticks_accuracy_row(&self, p: MoonPalette, cx: &App) -> Option<AnyElement> {
        if self.ticks.last_result.is_none()
            || matches!(self.ticks.sugg, super::state::SuggState::Running { .. })
        {
            return None;
        }
        let data = self.ticks.data.data()?;
        // Counted where the verdicts change (`TicksData::refresh_summary`), not per paint.
        let acc = data.accuracy;
        if acc.n() == 0 {
            return None;
        }
        let entry_modelled = data.entry_modelled();
        let entry = if entry_modelled {
            pct_text(acc.entry.pct())
        } else {
            t!("analytics.ticks.acc_entry_fact").to_string()
        };
        let text = t!(
            "analytics.ticks.acc_line",
            entry = entry,
            exit = pct_text(acc.exit.pct()),
            n = acc.n()
        )
        .to_string();
        let without_tape = data.rows.len().saturating_sub(acc.n());
        let fit = data.fit();
        Some(
            div()
                .id("an-ticks-accuracy")
                .w_full()
                .flex_none()
                .px(design::ui_px(cx, 12.0))
                .py(design::ui_px(cx, 4.0))
                .border_t_1()
                .border_color(moon(p.border))
                .truncate()
                .text_size(design::t_caption(cx))
                .font_family(design::ui_font())
                .text_color(moon(p.text_muted))
                // Built on hover only: the grid paints far more often than anyone reads it.
                .tooltip(move |window, cx| {
                    crate::panels::common::text_tooltip(tooltip(
                        &acc,
                        entry_modelled,
                        without_tape,
                        fit,
                    ))(window, cx)
                })
                .child(text)
                .into_any_element(),
        )
    }
}
