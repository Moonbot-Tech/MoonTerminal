//! The service's own two boards for the day: its coin top, and its trader top.
//!
//! Ours is the minute beside them; these two are the SERVICE's, computed there and handed over
//! whole. They are drawn from the same shape the minute is — the same metrics, the same movement,
//! the same figure split at its decimal point — so that three tables on one screen read as three
//! tables and not as three designs.

use std::collections::HashMap;
use std::time::Instant;

use gpui::{Div, ParentElement, Styled, div, px};
use moon_core::crowd::board::{CoinDay, DaySummary, Trader};
use moon_ui::rgba_from;
use rust_i18n::t;

use crate::design;

use super::{
    A_ASIDE, A_CAPTION, A_HEADING, A_MARK, A_TEXT, CLIMB_MARK, CoinClick, DAY_COIN_SEATS,
    DROP_MARK, GLOW_ALPHA, Look, TRADERS_SHOWN, coin_cell, leaving_rows, motion, placed, rows_box,
    signed, signed_compact, state_of, text,
};

/// The day's coins, in the bottom-right corner: what paid the crowd, beside what cost it.
///
/// TWO COLUMNS ACROSS rather than one list down. The board is two ends of a day — the five best
/// and the five worst — and stacking them made a table half the screen tall for ten short rows.
/// Side by side it is five rows, each carrying one winner and one loser, and the pair reads as the
/// comparison it actually is.
///
/// The rows do not slide here, and that is deliberate: a coin moving between the two halves is not
/// moving within a list, so an animation from one column to the other would draw a journey nobody
/// made. What they keep is the glow, which says the money moved.
pub fn day_coins(
    rows: &[CoinDay],
    moved: Option<(&motion::Motion<CoinDay>, Instant)>,
    on_coin: Option<CoinClick>,
    look: &Look,
) -> Div {
    // The board arrives ordered by money, so the split is where the sign turns.
    let (gains, losses): (Vec<&CoinDay>, Vec<&CoinDay>) = rows
        .iter()
        .take(DAY_COIN_SEATS)
        .partition(|row| row.profit >= 0.0);
    let seats = gains.len().max(losses.len()).max(DAY_COIN_SEATS / 2);
    let mut list: Vec<Div> = Vec::new();
    for place in 0..seats {
        let mut row = div().flex().gap(look.gap);
        row = row.child(day_coin_half(
            gains.get(place).copied(),
            moved,
            on_coin,
            look,
        ));
        row = row.child(day_coin_half(
            losses.get(place).copied(),
            moved,
            on_coin,
            look,
        ));
        list.push(
            row.absolute()
                .left(px(0.0))
                .top(px(place as f32 * f32::from(look.row_h)))
                .h(look.row_h),
        );
    }
    div()
        .absolute()
        .right(look.edge_x)
        .bottom(look.edge_bottom)
        .flex()
        .flex_col()
        .gap(look.gap_stack)
        .child(text(
            t!("crowd.day.coins").to_string(),
            look.caption,
            look.dim(A_CAPTION),
        ))
        .children(rows.is_empty().then(|| {
            text(
                t!("crowd.day.silent").to_string(),
                look.body,
                look.dim(A_HEADING),
            )
        }))
        .children((!rows.is_empty()).then(|| {
            div()
                .flex()
                .gap(look.gap)
                .child(look.heading(t!("crowd.col.coin").to_string(), look.w_coin))
                .child(look.heading_over(
                    t!("crowd.col.profit").to_string(),
                    look.w_day(),
                    look.w_day(),
                ))
                .child(look.heading(t!("crowd.col.coin").to_string(), look.w_coin))
                .child(look.heading_over(
                    t!("crowd.col.loss").to_string(),
                    look.w_day(),
                    look.w_day(),
                ))
        }))
        .children((!rows.is_empty()).then(|| {
            div()
                .relative()
                .w(look.w_day_coins())
                .h(px(seats as f32 * f32::from(look.row_h)))
                .children(list)
        }))
}

/// One side of a day row: a ticker and its figure, or the space one would have taken.
///
/// The blank is not padding — it is a seat kept empty, so a day with six winners and four losers
/// keeps its two columns aligned instead of shuffling the second one up.
///
/// Args:
///     row: The coin, when this side has one.
///     moved: What has moved since the last board, for the glow.
///     look: Metrics and colours for this frame.
fn day_coin_half(
    row: Option<&CoinDay>,
    moved: Option<(&motion::Motion<CoinDay>, Instant)>,
    on_coin: Option<CoinClick>,
    look: &Look,
) -> Div {
    let width = look.w_coin + look.gap + look.w_day();
    let Some(row) = row else {
        return div().w(width);
    };
    let mut half = div()
        .flex()
        .gap(look.gap)
        .w(width)
        .child(coin_cell(&row.coin, "day", on_coin, look))
        .child(signed(row.profit, look.w_day(), look));
    let state = state_of(moved, &row.coin);
    if state.glow > 0.0 {
        let colour = if state.gain {
            design::positive_color(look.palette)
        } else {
            design::danger_color(look.palette)
        };
        half = half.bg(rgba_from(colour, GLOW_ALPHA * state.glow));
    }
    half
}

