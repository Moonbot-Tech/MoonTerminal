//! The three crowd tables: the rolling minute, and the service's two boards for the day.
//!
//! Never a net figure. A coin that made $9k and lost $8k had a loud minute, and a single number
//! would report it as a quiet one; the two columns are the whole point of the minute table.
//!
//! **The tables MOVE.** A board that only redraws is a board nobody watches: the figures change
//! and the eye is already elsewhere. So a row that climbs slides to its new place, a row that
//! drops off sinks and fades, and a row something has just landed on lights up in the colour of
//! what happened. What moved is not something a row can know, so the caller keeps a
//! [`motion::Motion`] per table and hands it over; a caller that does not want any of it hands
//! over `None` and gets a still board.
//!
//! It is free when nothing is happening, and that is by construction rather than by tuning: a row
//! with nothing to say is a plain element, and only a row with something to say is drawn anywhere
//! other than where it stands. Nothing here is a framework animation, so nothing here asks the
//! window for a frame — the view decides when to draw and reads the movement at a moment of its
//! own choosing.
//!
//! Every colour comes from the runtime palette and every size from one of the three type steps in
//! [`crate::design`], so the Font slider moves this table with the rest of the terminal: the row
//! pitch is derived from the body size rather than fixed, or the rows would overlap the moment
//! somebody enlarged the type.

use std::time::Instant;

use gpui::{
    App, Div, Hsla, InteractiveElement, MouseButton, MouseDownEvent, ParentElement, Pixels,
    SharedString, Styled, div, px,
};
use moon_core::util::fmt::DeltaSign;
use moon_ui::{MoonPalette, rgba_from};

use crate::design;

pub mod motion;

pub use day::{day_coins, day_traders};
pub use minute::minute;

/// What a click on one coin does, built by the view that knows how to open a chart.
///
/// Boxed rather than generic because a table builds one per row: the factory hands out a fresh
/// listener per coin, and the table itself stays free of any context.
pub type CoinAction = Box<dyn Fn(&MouseDownEvent, &mut gpui::Window, &mut App) + 'static>;
/// The factory that hands one of those out, per coin.
pub type CoinClick<'a> = &'a dyn Fn(&str) -> CoinAction;

/// How many traders are shown. The service sends fifty; twenty is what fits down the side of a
/// panel without turning the screen into a spreadsheet.
pub const TRADERS_SHOWN: usize = 20;
/// How many coins the day board holds: the service's best five and its worst five.
pub const DAY_COIN_SEATS: usize = 10;
/// How many coins the MINUTE board holds.
pub const MINUTE_SEATS: usize = 10;

/// How far the whole arrangement stands off the panel's edges, in design units.
///
/// These are the FRAME's insets now, not any one table's — the boards no longer place themselves,
/// and `crowd::place` spends these on the three bands and their columns. One inset per edge, so two
/// blocks anchored to the same edge line up whatever they are.
pub(super) const EDGE_X: f32 = 18.0;
pub(super) const EDGE_BOTTOM: f32 = 18.0;
/// Where the TOP band starts.
///
/// Deeper than the other two on purpose: it clears the screen's settings button, which owns the
/// top-right corner (`chart_tabs/main_stack/empty.rs`). A caption drawn under that button is a
/// caption nobody can read, and insetting only the cell beneath it would break the line the top
/// row's captions share.
pub(super) const EDGE_TOP: f32 = 40.0;

/// Column widths in design units, so a header and its rows cannot disagree.
pub(super) const W_COIN: f32 = 64.0;

