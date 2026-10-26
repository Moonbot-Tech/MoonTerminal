//! Custom-EMA formulas and streaming evaluation over held tape, without database access.
//!
//! Written units select second EMAs, minute prices or hourly candle prices; MAX/MIN use
//! historical extrema and Vol compares average minute quote volumes.
//! AND binds tighter than OR; expression parentheses are not supported.
//! https://moonbot.pro/documentation/advanced-strategy-parameters/ema-strategy-and-its-parameters

use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use super::deltas::Bar;
use crate::feed::types::Tick;

/// Bound dense allocations even when a malformed tape spans years.
pub const MAX_SERIES_S: usize = 3 * 86_400;

/// Recognize custom-formula FilterCheck exits without conflating ordinary filter bounds.
pub fn is_custom_ema_reason(reason: &str) -> bool {
    let reason = reason.to_ascii_lowercase();
    super::verify::reason_starts_with(reason.trim(), "Sell by FilterCheck:")
        && [
            "ema(", "max(", "min(", "mavg(", "avg(", "vol(", "btc(", "arb(",
        ]
        .iter()
        .any(|term| reason.contains(term))
}

/// A supported function, or the original spelling of an unsupported function.
#[derive(Clone, Debug, PartialEq)]
pub enum FnName {
    Ema,
    Max,
    Min,
    Vol,
    Unsupported(String),
}

/// Written price family, preserved even when two durations have the same seconds.
/// https://moonbot.pro/documentation/advanced-strategy-parameters/ema-strategy-and-its-parameters
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitFamily {
    Sec,
    Min,
    Hour,
}

/// A function's X and Y durations in seconds, retaining their written price families.
/// https://moonbot.pro/documentation/advanced-strategy-parameters/ema-strategy-and-its-parameters
#[derive(Clone, Debug, PartialEq)]
pub struct Term {
    pub fn_name: FnName,
    pub window_s: u32,
    pub step_s: u32,
    pub window_family: UnitFamily,
    pub step_family: UnitFamily,
}

impl Term {
    /// Keep X/Y slot order identical in the streaming evaluator and the independent oracle.
    fn slots(&self) -> impl Iterator<Item = (u32, UnitFamily)> {
        [
            (self.window_s, self.window_family),
            (self.step_s, self.step_family),
        ]
        .into_iter()
    }
}

/// A comparison number and its source bytes, including its sign but excluding `%`.
#[derive(Clone, Debug, PartialEq)]
pub struct Threshold {
    pub value: f64,
    pub span: Range<usize>,
}

/// Strict comparison supported by the core's formula syntax.
/// https://moonbot.pro/documentation/advanced-strategy-parameters/ema-strategy-and-its-parameters
#[derive(Clone, Debug, PartialEq)]
pub enum Cmp {
    Lt,
    Gt,
}

/// Indices of a function and comparison number in their formula's tables.
#[derive(Clone, Debug, PartialEq)]
pub struct Cond {
    pub term: usize,
    pub cmp: Cmp,
    pub threshold: usize,
}

/// OR of AND groups, retaining thresholds' byte spans for string-preserving edits.
/// https://moonbot.pro/documentation/advanced-strategy-parameters/ema-strategy-and-its-parameters
#[derive(Clone, Debug, PartialEq)]
pub struct Formula {
    pub expr: Vec<Vec<Cond>>,
    pub terms: Vec<Term>,
    pub thresholds: Vec<Threshold>,
}

/// Invalid syntax at a byte offset in the original UTF-8 source.
#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    pub offset: usize,
}

/// Parse one nonempty formula, rejecting malformed supported terms and comparisons.
pub fn parse(src: &str) -> Result<Formula, ParseError> {
    let mut parser = Parser { src, pos: 0 };
    let mut formula = Formula {
        expr: vec![Vec::new()],
        terms: Vec::new(),
        thresholds: Vec::new(),
    };
    loop {
        let term = parser.term()?;
        parser.space();
        let cmp = match parser.byte() {
            Some(b'<') => Cmp::Lt,
            Some(b'>') => Cmp::Gt,
            _ => return Err(parser.error()),
        };
        parser.pos += 1;
        let threshold = parser.threshold()?;
        let cond = Cond {
            term: formula.terms.len(),
            cmp,
            threshold: formula.thresholds.len(),
        };
        formula.terms.push(term);
        formula.thresholds.push(threshold);
        // There is always a current AND group, including immediately after OR.
        if let Some(group) = formula.expr.last_mut() {
            group.push(cond);
        }
        let end = parser.pos;
        parser.space();
        if parser.pos == src.len() {
            return Ok(formula);
        }
        if parser.pos == end {
            return Err(parser.error());
        }
        let start = parser.pos;
        while parser.byte().is_some_and(|b| b.is_ascii_alphabetic()) {
            parser.pos += 1;
        }
        let connective = &src[start..parser.pos];
        if connective.eq_ignore_ascii_case("OR") {
            formula.expr.push(Vec::new());
        } else if !connective.eq_ignore_ascii_case("AND") {
            return Err(parser.error());
        }
        let end = parser.pos;
        parser.space();
        if parser.pos == end {
            return Err(parser.error());
        }
    }
}