/// The day's traders, down the left: place, who, what they made, how many trades it took.
///
/// A trader who has not consented to being named still counts — the service says so, and their
/// figures are in the totals — so the row is shown with a dash where the handle would be rather
/// than dropped. Dropping it would leave a gap in the numbering that reads as a bug.
///
/// Args:
///     rows: The board, in rank order.
///     moved: What has moved since the last board.
///     marks: How many places of RANK each account moved when the last board arrived.
///     look: Metrics and colours for this frame.
pub fn day_traders(
    rows: &[Trader],
    moved: Option<(&motion::Motion<Trader>, Instant)>,
    marks: &HashMap<u64, i32>,
    summary: Option<DaySummary>,
    look: &Look,
) -> Div {
    let mut list: Vec<Div> = Vec::new();
    for (place, row) in rows.iter().take(TRADERS_SHOWN).enumerate() {
        let state = state_of(moved, &row.id.to_string());
        let shift = marks.get(&row.id).copied().unwrap_or(0);
        list.push(placed(place, state, trader_row(row, shift, look), look));
    }
    // A row on its way out already has the leaving mark; the places it moved through on the way
    // are not what anybody is looking at.
    list.extend(leaving_rows(moved, look, |row, look| {
        trader_row(row, 0, look)
    }));
    div()
        .absolute()
        .left(look.edge_x)
        .top(look.edge_top)
        .flex()
        .flex_col()
        .gap(look.gap_stack)
        // The board is a TOP — fifty of them — and the total beside it is everybody. Without it a
        // reader adds up twenty rows and takes the answer for the day.
        .child(
            div()
                .flex()
                .gap(look.gap_traders)
                .child(text(
                    t!("crowd.day.traders").to_string(),
                    look.caption,
                    look.dim(A_CAPTION),
                ))
                .children(summary.map(|day| {
                    text(
                        t!(
                            "crowd.day.total",
                            money = signed_compact(day.profit).0,
                            trades = day.trades.to_string()
                        )
                        .to_string(),
                        look.caption,
                        look.dim(A_HEADING),
                    )
                })),
        )
        .children(rows.is_empty().then(|| {
            text(
                t!("crowd.day.silent").to_string(),
                look.body,
                look.dim(A_HEADING),
            )
        }))
        .children((!rows.is_empty()).then(|| {
            div()
                .flex()
                .gap(look.gap_traders)
                // Nothing over the marks: they are not a column, they are what the rank did.
                .child(div().w(look.w_shift))
                .child(look.heading_right(t!("crowd.col.rank").to_string(), look.w_place))
                .child(look.heading(t!("crowd.col.id").to_string(), look.w_id))
                .child(look.heading(t!("crowd.col.who").to_string(), look.w_who))
                .child(look.heading_over(
                    t!("crowd.col.profit").to_string(),
                    look.w_money(),
                    look.w_money(),
                ))
                .child(look.heading_right(t!("crowd.col.trades").to_string(), look.w_trades))
        }))
        .child(rows_box(TRADERS_SHOWN, look.w_day_traders(), look).children(list))
}

/// One trader's day: which way the rank moved, place, account, who, money, trades.
///
/// The ACCOUNT sits right after the move, before the handle, because it is the only name half
/// this board has: an anonymous row is `@` and there are many of them, while the number is the
/// service's own and does not change when somebody renames themselves.
///
/// Args:
///     row: The trader.
///     shift: How many places of RANK they moved when the last board arrived — positive for a
///         climb, zero for a row that stayed. It stands until the next board arrives, whenever
///         that is: this one is re-read about once a minute and a mark that faded in a second
///         would never be seen at all.
///     look: Metrics and colours for this frame.
fn trader_row(row: &Trader, shift: i32, look: &Look) -> Div {
    div()
        .flex()
        .gap(look.gap_traders)
        .child(shifted(shift, look).w(look.w_shift))
        .child(
            text(format!("{}", row.place), look.body, look.dim(A_HEADING))
                .w(look.w_place)
                .text_right(),
        )
        .child(text(format!("{}", row.id), look.body, look.dim(A_CAPTION)).w(look.w_id))
        .child(
            text(
                row.name().to_string(),
                look.body,
                look.dim(if row.anonymous() { A_ASIDE } else { A_TEXT }),
            )
            .w(look.w_who),
        )
        .child(signed(row.profit, look.w_money(), look))
        // A count has no point to line up on, so it lines up on its last digit instead: read down
        // the column, that is the same comparison.
        .child(
            text(format!("{}", row.trades), look.body, look.dim(A_ASIDE))
                .w(look.w_trades)
                .text_right(),
        )
}

/// Which way a row went on the board, and by how much.
///
/// A CLIMB is positive and green, a drop red, and a row that stayed where it was gets nothing at
/// all — an arrow on every row would be an arrow on none. The figure is in brackets beside the
/// arrow rather than instead of it: the arrow is read at a glance across twenty rows, the number
/// only by whoever stopped at one.
fn shifted(shift: i32, look: &Look) -> Div {
    let Some(mark) = shift_mark(shift) else {
        return div();
    };
    let colour = if shift > 0 {
        design::positive_color(look.palette)
    } else {
        design::danger_color(look.palette)
    };
    text(mark, look.body, rgba_from(colour, A_MARK))
}

/// What that mark says, or nothing at all for a row that stayed where it was.
///
/// Split out from the drawing because it is the only part of the mark that can be READ back: the
/// board it belongs to moves so rarely that waiting to see one on screen is not a test. Measured
/// while this was written — two polls sixty-five seconds apart returned the same twenty rows in
/// the same order, to the row.
///
/// The figure it is given is a difference of RANKS, not of screen rows: a row the parser drops
/// moves everything under it up the screen without moving anybody's rank, and a mark that counted
/// rows would then contradict the number printed beside it.
fn shift_mark(shift: i32) -> Option<String> {
    if shift == 0 {
        return None;
    }
    let mark = if shift > 0 { CLIMB_MARK } else { DROP_MARK };
    Some(format!("{mark}({shift:+})"))
}

#[cfg(test)]
mod tests;
