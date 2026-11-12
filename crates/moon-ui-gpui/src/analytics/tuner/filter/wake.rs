//! What a Save wakes: the limits that start rejecting coins because the save clears an Ignore
//! flag, and that the save itself does not rewrite.
//!
//! Clearing a flag turns a whole Moonbot section on, not the one bound just fitted, and the
//! deals behind the fit were traded with that section asleep. So the same answer serves three
//! readers: the what-if applies the woken bounds to its variants, the grid shows them beside
//! the fields, and the save dialog names them — with the limits no report column can model.

use std::collections::HashMap;

use moon_core::db::tuner::{Bound, FIELDS, FieldClass, FieldSpec, StratFilters, Variant};
use rust_i18n::t;

/// Which filter groups a set of bounds writes into.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Touched {
    pub delta: bool,
    pub volume: bool,
    pub bvsv: bool,
    pub ping: bool,
    pub base: bool,
    /// Any Filters-section threshold; a detect threshold alone is not one.
    pub filter: bool,
}

/// Where a save puts the slot fields it was asked to write.
#[derive(Debug, Default)]
pub(super) struct SlotPlan {
    /// `(slot 2|3, field, min, max)` in the order the slots were taken.
    pub assigned: Vec<(u8, &'static str, Option<f64>, Option<f64>)>,
    /// Fields that found no slot: there are only two.
    pub dropped: Vec<&'static str>,
    /// `(slot, the foreign type it held, the field that takes it)`: a live filter of a type
    /// the report has no column for, overwritten as a last resort.
    pub replaced: Vec<(u8, String, &'static str)>,
}

/// Assign slot fields to Delta2/Delta3: its own previous place first (if the type is already
/// set), then the first free one in order. Slots held by a type with no report column
/// (2h/30m/Pump5m — a 'foreign' live filter) are overwritten LAST.
///
/// Args:
///     strat: The strategy card the save is computed against.
///     wanted: Slot fields with a bound, `(field, min, max)`, in grid order.
pub(super) fn plan_slots(
    strat: &StratFilters,
    wanted: &[(&'static str, Option<f64>, Option<f64>)],
) -> SlotPlan {
    let mut plan = SlotPlan::default();
    if wanted.is_empty() {
        return plan;
    }
    let held = |slot: u8| {
        strat
            .slots
            .iter()
            .find(|(n, ..)| *n == slot)
            .map(|(_, f, ..)| *f)
    };
    let (cur2, cur3) = (held(2), held(3));
    let foreign = &strat.foreign_slots;
    let mut used = [false, false]; // [Delta2, Delta3]
    for (n, _) in foreign {
        used[(*n - 2) as usize] = true;
    }
    // First — the fields already sitting in their own slots.
    for (col, lo, hi) in wanted {
        if cur2 == Some(*col) && !used[0] {
            used[0] = true;
            plan.assigned.push((2, col, *lo, *hi));
        } else if cur3 == Some(*col) && !used[1] {
            used[1] = true;
            plan.assigned.push((3, col, *lo, *hi));
        }
    }
    // Then — the free slots; last — overwriting the 'foreign' ones.
    for overwrite_foreign in [false, true] {
        for (col, lo, hi) in wanted {
            if plan.assigned.iter().any(|(_, f, ..)| f == col) {
                continue;
            }
            let Some(i) = (0..2).find(|i| {
                !used[*i]
                    || (overwrite_foreign
                        && foreign.iter().any(|(n, _)| *n == *i as u8 + 2)
                        && !plan.assigned.iter().any(|(n, ..)| *n == *i as u8 + 2))
            }) else {
                if overwrite_foreign {
                    plan.dropped.push(col);
                }
                continue;
            };
            used[i] = true;
            if let Some((_, ty)) = foreign.iter().find(|(n, _)| *n == i as u8 + 2) {
                plan.replaced.push((i as u8 + 2, ty.clone(), col));
            }
            plan.assigned.push((i as u8 + 2, col, *lo, *hi));
        }
    }
    plan
}

/// The Ignore flags a save changes, as `(flag, ignore)`: the classes whose thresholds it writes
/// are enabled, and an explicit 'ignore' click on a subheader wins over that.
///
/// Args:
///     f: The strategy card the save is computed against.
///     touched: The groups the written thresholds belong to.
///     staged: The staged subheader toggles, in ignore terms.
///     force: Emit the enabling flags whatever the card holds (a bulk save that writes).
pub(super) fn flag_changes(
    f: &StratFilters,
    touched: Touched,
    staged: &HashMap<&'static str, bool>,
    force: bool,
) -> Vec<(&'static str, bool)> {
    let mut flags: Vec<(&'static str, bool)> = Vec::new();
    // A detect threshold is live under IgnoreFilters, so writing it alone clears nothing.
    if touched.filter && (force || f.ignore_filters) {
        flags.push(("IgnoreFilters", false));
    }
    if touched.delta && (force || f.ignore_delta) {
        flags.push(("IgnoreDelta", false));
    }
    // BV/SV is a Filters/Volume subgroup: its thresholds need IgnoreVolume cleared AND
    // the filter itself enabled.
    if (touched.volume || touched.bvsv) && (force || f.ignore_volume) {
        flags.push(("IgnoreVolume", false));
    }
    if touched.bvsv && (force || !f.use_bvsv) {
        flags.push(("UseBV_SV_Filter", false));
    }
    if touched.ping && (force || f.ignore_ping) {
        flags.push(("IgnorePing", false));
    }
    if touched.base && (force || f.ignore_base) {
        flags.push(("IgnoreBase", false));
    }
    for (&flag, &want) in staged {
        flags.retain(|(fl, _)| *fl != flag);
        let cur = match flag {
            "IgnoreFilters" => f.ignore_filters,
            "IgnorePing" => f.ignore_ping,
            "IgnoreDelta" => f.ignore_delta,
            "IgnoreVolume" => f.ignore_volume,
            "IgnoreBase" => f.ignore_base,
            "UseBV_SV_Filter" => !f.use_bvsv,
            _ => continue,
        };
        if want != cur {
            flags.push((flag, want));
        }
    }
    flags
}

/// The card's switches as a save leaves them — the flags alone, none of its bounds, so the
/// grid can ask every frame without copying the card. `UseBV_SV_Filter` is an enabler, so its
/// "ignore" is the switch turned off.
fn after(f: &StratFilters, flags: &[(&'static str, bool)]) -> StratFilters {
    let mut after = StratFilters {
        found: f.found,
        ignore_filters: f.ignore_filters,
        ignore_delta: f.ignore_delta,
        ignore_volume: f.ignore_volume,
        ignore_base: f.ignore_base,
        use_bvsv: f.use_bvsv,
        ignore_ping: f.ignore_ping,
        other_ignores: f.other_ignores.clone(),
        ..StratFilters::default()
    };
    for &(flag, ignore) in flags {
        match flag {
            "IgnoreFilters" => after.ignore_filters = ignore,
            "IgnorePing" => after.ignore_ping = ignore,
            "IgnoreDelta" => after.ignore_delta = ignore,
            "IgnoreVolume" => after.ignore_volume = ignore,
            "IgnoreBase" => after.ignore_base = ignore,
            "UseBV_SV_Filter" => after.use_bvsv = !ignore,
            _ => {}
        }
    }
    after
}

/// A stored bound of the grid that a save wakes.
#[derive(Clone)]
pub(super) struct Woken {
    pub spec: &'static FieldSpec,
    /// The Delta2/Delta3 slot that holds it, for a slot field.
    pub slot: Option<u8>,
    pub lo: Option<f64>,
    pub hi: Option<f64>,
    /// Whether it is the corridor the strategy was born with.
    pub factory: bool,
}

/// The stored bounds whose class is asleep on `before` and live on `after`.
fn woken(before: &StratFilters, after: &StratFilters) -> Vec<Woken> {
    if !before.found {
        return Vec::new();
    }
    FIELDS
        .iter()
        .filter(|spec| {
            spec.class != FieldClass::Detect
                && before.class_ignored(spec.class)
                && !after.class_ignored(spec.class)
        })
        .filter_map(|spec| {
            let (slot, lo, hi) = if spec.class == FieldClass::DeltaSlot {
                let (n, lo, hi) = before.slot_of(spec.col)?;
                (Some(n), lo, hi)
            } else {
                let (lo, hi) = before.bounds.get(spec.col).copied()?;
                (None, lo, hi)
            };
            (lo.is_some() || hi.is_some()).then(|| Woken {
                spec,
                slot,
                lo,
                hi,
                factory: before.factory.contains(spec.col),
            })
        })
        .collect()
}

/// What saving a set of what-if bounds wakes: the stored bounds that start working. A side
/// the bounds set themselves stays in the answer — the caller lays its own number over it.
///
/// A slot the save hands to another field keeps the thresholds that field does not set — the
/// save writes the type and only the sides it has — so they wake too, as bounds of the NEW
/// field: Delta2 held as `1h 0..20` and given to `15m` with only a cap goes on rejecting every
/// negative 15m delta by the old `0`.
///
/// Args:
///     strat: The anchor strategy's card.
///     bounds: One variant's bounds, as the what-if takes them.
///     staged: The staged subheader toggles.
///     drops_only: Whether the scope offers the Detect group.
pub(super) fn woken_by(
    strat: &StratFilters,
    bounds: &[Bound],
    staged: &HashMap<&'static str, bool>,
    drops_only: bool,
) -> Vec<Woken> {
    if !strat.found {
        return Vec::new();
    }
    let mut touched = Touched::default();
    let mut slot_wanted = Vec::new();
    for spec in FIELDS {
        let Some(bound) = bounds.iter().find(|b| b.field == spec.col) else {
            continue;
        };
        if !super::offer_field(spec.class, drops_only) {
            continue;
        }
        if spec.class == FieldClass::DeltaSlot {
            if bound.from.is_some() || bound.to.is_some() {
                slot_wanted.push((spec.col, bound.from, bound.to));
            }
            continue;
        }
        if !((bound.from.is_some() && spec.p_min.is_some())
            || (bound.to.is_some() && spec.p_max.is_some()))
        {
            continue;
        }
        touched.filter |= spec.class != FieldClass::Detect;
        match spec.class {
            FieldClass::Delta => touched.delta = true,
            FieldClass::Volume => touched.volume = true,
            FieldClass::BvSv => touched.bvsv = true,
            FieldClass::Ping => touched.ping = true,
            FieldClass::Base => touched.base = true,
            FieldClass::Filter | FieldClass::DeltaSlot | FieldClass::Detect => {}
        }
    }
    let plan = plan_slots(strat, &slot_wanted);
    if !plan.assigned.is_empty() {
        touched.delta = true;
        touched.filter = true;
    }
    let flags = flag_changes(strat, touched, staged, false);
    woken(strat, &after(strat, &flags))
        .into_iter()
        .filter_map(|w| {
            let taken_by = w.slot.and_then(|slot| {
                plan.assigned
                    .iter()
                    .find(|(n, col, ..)| *n == slot && *col != w.spec.col)
                    .map(|(_, col, ..)| *col)
            });
            match taken_by {
                Some(col) => FIELDS
                    .iter()
                    .find(|spec| spec.col == col)
                    .map(|spec| Woken {
                        spec,
                        factory: false,
                        ..w
                    }),
                None => Some(w),
            }
        })
        .collect()
}

/// The what-if variants with the bounds their own save would wake laid under them: a side a
/// variant sets wins, a side it leaves open takes the stored number. The first variant is the
/// fact and stays as it is, and so does a variant with no bound of its own: an empty column is
/// no what-if, whatever the staged toggles would wake.
///
/// Args:
///     variants: `[fact, v1..vN]` as `TunerState::variants` builds them.
///     strat: The card of the strategy the query is scoped to, read with the query.
///     staged: The staged subheader toggles.
///     drops_only: Whether the scope offers the Detect group.
pub(super) fn with_woken(
    mut variants: Vec<Variant>,
    strat: &StratFilters,
    staged: &HashMap<&'static str, bool>,
    drops_only: bool,
) -> Vec<Variant> {
    for variant in variants.iter_mut().skip(1) {
        if variant.bounds.is_empty() {
            continue;
        }
        for w in woken_by(strat, &variant.bounds, staged, drops_only) {
            match variant.bounds.iter_mut().find(|b| b.field == w.spec.col) {
                Some(bound) => {
                    bound.from = bound.from.or(w.lo);
                    bound.to = bound.to.or(w.hi);
                }
                None => variant.bounds.push(Bound {
                    field: w.spec.col.to_string(),
                    from: w.lo,
                    to: w.hi,
                }),
            }
        }
    }
    variants
}

/// The Ignore flags a finished change list writes, as `(flag, ignore)`.
fn flags_in(changes: &[(String, String)]) -> Vec<(&'static str, bool)> {
    const FLAGS: [&str; 6] = [
        "IgnoreFilters",
        "IgnorePing",
        "IgnoreDelta",
        "IgnoreVolume",
        "IgnoreBase",
        "UseBV_SV_Filter",
    ];
    changes
        .iter()
        .filter_map(|(key, value)| {
            let flag = FLAGS.into_iter().find(|flag| flag == key)?;
            let yes = value.eq_ignore_ascii_case("YES");
            // The enabler is "ignored" while it is off.
            Some((flag, if flag == "UseBV_SV_Filter" { !yes } else { yes }))
        })
        .collect()
}

/// The save dialog's lines about what else starts working: the stored bounds a cleared flag
/// wakes and this save does not rewrite, then the limits outside the grid. Numbers print as
/// stored, unrounded: a price step of `0.00004` rounded to `0` would read as "blocks every
/// coin". A bulk save gets one conditional line instead — it writes the enabling flags to
/// every target whatever each one holds, each target wakes its own numbers, and the anchor's
/// are not theirs.
///
/// Args:
///     strat: The anchor strategy's card.
///     changes: The change list the dialog confirms.
///     bulk: Whether the save fans out to several strategies.
pub(super) fn warnings(
    strat: &StratFilters,
    changes: &[(String, String)],
    bulk: bool,
) -> Vec<String> {
    let flags = flags_in(changes);
    if bulk {
        return flags
            .iter()
            .any(|(_, ignore)| !ignore)
            .then(|| t!("analytics.tuner.wake_bulk").to_string())
            .into_iter()
            .collect();
    }
    if !strat.found {
        return Vec::new();
    }
    let after = after(strat, &flags);
    let written = |key: &str| changes.iter().any(|(k, _)| k == key);
    let mut bounds = Vec::new();
    for w in woken(strat, &after) {
        // A slot handed to another field keeps the side that field does not set: the keys are
        // the slot's own, so what the save writes drops out below and the rest is named.
        let (min_key, max_key) = match w.slot {
            Some(n) => (Some(format!("Delta{n}_Min")), Some(format!("Delta{n}_Max"))),
            None => (
                w.spec.p_min.map(String::from),
                w.spec.p_max.map(String::from),
            ),
        };
        let parts: Vec<String> = [(min_key, w.lo), (max_key, w.hi)]
            .into_iter()
            .filter_map(|(key, value)| {
                let (key, value) = (key?, value?);
                (!written(&key)).then(|| format!("{key} = {value}"))
            })
            .collect();
        if parts.is_empty() {
            continue;
        }
        let text = parts.join(", ");
        bounds.push(if w.factory {
            format!("{text} ({})", t!("analytics.tuner.factory_mark"))
        } else {
            text
        });
    }
    let mut extras: Vec<String> = strat
        .extras
        .iter()
        .filter(|(extra, _)| {
            extra.gates.iter().any(|gate| strat.ignores(gate))
                && !extra.gates.iter().any(|gate| after.ignores(gate))
        })
        .map(|(extra, value)| format!("{} = {value}", extra.key))
        .collect();
    // A slot of a type the report has no column for (2h/30m/Pump5m) wakes with the deltas and
    // is no bound of the grid: named by its type unless this save takes the slot.
    if strat.class_ignored(FieldClass::Delta) && !after.class_ignored(FieldClass::Delta) {
        extras.extend(
            strat
                .foreign_slots
                .iter()
                .filter(|(n, _)| !written(&format!("Delta{n}_Type")))
                .map(|(n, ty)| format!("Delta{n}_Type = {ty}")),
        );
    }
    let mut lines = Vec::new();
    if !bounds.is_empty() {
        lines.push(t!("analytics.tuner.wake_bounds", list = bounds.join("; ")).to_string());
    }
    if !extras.is_empty() {
        lines.push(t!("analytics.tuner.wake_extras", list = extras.join(", ")).to_string());
    }
    lines
}

#[cfg(test)]
mod tests;