/// The money columns are measured in DIGITS of the face they are drawn in, not in design units.
///
/// A figure is laid out around its decimal point — the whole part right-aligned, the fraction and
/// any suffix left-aligned — so that a column of them lines up on the point rather than on
/// whichever end happens to be longer. "4444.44", "0.1" and "10K" left-aligned read as three
/// unrelated numbers; over one point they read as a column. That only holds if the two halves are
/// a fixed number of characters wide, which is a measurement of the FONT and not a design token.
///
/// Five before the point and four after, which is what the figures actually reach: anything from
/// a thousand up carries a suffix, so the whole part is at most three digits and a sign
/// ("+999.99K"), and the fraction is always exactly two digits plus that suffix.
pub(super) const INT_CHARS: f32 = 5.0;
pub(super) const FRAC_CHARS: f32 = 4.0;
/// And the minute's count of trades beside it: "(999)". A minute of one coin does not reach four
/// digits — the busiest second the whole market has shown is a few trades — and a column sized for
/// a number that cannot happen is width taken from the figures that can.
pub(super) const COUNT_CHARS: f32 = 5.0;
/// The trader board's own columns: place, account, handle, money, trades, then movement.
///
/// The move follows the figures and has no heading of its own — it
/// is a mark, the way the leaving arrow is a mark, and a titled column over twenty mostly empty
/// cells would be a heading for nothing.
pub(super) const W_SHIFT: f32 = 40.0;
pub(super) const W_PLACE: f32 = 20.0;
pub(super) const W_ID: f32 = 54.0;
pub(super) const W_WHO: f32 = 118.0;
pub(super) const W_TRADES: f32 = 52.0;

/// The gap between columns, and between a caption and the rows under it.
pub(super) const GAP: f32 = 10.0;
pub(super) const GAP_TRADERS: f32 = 8.0;
pub(super) const GAP_STACK: f32 = 3.0;

/// Line height GPUI gives a line of text: its default `phi`, which is what the row pitch has to
/// clear or two rows overlap by the difference.
pub(super) const LINE_HEIGHT: f32 = 1.618;

/// How far a row that has left sinks while it fades, in rows.
pub(super) const SINK: f32 = 1.4;
/// How brightly a row is lit when its figures have just moved.
pub(super) const GLOW_ALPHA: f32 = 0.18;
/// Alpha of a heading, a dimmed figure, and a row's own text.
pub(super) const A_HEADING: f32 = 0.45;
pub(super) const A_CAPTION: f32 = 0.55;
pub(super) const A_TEXT: f32 = 0.9;
pub(super) const A_MARK: f32 = 0.85;
/// A figure that is not the row's subject: its trade count, or an anonymous handle.
pub(super) const A_ASIDE: f32 = 0.5;
/// How brightly a clickable ticker lights under the cursor.
pub(super) const HOVER_ALPHA: f32 = 0.08;

/// The page every figure on this screen comes from, and the name it is shown under.
///
/// Written out rather than derived from `moon_core::crowd::STAT_ORIGIN`: that constant is where
/// the terminal READS from and may one day carry a port or a path, while this is what a person
/// clicks and reads. They agree today, and the day they stop agreeing the reader must be sent to
/// the page, not to the endpoint.
pub(super) const SOURCE_URL: &str = "https://stat.moonbot.pro";
pub(super) const SOURCE_HOST: &str = "stat.moonbot.pro";

/// The mark a row wears while it is dropping off the board, and how far to the left of the row it
/// hangs, in design units.
///
/// Beside the row rather than inside it: it appears for the second the row is leaving, and a mark
/// that pushed the columns aside as it came and went would make the whole table twitch. It is
/// there because a row sliding away is easy to miss on a board that is also reshuffling — the
/// movement says something LEFT, the arrow says which way it went.
pub(super) const LEAVING_MARK: &str = "\u{2193}";
pub(super) const LEAVING_MARK_LEFT: f32 = 11.0;
/// The marks a row wears while it is still where the last board's move left it.
pub(super) const CLIMB_MARK: &str = "\u{2191}";
pub(super) const DROP_MARK: &str = "\u{2193}";

