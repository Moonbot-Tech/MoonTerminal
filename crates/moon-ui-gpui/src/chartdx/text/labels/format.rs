use super::*;

/// A PRICE caption: the shared adaptive formatter, with the field's prefix when it asks for one.
///
/// Five fields print a price this way — bid, ask, mark, step, liquidation — and spelling the same
/// pair of calls five times is how one of them ends up on a different formatter later.
pub(super) fn price_label(v: f64) -> (String, Option<DeltaSign>) {
    (fmt::adaptive(v), None)
}

/// A value with no sign to state.
pub(super) fn plain(text: &str) -> String {
    text.to_string()
}

/// One figure off the market readout, absent when the readout itself is.
pub(super) fn figure(
    inputs: &LabelInputs,
    pick: impl Fn(&MarketFiguresReadout) -> Option<f64>,
) -> Option<f64> {
    inputs.figures.as_ref().and_then(pick)
}

/// The window figures this caption is configured to read.
pub(super) fn window(
    inputs: &LabelInputs,
    part: &ChartLabelPart,
) -> Option<moon_core::market::WindowFigures> {
    let windows = inputs.windows.as_ref()?;
    windows.windows.get(part.window.index()).copied()
}

/// The period one caption reads: its span, and where that span sits on the time axis.
///
/// `None` when the caption measures around the pointer and the pointer is not on this pane. That is
/// the whole reason it is an `Option`: a measuring caption with nothing to measure prints a dash,
/// not the live edge's figure under a heading that says otherwise.
pub(super) fn volume_key(
    inputs: &LabelInputs,
    part: &ChartLabelPart,
) -> Option<(VolumeSpan, VolumeAt)> {
    let span = VolumeSpan::from_label(part.span, part.window);
    let at = match part.anchor {
        SpanAnchor::Now => VolumeAt::Now,
        SpanAnchor::Cursor => VolumeAt::Around(inputs.cursor_ms?),
    };
    Some((span, at))
}

/// The traded amounts this caption is configured to read.
pub(super) fn volume(inputs: &LabelInputs, part: &ChartLabelPart) -> Option<VolumeSpanReadout> {
    let want = volume_key(inputs, part)?;
    inputs
        .volumes
        .iter()
        .find(|(key, _)| *key == want)
        .map(|(_, readout)| *readout)
}

/// What was liquidated over this caption's period.
pub(super) fn liquidations(inputs: &LabelInputs, part: &ChartLabelPart) -> Option<LiqSpanReadout> {
    let want = volume_key(inputs, part)?;
    inputs
        .liquidations
        .iter()
        .find(|(key, _)| *key == want)
        .map(|(_, readout)| *readout)
}

/// Share of the period's traded value this caption's side accounts for, for the bar beside it.
///
/// `None` whenever there is nothing to compare: a caption that is not a side, a bar switched off,
/// or a market that did not trade — a full bar over a silent market would read as "all buying".
pub(super) fn volume_bar(part: &ChartLabelPart, inputs: &LabelInputs) -> Option<VolumeBar> {
    if !part.bar || !part.field.uses_volume_bar() {
        return None;
    }
    let readout = volume(inputs, part)?;
    // Finite AND positive: a market that did not trade has nothing to divide by, and a non-finite
    // total would put a NaN share into the cache key — where a value that does not equal itself
    // re-formats the whole corner on every revision.
    let total = readout.total_quote();
    if !total.is_finite() || total <= 0.0 {
        return None;
    }
    let sell = part.field == ChartLabelField::WindowSellVolume;
    let side = match sell {
        true => readout.sell_quote,
        false => readout.buy_quote,
    };
    Some(VolumeBar {
        fill: (side / total).clamp(0.0, 1.0) as f32,
        sell,
    })
}

/// The dash a MEASURING caption prints while the pointer is off the plot, if this is one.
///
/// `None` for a caption anchored at the live edge, which is simply absent when its market has no
/// history: the two look alike on screen and mean opposite things, so they are told apart here
/// rather than by whichever branch happened to run.
///
/// Applied to every figure in the block, not just one: the dash is what keeps the module its SHAPE
/// and its right-click target while there is nothing to measure. A block whose lines vanished one
/// by one would leave the reader nothing to aim at to switch the anchor back.
pub(super) fn no_cursor_dash(part: &ChartLabelPart) -> Option<(String, Option<DeltaSign>)> {
    (part.anchor == SpanAnchor::Cursor).then(|| (NO_CURSOR.to_string(), None))
}

