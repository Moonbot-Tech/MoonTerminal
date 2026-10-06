//! Snapshot-owned Summary derivations reused by hover-triggered renders.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{Context, Hsla};
use moon_core::db::analytics::{DayPoint, Summary};
use moon_ui::MoonPalette;

use super::{charts, cumulative};
use crate::analytics::AnalyticsView;
use crate::load_state::Note;

/// Retains the snapshot so an allocator cannot recycle its address while the key is live.
#[derive(Clone)]
struct DerivedKey {
    /// Snapshot identity, compared with Arc::ptr_eq rather than its allocation address alone.
    data: Arc<Summary>,
    /// Exact first-match RGB for every core in the query's order.
    configured: Vec<(u64, Option<[u8; 3]>)>,
    /// First query index for each uid; built only when the retained snapshot changes.
    by_uid: HashMap<u64, usize>,
    /// Reused first-wins marks avoid allocating while checking the current servers.
    seen: Vec<bool>,
}

impl DerivedKey {
    /// Compare current first-match RGBs without allocating or changing duplicate-id precedence.
    fn matches_colors(&mut self, servers: impl Iterator<Item = (u64, [u8; 3])>) -> bool {
        self.seen.fill(false);
        for (uid, color) in servers {
            if let Some(&index) = self.by_uid.get(&uid)
                && !self.seen[index]
            {
                self.seen[index] = true;
                if self.configured[index].1 != Some(color) {
                    return false;
                }
            }
        }
        self.configured
            .iter()
            .all(|(uid, color)| color.is_some() == self.seen[self.by_uid[uid]])
    }
}

/// Pure data shared by every chart consumer of one loaded Summary.
#[derive(Clone)]
pub(in crate::analytics) struct SummaryDerived {
    /// Snapshot and current configured colours that produced this entry.
    key: DerivedKey,
    /// One resolved colour per core, reused across legends, bars and popup dots.
    pub(super) colors: Rc<[Hsla]>,
    /// Running money totals, retaining the original f64 accumulation order.
    pub(super) cum: Rc<[f64]>,
    /// Pixel-math copy of the total curve.
    pub(super) pts: Rc<[f32]>,
    /// Stable absolute-contribution order shared by the canvas and legend.
    pub(super) order: Rc<[usize]>,
    /// Colourless running core curves aligned to order, with the original f32 sums.
    pub(super) curves: Rc<[Rc<[f32]>]>,
    /// Shared range over the total then each drawn core, in original fold order.
    pub(super) vmin: f32,
    /// Upper shared range bound, floored exactly like the original canvas input.
    pub(super) vmax: f32,
    /// Total-curve swing indices; width-dependent placement still runs during rendering.
    pub(super) swings: Rc<[usize]>,
    /// Daily text and width-keyed thinning, scoped to this retained snapshot.
    pub(super) daily: DailyLabels,
}

/// Formatting is suffix-keyed; only the thinning result remembers measured width bits.
#[derive(Clone)]
pub(super) struct DailyLabels {
    /// Unit used by the retained strings, unset until render arms the profit unit.
    suffix: Option<String>,
    /// Candidate text for each bucket; drawn labels borrow this rather than reformatting.
    pub(super) texts: Rc<[String]>,
    /// Last longest-by-character candidate, matching Iterator::max_by_key's tie rule.
    widest: Option<usize>,
    /// Snapshot-only profits consumed by the unchanged thinning algorithm.
    profits: Rc<[f64]>,
    /// Interior mutability keeps the chart renderer's shared borrow without scheduling work.
    thinning: RefCell<DailyThinning>,
}

/// One thinning result keyed on the current measured width's exact bits.
#[derive(Clone, Default)]
struct DailyThinning {
    /// None until the first actual chart render measures a candidate label.
    width_bits: Option<u32>,
    /// Set membership used by the bar-building loop.
    labelled: Rc<BTreeSet<usize>>,
}

impl DailyLabels {
    /// Retain bucket profits once; formatting waits for the active render unit.
    fn new(days: &[DayPoint]) -> Self {
        Self {
            suffix: None,
            texts: Rc::from([]),
            widest: None,
            profits: days.iter().map(|d| d.profit).collect(),
            thinning: RefCell::new(DailyThinning::default()),
        }
    }

    /// Refresh formatted candidates only on a suffix change; returns whether text derived.
    fn ensure_texts(&mut self, suffix: &str) -> bool {
        if self.suffix.as_deref() == Some(suffix) {
            return false;
        }
        self.texts = self
            .profits
            .iter()
            .map(|&profit| format!("{}{}", moon_core::util::fmt::compact(profit, 0), suffix))
            .collect();
        self.widest = self
            .texts
            .iter()
            .enumerate()
            .max_by_key(|(_, text)| text.chars().count())
            .map(|(i, _)| i);
        self.suffix = Some(suffix.to_owned());
        true
    }

    /// Measure exactly one cached widest string each render, without caching the width itself.
    pub(super) fn label_w(&self, measure: impl FnOnce(&str) -> f32) -> f32 {
        self.widest.map(|i| measure(&self.texts[i])).unwrap_or(0.0)
    }