/// Everything the tables measure and colour themselves by, resolved once per frame.
///
/// One read of the theme per frame rather than one per cell: every figure here is scaled by the
/// Font slider, and a table of twenty rows and six columns would otherwise ask the theme a
/// hundred and twenty times to be told the same number.
#[derive(Clone, Copy)]
pub struct Look {
    pub(super) palette: MoonPalette,
    /// Heading type size.
    pub(super) caption: Pixels,
    /// Row type size.
    pub(super) body: Pixels,
    /// Pitch of one row, derived from [`Self::body`] and never fixed.
    pub(super) row_h: Pixels,
    pub(super) gap: Pixels,
    pub(super) gap_traders: Pixels,
    pub(super) gap_stack: Pixels,
    pub(super) edge_x: Pixels,
    pub(super) edge_top: Pixels,
    pub(super) edge_bottom: Pixels,
    pub(super) leaving_left: Pixels,
    /// Width of one digit of the face the figures are drawn in.
    pub(super) digit: Pixels,
    pub(super) w_coin: Pixels,
    pub(super) w_shift: Pixels,
    pub(super) w_place: Pixels,
    pub(super) w_id: Pixels,
    pub(super) w_who: Pixels,
    pub(super) w_trades: Pixels,
}

impl Look {
    /// Align coin text with the first digit of the trader board's two-digit ranks.
    pub(super) fn rank_indent(&self) -> Pixels {
        (self.w_place - self.digit * 2.0).max(px(0.0))
    }

    /// Resolve the palette and every scaled measurement from the active theme.
    pub fn of(cx: &App) -> Self {
        let body = design::t_body(cx);
        Self {
            palette: MoonPalette::active(cx),
            caption: design::t_caption(cx),
            body,
            // The pitch a stacked column would have produced, computed rather than copied: the
            // line box is `round(size * phi)`, and the gap under it is the same gap the captions
            // use. A constant here would overlap the rows on any Font-slider setting but one.
            row_h: px((f32::from(body) * LINE_HEIGHT).round() + design::ui_value(cx, GAP_STACK)),
            gap: design::ui_px(cx, GAP),
            gap_traders: design::ui_px(cx, GAP_TRADERS),
            gap_stack: design::ui_px(cx, GAP_STACK),
            edge_x: design::ui_px(cx, EDGE_X),
            edge_top: design::ui_px(cx, EDGE_TOP),
            edge_bottom: design::ui_px(cx, EDGE_BOTTOM),
            leaving_left: design::ui_px(cx, LEAVING_MARK_LEFT),
            digit: px(design::mono_body_text_width(cx, "0", 400.0)),
            w_coin: design::font_w_px(cx, W_COIN),
            w_shift: design::font_w_px(cx, W_SHIFT),
            w_place: design::font_w_px(cx, W_PLACE),
            w_id: design::font_w_px(cx, W_ID),
            w_who: design::font_w_px(cx, W_WHO),
            w_trades: design::font_w_px(cx, W_TRADES),
        }
    }

    /// Width of one money figure: the whole part, the point, the fraction and any suffix.
    pub(super) fn w_money(&self) -> Pixels {
        self.digit * (INT_CHARS + FRAC_CHARS)
    }

    /// Width of one minute cell: a figure and the count of trades behind it.
    pub(super) fn w_side(&self) -> Pixels {
        self.w_money() + self.gap + self.digit * COUNT_CHARS
    }

    /// A dimmed shade of the ordinary text colour.
    pub(super) fn dim(&self, alpha: f32) -> Hsla {
        rgba_from(self.palette.text, alpha)
    }

    /// One heading cell, at its own size and weight. Left, over a column of words.
    pub(super) fn heading(&self, label: String, width: Pixels) -> Div {
        text(label, self.caption, self.dim(A_HEADING)).w(width)
    }

