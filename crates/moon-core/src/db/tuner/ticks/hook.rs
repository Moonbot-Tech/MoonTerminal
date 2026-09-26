//! MoonHook's own take rule, and the numbers the core writes about its detect.
//!
//! A MoonHook carries **no `SellPrice` field at all** — its sell lives in `HookSellLevel`, in
//! per cent OF THE DETECT DEPTH (core FAQ: "HookSellLevel: заменяет SellPrice. Задается в
//! процентах от глубины детекта… 100 % означает продажу в верхней точке, из которой начался
//! прострел"). A model that reads `SellPrice` for a hook gets the schema default and misses
//! every trade: 117 of 118 on the live sample of 2026-09-22.
//!
//! The depth is a property of the TRADE, not of the strategy, and the report carries it only
//! inside the row's `comment`, which the core writes as
//!
//! ```text
//! <HookTestN1>Hook Long Depth: 2.54% [2.54%] R: 62% d: 2.52% (High: 0.007838  Min: 0.007644
//!   …) InitialPrice: 0.007644 Buffer: [1.25%..1.87%] SellPrice: 1.27%  AbsT: 8.3
//! ```
//!
//! `Depth` is the detect's own depth and `SellPrice` the level the core actually placed —
//! `HookSellLevel · Depth / 100`, which is how the formula below was checked: it reproduces
//! that number on 90 of 118 trades within 0.01 pp, with the buy price as the base (median of
//! fact against prediction +0.003 %). The remaining 28 all sit HIGHER than the formula, never
//! lower; the depth in the comment is written at close time while the take was placed at fill
//! time, and the detect's state in between is not in the report. So the model runs the formula
//! on the depth the stated take implies (`record::placed_hook_depth`) wherever the comment
//! states one. See `docs-internal/STRATEGY_FORMULAS/moonhook.md`.
//!
//! The archive cannot stand in for this: the core files a line only when it was RE-PLACED, and
//! a take that never moved has none — 0 of 155 take-closed trades on the same sample carry an
//! Exit line, against 1483 of 1484 for `Auto Price Down`.

/// Strategy kind name, as `strategies.sqlite` spells it.
pub const KIND_MOONHOOK: &str = "MoonHook";

/// What the core wrote about one hook trade's detect, parsed out of the report's `comment`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HookDetect {
    /// `Depth: X%` — the detect's depth, per cent. The base of the take rule.
    pub depth_pct: f64,
    /// `SellPrice: Y%` — the take the core actually placed, per cent from the buy, before the
    /// delta modifiers. Never a variant's take (a variant asks what another `HookSellLevel`
    /// would have done, and the core's number answers only for the one it used), but the depth
    /// the formula runs on is read back off it (`record::placed_hook_depth`).
    pub stated_take_pct: Option<f64>,
}

/// The take distance of a hook trade, per cent from the fill.
///
/// Args:
///     depth_pct: The trade's detect depth (`HookDetect::depth_pct`).
///     sell_level_pct: `HookSellLevel` of the strategy, per cent of that depth.
pub fn hook_take_pct(depth_pct: f64, sell_level_pct: f64) -> f64 {
    depth_pct * sell_level_pct / 100.0
}

/// Read `Depth:` and `SellPrice:` out of a report row's comment.
///
/// Returns `None` for a comment that carries no hook detect — every other kind's comment, and a
/// hook row whose depth the core did not write.
///
/// Args:
///     comment: The report row's `comment`, as stored.
pub fn parse_hook_detect(comment: &str) -> Option<HookDetect> {
    let depth_pct = percent_after(comment, "Depth:")?;
    (depth_pct.is_finite() && depth_pct > 0.0).then_some(HookDetect {
        depth_pct,
        stated_take_pct: percent_after(comment, "SellPrice:").filter(|v| v.is_finite()),
    })
}

/// The per-cent number that follows `label` in the comment: `"Depth: 2.54%"` → `2.54`.
///
/// Written by hand rather than with a regex: the crate carries no regex dependency, and the
/// shape is fixed — a label, spaces, a number, a per-cent sign.
fn percent_after(text: &str, label: &str) -> Option<f64> {
    let rest = text.split_once(label)?.1.trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(rest.len());
    let (num, tail) = rest.split_at(end);
    // The per-cent sign is what tells a level from a price: `InitialPrice: 0.0076` must never
    // be read as a per cent, and `Depth: 2.54%` must.
    tail.starts_with('%').then(|| num.parse::<f64>().ok())?
}

#[cfg(test)]
mod tests;
