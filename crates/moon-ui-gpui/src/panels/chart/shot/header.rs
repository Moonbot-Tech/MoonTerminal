//! The text burnt into the shot: one header line above the chart.
//!
//! A picture of a chart with no writing on it explains nothing once it leaves the application —
//! which coin, which exchange, when, at what candle size and at what vertical scale are all facts
//! the pixels do not carry. This module decides WHAT that line says, HOW ITS PARTS ARE RANKED,
//! IN WHICH ORDER it yields when there is not enough room, and WHERE the surviving run starts;
//! `super::paint_win` decides what that ranking looks like in pixels and `super::ink` decides what
//! colour it is drawn in.
//!
//! # Hierarchy, and why it is expressed as data rather than as drawing
//!
//! A header where every field is the same size, the same weight and the same colour is a PILE: the
//! reader has to parse it word by word to find the one number they opened the picture for. So a
//! field is not a string here — it is a sequence of styled [`TextRun`]s, and the style is a ROLE
//! ([`RunStyle`]) rather than a font. Three roles is the whole vocabulary: the coin is the subject,
//! the movement figures are what a reader scans for, and everything else is context. Assigning
//! them here rather than inside the `TextOutW` loop is what makes the hierarchy testable on a
//! platform that cannot draw.
//!
//! For the same reason the fields carry a [`LeadGap`] instead of the drawing code inferring one:
//! the line is three GROUPS — identity, the VIEW, the MARKET — separated by nothing but a wider
//! space. There are no separator glyphs at all. A rule or a dot between two fields groups them; a
//! rule between EVERY pair is a fence, and on a narrow picture every glyph is one more thing that
//! has to fit.
//!
//! # Why the strip clips instead of wrapping
//!
//! The strip is a fixed height in final pixels, so a second line cannot appear — there is nowhere
//! for it to go. The house answer to that shape is `panels/report/totals.rs::footer_facts`: a HEAD
//! that never yields, then a TAIL whose left-to-right order IS the clip priority. Building it as a
//! pure function is what makes the priority testable at all; a layout that only exists inside a
//! drawing call can only be checked by looking at it.
//!
//! The same argument is why [`centred_start_x`] lives here rather than beside the `TextOutW` that
//! consumes it: `super::paint_win` is Windows-only, so arithmetic placed there is arithmetic no
//! test on another platform ever executes.
//!
//! Deliberately free of GPUI, of GDI and of the `windows` crate: nothing here measures or draws,
//! and that is what lets its unit tests run on every platform.

use chrono::TimeZone as _;
use chrono_tz::Tz;
use moon_core::util::fmt;
use rust_i18n::t;

/// What one run of text IS to the reader, which the drawing pass turns into a size and a weight.
///
/// A ROLE and never a direction. `Primary` in particular does NOT mean "up" or "good": the
/// movement figures are unsigned magnitudes (see [`window_field`]), and a header that coloured
/// them by sign would assert a direction the number does not carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RunStyle {
    /// The coin. The picture's subject, and the only field set larger than the rest.
    Lead,
    /// The numbers a reader actually scans for.
    Primary,
    /// Context and view metadata: the venue, the stamp, the timeframe, the scale, and the window
    /// token in front of each movement figure.
    Secondary,
}

/// The space in front of a field, which is also the only grouping mark the strip has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LeadGap {
    /// Another field of the same group.
    Field,
    /// The first field of a new group.
    Group,
}

/// One styled piece of a field's text.
pub(super) struct TextRun {
    pub(super) text: String,
    pub(super) style: RunStyle,
}

/// One printable field of a strip: the unit that survives or is dropped WHOLE.
pub(super) struct StripField {
    /// Drawn left to right with no space between them beyond what their own text carries.
    pub(super) runs: Vec<TextRun>,
    /// The space charged in FRONT of this field — ignored when it is the first field placed.
    pub(super) lead_gap: LeadGap,
}

/// One line of burnt-in text, in two priority groups.
///
/// Modelled on `report::totals::FooterFacts` and clipped the same way.
pub(super) struct ShotStrip {
    /// Never clipped. Drawn first, from the left.
    pub(super) head: Vec<StripField>,
    /// Clipped from the RIGHT, one whole field at a time. The order IS the priority.
    pub(super) tail: Vec<StripField>,
}