    /// Return retained thinning and whether it was rebuilt for different measured width bits.
    pub(super) fn ensure_labelled(&self, label_w: f32) -> (Rc<BTreeSet<usize>>, bool) {
        let mut cached = self.thinning.borrow_mut();
        if cached.width_bits == Some(label_w.to_bits()) {
            return (cached.labelled.clone(), false);
        }
        cached.labelled = Rc::new(
            charts::thinned_labels(&self.profits, charts::PLOT_W_NOMINAL, label_w)
                .into_iter()
                .collect(),
        );
        cached.width_bits = Some(label_w.to_bits());
        (cached.labelled.clone(), true)
    }
}

impl SummaryDerived {
    /// Borrow the exact retained snapshot whose chart data this entry describes.
    pub(super) fn data(&self) -> &Arc<Summary> {
        &self.key.data
    }

    /// Build snapshot-only curves once; changing configured RGBs does not recreate them.
    fn new(
        data: &Arc<Summary>,
        configured: Vec<(u64, Option<[u8; 3]>)>,
        colors: Rc<[Hsla]>,
    ) -> Self {
        // Non-finite profits must not feed NaN coordinates into GPUI layout.
        let fin = |v: f64| if v.is_finite() { v } else { 0.0 };
        let mut acc = 0.0f64;
        let cum: Rc<[f64]> = data
            .days
            .iter()
            .map(|d| {
                acc += fin(d.profit);
                acc
            })
            .collect();
        let pts: Rc<[f32]> = cum.iter().map(|&v| v as f32).collect();
        let order: Rc<[usize]> = cumulative::drawn_core_order(&data.core_days).into();
        let curves: Rc<[Rc<[f32]>]> = order
            .iter()
            .map(|&ci| {
                let mut c = 0.0f32;
                data.core_days[ci]
                    .per_bucket
                    .iter()
                    .take(data.days.len())
                    .map(|v| {
                        c += fin(*v) as f32;
                        c
                    })
                    .collect()
            })
            .collect();
        let mut vmax = pts.iter().copied().fold(0.0f32, f32::max);
        let mut vmin = pts.iter().copied().fold(0.0f32, f32::min);
        for curve in curves.iter() {
            for &v in curve.iter() {
                vmax = vmax.max(v);
                vmin = vmin.min(v);
            }
        }
        let swings = cumulative::swing_labels(&pts).into();
        let mut by_uid = HashMap::new();
        for (index, (uid, _)) in configured.iter().enumerate() {
            by_uid.entry(*uid).or_insert(index);
        }
        let seen = vec![false; configured.len()];
        Self {
            key: DerivedKey {
                data: data.clone(),
                configured,
                by_uid,
                seen,
            },
            colors,
            cum,
            pts,
            order,
            curves,
            vmin: vmin.min(0.0),
            vmax: vmax.max(1e-6),
            swings,
            daily: DailyLabels::new(&data.days),
        }
    }
}

/// Resolve configured RGBs in linear time, retaining iter().find's FIRST duplicate id.
fn configured_colors(
    data: &Summary,
    servers: impl Iterator<Item = (u64, [u8; 3])>,
) -> Vec<(u64, Option<[u8; 3]>)> {
    let mut by_uid = HashMap::new();
    for (id, color) in servers {
        by_uid.entry(id).or_insert(color);
    }
    data.core_days
        .iter()
        .map(|c| (c.uid, by_uid.get(&c.uid).copied()))
        .collect()
}

/// Return the prepared entry and whether colours changed, without allocating on cache hits.
fn ensure_derived(
    slot: &mut Option<Rc<SummaryDerived>>,
    data: Arc<Summary>,
    servers: impl Iterator<Item = (u64, [u8; 3])> + Clone,
    p: MoonPalette,
) -> (&mut Rc<SummaryDerived>, bool) {
    if slot
        .as_ref()
        .is_some_and(|cached| !Arc::ptr_eq(cached.data(), &data))
    {
        *slot = None;
    }
    let mut changed = false;
    let cached = slot.get_or_insert_with(|| {
        changed = true;
        let configured = configured_colors(&data, servers.clone());
        // fallback_core_color is reached only when picker_palette's 36 constant swatches are exhausted.
        let colors = charts::distinct_core_colors(&configured, p).into();
        Rc::new(SummaryDerived::new(&data, configured, colors))
    });
    if !changed {
        let entry = Rc::make_mut(cached);
        if !entry.key.matches_colors(servers.clone()) {
            let configured = configured_colors(&data, servers);
            entry.colors = charts::distinct_core_colors(&configured, p).into();
            entry.key.configured = configured;
            changed = true;
        }
    }
    (cached, changed)
}

impl AnalyticsView {
    /// Return ready Summary derivations or its exact placeholder, without scheduling repaint work.
    pub(in crate::analytics) fn ensure_summary_derived(
        &mut self,
        cx: &Context<Self>,
    ) -> Result<Rc<SummaryDerived>, Note> {
        let data = self.data.view(|d| d.cur.n == 0)?;
        let backend = self.backend.read(cx);
        let (cached, _) = ensure_derived(
            &mut self.summary_derived,
            data.clone(),
            backend.config.servers.iter().map(|s| (s.id, s.color)),
            MoonPalette::active(cx),
        );
        Rc::make_mut(cached)
            .daily
            .ensure_texts(crate::analytics::pnl_suffix());
        Ok(cached.clone())
    }
}

#[cfg(test)]
mod tests;