/// What a measuring caption prints while the pointer is off the plot.
///
/// An em dash rather than nothing: the block keeps its shape and its right-click target, and the
/// reader can see it is waiting for a place to measure rather than broken.
pub(super) const NO_CURSOR: &str = "—";

/// The period a volume caption names, as the chart spells it: `1м`, `500 сд`.
///
/// One spelling for three shapes, so the block's own heading and the prefix on the figures under it
/// cannot disagree about what period is being shown.
pub(super) fn span_label(part: &ChartLabelPart) -> String {
    let period = match part.span {
        LabelSpan::Window => t!(part.window.locale_key()).to_string(),
        LabelSpan::Seconds(n) => t!("chart_labels.span.seconds", n = n).to_string(),
        LabelSpan::Minutes(n) => t!("chart_labels.span.minutes", n = n).to_string(),
        LabelSpan::Trades(n) => t!("chart_labels.span.trades", n = n).to_string(),
    };
    match part.anchor {
        SpanAnchor::Now => period,
        // Named on the caption, not only in the settings: the same `5м` means two different things
        // depending on where it is measured, and the block is read at a glance.
        SpanAnchor::Cursor => t!("chart_labels.span.at_cursor", period = period).to_string(),
    }
}

/// One traded amount, in the unit the caption asks for and marked when it is not whole.
///
/// The mark is the whole point of the pair of flags the readout carries: a period the retained
/// history does not reach back over still has a real figure in it — it just covers less than the
/// caption's own heading claims — and a coin figure over a stretch served by mini-candles is
/// missing that stretch entirely. Neither is hidden and neither is presented as complete.
pub(super) fn volume_amount(
    part: &ChartLabelPart,
    readout: VolumeSpanReadout,
    quote: f64,
    base: f64,
) -> (String, Option<DeltaSign>) {
    let (value, exact) = match part.units {
        VolumeUnits::Quote => (quote, true),
        VolumeUnits::Base => (base, readout.base_exact),
    };
    (
        marked(fmt::compact_si(value), readout.complete && exact),
        None,
    )
}

/// Mark a figure that covers less than the caption's own heading claims.
///
/// One spelling for every such figure, so a reader learns the mark once: `~` in front, the value
/// unchanged behind it.
pub(super) fn marked(text: String, whole: bool) -> String {
    match whole {
        true => text,
        false => format!("~{text}"),
    }
}

