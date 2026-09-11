//! The rolling minute: what the crowd won and lost on each coin in the last sixty seconds.
//!
//! Never a net figure. A coin that made $9k and lost $8k had a loud minute, and a single number
//! would report it as a quiet one; the two columns are the whole point of this table.
//!
//! The shape every row is built from — the metrics, the movement, the figure split at its decimal
//! point — lives in the parent module and is shared with the day boards, so the two cannot drift
//! apart in what a row MEANS while differing in what they show.

use std::time::Instant;

use gpui::{
    App, Div, InteractiveElement, MouseButton, ParentElement, StatefulInteractiveElement, Styled,
    div, px,
};
use moon_core::crowd::{Standing, Wire};
use moon_ui::{MoonTone, rgba_from};
use rust_i18n::t;

use super::{
    A_CAPTION, A_HEADING, COUNT_CHARS, CoinClick, HOVER_ALPHA, Look, MINUTE_SEATS, SOURCE_HOST,
    SOURCE_URL, coin_cell, dollars, figure, leaving_rows, motion, placed, rows_box, state_of, text,
};

/// The minute table: what the crowd won and lost on each coin.
///
/// It places itself NOWHERE. Where this board is drawn is a saved choice now
/// (`moon_core::config::layout::EmptyPlaces`), and the frame in `crowd::place` is the one thing
/// that knows about anchors — a board that still nailed itself to a corner could not be moved out
/// of it.
///
/// Args:
///     rows: The coins to show, already in the order they should be read.
///     wire: What can honestly be said about the feed right now.
///     moved: What has moved since the last board, or `None` for a still board.
///     look: Metrics and colours for this frame.
pub fn minute(
    rows: &[Standing],
    wire: Wire,
    moved: Option<(&motion::Motion<Standing>, Instant)>,
    on_coin: Option<CoinClick>,
    look: &Look,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(look.gap_stack)
        .child(caption(wire, look))
        .child(minute_table(rows, moved, on_coin, look))
}

/// The minute's heading: what this table is, and where every figure on the screen comes from.
///
/// The service is NAMED rather than described, and it is a link: whoever reads these numbers is
/// entitled to know whose numbers they are and to go and look at the page they come from. What the
/// wire is doing is said by the colour instead of by a second line of words — lit while it is
/// delivering, dim while it is opening or down — which is the same fact in the space of nothing.
///
/// A stand-in says so in words, because there the figures are invented and no colour can carry
/// that.
pub(super) fn caption(wire: Wire, look: &Look) -> Div {
    let mut row = div().flex().gap(look.gap_traders).child(text(
        t!("crowd.minute.title").to_string(),
        look.caption,
        look.dim(A_CAPTION),
    ));
    if wire == Wire::Synthetic {
        return row.child(text(
            t!("crowd.wire.synthetic").to_string(),
            look.caption,
            look.dim(A_HEADING),
        ));
    }
    let colour = match wire {
        Wire::Live => rgba_from(MoonTone::Positive.color(look.palette), 1.0),
        _ => look.dim(A_HEADING),
    };
    row = row.child(
        div()
            .id("crowd-source")
            .cursor_pointer()
            .hover(|style| style.bg(rgba_from(look.palette.text, HOVER_ALPHA)))
            .tooltip(crate::panels::common::text_tooltip(SOURCE_URL))
            .on_mouse_down(MouseButton::Left, |_, _, app: &mut App| {
                app.stop_propagation();
                app.open_url(SOURCE_URL);
            })
            .child(text(SOURCE_HOST.to_string(), look.caption, colour)),
    );
    row
}