    /// A heading over a column of FIGURES, ending where their decimal points are.
    ///
    /// A money column is read from the point outwards, so a heading pinned to the cell's left edge
    /// stands beside its column rather than over it — the wider the cell, the further away. Ending
    /// it at the point puts the word over the digits it names.
    ///
    /// The cell and the figure are given SEPARATELY, and that is the whole of what this gets
    /// wrong when they are confused: the minute's cell carries a count of trades after the money,
    /// so a heading measured against the cell lands over the count instead of over the figure.
    ///
    /// Args:
    ///     label: The heading.
    ///     cell: The whole column, so the headings keep the row's spacing.
    ///     figure: The part of it the money is drawn in.
    pub(super) fn heading_over(&self, label: String, cell: Pixels, figure: Pixels) -> Div {
        let fraction = self.digit * FRAC_CHARS;
        div().flex().w(cell).child(
            text(label, self.caption, self.dim(A_HEADING))
                .w((figure - fraction).max(px(0.0)))
                .text_right(),
        )
    }

    /// A heading over a column of whole numbers, ending where their last digit does.
    pub(super) fn heading_right(&self, label: String, width: Pixels) -> Div {
        text(label, self.caption, self.dim(A_HEADING))
            .w(width)
            .text_right()
    }

    /// Width of a whole board, so its rows and its heading agree.
    pub(super) fn w_minute(&self) -> Pixels {
        self.w_coin + (self.w_side() + self.gap) * 2.0
    }

    /// Width of the day's coin board — the same as the minute's, and derived from it rather than
    /// tuned to match: the two are stacked in one column down the right-hand edge, and two nearly
    /// equal widths read as a mistake.
    pub(super) fn w_day_coins(&self) -> Pixels {
        self.w_minute()
    }

    /// Width of one figure in that board, being whatever is left of a half after the ticker.
    ///
    /// The day's figures are short — "+6.69K", "-619.75" — where the minute's carry a count in
    /// brackets too, so the narrower column is not a compromise: it is the room the figures
    /// actually need, spent on making the two boards line up.
    pub(super) fn w_day(&self) -> Pixels {
        (self.w_day_coins() - self.gap) / 2.0 - self.w_coin - self.gap
    }

    pub(super) fn w_day_traders(&self) -> Pixels {
        self.w_shift
            + self.w_place
            + self.w_id
            + self.w_who
            + self.w_money()
            + self.w_trades
            + self.gap_traders * 5.0
    }
}

/// One ticker cell, cut off rather than allowed to run into the figure beside it.
///
/// The service spells a coin however the exchange does, and some of them are long
/// ("CSOPSS2LHKD"): with the columns placed by hand there is nothing to push aside, so an
/// unclipped name is simply drawn over the money. Cut, the row still says which coin it is —
/// the figures beside it stay readable, which is what the table is for.
///
/// Args:
///     coin: Ticker.
///     look: Metrics and colours for this frame.
pub(super) fn ticker(coin: &str, look: &Look) -> Div {
    text(coin.to_string(), look.body, look.dim(A_TEXT))
        .w(look.w_coin)
        .overflow_hidden()
        .text_ellipsis()
}

/// The same cell as a BUTTON: what a trader wants from a market row is that coin's chart.
///
/// The button goes INSIDE a plain row rather than being one: an element with an id is a
/// `Stateful<Div>`, and what the table places and animates has to be a plain `Div`.
///
/// Keyed by the coin and the board it is on, never by the row's place: the boards reorder under
/// the cursor, and an id meaning "third row" would leave the hover behind on whatever moved into
/// third place.
///
/// Args:
///     coin: Ticker.
///     board: Which table this cell belongs to, so two boards showing one coin keep two hovers.
///     on_coin: The factory, or `None` on a board whose caller wants no clicks.
///     look: Metrics and colours for this frame.
pub(super) fn coin_cell(coin: &str, board: &str, on_coin: Option<CoinClick>, look: &Look) -> Div {
    let Some(on_coin) = on_coin else {
        return ticker(coin, look);
    };
    let click = on_coin(coin);
    div().w(look.w_coin).child(
        div()
            .id(SharedString::from(format!("crowd-{board}-{coin}")))
            .cursor_pointer()
            .hover(|style| style.bg(rgba_from(look.palette.text, HOVER_ALPHA)))
            .on_mouse_down(MouseButton::Left, move |event, window, app| {
                app.stop_propagation();
                click(event, window, app);
            })
            .child(ticker(coin, look)),
    )
}

