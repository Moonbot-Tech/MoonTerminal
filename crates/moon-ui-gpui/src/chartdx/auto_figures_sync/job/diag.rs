//! The auto-figure diagnostics channel (`channels.auto_figures` in `cfg/diagnostics.toml`, file
//! `logs/auto_figures_diag.log`): what one detector job of a chart pane saw and produced, row by row.
//!
//! Written from the job itself, on the background executor, so a followed market costs file I/O
//! there and never on the UI thread; a market the channel does not follow costs one atomic load per
//! job (`moon_core::diagnostics::auto_figures_follows`, asked when the job is built). The lines
//! answer the one question the chart cannot answer afterwards — which bars a row really read —
//! because the chart's own series, its history page by page and the cache all move under it.

use moon_chart::auto_figures::tf::BarSource;
use moon_chart::auto_figures::{Found, FoundShape, last_price, take};
use moon_core::config::auto_figures::Take;
use moon_core::market::ChartCandle;

use super::{PaneJob, RowKey};

/// The channel's file under `logs/`.
pub(crate) const FILE: &str = "auto_figures_diag.log";

/// What a job needs to write to the channel, present only when the channel follows its market.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JobDiag {
    /// Who the pane shows: `core/market`, as the line's first word.
    pub label: String,
    /// The `Take` of every row the job runs, by row index — what the row keeps is logged against
    /// the last close (the chart's own selection runs later, against the live price).
    pub takes: Vec<(usize, Take)>,
}

/// What one row of a job read and found, as [`row_lines`] reports it.
pub(crate) struct RowSeen<'a> {
    pub ix: usize,
    pub key: &'a RowKey,
    pub source: BarSource,
    pub needed: u32,
    /// The detector's input: the newest `needed` closed bars.
    pub bars: &'a [ChartCandle],
    pub found: &'a [Found],
    pub read_failed: bool,
    pub lagging: bool,
}

/// `YYYY-MM-DD HH:MM` (UTC) of a Unix-ms time.
fn at(ms: f64) -> String {
    let (date, hms) = moon_core::applog::split_unix_ms(ms as i64);
    format!("{date} {}", hms.get(..5).unwrap_or(&hms))
}

/// The job's header: its moment, the chart series every source was chosen against, the tick, and
/// the 5-minute base the cache gave. `base` is `None` when no row needed the base, `Some(None)`
/// when its read did not happen (the rows are then retried), and `Some(Some((rows, ring)))` its
/// rows and how many of them are ring rows without a close.
pub(crate) fn job_line(
    diag: &JobDiag,
    job: &PaneJob,
    base: Option<Option<(usize, usize)>>,
) -> String {
    let chart = match (job.chart.first(), job.chart.last()) {
        (Some(first), Some(last)) => format!(
            "{} bars of {}m [{} .. {}]",
            job.chart.len(),
            job.chart_tf_ms / 60_000,
            at(first.t_open_ms),
            at(last.t_open_ms)
        ),
        _ => "none".to_string(),
    };
    let base = match base {
        None => "not needed".to_string(),
        Some(None) => "READ FAILED".to_string(),
        Some(Some((rows, ring))) => format!("{rows} rows, {ring} from the ring"),
    };
    format!(
        "{} job now={} chart: {chart}; cache 5m base: {base}; tick={:?}",
        diag.label,
        at(job.now_ms),
        job.price_tick
    )
}

/// The lines of one row: its key and source, its bars' span and range, every found figure, what
/// the row's take keeps against the last close, and every input bar.
pub(crate) fn row_lines(diag: &JobDiag, row: &RowSeen<'_>, now_ms: f64, out: &mut Vec<String>) {
    let span = match (row.bars.first(), row.bars.last()) {
        (Some(first), Some(last)) => format!("[{} .. {}]", at(first.t_open_ms), at(last.t_open_ms)),
        _ => "[]".to_string(),
    };
    // Over finite prices only, and `-` when there is none: an empty row (no history, a failed
    // read) must not print a range of infinities as if it had bars.
    let range = |pick: fn(&ChartCandle) -> f32, better: fn(f32, f32) -> f32| {
        row.bars
            .iter()
            .map(pick)
            .filter(|v| v.is_finite())
            .reduce(better)
            .map_or("-".to_string(), |v| v.to_string())
    };
    let (lo, hi) = (range(|b| b.low, f32::min), range(|b| b.high, f32::max));
    let no_close = row.bars.iter().filter(|b| b.close.is_nan()).count();
    out.push(format!(
        "  row {} {:?} {:?} key_closed={} chart_ready={} source={:?} bars={}/{} {span} low={lo} \
         high={hi} no_close={no_close} read_failed={} lagging={}",
        row.ix,
        row.key.detector.kind(),
        row.key.tf,
        at(row.key.last_closed_ms as f64),
        row.key.chart_ready,
        row.source,
        row.bars.len(),
        row.needed,
        row.read_failed,
        row.lagging
    ));
    for (i, f) in row.found.iter().enumerate() {
        out.push(format!(
            "    found[{i}] {} touches={} [{} .. {}] tol={:.4}% broken={:?} origin={:?}",
            shape(&f.shape),
            f.touches.len(),
            at(f.first_ms),
            at(f.last_ms),
            f.tolerance_frac * 100.0,
            f.broken
                .map(|b| (at(b.at_ms), b.bars_since, b.longest_bars)),
            f.origin
        ));
    }
    if let (Some(take_cfg), Some(price)) = (
        diag.takes
            .iter()
            .find(|(ix, _)| *ix == row.ix)
            .map(|(_, t)| t),
        last_price(row.bars),
    ) {
        let kept = take::select(row.found, take_cfg, price, now_ms);
        out.push(format!("    kept against the last close {price}: {kept:?}"));
    }
    let bars: Vec<String> = row
        .bars
        .iter()
        .map(|b| {
            let close = if b.close.is_nan() {
                "-".to_string()
            } else {
                b.close.to_string()
            };
            format!("{} {}/{}/{close}", at(b.t_open_ms), b.high, b.low)
        })
        .collect();
    out.push(format!("    bars (high/low/close): {}", bars.join(" | ")));
}

/// A found figure's geometry in one word or two.
fn shape(s: &FoundShape) -> String {
    match *s {
        FoundShape::Horizontal { price } => format!("level {price}"),
        FoundShape::Sloped { p0, p1, .. } => format!("line {p0} -> {p1}"),
        FoundShape::Squeeze {
            level,
            p0,
            p1,
            pressed,
            ..
        } => format!("squeeze level {level} line {p0} -> {p1} pressed={pressed}"),
        FoundShape::Pair { upper, lower } => {
            format!(
                "pair upper {} -> {} lower {} -> {}",
                upper.p0, upper.p1, lower.p0, lower.p1
            )
        }
    }
}

/// Write a job's block: the header stamped with the wall clock and the rows under it, in one write
/// — jobs of other panes run in parallel and must not land between them.
pub(crate) fn write(header: &str, lines: &[String]) {
    moon_core::diagnostics::stamped_block(FILE, header, lines);
}

#[cfg(test)]
mod tests;
