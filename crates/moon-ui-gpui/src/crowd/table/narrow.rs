//! Wrapping crowd records for a narrow Main panel. Values occupy their own lines, retain cents,
//! and grow vertically rather than disappearing behind a clipped fixed-width table column.

use gpui::{
    Div, Hsla, InteractiveElement, MouseButton, ParentElement, Pixels, SharedString, Styled, div,
};
use moon_core::crowd::board::{CoinDay, DaySummary, Trader};
use moon_core::crowd::{Standing, Wire};
use moon_core::util::fmt::{DeltaSign, signed_fixed};
use moon_ui::rgba_from;
use rust_i18n::t;

use super::{
    A_CAPTION, A_HEADING, A_TEXT, CoinClick, DAY_COIN_SEATS, Look, MINUTE_SEATS, TRADERS_SHOWN,
};
use crate::design;

/// A full dollar amount with its rounded sign; never an SI abbreviation or a substring.
fn money(value: f64) -> (String, DeltaSign) {
    signed_fixed(value, 2).unwrap_or_else(|| ("—".into(), DeltaSign::Zero))
}

/// A line can wrap even inside a long number; GPUI falls back to character boundaries when a
/// word exceeds the available width. No row height, ellipsis or line clamp discards its tail.
fn line(value: impl Into<String>, color: Hsla, look: &Look) -> Div {
    wrapping_line(value.into(), color, look.body)
}

/// Geometry shared by all narrow values, kept independent of the theme lookup for regression
/// checks that reject fixed-height or nonwrapping financial text.
fn wrapping_line(value: String, color: Hsla, size: Pixels) -> Div {
    div()
        .w_full()
        .min_w_0()
        .flex_none()
        .whitespace_normal()
        .text_size(size)
        .text_color(color)
        .child(value)
}

/// The record's vertical flow preserves its natural height inside the outer scrolling frame.
fn stack(look: &Look) -> Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .min_w_0()
        .flex_none()
        .gap(look.gap_stack)
}

/// Keep the caption and its value on separate lines, so the label cannot consume the money's
/// width. Labels use the interface font; values inherit the board's numeric font.
fn fact(label: &str, value: String, color: Hsla, look: &Look) -> Div {
    stack(look)
        .child(line(label, look.dim(A_HEADING), look).font_family(design::ui_font()))
        .child(line(value, color, look))
}

/// Full cents and sign determine both the displayed money and its color.
fn amount(label: &str, value: f64, look: &Look) -> Div {
    let (value, sign) = money(value);
    fact(
        label,
        value,
        rgba_from(design::delta_tone(sign).color(look.palette), 1.0),
        look,
    )
}

/// Preserve the existing chart-opening gesture without the table's fixed ticker width.
fn coin(coin: &str, board: &str, click: Option<CoinClick>, look: &Look) -> Div {
    let label = line(coin, look.dim(A_TEXT), look);
    let Some(click) = click else {
        return label;
    };
    let action = click(coin);
    div().w_full().child(
        div()
            .id(SharedString::from(format!("crowd-narrow-{board}-{coin}")))
            .cursor_pointer()
            .hover(|style| style.bg(rgba_from(look.palette.text, super::HOVER_ALPHA)))
            .on_mouse_down(MouseButton::Left, move |event, window, app| {
                app.stop_propagation();
                action(event, window, app);
            })
            .child(label),
    )
}

/// Retain separate winning and losing amounts and their counts; a quiet side remains absent.
pub(crate) fn minute(rows: &[Standing], wire: Wire, click: Option<CoinClick>, look: &Look) -> Div {
    let mut board = stack(look).child(super::minute::caption(wire, look).flex_wrap());
    if rows.is_empty() {
        return board.child(line(t!("crowd.minute.empty"), look.dim(A_HEADING), look));
    }
    for row in rows.iter().take(MINUTE_SEATS) {
        let mut record = stack(look).child(coin(&row.coin, "minute", click, look));
        for (label, value, count) in minute_sides(row) {
            record = record.child(amount(&t!(label), value, look)).child(fact(
                &t!("crowd.col.trades"),
                count.to_string(),
                look.dim(A_TEXT),
                look,
            ));
        }
        board = board.child(record);
    }
    board
}

/// One record per traded side, preserving losses rather than subtracting them from profits.
fn minute_sides(row: &Standing) -> impl Iterator<Item = (&'static str, f64, u32)> {
    [
        ("crowd.col.profit", row.plus, row.trades_plus),
        ("crowd.col.loss", -row.minus, row.trades_minus),
    ]
    .into_iter()
    .filter(|(_, _, count)| *count != 0)
}

/// The day coin board uses one full-width record per coin, retaining every displayed seat.
pub(crate) fn coins(rows: &[CoinDay], click: Option<CoinClick>, look: &Look) -> Div {
    let mut board = stack(look).child(line(t!("crowd.day.coins"), look.dim(A_CAPTION), look));
    if rows.is_empty() {
        return board.child(line(t!("crowd.day.silent"), look.dim(A_HEADING), look));
    }
    for row in rows.iter().take(DAY_COIN_SEATS) {
        board = board.child(
            stack(look)
                .child(coin(&row.coin, "day", click, look))
                .child(amount(&t!("crowd.col.profit"), row.profit, look)),
        );
    }
    board
}

/// The trader board retains rank, stable ID, name, money, count and the service-wide total.
pub(crate) fn traders(rows: &[Trader], summary: Option<DaySummary>, look: &Look) -> Div {
    let mut board = stack(look).child(line(t!("crowd.day.traders"), look.dim(A_CAPTION), look));
    if let Some(day) = summary {
        board = board.child(line(
            t!(
                "crowd.day.total",
                money = money(day.profit).0,
                trades = day.trades.to_string()
            ),
            look.dim(A_HEADING),
            look,
        ));
    }
    if rows.is_empty() {
        return board.child(line(t!("crowd.day.silent"), look.dim(A_HEADING), look));
    }
    for row in rows.iter().take(TRADERS_SHOWN) {
        board = board.child(
            stack(look)
                .child(fact(
                    &t!("crowd.col.rank"),
                    row.place.to_string(),
                    look.dim(A_TEXT),
                    look,
                ))
                .child(fact(
                    &t!("crowd.col.id"),
                    row.id.to_string(),
                    look.dim(A_TEXT),
                    look,
                ))
                .child(fact(
                    &t!("crowd.col.who"),
                    row.name().to_string(),
                    look.dim(A_TEXT),
                    look,
                ))
                .child(amount(&t!("crowd.col.profit"), row.profit, look))
                .child(fact(
                    &t!("crowd.col.trades"),
                    row.trades.to_string(),
                    look.dim(A_TEXT),
                    look,
                )),
        );
    }
    board
}

#[cfg(test)]
mod tests;