/// The two spacings the strip uses, in final pixels.
///
/// Two numbers and not a per-field value: grouping is a rhythm, and a rhythm with more than two
/// intervals stops being read as one.
#[derive(Clone, Copy, Debug)]
pub(super) struct Gaps {
    /// Between two fields of the same group.
    pub(super) field: i32,
    /// Between two groups.
    pub(super) group: i32,
}

/// One field reduced to what the layout arithmetic needs: how wide it renders and what space is
/// charged in front of it.
///
/// The measuring itself needs a device context and therefore lives in `super::paint_win`; this is
/// the shape it hands back, so every formula stays here where a test can run it.
#[derive(Clone, Copy, Debug)]
pub(super) struct Measured {
    pub(super) width: i32,
    pub(super) lead_gap: LeadGap,
}

/// Everything the header states, resolved at the moment the picture was taken.
pub(super) struct HeaderInputs {
    /// Ticker as the chart's own corner caption spells it. RAW wire text, never translated.
    pub(super) coin: Option<String>,
    /// Exchange caption. RAW wire text, and the SAME value the privacy substitution puts on the
    /// chart itself — see `super::caption`.
    pub(super) venue: String,
    /// Capture instant, Unix milliseconds UTC.
    pub(super) when_ms: i64,
    /// The user's selected display zone, the one every chart axis already states times in.
    pub(super) zone: Tz,
    /// Candle timeframe in minutes.
    pub(super) tf_min: u32,
    /// The chart's own Y-scale badge: the visible price range as a whole percentage.
    ///
    /// `None` means the badge is HIDDEN, not zero — the chart hides it whenever an untouched fixed
    /// percentage already matches the selected step. See [`scale_field`] for the `0` convention.
    pub(super) scale_pct: Option<i32>,
    /// Price movement over the last three hours, in percent. UNSIGNED — see [`window_field`].
    pub(super) delta_3h: Option<f64>,
    /// Price movement over the last hour, in percent. UNSIGNED.
    pub(super) delta_1h: Option<f64>,
    /// Price movement over the last fifteen minutes, in percent. UNSIGNED.
    pub(super) delta_15m: Option<f64>,
}

/// Assemble the header line.
///
/// The ordering principle, stated once so the table below is not arbitrary: **everything the
/// pixels CANNOT tell you outranks everything they can**, and among the movement figures the
/// shorter the window the more completely the visible candles already show it.
///
/// - **Coin** and **venue** are the head, and the first GROUP: identity. A picture of a chart with
///   no coin on it is worthless, and a price with no exchange is ambiguous across venues — naming
///   the venue is also the entire point of the privacy substitution, so dropping it would leave the
///   picture anonymous in precisely the wrong direction.
/// - **Date and time**, **timeframe** and **scale** are the second group: THE VIEW. The stamp leads
///   it, because once the picture is in a chat the moment it was taken is recoverable from nothing
///   else in the frame. The timeframe follows, because it changes what the candles MEAN — reading
///   a 1m chart as a 15m one is a real trading error and nothing in the pixels states which it is.
///   The scale closes it, because it is the last fact about HOW the chart is drawn rather than
///   about what the market did.
/// - **3h, 1h, 15m** are the third group: THE MARKET. They close the tail in that order, so **15m
///   is dropped first**: the shortest horizon is the one the plotted candles already show.
///
/// Why the view and the market are separated at all: `TF`, the scale and the stamp describe the
/// PICTURE, the movement figures describe the WORLD, and running the two together with one uniform
/// space is exactly what makes a header read as a pile of tokens rather than as a caption.
///
/// **Absence is not clipping.** A field with no value is omitted entirely, never printed as a
/// placeholder or a zero — the same rule `footer_facts` applies when it refuses to print `+0.00`,
/// and for the same reason: an unknown market and a quiet market must not look alike.
///
/// Args:
///     inputs: What the chart knew at the instant the picture was taken.
///
/// Returns:
///     The header's fields, grouped by clip priority.
pub(super) fn header_strip(inputs: &HeaderInputs) -> ShotStrip {
    let mut head = Vec::with_capacity(2);
    // The head is "never CLIPPED", not "always present": a chart whose catalog has not answered
    // yet has no ticker, and inventing one would be worse than saying nothing.
    if let Some(coin) = inputs.coin.as_deref().filter(|c| !c.trim().is_empty()) {
        head.push(plain(coin, RunStyle::Lead, LeadGap::Field));
    }
    if !inputs.venue.trim().is_empty() {
        head.push(plain(&inputs.venue, RunStyle::Secondary, LeadGap::Field));
    }

    let mut tail = Vec::with_capacity(6);
    // The stamp opens the VIEW group. When the zone cannot resolve the instant the stamp is absent
    // and the timeframe leads the tail carrying a `Field` gap — harmless, because the first field
    // actually PLACED is never charged a leading gap at all (see `fit_tail` and `group_width`).
    // Said out loud because it reads like a missing case otherwise.
    if let Some(stamp) = stamp(inputs.when_ms, inputs.zone) {
        tail.push(plain(&stamp, RunStyle::Secondary, LeadGap::Group));
    }
    tail.push(plain(
        &t!("hotkeys.chart_shot_tf", tf = tf_label(inputs.tf_min)),
        RunStyle::Secondary,
        LeadGap::Field,
    ));
    tail.extend(scale_field(inputs.scale_pct, LeadGap::Field));
    tail.extend(window_field("3h", inputs.delta_3h, LeadGap::Group));
    tail.extend(window_field("1h", inputs.delta_1h, LeadGap::Field));
    tail.extend(window_field("15m", inputs.delta_15m, LeadGap::Field));

    ShotStrip { head, tail }
}