/// The minute itself: a header and one row per standing, in the order they were given.
fn minute_table(
    rows: &[Standing],
    moved: Option<(&motion::Motion<Standing>, Instant)>,
    on_coin: Option<CoinClick>,
    look: &Look,
) -> Div {
    // ПРОФИТ / УБЫТОК: the pair a trader actually says. The sign belongs to the COLUMN here, not
    // to the figure — one column is everything won and the other everything lost — which is why
    // neither of them goes through the signed formatter.
    let header = div()
        .flex()
        .gap(look.gap)
        .child(look.heading(t!("crowd.col.coin").to_string(), look.w_coin))
        // Over the MONEY, not over the whole cell: the count of trades beside it has no heading,
        // and a word measured against the whole cell lands over the count.
        .child(look.heading_over(
            t!("crowd.col.profit").to_string(),
            look.w_side(),
            look.w_money(),
        ))
        .child(look.heading_over(
            t!("crowd.col.loss").to_string(),
            look.w_side(),
            look.w_money(),
        ));

    if rows.is_empty() {
        // A market with nothing in it is a legitimate state — nights are quiet — and it has to SAY
        // so, or an empty table reads as one that is broken.
        //
        // The last coins still get to leave, UNDER THE HEADER they were standing under: a board
        // emptying is the one moment a departure says the most, and taking the columns away at the
        // same instant would jump the rows up by the header's height as they went. The notice
        // stands where the first row stood, and whoever is still leaving sinks away underneath it.
        return div()
            .flex()
            .flex_col()
            .gap(look.gap_stack)
            .child(header)
            .child(
                rows_box(MINUTE_SEATS, look.w_minute(), look)
                    .child(div().absolute().left(px(0.0)).top(px(0.0)).child(text(
                        t!("crowd.minute.empty").to_string(),
                        look.body,
                        look.dim(A_HEADING),
                    )))
                    .children(leaving_rows(moved, look, |row, look| {
                        minute_row(row, None, look)
                    })),
            );
    }

    let mut list: Vec<Div> = Vec::new();
    for (place, row) in rows.iter().take(MINUTE_SEATS).enumerate() {
        let state = state_of(moved, &row.coin);
        list.push(placed(place, state, minute_row(row, on_coin, look), look));
    }
    // A row on its way off the board is not a button: it is a picture of something that has
    // already happened, and it is under the cursor for half a second on its way out.
    list.extend(leaving_rows(moved, look, |row, look| {
        minute_row(row, None, look)
    }));

    div()
        .flex()
        .flex_col()
        .gap(look.gap_stack)
        .child(header)
        .child(rows_box(MINUTE_SEATS, look.w_minute(), look).children(list))
}

/// One coin's minute: the name, what the crowd made on it, what it lost.
///
/// The sign belongs to the COLUMN here, not to the figure: one column is everything won and the
/// other everything lost, both already positive, so neither goes through the signed formatter.
fn minute_row(row: &Standing, on_coin: Option<CoinClick>, look: &Look) -> Div {
    // The money and the count of trades behind it are two columns, not one string: the count
    // varies from one digit to four, and carried inside the figure it would push every point in
    // the column somewhere different.
    let side = |amount: f64, trades: u32, sign: char, tone: MoonTone| {
        // A side with no trade on it is BLANK, not a zero. "+0 (0)" is a figure for something that
        // did not happen, and printed in the colour of a gain it reads as one; the empty half of
        // the row says "only losses this minute" more clearly than any number can.
        if trades == 0 {
            return div().w(look.w_side());
        }
        let colour = rgba_from(tone.color(look.palette), 1.0);
        // Both halves are given their width, and the cell is given its own. Left to size itself
        // from its contents, this cell is as wide as the count inside it happens to print — so a
        // coin with four digits of trades pushed the whole LOSS column sideways, and with it every
        // point in that column and the heading over them.
        div()
            .flex()
            .gap(look.gap)
            .w(look.w_side())
            .child(figure(
                format!("{sign}{}", dollars(amount)),
                colour,
                look.w_money(),
                look,
            ))
            .child(
                text(format!("({trades})"), look.body, colour)
                    .w(look.digit * COUNT_CHARS)
                    .text_right(),
            )
    };
    div()
        .flex()
        .gap(look.gap)
        .child(coin_cell(&row.coin, "minute", on_coin, look))
        .child(side(row.plus, row.trades_plus, '+', MoonTone::Positive))
        .child(side(row.minus, row.trades_minus, '-', MoonTone::Danger))
}
