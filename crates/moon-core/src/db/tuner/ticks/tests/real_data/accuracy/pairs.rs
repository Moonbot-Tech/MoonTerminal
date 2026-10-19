//! Pair admission uses report identity and tape availability, never a model verdict.

use crate::db::tuner::ticks::{ParamGroup, ParamKind, TICK_PARAMS};
use std::collections::{BTreeMap, HashMap};

/// Printed verbatim so the pair population can be audited outside the repository.
pub(super) const RULE: &str = "Ordered (a,b): different core_uid, same venue, coin_match_key, is_short and kind; |a.buy_ms-b.buy_ms| <= MOON_TICKS_PAIR_MS (default 3000). Each b keeps its nearest a per A core (ties: lower report_uid). Both directions kept; unordered count reported too. No-op settings retained as controls; effective_diff compares applicable TICK_PARAMS with known schema defaults normalized. same_name is read-only strategy identity or null; version coverage is reported separately.";

/// Model-independent facts used to match a row; synthetic tests need no report database.
#[derive(Clone)]
pub(super) struct Side {
    pub(super) core: u64,
    pub(super) uid: i64,
    pub(super) venue: String,
    pub(super) coin: String,
    pub(super) short: bool,
    pub(super) kind: String,
    pub(super) buy_ms: i64,
    pub(super) values: HashMap<String, String>,
}

impl Side {
    /// Extract match facts before examining any fit verdict.
    pub(super) fn of(row: &super::load::Loaded) -> Self {
        Self {
            core: row.deal.core_uid,
            uid: row.deal.report_uid,
            venue: row.venue.clone(),
            coin: crate::symbol::coin_match_key(&row.deal.coin),
            short: row.deal.is_short,
            kind: row.deal.kind.clone(),
            buy_ms: row.deal.buy_ms,
            values: row.values.clone(),
        }
    }
}

/// Compare only knobs offered for the paired kind, treating known explicit defaults as absent.
pub(super) fn differs(a: &Side, b: &Side, defaults: &HashMap<String, f64>) -> bool {
    knobs_differ(a, b, defaults, false)
}

/// Flag effective entry-knob differences while retaining A's archived entry trajectory.
pub(super) fn entry_differs(a: &Side, b: &Side, defaults: &HashMap<String, f64>) -> bool {
    knobs_differ(a, b, defaults, true)
}

/// Share applicability and default normalization between complete and entry-only cuts.
fn knobs_differ(a: &Side, b: &Side, defaults: &HashMap<String, f64>, entry_only: bool) -> bool {
    TICK_PARAMS
        .iter()
        .filter(|p| {
            (!entry_only || p.group == ParamGroup::Entry)
                && (p.kinds.is_empty() || p.kinds.contains(&a.kind.as_str()))
                && !p.not_kinds.contains(&a.kind.as_str())
        })
        .any(|p| {
            let default = defaults.get(&p.key.to_ascii_lowercase()).copied();
            normalized(a.values.get(p.key), default, p.kind)
                != normalized(b.values.get(p.key), default, p.kind)
        })
}

/// Canonicalize numeric and boolean spellings without guessing unknown builder fallbacks.
fn normalized(value: Option<&String>, default: Option<f64>, kind: ParamKind) -> Option<String> {
    let numeric = value.and_then(|v| match kind {
        ParamKind::Bool => match v.trim().to_ascii_lowercase().as_str() {
            "yes" | "true" | "1" => Some(1.0),
            "no" | "false" | "0" => Some(0.0),
            _ => None,
        },
        ParamKind::Num | ParamKind::Enum(_) => v
            .trim()
            .trim_end_matches('%')
            .replace(',', ".")
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite()),
    });
    let default = default.map(|v| match kind {
        ParamKind::Bool => {
            if v != 0.0 {
                1.0
            } else {
                0.0
            }
        }
        _ => v,
    });
    if let Some(number) = numeric.or_else(|| value.is_none().then_some(default).flatten()) {
        Some(if number == 0.0 {
            "0".to_owned()
        } else {
            number.to_string()
        })
    } else {
        value.map(|v| v.trim().to_owned())
    }
}

/// Keep one nearest A row for each B and A-core, retaining both ordered directions.
pub(super) fn match_pairs(sides: &[Side], window_ms: u64) -> Vec<(usize, usize)> {
    let mut groups = BTreeMap::new();
    for (i, side) in sides.iter().enumerate() {
        groups
            .entry((&side.venue, &side.coin, side.short, &side.kind))
            .or_insert_with(Vec::new)
            .push(i);
    }
    let mut result = Vec::new();
    for indices in groups.values() {
        for &b in indices {
            let mut nearest = BTreeMap::<u64, (u64, i64, usize)>::new();
            for &a in indices {
                if sides[a].core == sides[b].core {
                    continue;
                }
                let distance = sides[a].buy_ms.abs_diff(sides[b].buy_ms);
                if distance > window_ms {
                    continue;
                }
                let candidate = (distance, sides[a].uid, a);
                nearest
                    .entry(sides[a].core)
                    .and_modify(|old| *old = (*old).min(candidate))
                    .or_insert(candidate);
            }
            result.extend(nearest.into_values().map(|(_, _, a)| (a, b)));
        }
    }
    result.sort_unstable();
    result
}

#[cfg(test)]
mod tests;