/// How many of `tail`'s fields fit, given what the head has already taken.
///
/// Whole fields only: half a percentage is not a smaller fact, it is a wrong one. No ellipsis
/// either, matching the house pattern — a clipped tail says nothing about what is missing, and a
/// picture people crop is the last place to put a marker that only makes sense in place.
///
/// Each field is charged ITS OWN leading gap, so a group boundary costs more room than an ordinary
/// one and a field can therefore be dropped by the rhythm alone. That is the grouping being real
/// rather than decorative: it competes for width like everything else on the line.
///
/// Args:
///     head: The head fields, which are never dropped.
///     tail: The tail fields, in priority order.
///     gaps: The two spacings, in final pixels.
///     avail: Total pixels the line may occupy.
///
/// Returns:
///     How many leading entries of `tail` survive. Zero when the head alone already fills the
///     line — the head still draws, and overflowing is the honest outcome of a picture too narrow
///     to name its own coin.
pub(super) fn fit_tail(head: &[Measured], tail: &[Measured], gaps: Gaps, avail: i32) -> usize {
    // The gap BETWEEN the fixed head and the first tail field is charged to that field below, so
    // it is not double-counted here.
    let mut used = group_width(head, gaps);
    let mut fitted = 0usize;
    for field in tail {
        let step = field.width.saturating_add(if used > 0 {
            lead_width(field.lead_gap, gaps)
        } else {
            0
        });
        if used.saturating_add(step) > avail {
            break;
        }
        used = used.saturating_add(step);
        fitted += 1;
    }
    fitted
}