/// The coin's tags as one caption: `Seed · Alpha`.
///
/// One caption rather than one per tag, because the set is what is read — a coin is "a seed listing
/// that is also alpha" — and because a tag list that grew a column would push every figure beside
/// it off the pane.
pub(super) fn tags_text(tags: &[CoinTag]) -> String {
    tags.iter()
        .map(|t| t.name())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Format a signed percentage, dropping a non-finite value.
///
/// The caption's colour THRESHOLD is applied here, where the sign is born: below it the caption
/// keeps the theme colour and still prints its figure. Doing it here rather than at draw time is
/// what keeps one rule for every by-sign percentage — the deltas, funding, the arbitrage spreads —
/// instead of a check per drawing site.
pub(super) fn signed_pct_label(
    part: &ChartLabelPart,
    v: f64,
) -> Option<(String, Option<DeltaSign>)> {
    let (text, sign) = fmt::signed_pct(v, 2)?;
    Some((
        text,
        colored_sign(part.resolved_style().color_min_pct, v, sign),
    ))
}

/// The sign a percentage is coloured by, or `None` when it is too small to be worth painting.
///
/// `None` is not "no sign": it is what [`super::super::RenderState::caption_color`] already reads as "keep
/// the theme colour", which is exactly what a figure below the threshold should do.
pub(super) fn colored_sign(min_pct: f32, v: f64, sign: DeltaSign) -> Option<DeltaSign> {
    (v.abs() >= f64::from(min_pct)).then_some(sign)
}

/// Format a countdown as `2ч 05м`, `47м` or `<1м`, dropping one that has already elapsed.
///
/// A funding time in the past is not printed: the core republishes the next one within seconds, and
/// a negative countdown on screen reads as a stuck chart rather than as a stale field. Hours are
/// not carried past a day — funding intervals are hours, so a day-long remainder means the field is
/// wrong, and printing `27ч` says that more honestly than `1д 3ч`.
pub(super) fn fmt_countdown(remaining_ms: i64) -> Option<String> {
    if remaining_ms < 0 {
        return None;
    }
    let total_min = remaining_ms / 60_000;
    let (hours, minutes) = (total_min / 60, total_min % 60);
    Some(match (hours, minutes) {
        (0, 0) => t!("chart_labels.funding_soon").to_string(),
        (0, m) => format!("{m}{}", t!("chart_labels.unit_minute")),
        (h, m) => hours_and_minutes(h, m),
    })
}

/// `2ч 05м` — the one spelling both countdowns print once they are past an hour.
///
/// Shared rather than typed twice: the two callers round differently and suppress differently, but
/// what they PRINT at this range is one string, and two copies of it drift the moment a locale unit
/// or the zero-padding changes.
pub(in crate::chartdx) fn hours_and_minutes(hours: i64, minutes: i64) -> String {
    format!(
        "{hours}{} {minutes:02}{}",
        t!("chart_labels.unit_hour"),
        t!("chart_labels.unit_minute")
    )
}

/// The star a favourite button draws, in its two states.
///
/// Glyphs rather than icon assets, for the reason the lock beside it is one: a control drawn from a
/// font the whole application already loads.
pub(super) const STAR_ON: &str = "\u{2605}";
pub(super) const STAR_OFF: &str = "\u{2606}";

/// The lock a temporary-ban button draws, in its two states.
///
/// Glyphs rather than icon assets, matching the chart's own pin, lock and broom buttons beside
/// them: one control drawn from a font the whole application already loads.
pub(super) const LOCK_OPEN: &str = "\u{1F513}";
pub(super) const LOCK_CLOSED: &str = "\u{1F512}";

/// Whole units of `step` in `value`, rounded UP, with a negative value answering zero.
pub(super) fn ceil_div(value: i64, step: i64) -> i64 {
    let value = value.max(0);
    value.div_euclid(step) + i64::from(value.rem_euclid(step) > 0)
}

/// Format a candle countdown as `23ч 05м`, `47м 03с` or `42с`.
///
/// Three steps rather than one shape, because the figure a reader needs changes with the distance:
/// seconds are noise an hour out, and minutes alone are useless in the last one. The step is the
/// same threshold the sync quantizes its clock by, so the seconds shown are always seconds the
/// clock actually advances — a display finer than its own clock would print a frozen number.
///
/// Rounded UP at EVERY step, and that is a correctness rule rather than a taste: the clock this is
/// handed is quantized, and the quantum changes with what else the chart draws. Because the bucket
/// grid and both quanta are multiples of each other, a quantized clock yields exactly
/// `ceil(remaining / quantum) * quantum` — so rounding up again is idempotent and the SAME instant
/// prints the same figure on either step. Flooring the minutes while the seconds rounded up made
/// the caption read a whole minute lower whenever a second caption pulled the clock to its fine
/// step, which is a figure that changes for a reason the reader cannot see.
///
/// Rounding up is also what the last second needs: it reads `1с` and then the candle rolls, where
/// rounding down would print `0с` for a whole second and read as a stopped chart.
pub(super) fn fmt_tf_countdown(remaining_ms: i64) -> String {
    let secs = ceil_div(remaining_ms, 1000);
    if secs >= 3600 {
        let mins = ceil_div(secs, 60);
        return hours_and_minutes(mins / 60, mins % 60);
    }
    let (minutes, seconds) = (secs / 60, secs % 60);
    if minutes > 0 {
        return format!(
            "{minutes}{} {seconds:02}{}",
            t!("chart_labels.unit_minute"),
            t!("chart_labels.unit_second")
        );
    }
    format!("{seconds}{}", t!("chart_labels.unit_second"))
}

/// Longest detect line kept, in characters.
///
/// Not a layout figure — the chart lays this caption out by WIDTH, and wraps it — but a bound on
/// what is shaped at all: a core is free to send a paragraph, and the text pass would measure every
/// glyph of it before deciding what fits.
///
/// The feed's own bound rather than a second literal: this caption WRAPS, so cutting it at one
/// line's worth would throw away exactly what the second and third lines exist to show, and there
/// is nothing left to cut below what the ring already keeps.
pub(super) const DETECT_MSG_MAX: usize = moon_core::feed::DETECT_MSG_KEEP;

/// A detect line without the `(strategy <NAME>)` the core ends every one of them with.
///
/// The core writes that tail for its own log, where nothing else says which strategy fired. On a
/// chart it is the widest part of the line and says the least: the strategy has its own caption
/// beside this one, and what a reader wants from THIS caption is the numbers the detect fired on.
///
/// The tail is recognised only where it actually sits, so a line that MENTIONS a strategy
/// mid-sentence keeps every word of it. The format arrives on `moon_core::feed::DetectRow::msg`.
pub(super) fn detect_line(s: &str) -> String {
    let body = s.trim_end();
    let Some(at) = strategy_tail_start(body) else {
        return body.to_string();
    };
    // What is left of a line that carried nothing else is its own opening — "MoonStrike:" — and
    // that colon introduced a value which never existed on the wire. Only here: a line that keeps
    // its whole text keeps its own punctuation with it.
    body[..at]
        .trim_end()
        .trim_end_matches(':')
        .trim_end()
        .to_string()
}

/// Where a trailing `(strategy <NAME>)` begins, or `None` when the line does not end with one.
///
/// Anchored at BOTH ends: the group has to close the line, and what stands between the angle
/// brackets has to be a name and only a name. Matching the opening alone would cut a line off at
/// the first place it happened to say the word. Round brackets are NOT excluded — a user is free to
/// call a strategy `SP (long)`, and rejecting that would leave the tail on exactly those lines.
pub(super) fn strategy_tail_start(body: &str) -> Option<usize> {
    const OPEN: &str = "(strategy <";
    let at = body.rfind(OPEN)?;
    let inner = body.get(at + OPEN.len()..)?.strip_suffix(">)")?;
    (!inner.contains(['<', '>'])).then_some(at)
}

/// Cut a core-supplied line to something a caption can carry.
pub(super) fn cut(s: &str) -> String {
    if s.chars().count() <= DETECT_MSG_MAX {
        return s.to_string();
    }
    let kept: String = s.chars().take(DETECT_MSG_MAX).collect();
    format!("{}…", kept.trim_end())
}

pub(super) fn non_empty(s: &str) -> Option<String> {
    (!s.trim().is_empty()).then(|| s.to_string())
}

/// The caption's own prefix — `"PnL: "`, `"Δ24ч: "` — or nothing when it prints none.
///
/// Built beside the value rather than glued onto it, because the two are COLOURED separately: a
/// by-sign caption paints the figure and leaves the word in the theme's colour. The words come from
/// the dictionary, like everything else this feature prints.
///
/// A window figure names itself with its window — two "Δ" captions on one line are unreadable
/// unless each says whether it is the minute or the day — and a candle countdown names itself with
/// its timeframe for exactly the same reason.
pub(super) fn caption_prefix(part: &ChartLabelPart, on: bool, chart_tf_ms: i64) -> String {
    // The timeframe is this caption's IDENTITY, not its decoration: a chart carrying the hour's and
    // the day's countdowns beside each other is unreadable without it, and the countdowns are the
    // one figure a reader stacks several of. So the switch drops the WORD and keeps the period —
    // the mirror of the window rule below, which keeps the word and drops the period, and for the
    // same reason: whichever half says which figure this is, stays.
    //
    // `Авто` is resolved to the timeframe it currently means. Naming the setting would print the
    // same prefix on a minute chart and a day chart, which is the confusion this prefix exists to
    // remove.
    if part.field.uses_tf() {
        let period = t!(part.tf.resolved(chart_tf_ms).locale_key()).to_string();
        return match part.field.caption_key().filter(|_| on) {
            Some(key) => format!("{} {period}: ", t!(key).trim_end()),
            None => format!("{period}: "),
        };
    }
    let Some(key) = part.field.caption_key() else {
        return String::new();
    };
    let name = t!(key);
    // On a caption that reads a PERIOD the switch governs the period alone. `Bv` is not decoration
    // there — it is which side the figure is, and a block whose lines lost their names would be two
    // bare numbers under one heading. So switching the prefix off drops `1м` and keeps `Bv`, which
    // is exactly what a reader asks for once the heading above already states the period.
    if part.field.uses_window() {
        let period = match on {
            true => span_label(part),
            false => String::new(),
        };
        return format!("{}{period}: ", name.trim_end());
    }
    // Everywhere else the switch is what it always was: print the caption, or print none.
    match on {
        true => format!("{name}: "),
        false => String::new(),
    }
}
