//! Snapshot-owned Summary derivations reused by hover-triggered renders.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{Context, Hsla};
use moon_core::db::analytics::Summary;
use moon_ui::MoonPalette;

use super::charts;
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
    /// Running curve of EVERY core, indexed like `Summary::core_days`.
    pub(super) core_curves: Rc<[Rc<[f32]>]>,
    /// Running peak of the total curve, for the drawdown band.
    pub(super) peaks: Rc<[f32]>,
    /// Bucket of the peak that opens the maximum drawdown.
    pub(super) dd_peak_ix: Option<usize>,
    /// Bucket at the bottom of the maximum drawdown.
    pub(super) dd_trough_ix: Option<usize>,
    /// Bucket with the largest profit.
    pub(super) day_max_ix: Option<usize>,
    /// Bucket with the smallest profit.
    pub(super) day_min_ix: Option<usize>,
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
        let core_curves: Rc<[Rc<[f32]>]> = data
            .core_days
            .iter()
            .map(|core| {
                let mut c = 0.0f32;
                core.per_bucket
                    .iter()
                    .take(data.days.len())
                    .map(|v| {
                        c += fin(*v) as f32;
                        c
                    })
                    .collect()
            })
            .collect();
        let (peaks, dd_peak_ix, dd_trough_ix) = drawdown_marks(&pts);
        let profit_ix = |better: fn(f64, f64) -> bool| {
            data.days
                .iter()
                .enumerate()
                .filter(|(_, d)| d.profit.is_finite())
                .fold(None::<(usize, f64)>, |best, (i, d)| match best {
                    Some((_, v)) if !better(d.profit, v) => best,
                    _ => Some((i, d.profit)),
                })
                .map(|(i, _)| i)
        };
        let day_max_ix = profit_ix(|a, b| a > b);
        let day_min_ix = profit_ix(|a, b| a < b);
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
            core_curves,
            peaks,
            dd_peak_ix,
            dd_trough_ix,
            day_max_ix,
            day_min_ix,
        }
    }
}

/// Running peak of a curve plus the peak and trough buckets of its deepest drawdown.
///
/// Args:
///     pts: Cumulative total curve.
///
/// Returns:
///     The running peak per bucket, then the bucket of the peak opening the maximum drawdown and
///     the bucket at its bottom; both `None` when the curve never falls below a prior peak.
pub(super) fn drawdown_marks(pts: &[f32]) -> (Rc<[f32]>, Option<usize>, Option<usize>) {
    let mut peak = f32::NEG_INFINITY;
    let mut peak_ix = 0;
    let mut deepest = (0.0f32, None);
    let peaks = pts
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            if v > peak {
                peak = v;
                peak_ix = i;
            }
            if peak - v > deepest.0 {
                deepest = (peak - v, Some((peak_ix, i)));
            }
            peak
        })
        .collect();
    let (peak_ix, trough_ix) = deepest.1.unzip();
    (peaks, peak_ix, trough_ix)
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

/// Derive a snapshot outside a view, for tests of the blocks that render from it.
#[cfg(test)]
pub(in crate::analytics) fn derive_for_test(
    data: Arc<Summary>,
    p: MoonPalette,
) -> Rc<SummaryDerived> {
    let mut slot = None;
    ensure_derived(&mut slot, data, std::iter::empty(), p);
    slot.expect("derived")
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
        Ok(cached.clone())
    }
}

#[cfg(test)]
mod tests;