/// Where the surviving run starts so that it sits CENTRED in the strip.
///
/// **Fit first, centre second, and the order is load-bearing.** The run this centres is the one
/// [`fit_tail`] already decided on — head plus whichever tail fields survived. Centring a line
/// measured BEFORE clipping would centre it around a width it does not occupy, which puts the
/// visible text off to one side by exactly half of what was dropped. That is why this takes
/// already-fitted fields rather than the whole strip's, and why it lives after `fit_tail` in this
/// file: the read order is the call order.
///
/// **Clamped to the inset, never to zero.** A run wider than the strip would otherwise start at a
/// negative x, and a negative coordinate handed to a GDI text call is a thing the next reader
/// cannot reason about locally. The inset is also exactly where the line used to start, so the
/// degenerate case degrades to the previous behaviour rather than to something new. The clamp
/// binds if and only if `run_w > strip_w - 2 * inset` — i.e. the run is wider than `fit_tail`'s
/// own `avail` — and at `run_w == avail` exactly the un-clamped formula already answers `inset`,
/// so there is no discontinuity at the boundary. Since `fit_tail` guarantees the fitted run is
/// within `avail` whenever the head itself fits, the clamp is reachable ONLY when the head alone
/// overflows: a pane too narrow to name its own coin.
///
/// Args:
///     drawn: The fields that will actually be drawn, in draw order — the head followed by the
///         surviving tail.
///     gaps: The two spacings, the same values `fit_tail` was charged.
///     strip_w: The strip's FULL width, not the fitting width: the margins this centres between
///         are the picture's own edges.
///     inset: The left margin, and the floor the start is clamped to.
///
/// Returns:
///     The x the first field is drawn at. `inset` for an empty run, so an empty strip does not
///     answer "half the picture" — a defined value that means nothing.
pub(super) fn centred_start_x(drawn: &[Measured], gaps: Gaps, strip_w: i32, inset: i32) -> i32 {
    if drawn.is_empty() {
        return inset;
    }
    let run_w = group_width(drawn, gaps);
    // Integer division truncates, so an odd leftover pixel goes to the RIGHT margin. Which side
    // gets it is arbitrary; that it is decided the same way every time is not.
    let start = strip_w.saturating_sub(run_w) / 2;
    start.max(inset)
}

/// Total width of one run of fields drawn end to end, leading gaps included.
///
/// The FIRST field's own `lead_gap` is deliberately ignored: it describes the space between that
/// field and whatever precedes it, and nothing precedes the first one. Charging it would push the
/// whole centred run off by one gap, and by a DIFFERENT amount depending on whether the run
/// happens to start on a group boundary.
///
/// Args:
///     fields: The fields, in draw order.
///     gaps: The two spacings.
///
/// Returns:
///     The run's width, or zero when it has no fields.
fn group_width(fields: &[Measured], gaps: Gaps) -> i32 {
    let mut total = 0i32;
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            total = total.saturating_add(lead_width(field.lead_gap, gaps));
        }
        total = total.saturating_add(field.width);
    }
    total
}

/// The space charged in front of one field.
///
/// Args:
///     lead_gap: Which kind of boundary this field opens.
///     gaps: The two spacings.
///
/// Returns:
///     The pixels to advance before drawing it.
pub(super) fn lead_width(lead_gap: LeadGap, gaps: Gaps) -> i32 {
    match lead_gap {
        LeadGap::Field => gaps.field,
        LeadGap::Group => gaps.group,
    }
}

/// The chart's Y-scale badge as a printable field, or nothing when the chart is not showing one.
///
/// **The convention is copied VERBATIM from the badge the chart itself draws**
/// (`chartdx/text/labels.rs`, `ChartLabelField::ScaleBadge`): a range that rounds below a whole
/// percent in a quiet Auto market reads `<1%` and never `0%`, because a zero would claim the chart
/// has no vertical span at all. Copied rather than shared because the chart's own resolver is
/// reached through a `ChartLabelPart` and a `LabelInputs` snapshot, neither of which exists on this
/// side of the capture — but the two spellings must stay identical, and a static contract pins
/// that they do: a screenshot disagreeing with the badge inside it is the worst outcome available
/// here.
///
/// The text carries NO label of its own. That is deliberate: it is the same token the chart prints
/// beside the coin badge, so a reader maps it to what they can see in the picture immediately,
/// where a prefix would invent a name the application never uses anywhere else. Its group tells
/// them what kind of fact it is.
///
/// Args:
///     pct: The badge's whole percentage, or `None` when the chart hides it.
///     lead_gap: The space charged in front of the field.
///
/// Returns:
///     One field, or nothing at all. Never a placeholder standing in for a hidden badge.
fn scale_field(pct: Option<i32>, lead_gap: LeadGap) -> Option<StripField> {
    let pct = pct?;
    let text = if pct == 0 {
        "<1%".to_string()
    } else {
        format!("{pct}%")
    };
    Some(plain(&text, RunStyle::Secondary, lead_gap))
}

