//! MoonStrike's own take rule, and the numbers the core writes about its detect.
//!
//! A MoonStrike carries **no `SellPrice` field at all** — its take lives in `MStrikeSellLevel`,
//! in per cent OF THE DETECT DEPTH, the same shape as MoonHook's `HookSellLevel`
//! ([`super::hook`]). A model that reads `SellPrice` for a strike gets the schema default.
//!
//! The depth is a property of the TRADE, and the report carries it only inside the row's
//! `comment`, which the core writes as
//!
//! ```text
//!  MoonStrike USDT-XALPHA <(strategy <UNIT_01>)> [Short]  DetectTime: ..  TradeTime: ..
//!   Latency: 4  LastBID: 0.0104  TradePrice: 0.0100  Depth: 4.00  Vol: 1000.00$ ...
//! ```
//!
//! Unlike the hook's, the strike's `Depth:` carries no per-cent sign, and `TradePrice:` is the
//! price at the detect — the bottom of the strike for a long, its top for a short. The take
//! spans the strike from its deeper end back toward where it started:
//!
//! - `top = TradePrice · (1 + s · Depth / 100)`, `s = +1` for a long, `−1` for a short;
//! - `low` = the deeper of `TradePrice` and the fill (the lower for a long, the higher for a
//!   short);
//! - the take price `= low + MStrikeSellLevel / 100 · (top − low)`.
//!
//! Checked against 505 take-closed live trades (2026-10-15): fact vs prediction median within
//! 0.05 %, 239 of 505 within 0.05 %; alternatives off the buy price or off
//! `TradePrice` alone fit worse. `MStrikeSellAdjust` is not separable by that data and is not
//! modelled.

/// Strategy kind name, as `strategies.sqlite` spells it.
pub const KIND_MOONSTRIKE: &str = "MoonStrike";

/// What the core wrote about one strike trade's detect, parsed out of the report's `comment`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrikeDetect {
    /// `Depth: X` — the strike's depth, per cent, written without a per-cent sign.
    pub depth_pct: f64,
    /// `TradePrice: P` — the price at the detect, the strike's deeper end.
    pub trade_price: f64,
}

/// The take PRICE of a strike trade, before the delta modifiers.
///
/// The strike runs from `TradePrice` back up (a short: down) by its depth to `top`; the take
/// stands `MStrikeSellLevel` per cent of the way from the deeper of `TradePrice` and the fill
/// to that top — a fill deeper than the detect price shortens nothing it did not have to.
///
/// Args:
///     d: The trade's detect.
///     fill_price: The entry's fill.
///     sell_level_pct: `MStrikeSellLevel` of the strategy, per cent of the span.
///     is_long: The position's side.
pub fn strike_take_level(
    d: StrikeDetect,
    fill_price: f64,
    sell_level_pct: f64,
    is_long: bool,
) -> f64 {
    let s = if is_long { 1.0 } else { -1.0 };
    let top = d.trade_price * (1.0 + s * d.depth_pct / 100.0);
    let low = if is_long {
        d.trade_price.min(fill_price)
    } else {
        d.trade_price.max(fill_price)
    };
    low + sell_level_pct / 100.0 * (top - low)
}

/// Read `Depth:` and `TradePrice:` out of a report row's comment.
///
/// Returns `None` unless both are present, finite and positive — every other kind's comment,
/// and a strike row whose detect the core did not write.
///
/// Args:
///     comment: The report row's `comment`, as stored.
pub fn parse_strike_detect(comment: &str) -> Option<StrikeDetect> {
    let depth_pct = number_after(comment, "Depth:")?;
    let trade_price = number_after(comment, "TradePrice:")?;
    let ok = |v: f64| v.is_finite() && v > 0.0;
    (ok(depth_pct) && ok(trade_price)).then_some(StrikeDetect {
        depth_pct,
        trade_price,
    })
}

/// The number that follows `label` in the comment: `"Depth: 4.05"` → `4.05`. No per-cent sign
/// is required — the strike writes its depth bare, unlike the hook (`hook::percent_after`).
///
/// The label must start a word: `MaxDepth:` is not `Depth:`.
fn number_after(text: &str, label: &str) -> Option<f64> {
    let at = text.match_indices(label).map(|(i, _)| i).find(|&i| {
        text[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric())
    })?;
    let rest = text[at + label.len()..].trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(rest.len());
    rest[..end].parse::<f64>().ok()
}

#[cfg(test)]
mod tests;