/// Byte cursor; only ASCII syntax is consumed, keeping source spans exact.
struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

impl Parser<'_> {
    /// Return the current source byte without advancing.
    fn byte(&self) -> Option<u8> {
        self.src.as_bytes().get(self.pos).copied()
    }

    /// Skip UTF-8 whitespace without losing byte boundaries.
    fn space(&mut self) {
        self.pos += self.src[self.pos..].len() - self.src[self.pos..].trim_start().len();
    }

    /// Describe a syntax failure at the current byte.
    fn error(&self) -> ParseError {
        ParseError { offset: self.pos }
    }

    /// Parse documented duration ranges; cross-exchange calls retain opaque arguments.
    /// https://moonbot.pro/documentation/advanced-strategy-parameters/ema-strategy-and-its-parameters
    fn term(&mut self) -> Result<Term, ParseError> {
        self.space();
        let start = self.pos;
        while self.byte().is_some_and(|b| b.is_ascii_alphabetic()) {
            self.pos += 1;
        }
        if start == self.pos {
            return Err(self.error());
        }
        let name = &self.src[start..self.pos];
        let fn_name = if name.eq_ignore_ascii_case("EMA") {
            FnName::Ema
        } else if name.eq_ignore_ascii_case("MAX") {
            FnName::Max
        } else if name.eq_ignore_ascii_case("MIN") {
            FnName::Min
        } else if name.eq_ignore_ascii_case("VOL") {
            FnName::Vol
        } else {
            FnName::Unsupported(name.to_owned())
        };
        self.space();
        if self.byte() != Some(b'(') {
            return Err(self.error());
        }
        self.pos += 1;
        let start = self.pos;
        let Some(end) = self.src[start..].find(')') else {
            return Err(self.error());
        };
        let args = &self.src[start..start + end];
        if args.trim().is_empty() || args.contains('(') {
            return Err(self.error());
        }
        let numeric = args
            .split_once(',')
            .and_then(|(window, step)| Some((duration(window)?, duration(step)?)));
        let ((window_s, window_family), (step_s, step_family)) = match numeric {
            Some(values) => values,
            None if matches!(fn_name, FnName::Unsupported(_)) => {
                ((0, UnitFamily::Sec), (0, UnitFamily::Sec))
            }
            None => return Err(self.error()),
        };
        let valid_price = |seconds, family| match family {
            UnitFamily::Sec => (1..=300).contains(&seconds),
            UnitFamily::Min => (120..=5400).contains(&seconds),
            UnitFamily::Hour => (3600..=147600).contains(&seconds),
        };
        let valid = match &fn_name {
            FnName::Vol => {
                let allowed = |s, family| match family {
                    UnitFamily::Sec => [5, 15, 30].contains(&s),
                    UnitFamily::Min => [60, 180, 300, 900, 1800].contains(&s),
                    UnitFamily::Hour => [3600, 10800, 86400].contains(&s),
                };
                (window_s == 0 || allowed(window_s, window_family)) && allowed(step_s, step_family)
            }
            FnName::Max | FnName::Min => {
                ((window_family == UnitFamily::Min && (300..=5400).contains(&window_s))
                    || (window_family == UnitFamily::Hour && (3600..=147600).contains(&window_s)))
                    && valid_price(step_s, step_family)
            }
            FnName::Unsupported(name)
                if !["BTC", "MAVG", "AVG"]
                    .iter()
                    .any(|n| name.eq_ignore_ascii_case(n)) =>
            {
                true
            }
            _ => valid_price(window_s, window_family) && valid_price(step_s, step_family),
        };
        if !valid {
            return Err(self.error());
        }
        self.pos = start + end + 1;
        Ok(Term {
            fn_name,
            window_s,
            step_s,
            window_family,
            step_family,
        })
    }

    /// Parse a finite signed decimal (optionally scientific) and an optional percent sign.
    fn threshold(&mut self) -> Result<Threshold, ParseError> {
        self.space();
        let start = self.pos;
        if matches!(self.byte(), Some(b'+' | b'-')) {
            self.pos += 1;
        }
        while self.byte().is_some_and(|b| b.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.byte() == Some(b'.') {
            self.pos += 1;
            while self.byte().is_some_and(|b| b.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if matches!(self.byte(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.byte(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            while self.byte().is_some_and(|b| b.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        let span = start..self.pos;
        let value = self.src[span.clone()]
            .parse::<f64>()
            .map_err(|_| self.error())?;
        if !value.is_finite() {
            return Err(self.error());
        }
        if self.byte() == Some(b'%') {
            self.pos += 1;
        }
        Ok(Threshold { value, span })
    }
}

/// Parse a whole duration without discarding its written family or overflowing.
fn duration(src: &str) -> Option<(u32, UnitFamily)> {
    let src = src.trim();
    let end = src.bytes().take_while(u8::is_ascii_digit).count();
    let number = src[..end].parse::<u32>().ok()?;
    let (multiplier, family) = match src[end..].trim().to_ascii_lowercase().as_str() {
        "" | "s" | "sec" => (1, UnitFamily::Sec),
        "m" | "min" => (60, UnitFamily::Min),
        "h" | "hour" | "hours" => (3600, UnitFamily::Hour),
        _ => return None,
    };
    Some((number.checked_mul(multiplier)?, family))
}

/// Dense inclusive UTC-second range, with no price before its first valid print.
#[derive(Clone, Debug, PartialEq)]
pub struct SecondSeries {
    from_s: i64,
    last: Vec<Option<f64>>,
    quote_vol: Vec<f64>,
    quote_prefix: Vec<f64>,
    minute_from_s: i64,
    minute_prices: Vec<Option<f64>>,
}

/// A live prepared tape owns cache identity; weak ownership prevents pointer-reuse hits.
struct CachedSeries {
    tape: Weak<[Tick]>,
    series: Arc<SecondSeries>,
}

/// Pointer identity, slice length and inclusive dense-series window.
type SeriesKey = (usize, usize, i64, i64);
/// Shared results keyed by prepared tape and dense window.
type SeriesEntries = std::collections::HashMap<SeriesKey, CachedSeries>;

/// Amortize expired-tape scans over cache growth rather than every missed window.
#[derive(Default)]
struct SeriesCache {
    entries: SeriesEntries,
    prune_at: usize,
}

thread_local! {
    /// Only search replays provide immutable, shared tape ownership to the cache.
    static SERIES_TAPE: std::cell::RefCell<Option<Weak<[Tick]>>> = const { std::cell::RefCell::new(None) };
}

/// Restore an outer replay's tape even if the inner replay unwinds.
struct TapeScope(Option<Weak<[Tick]>>);

impl Drop for TapeScope {
    /// Leave no borrowed replay identity in a rayon worker after a request ends.
    fn drop(&mut self) {
        SERIES_TAPE.with(|slot| *slot.borrow_mut() = self.0.take());
    }
}

/// Share dense series across variants while the prepared tape remains alive.
pub(super) fn with_series_tape<T>(tape: &Arc<[Tick]>, replay: impl FnOnce() -> T) -> T {
    let _scope = TapeScope(SERIES_TAPE.with(|slot| slot.replace(Some(Arc::downgrade(tape)))));
    replay()
}

/// Cache by immutable tape identity and exact inclusive window; ordinary borrowed tapes bypass it.
pub(super) fn cached_series(ticks: &[Tick], from_s: i64, to_s: i64) -> Arc<SecondSeries> {
    static CACHE: OnceLock<Mutex<SeriesCache>> = OnceLock::new();
    let tape = SERIES_TAPE.with(|slot| slot.borrow().as_ref().and_then(Weak::upgrade));
    let Some(tape) = tape.filter(|tape| std::ptr::eq(tape.as_ref(), ticks)) else {
        return Arc::new(SecondSeries::from_ticks(ticks, from_s, to_s));
    };
    let mut cache = CACHE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let key = (tape.as_ptr() as usize, tape.len(), from_s, to_s);
    if let Some(entry) = cache.entries.get(&key).filter(|entry| {
        entry
            .tape
            .upgrade()
            .is_some_and(|own| Arc::ptr_eq(&own, &tape))
    }) {
        return Arc::clone(&entry.series);
    }
    if cache.entries.len() >= cache.prune_at.max(64) {
        cache
            .entries
            .retain(|_, entry| entry.tape.strong_count() > 0);
        cache.prune_at = cache.entries.len().saturating_mul(2).max(64);
    }
    drop(cache);
    let series = Arc::new(SecondSeries::from_ticks(ticks, from_s, to_s));
    CACHE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .entries
        .entry(key)
        .or_insert_with(|| CachedSeries {
            tape: Arc::downgrade(&tape),
            series,
        })
        .series
        .clone()
}

impl SecondSeries {
    /// Aggregate valid prints within `from_s..=to_s`, then forward-fill empty prices.
    /// Input order need not be chronological; equal-time prints retain slice order.
    /// The latest valid earlier print seeds price only, preserving clipped warm-up history.
    /// Invalid quantities are skipped; oversized ranges yield empty history.
    pub fn from_ticks(ticks: &[Tick], from_s: i64, to_s: i64) -> Self {
        let len = to_s
            .checked_sub(from_s)
            .and_then(|n| n.checked_add(1))
            .and_then(|n| usize::try_from(n).ok())
            .filter(|&n| n <= MAX_SERIES_S)
            .unwrap_or(0);
        let mut series = Self {
            from_s,
            last: vec![None; len],
            quote_vol: vec![0.0; len],
            quote_prefix: vec![0.0; len + 1],
            minute_from_s: from_s.saturating_add((60 - from_s.rem_euclid(60)) % 60),
            minute_prices: Vec::new(),
        };
        let mut observed = vec![false; len];
        let mut last_times = vec![f64::NEG_INFINITY; len];
        let mut previous = None;
        let mut previous_time = f64::NEG_INFINITY;
        for tick in ticks {
            if !tick.time_ms.is_finite()
                || !tick.price.is_finite()
                || tick.price <= 0.0
                || !tick.qty.is_finite()
                || tick.qty < 0.0
            {
                continue;
            }
            let second = (tick.time_ms / 1000.0).floor();
            if second < from_s as f64 {
                if tick.time_ms >= previous_time {
                    previous_time = tick.time_ms;
                    previous = Some(f64::from(tick.price));
                }
                continue;
            }
            if second > to_s as f64 {
                continue;
            }
            let Some(index) = (second as i64)
                .checked_sub(from_s)
                .and_then(|n| usize::try_from(n).ok())
                .filter(|&i| i < len)
            else {
                continue;
            };
            if tick.time_ms >= last_times[index] {
                last_times[index] = tick.time_ms;
                series.last[index] = Some(f64::from(tick.price));
                observed[index] = true;
            }
            series.quote_vol[index] += f64::from(tick.price) * f64::from(tick.qty);
        }
        for last in &mut series.last {
            if last.is_some() {
                previous = *last;
            } else {
                *last = previous;
            }
        }
        for (index, volume) in series.quote_vol.iter().enumerate() {
            series.quote_prefix[index + 1] = series.quote_prefix[index] + volume;
        }
        let mut previous_minute = None;
        if let Some(first) = series.index(series.minute_from_s) {
            for start in (first..len)
                .step_by(60)
                .take_while(|start| *start + 60 <= len)
            {
                if observed[start..start + 60].iter().any(|seen| *seen) {
                    previous_minute = series.last[start..start + 60]
                        .iter()
                        .copied()
                        .sum::<Option<f64>>()
                        .map(|sum| sum / 60.0);
                }
                series.minute_prices.push(previous_minute);
            }
        }
        series
    }

    /// Locate a held second; outside the inclusive range is missing history.
    fn index(&self, second: i64) -> Option<usize> {
        usize::try_from(second.checked_sub(self.from_s)?)
            .ok()
            .filter(|&i| i < self.last.len())
    }

    /// Read the forward-filled last price of a held second.
    fn price(&self, second: i64) -> Option<f64> {
        self.last[self.index(second)?]
    }
}

/// Compare two second-family prices, each smoothed independently over per-second last prices.
/// At boundary B, pass `at_s = B/1000 - 1`: its entire bucket precedes B.
/// The documented weight is 1/(N+1); calibrated EMA(60s) matches within 0.05 pp in
/// 56 percent of probes at 2/(N+1), versus 33 percent at the documented weight, so keep 2/(N+1).
/// https://moonbot.pro/documentation/advanced-strategy-parameters/ema-strategy-and-its-parameters
pub fn ema_term(series: &SecondSeries, at_s: i64, window_s: u32, step_s: u32) -> Option<f64> {
    Some(
        (second_price(series, at_s, step_s)? / second_price(series, at_s, window_s)? - 1.0) * 100.0,
    )
}

/// Pure second-price oracle; recursive history starts with the first available price.
/// https://moonbot.pro/documentation/advanced-strategy-parameters/ema-strategy-and-its-parameters
fn second_price(series: &SecondSeries, at_s: i64, seconds: u32) -> Option<f64> {
    if seconds == 0 {
        return None;
    }
    let end = series.index(at_s)?;
    let alpha = 2.0 / (f64::from(seconds) + 1.0);
    let mut average = None;
    for price in series.last[..=end].iter().flatten() {
        average = Some(match average {
            Some(old) => old + alpha * (price - old),
            None => *price,
        });
    }
    average
}

/// Average minute quote volume over Y divided by that over X; X=0 returns Y's mean.
/// At boundary B, pass `at_s = B/1000 - 1`; prints at B are excluded.
/// https://moonbot.pro/documentation/advanced-strategy-parameters/ema-strategy-and-its-parameters
pub fn vol_term(series: &SecondSeries, at_s: i64, window_s: u32, step_s: u32) -> Option<f64> {
    if step_s == 0 {
        return None;
    }
    let stop = series.index(at_s)? + 1;
    let mean = |seconds: u32| {
        let start = stop.checked_sub(seconds as usize)?;
        Some((series.quote_prefix[stop] - series.quote_prefix[start]) * 60.0 / f64::from(seconds))
    };
    let recent = mean(step_s)?;
    if window_s == 0 {
        return Some(recent);
    }
    let base = mean(window_s)?;
    Some(if base > 0.0 { recent / base } else { 0.0 })
}

/// Minute N back uses its UTC-aligned bucket; an empty bucket carries the preceding mean.
/// `at_s` is the second completed just before the current boundary.
/// https://moonbot.pro/documentation/advanced-strategy-parameters/ema-strategy-and-its-parameters
fn minute_price(series: &SecondSeries, at_s: i64, seconds: u32) -> Option<f64> {
    let start = (at_s + 1).div_euclid(60) * 60 - i64::from(seconds);
    let index = usize::try_from((start - series.minute_from_s).div_euclid(60)).ok()?;
    series.minute_prices.get(index).copied().flatten()
}

/// Hour prices weight closes by duration; hourly extrema use candle highs/lows.
/// Minute bars replace overlapping coarse bars without rescanning the whole history per bar.
/// https://moonbot.pro/documentation/advanced-strategy-parameters/ema-strategy-and-its-parameters
fn bar_price(bars: &[Bar], at_s: i64, seconds: u32, extreme: Option<bool>) -> Option<f64> {
    let alignment_s = if extreme.is_some() { 300 } else { 3600 };
    let hour_ms = (at_s + 1).div_euclid(alignment_s) * alignment_s * 1000;
    let start = hour_ms - i64::from(seconds) * 1000 - if extreme.is_some() { 3_600_000 } else { 0 };
    let stop = if extreme.is_some() {
        hour_ms - 3_600_000
    } else {
        start + 3_600_000
    };
    let values = bars.iter().filter(|bar| {
        bar.from_ms >= start
            && bar.to_ms <= stop
            && [60_000, 300_000].contains(&(bar.to_ms - bar.from_ms))
            && bar.close.is_finite()
            && bar.close > 0.0
    });
    if let Some(maximum) = extreme {
        values
            .filter(|bar| bar.to_ms - bar.from_ms == 300_000)
            .map(|bar| if maximum { bar.high } else { bar.low })
            .filter(|value| value.is_finite() && *value > 0.0)
            .reduce(|a, b| if maximum { a.max(b) } else { a.min(b) })
    } else {
        let mut minutes: Vec<_> = bars
            .iter()
            .filter(|bar| bar.to_ms - bar.from_ms == 60_000)
            .collect();
        minutes.sort_unstable_by_key(|bar| bar.to_ms);
        let values = values.filter(|bar| {
            bar.to_ms - bar.from_ms != 300_000
                || minutes
                    .get(minutes.partition_point(|minute| minute.to_ms <= bar.from_ms))
                    .is_none_or(|minute| minute.from_ms >= bar.to_ms)
        });
        let (sum, duration) = values.fold((0.0, 0_i64), |(sum, duration), bar| {
            let weight = bar.to_ms - bar.from_ms;
            (sum + bar.close * weight as f64, duration + weight)
        });
        (duration > 0).then(|| sum / duration as f64)
    }
}

/// Resolve a written family without substituting tape for hourly candle history.
/// https://moonbot.pro/documentation/advanced-strategy-parameters/ema-strategy-and-its-parameters
fn price(
    series: &SecondSeries,
    bars: &[Bar],
    at_s: i64,
    seconds: u32,
    family: UnitFamily,
) -> Option<f64> {
    match family {
        UnitFamily::Sec => second_price(series, at_s, seconds),
        UnitFamily::Min => minute_price(series, at_s, seconds),
        UnitFamily::Hour => bar_price(bars, at_s, seconds, None),
    }
}

/// Historical MAX/MIN exclude the latest minute or hour, respectively.
/// https://moonbot.pro/documentation/advanced-strategy-parameters/ema-strategy-and-its-parameters
fn extreme_price(series: &SecondSeries, bars: &[Bar], at_s: i64, term: &Term) -> Option<f64> {
    let maximum = term.fn_name == FnName::Max;
    match term.window_family {
        UnitFamily::Min => (2..=term.window_s / 60)
            .try_fold(None, |extreme, n| {
                let value = minute_price(series, at_s, n * 60)?;
                Some(Some(extreme.map_or(value, |old: f64| {
                    if maximum {
                        old.max(value)
                    } else {
                        old.min(value)
                    }
                })))
            })
            .flatten(),
        UnitFamily::Hour => bar_price(bars, at_s, term.window_s, Some(maximum)),
        UnitFamily::Sec => None,
    }
}

/// Why a formula cannot honestly be evaluated on the held tape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Unsupported,
    Unparsed,
    ShortHistory,
}

impl Formula {
    /// Rewrite comparison positions without changing any other source bytes or sentinels.
    pub fn with_thresholds(&self, src: &str, new: &[(usize, f64)]) -> String {
        let mut out = src.to_owned();
        for (slot, threshold) in self.thresholds.iter().enumerate().rev() {
            if threshold.value.abs() >= 888.0 {
                continue;
            }
            if let Some((_, value)) = new
                .iter()
                .rev()
                .find(|(index, value)| *index == slot && value.is_finite() && value.abs() < 888.0)
            {
                out.replace_range(
                    threshold.span.clone(),
                    &super::params::range::spell_number(*value),
                );
            }
        }
        out
    }

    /// Structural identity, independent of threshold spelling, whitespace and function case.
    pub fn skeleton(&self) -> String {
        format!(
            "{:?}|{:?}",
            self.terms
                .iter()
                .map(|term| (
                    match &term.fn_name {
                        FnName::Unsupported(name) => name.to_ascii_uppercase(),
                        name => format!("{name:?}"),
                    },
                    term.window_s,
                    term.step_s,
                    term.window_family,
                    term.step_family
                ))
                .collect::<Vec<_>>(),
            self.expr
        )
    }

    /// Sentinel positions are part of compatibility, even though their digits are masked.
    pub fn sentinel_mask(&self) -> Vec<bool> {
        self.thresholds
            .iter()
            .map(|threshold| threshold.value.abs() >= 888.0)
            .collect()
    }

    /// Largest calibrated window, ignoring opaque unsupported terms.
    pub fn longest_window_s(&self) -> u32 {
        self.terms
            .iter()
            .filter(|term| !matches!(term.fn_name, FnName::Unsupported(_)))
            .map(|term| term.window_s.max(term.step_s))
            .max()
            .unwrap_or(0)
    }

    /// First unsupported function, with its original source spelling.
    pub fn unsupported(&self) -> Option<&str> {
        self.terms.iter().find_map(|term| match &term.fn_name {
            FnName::Unsupported(name) => Some(name.as_str()),
            _ => None,
        })
    }

    /// Warm seconds for three EMA periods, whole historical buckets for minutes/hours, or volume windows.
    /// https://moonbot.pro/documentation/advanced-strategy-parameters/ema-strategy-and-its-parameters
    pub fn warm_up_s(&self) -> u32 {
        self.terms.iter().map(term_warm_up).max().unwrap_or(0)
    }

    /// Function values at this second; unsupported or data-less terms are None.
    pub fn term_values(&self, series: &SecondSeries, bars: &[Bar], at_s: i64) -> Vec<Option<f64>> {
        self.terms
            .iter()
            .map(|term| match term.fn_name {
                FnName::Ema | FnName::Max | FnName::Min => {
                    let prices: Vec<_> = term
                        .slots()
                        .map(|(seconds, family)| price(series, bars, at_s, seconds, family))
                        .collect();
                    let base = if term.fn_name == FnName::Ema {
                        prices[0]
                    } else {
                        extreme_price(series, bars, at_s, term)
                    };
                    Some((prices[1]? / base? - 1.0) * 100.0)
                }
                FnName::Vol => vol_term(series, at_s, term.window_s, term.step_s),
                FnName::Unsupported(_) => None,
            })
            .collect()
    }

    /// Evaluate the last completed second at boundary B using `at_s = B/1000 - 1`.
    /// Each bucket ends before B; sentinel thresholds still require that term's history.
    /// Missing values refuse only when they can change the Boolean outcome.
    /// https://moonbot.pro/documentation/advanced-strategy-parameters/ema-strategy-and-its-parameters
    pub fn eval(&self, series: &SecondSeries, bars: &[Bar], at_s: i64) -> Result<bool, Refusal> {
        let values = self
            .term_values(series, bars, at_s)
            .into_iter()
            .zip(&self.terms)
            .map(|(value, term)| {
                let covered = if term.fn_name == FnName::Vol {
                    tape_covers(series, at_s, term)
                } else {
                    term.slots().all(|(seconds, family)| match family {
                        UnitFamily::Hour => true,
                        _ => series
                            .price(at_s - i64::from(price_warm_up(seconds, family)))
                            .is_some(),
                    })
                };
                covered.then_some(value).flatten()
            })
            .collect::<Vec<_>>();
        self.eval_values(&values)
    }

    /// Combine known and missing terms without making an irrelevant arm refuse the formula.
    fn eval_values(&self, values: &[Option<f64>]) -> Result<bool, Refusal> {
        if self.unsupported().is_some() {
            return Err(Refusal::Unsupported);
        }
        if self.expr.is_empty() || self.expr.iter().any(Vec::is_empty) {
            return Err(Refusal::Unparsed);
        }
        let mut missing_arm = false;
        for group in &self.expr {
            let mut all = true;
            let mut missing = false;
            for cond in group {
                let value = values.get(cond.term).ok_or(Refusal::Unparsed)?;
                let threshold = self
                    .thresholds
                    .get(cond.threshold)
                    .ok_or(Refusal::Unparsed)?
                    .value;
                let Some(value) = value else {
                    missing = true;
                    continue;
                };
                let passes = if threshold.abs() >= 888.0 {
                    true
                } else {
                    match cond.cmp {
                        Cmp::Lt => *value < threshold,
                        Cmp::Gt => *value > threshold,
                    }
                };
                all &= passes;
            }
            if all && !missing {
                return Ok(true);
            }
            missing_arm |= all && missing;
        }
        if missing_arm {
            Err(Refusal::ShortHistory)
        } else {
            Ok(false)
        }
    }
}

/// Family-specific history needed by one price.
/// https://moonbot.pro/documentation/advanced-strategy-parameters/ema-strategy-and-its-parameters
fn price_warm_up(seconds: u32, family: UnitFamily) -> u32 {
    match family {
        UnitFamily::Sec => seconds * 3,
        UnitFamily::Min => seconds + 60,
        UnitFamily::Hour => seconds + 3600,
    }
}

/// History needed by one term, allowing independent OR/AND coverage decisions.
/// https://moonbot.pro/documentation/advanced-strategy-parameters/ema-strategy-and-its-parameters
fn term_warm_up(term: &Term) -> u32 {
    match term.fn_name {
        FnName::Vol => term.window_s.max(term.step_s),
        FnName::Unsupported(_) => 0,
        _ => price_warm_up(term.window_s, term.window_family)
            .max(price_warm_up(term.step_s, term.step_family)),
    }
}

/// Whether both the warm boundary and evaluation second have tape prices.
fn tape_covers(series: &SecondSeries, at_s: i64, term: &Term) -> bool {
    if term.window_family == UnitFamily::Hour || term.step_family == UnitFamily::Hour {
        return false;
    }
    at_s.checked_sub(i64::from(term_warm_up(term)))
        .and_then(|start| series.price(start))
        .is_some()
        && series.price(at_s).is_some()
}

/// Which source supplied the first evaluable formula check's EMA history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmaHistoryPath {
    Tape,
    KlineWarmed,
}

/// Independent second-family averages for X and Y, advancing once over the tape.
struct EmaState {
    averages: Vec<Option<f64>>,
    next: usize,
    warmed_from_s: Option<i64>,
    warmed: [bool; 2],
    historical: [Option<(i64, Option<f64>)>; 2],
}

/// Streaming formula evaluator borrowing the single dense series built for a walk.
pub struct FormulaState<'a> {
    formula: &'a Formula,
    series: &'a SecondSeries,
    ema: Vec<EmaState>,
    values: Vec<Option<f64>>,
    at_s: Option<i64>,
    used_bars: bool,
    bars: Vec<Bar>,
    extreme_bars: Vec<Bar>,
}

impl<'a> FormulaState<'a> {
    /// Start with the oracle's first-nonempty-sample seeding and no kline history.
    pub fn new(formula: &'a Formula, series: &'a SecondSeries) -> Self {
        Self {
            formula,
            series,
            ema: formula
                .terms
                .iter()
                .map(|_| EmaState {
                    averages: vec![None; 2],
                    next: 0,
                    warmed_from_s: None,
                    warmed: [false; 2],
                    historical: [None; 2],
                })
                .collect(),
            values: vec![None; formula.terms.len()],
            at_s: None,
            used_bars: false,
            bars: Vec::new(),
            extreme_bars: Vec::new(),
        }
    }

    /// Keep minute-first candle history and seed second EMAs missing a continuous tape prefix.
    /// Hour prices use candles directly; candle volume never supplies tape volume.
    /// https://moonbot.pro/documentation/advanced-strategy-parameters/ema-strategy-and-its-parameters
    pub fn warm_from_bars(&mut self, bars: &[Bar], first_at_s: i64) {
        self.extreme_bars = bars
            .iter()
            .filter(|bar| bar.to_ms - bar.from_ms == 300_000)
            .copied()
            .collect();
        for state in &mut self.ema {
            state.historical = [None; 2];
        }
        if self
            .formula
            .terms
            .iter()
            .all(|term| tape_covers(self.series, first_at_s, term))
        {
            return;
        }
        // Deal::bars is time-sorted; both cursors visit each bar at most once.
        let mut minutes = bars
            .iter()
            .filter(|bar| bar.to_ms - bar.from_ms == 60_000)
            .peekable();
        self.bars = bars
            .iter()
            .filter(|bar| {
                if bar.to_ms - bar.from_ms != 300_000 {
                    return true;
                }
                while minutes
                    .peek()
                    .is_some_and(|minute| minute.from_ms < bar.from_ms)
                {
                    minutes.next();
                }
                !minutes
                    .peek()
                    .is_some_and(|minute| minute.from_ms < bar.to_ms)
            })
            .copied()
            .collect::<Vec<_>>();
        for (term, state) in self.formula.terms.iter().zip(&mut self.ema) {
            if !matches!(term.fn_name, FnName::Ema | FnName::Max | FnName::Min) {
                continue;
            }
            for (slot, (seconds, family)) in term.slots().enumerate() {
                if family != UnitFamily::Sec || (slot == 0 && term.fn_name != FnName::Ema) {
                    continue;
                }
                let start_s = first_at_s - i64::from(price_warm_up(seconds, family));
                if self.series.price(start_s).is_some() {
                    continue;
                }
                let Some(start_ms) = start_s.checked_mul(1000) else {
                    continue;
                };
                let Some(tape_ms) = self.series.from_s.checked_mul(1000) else {
                    continue;
                };
                let alpha = 2.0 / (f64::from(seconds) + 1.0);
                let mut cursor = start_ms;
                let mut average = None;
                let mut last_close = None;
                for bar in self
                    .bars
                    .iter()
                    .filter(|bar| bar.to_ms > start_ms && bar.to_ms <= tape_ms)
                {
                    if bar.from_ms > cursor
                        || bar.to_ms <= cursor
                        || !bar.close.is_finite()
                        || bar.close <= 0.0
                    {
                        continue;
                    }
                    let duration_s = (bar.to_ms - cursor).div_euclid(1000);
                    let steps = duration_s as f64;
                    average = Some(match average {
                        None => bar.close,
                        Some(old) => bar.close + (old - bar.close) * (1.0_f64 - alpha).powf(steps),
                    });
                    cursor = bar.to_ms;
                    last_close = Some(bar.close);
                }
                // A bar spanning the tape's start proves that the closed prefix has no missing
                // tail. Its close is never sampled: the previous closed price holds until tape.
                let contiguous_tail = cursor == tape_ms
                    || self
                        .bars
                        .iter()
                        .any(|bar| bar.from_ms <= cursor && bar.to_ms > tape_ms);
                if contiguous_tail && average.is_some() {
                    if let (Some(old), Some(close)) = (average, last_close) {
                        let steps = (tape_ms - cursor) as f64 / 1000.0;
                        average = Some(close + (old - close) * (1.0_f64 - alpha).powf(steps));
                    }
                    state.averages[slot] = average;
                    state.warmed_from_s = Some(start_s);
                    state.warmed[slot] = true;
                }
            }
        }
    }

    /// Advance monotonically to the completed second; EMA work is linear over the walk.
    /// Volume uses prefix differences, so its cost does not grow with its window.
    /// https://moonbot.pro/documentation/advanced-strategy-parameters/ema-strategy-and-its-parameters
    pub fn advance_to(&mut self, at_s: i64) -> Result<bool, Refusal> {
        if self.at_s.is_some_and(|old| old > at_s) {
            return Err(Refusal::Unparsed);
        }
        self.at_s = Some(at_s);
        self.used_bars = false;
        for (index, (term, state)) in self.formula.terms.iter().zip(&mut self.ema).enumerate() {
            let end = self.series.index(at_s);
            let value = match (end, &term.fn_name) {
                (Some(end), FnName::Ema | FnName::Max | FnName::Min) => {
                    for i in state.next..=end {
                        if let Some(price) = self.series.last[i] {
                            for (slot, (seconds, family)) in term.slots().enumerate() {
                                if family != UnitFamily::Sec
                                    || (slot == 0 && term.fn_name != FnName::Ema)
                                {
                                    continue;
                                }
                                let alpha = 2.0 / (f64::from(seconds) + 1.0);
                                let avg = &mut state.averages[slot];
                                *avg = Some(avg.map_or(price, |old| old + alpha * (price - old)));
                            }
                        }
                    }
                    state.next = end + 1;
                    let mut prices = [None; 2];
                    for (slot, (seconds, family)) in term.slots().enumerate() {
                        let extreme = slot == 0 && term.fn_name != FnName::Ema;
                        prices[slot] = if family == UnitFamily::Sec {
                            (state.warmed[slot]
                                || self
                                    .series
                                    .price(at_s - i64::from(price_warm_up(seconds, family)))
                                    .is_some())
                            .then_some(state.averages[slot])
                            .flatten()
                        } else {
                            let period = match (family, extreme) {
                                (UnitFamily::Hour, true) => 300,
                                (UnitFamily::Hour, false) => 3600,
                                _ => 60,
                            };
                            let key = (at_s + 1).div_euclid(period);
                            let cached = &mut state.historical[slot];
                            if cached.is_none_or(|(old, _)| old != key) {
                                let value = if extreme {
                                    extreme_price(self.series, &self.extreme_bars, at_s, term)
                                } else {
                                    price(self.series, &self.bars, at_s, seconds, family)
                                };
                                *cached = Some((key, value));
                            }
                            cached.and_then(|(_, value)| value)
                        };
                        self.used_bars |= prices[slot].is_some()
                            && (family == UnitFamily::Hour || state.warmed[slot]);
                    }
                    prices[0].zip(prices[1]).map(|(x, y)| (y / x - 1.0) * 100.0)
                }
                (Some(_), FnName::Vol) if tape_covers(self.series, at_s, term) => {
                    vol_term(self.series, at_s, term.window_s, term.step_s)
                }
                _ => None,
            };
            self.values[index] = value;
        }
        self.formula.eval_values(&self.values)
    }

    /// Source used by the current check, recorded once when the clock arms successfully.
    pub fn history_path(&self) -> EmaHistoryPath {
        if self.used_bars {
            EmaHistoryPath::KlineWarmed
        } else {
            EmaHistoryPath::Tape
        }
    }
}

#[cfg(test)]
mod tests;