/// One number, laid out around its decimal point.
///
/// The whole part is right-aligned against the point and everything after it — the fraction, and
/// any `K`/`M` suffix — is left-aligned away from it, so every figure in a column has its point at
/// the same place whatever its magnitude. A number with no fraction simply leaves that half empty,
/// which is exactly where its point would have been.
///
/// Args:
///     label: The figure, already formatted.
///     colour: What to draw it in.
///     width: The whole cell.
///     look: Metrics for this frame.
pub(super) fn figure(label: String, colour: Hsla, width: Pixels, look: &Look) -> Div {
    let (whole, rest) = match label.find('.') {
        Some(point) => (label[..point].to_string(), label[point..].to_string()),
        None => (label, String::new()),
    };
    let fraction = look.digit * FRAC_CHARS;
    div()
        .flex()
        .w(width)
        .child(
            text(whole, look.body, colour)
                .w((width - fraction).max(px(0.0)))
                .text_right(),
        )
        .child(text(rest, look.body, colour).w(fraction))
}

/// A line of text at a size and a colour — the one shape these tables are built from.
pub(super) fn text(label: impl Into<String>, size: Pixels, colour: Hsla) -> Div {
    div()
        // Rows are placed by hand at a fixed pitch, so a cell that folded onto a second line would
        // be drawn straight through the row beneath it. A figure too wide for its column overflows
        // instead, which is visible and wrong in one place rather than in two.
        .whitespace_nowrap()
        .text_size(size)
        .text_color(colour)
        .child(label.into())
}

/// Put one row where it is at this moment.
///
/// Every effect is read straight out of [`motion::Move`], which is a pure function of how long ago
/// the effect started — so the table draws whatever the movement looks like NOW and never asks the
/// framework to keep animating it. That is what puts the frame rate in the view's hands: a view
/// that redraws twelve times a second gets twelve frames of the same slide, and one that redraws
/// once gets the end of it.
///
/// A row with nothing to say is a plain element in a plain place, and costs nothing at all.
///
/// Args:
///     place: Where it sits now, from zero.
///     state: What it is doing, from [`motion::Motion`].
///     row: The row itself, already built.
///     look: Metrics and colours for this frame.
pub(super) fn placed(place: usize, state: motion::Move, row: Div, look: &Look) -> Div {
    let pitch = f32::from(look.row_h);
    let top = place as f32 * pitch;
    if state.still() {
        return row.absolute().left(px(0.0)).top(px(top)).h(look.row_h);
    }
    // Where it is between the two places, eased at the END rather than the start: the eye follows
    // a row to where it stops rather than away from where it was.
    let top = match state.from {
        Some(was) => {
            let from = was as f32 * pitch;
            from + (top - from) * (1.0 - (1.0 - state.slid).powi(3))
        }
        None => top,
    };
    let leaving = state.leaving.unwrap_or(0.0);
    let mut row = row
        .absolute()
        .left(px(0.0))
        // Out and down: the way a row that has stopped mattering leaves a list.
        .top(px(top + SINK * pitch * leaving))
        .h(look.row_h);
    if state.leaving.is_some() {
        row = row
            .opacity(1.0 - leaving)
            .child(div().absolute().left(-look.leaving_left).child(text(
                LEAVING_MARK,
                look.caption,
                rgba_from(design::danger_color(look.palette), A_MARK),
            )));
    }
    if state.glow > 0.0 {
        let colour = if state.gain {
            design::positive_color(look.palette)
        } else {
            design::danger_color(look.palette)
        };
        row = row.bg(rgba_from(colour, GLOW_ALPHA * state.glow));
    }
    row
}

