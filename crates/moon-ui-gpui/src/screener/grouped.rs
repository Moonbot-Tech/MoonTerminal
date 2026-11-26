//! Account totals for a market shared by multiple cores on one venue.

use moon_core::session::CoreId;
use std::collections::HashMap;
use std::hash::Hash;

/// One core's account facts for the market, in publication order.
#[derive(Clone, Copy, Default)]
pub(super) struct MemberAcct {
    pub core: CoreId,
    pub leverage_x: i32,
    pub max_leverage: i32,
    pub isolated: Option<bool>,
    pub pnl: f64,
    pub session: Option<f64>,
    pub pos: f64,
    pub orders: u32,
}

/// Totals across the visible member cores.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Aggregate {
    pub orders: u64,
    pub pnl: f64,
    pub session: Option<f64>,
    pub pos: f64,
    pub lev: LevSummary,
}

/// Active leverage range and the highest maximum stated by any member.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum LevSummary {
    #[default]
    None,
    Max(i32),
    Same {
        x: i32,
        max: i32,
        mode: Mode,
    },
    Range {
        lo: i32,
        hi: i32,
        max: i32,
        mode: Mode,
    },
}

/// Margin modes stated by the members.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    I,
    C,
    Mixed,
    Unknown,
}

/// Sum member counters and preserve the absence of an unstated session or leverage.
pub(super) fn aggregate(members: impl IntoIterator<Item = MemberAcct>) -> Aggregate {
    let mut agg = Aggregate::default();
    let mut lo = i32::MAX;
    let mut hi = 0;
    let mut max = 0;
    let mut isolated = false;
    let mut cross = false;
    for member in members {
        agg.orders += u64::from(member.orders);
        agg.pnl += member.pnl;
        if let Some(session) = member.session {
            *agg.session.get_or_insert(0.0) += session;
        }
        agg.pos += member.pos;
        max = max.max(member.max_leverage);
        if member.leverage_x > 0 {
            lo = lo.min(member.leverage_x);
            hi = hi.max(member.leverage_x);
        }
        isolated |= member.isolated == Some(true);
        cross |= member.isolated == Some(false);
    }
    let mode = match (isolated, cross) {
        (true, true) => Mode::Mixed,
        (true, false) => Mode::I,
        (false, true) => Mode::C,
        (false, false) => Mode::Unknown,
    };
    agg.lev = match (hi, max) {
        (0, 0) => LevSummary::None,
        (0, _) => LevSummary::Max(max),
        _ if lo == hi => LevSummary::Same { x: hi, max, mode },
        _ => LevSummary::Range { lo, hi, max, mode },
    };
    agg
}

/// Prefer the first core with a position or orders, falling back to the first visible core.
pub(super) fn chart_core(members: impl IntoIterator<Item = MemberAcct>) -> Option<CoreId> {
    let mut members = members.into_iter();
    let first = members.next()?;
    Some(
        std::iter::once(first)
            .chain(members)
            .find(|member| member.pos != 0.0 || member.orders > 0)
            .unwrap_or(first)
            .core,
    )
}

/// Format a leverage summary with a compact suffix for its known margin modes.
pub(super) fn lev_text(lev: LevSummary) -> String {
    let suffix = |mode| match mode {
        Mode::I => " I",
        Mode::C => " C",
        Mode::Mixed => " I/C",
        Mode::Unknown => "",
    };
    match lev {
        LevSummary::None => "—".into(),
        LevSummary::Max(max) => max.to_string(),
        LevSummary::Same { x, max, mode } => format!("{x} / {max}{}", suffix(mode)),
        LevSummary::Range { lo, hi, max, mode } => format!("{lo}–{hi} / {max}{}", suffix(mode)),
    }
}

/// Index rows by key without disturbing first-seen group or member order.
pub(super) fn group_slots<K: Eq + Hash>(keys: impl IntoIterator<Item = K>) -> Vec<Vec<usize>> {
    let mut slots = HashMap::new();
    let mut groups = Vec::new();
    for (index, key) in keys.into_iter().enumerate() {
        let slot = *slots.entry(key).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[slot].push(index);
    }
    groups
}

#[cfg(test)]
mod tests;