/// One movement window as a printable field, or nothing when the figure is unknown.
///
/// **The figure is an UNSIGNED range magnitude, and it is rendered as one.**
/// `moon_core::market::source::WindowFigures::delta_pct` is documented verbatim as "UNSIGNED —
/// this is the range magnitude the Screener's `Δ` columns show, not a signed change from an
/// average" (`moon-core/src/market/source/mod.rs:534-535`), and it is produced by `positive(delta)`
/// at `market/source/read.rs:897`. Both citations were re-checked against the files rather than
/// carried over: the previous text pointed at `read.rs:806`, which is a price fallback and has
/// nothing to do with this figure.
/// So this uses [`fmt::pct`] and NEVER the signed formatter beside it,
/// and **the caller must not colour it by direction either**: attaching a sign — or a green — to a
/// magnitude claims a direction the number does not carry, on a picture people trade off. That is
/// why no sign classification reaches this module at all rather than being carried and ignored;
/// the rounding trap in the shared formatter's sign picker is avoided by never entering it.
///
/// Two runs, not one: the window TOKEN is context and the FIGURE is the thing being scanned for,
/// so they are ranked differently even though they read as a single phrase. The separating space
/// belongs to the token's own text, which keeps the pair one field for both measuring and
/// clipping — a gap here would make them look like two.
///
/// Args:
///     label: The window's own token — `3h`, `1h`, `15m`. Untranslated, exactly as the chart's own
///         labels spell it.
///     value: The window's movement, or `None` when the market's history has not answered.
///     lead_gap: The space charged in front of the field.
///
/// Returns:
///     One field, or nothing at all. Never a zero standing in for an unknown.
fn window_field(label: &str, value: Option<f64>, lead_gap: LeadGap) -> Option<StripField> {
    let (text, _sign) = fmt::pct(value?, 1)?;
    Some(StripField {
        runs: vec![
            TextRun {
                text: format!("{label} "),
                style: RunStyle::Secondary,
            },
            TextRun {
                text,
                style: RunStyle::Primary,
            },
        ],
        lead_gap,
    })
}

/// The capture instant as a full civil date and time in the user's selected zone.
///
/// Always carries the DATE, unlike the chart's own time axis, which drops it inside the current
/// day. An axis is read while the chart is open; this picture is read tomorrow, in a chat, by
/// somebody who was not there.
///
/// Args:
///     when_ms: Capture instant, Unix milliseconds UTC.
///     zone: The user's selected display zone.
///
/// Returns:
///     `DD.MM.YYYY HH:MM:SS`, or nothing when the zone cannot resolve the instant unambiguously.
fn stamp(when_ms: i64, zone: Tz) -> Option<String> {
    Some(
        zone.timestamp_millis_opt(when_ms)
            .single()?
            .format("%d.%m.%Y %H:%M:%S")
            .to_string(),
    )
}

/// The candle timeframe as the chart's own controls spell it.
///
/// Args:
///     tf_min: Timeframe in minutes, one of `CANDLE_TF_CHOICES_MIN`.
///
/// Returns:
///     A compact token: `1m`, `5m`, `30m`, `1h`, `4h`, `1d`. An unrecognized value falls back to
///     minutes rather than being hidden — a chart on a timeframe this build does not enumerate is
///     still a chart, and the number is still true.
fn tf_label(tf_min: u32) -> String {
    match tf_min {
        0 => "1m".to_string(),
        m if m % 1440 == 0 => format!("{}d", m / 1440),
        m if m % 60 == 0 => format!("{}h", m / 60),
        m => format!("{m}m"),
    }
}

/// Wrap one already-formatted string as a single-run field.
///
/// Args:
///     text: The rendered text.
///     style: What that text is to the reader.
///     lead_gap: The space charged in front of the field.
///
/// Returns:
///     The field.
fn plain(text: &str, style: RunStyle, lead_gap: LeadGap) -> StripField {
    StripField {
        runs: vec![TextRun {
            text: text.to_string(),
            style,
        }],
        lead_gap,
    }
}

#[cfg(test)]
mod tests;