/// The box the rows are placed in, tall enough for what is on its way out of it.
///
/// A leaving row is drawn where it WAS, which can be past the end of a board that has since
/// shrunk; a box sized to the live rows alone would leave it hanging outside, over whatever is
/// under the table. The height also decides where a BOTTOM-anchored table starts, so it is the
/// board's full size rather than however many rows happen to have arrived — a box that resized
/// itself would walk the whole table up and down the screen every time a coin came or went.
///
/// Args:
///     seats: How many rows this board holds when it is full.
///     width: How wide its rows are.
///     look: Metrics for this frame.
pub(super) fn rows_box(seats: usize, width: Pixels, look: &Look) -> Div {
    div()
        .relative()
        .w(width)
        .h(px((seats as f32 + SINK) * f32::from(look.row_h)))
}

/// Dollars, always to the hundredth, with an SI suffix once they run past a thousand.
///
/// ALWAYS two decimals, and that is the point: a column where one row prints `2`, the next `0.1`
/// and the next `10K` gives the eye three different shapes to compare, and the shape is what it
/// compares first. Two decimals and one point make every row the same shape, so what differs
/// between them is the number.
///
/// The suffix keeps the column narrow — the day's figures reach six digits — and it is chosen the
/// way [`fmt::compact_si`] chooses it, at the same thresholds, so a figure here and the same
/// figure anywhere else in the terminal name the same magnitude.
///
/// Args:
///     value: Dollars, sign already spent or about to be added by the caller.
pub(super) fn dollars(value: f64) -> String {
    const UNITS: [(f64, &str); 4] = [(1e12, "T"), (1e9, "B"), (1e6, "M"), (1e3, "K")];
    let size = value.abs();
    match UNITS.iter().find(|(scale, _)| size >= *scale) {
        Some((scale, suffix)) => format!("{:.2}{suffix}", size / scale),
        None => format!("{size:.2}"),
    }
}

/// A money figure with the sign the crowd earned, and the sign the colour follows.
///
/// One rounding decides both the digits and the tone: the magnitude is what [`dollars`] actually
/// PRINTS, and a figure that prints as zero is classified as zero rather than as the gain its raw
/// sixteenth decimal place would claim.
///
/// Args:
///     value: Signed dollars.
///
/// Returns:
///     What to write, and what it says.
pub(super) fn signed_compact(value: f64) -> (String, DeltaSign) {
    let magnitude = dollars(value);
    let sign = if magnitude.chars().all(|glyph| matches!(glyph, '0' | '.')) {
        DeltaSign::Zero
    } else if value < 0.0 {
        DeltaSign::Negative
    } else {
        DeltaSign::Positive
    };
    (format!("{}{magnitude}", sign.pick("+", "-", "")), sign)
}

/// One signed day figure, in the tone its sign earns, aligned on its point.
///
/// Args:
///     value: Signed dollars.
///     width: The cell it sits in.
///     look: Metrics and colours for this frame.
pub(super) fn signed(value: f64, width: Pixels, look: &Look) -> Div {
    let (label, sign) = signed_compact(value);
    figure(
        label,
        rgba_from(design::delta_tone(sign).color(look.palette), 1.0),
        width,
        look,
    )
}

/// What one row is doing, or nothing at all when the caller tracks no movement.
pub(super) fn state_of<T: Clone>(
    moved: Option<(&motion::Motion<T>, Instant)>,
    key: &str,
) -> motion::Move {
    moved
        .map(|(moved, now)| moved.of(key, now))
        .unwrap_or_default()
}

/// The rows that have left a board and are still on their way out, placed where they were.
pub(super) fn leaving_rows<T: Clone>(
    moved: Option<(&motion::Motion<T>, Instant)>,
    look: &Look,
    draw: impl Fn(&T, &Look) -> Div,
) -> Vec<Div> {
    let Some((moved, now)) = moved else {
        return Vec::new();
    };
    moved
        .leaving(now)
        .map(|(_, row, place, state)| placed(place, state, draw(row, look), look))
        .collect()
}

pub mod day;
pub mod minute;
pub(super) mod narrow;

#[cfg(test)]
mod tests;
