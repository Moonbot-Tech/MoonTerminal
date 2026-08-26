//! Logic-only container for one chart panel: open, auto, prune, and layout without GPU state. It
//! ports the semantics of `moon_chart::container`, but the panel stores only `ChartView` view math,
//! not a wgpu engine, because rendering uses the native own-pass layers such as `super::combo`.
//! Layer GPU state lives separately in `RenderState` (see `mod.rs`) and synchronizes to these panels
//! by index.

use moon_chart::view::{ChartView, Rect};
use moon_core::session::CoreId;

// Reuse shared panel view and source types, but NOT layout modes: one terminal `ChartEngine` owns at
// most one market.
pub use moon_chart::container::{ContainerKind, PaneSource};

/// Apply price scale to a view: `None` means Auto; `Some(fraction)` is a fraction of price.
fn apply_scale(view: &mut ChartView, pct: Option<f32>) {
    match pct {
        None => view.set_auto(),
        Some(p) => view.set_scale_percent(p),
    }
}

/// One panel containing core, market, source, and coordinate view; GPU layers live by index in `RenderState`.
#[derive(Clone)]
pub struct Pane {
    pub core: CoreId,
    pub market: String,
    pub source: PaneSource,
    pub view: ChartView,
    /// Requirement 2: a user-pinned AddToChart panel does not close on TTL.
    ///
    /// This does not affect Manual panels, which already live indefinitely. The flag is session-only
    /// because panels themselves are not persisted.
    pub pinned: bool,
}

#[derive(Clone)]
pub struct Container {
    /// Tab identity (`Main` or `Chart{num}`), reserved for later layout persistence.
    #[allow(dead_code)]
    pub kind: ContainerKind,
    pane: Option<Pane>,
    /// Current container price scale, with `None` for Auto; new panels start with this value.
    scale: Option<f32>,
}

impl Container {
    pub fn new(kind: ContainerKind) -> Self {
        Self {
            kind,
            pane: None,
            scale: None,
        }
    }

    fn new_view(&self, epoch_ms: f64) -> ChartView {
        let mut view = ChartView::new(epoch_ms);
        apply_scale(&mut view, self.scale);
        view
    }

    fn find(&self, core: CoreId, market: &str) -> Option<usize> {
        self.pane
            .as_ref()
            .is_some_and(|p| p.core == core && p.market == market)
            .then_some(0)
    }

    pub fn view_mut(&mut self, idx: usize) -> Option<&mut ChartView> {
        if idx == 0 {
            self.pane.as_mut().map(|p| &mut p.view)
        } else {
            None
        }
    }

    pub fn target(&self, idx: usize) -> Option<(CoreId, String)> {
        if idx == 0 {
            self.pane.as_ref().map(|p| (p.core, p.market.clone()))
        } else {
            None
        }
    }

    pub fn target_ref(&self, idx: usize) -> Option<(CoreId, &str)> {
        if idx == 0 {
            self.pane.as_ref().map(|p| (p.core, p.market.as_str()))
        } else {
            None
        }
    }

    pub fn pane(&self, idx: usize) -> Option<&Pane> {
        if idx == 0 { self.pane.as_ref() } else { None }
    }

    pub fn pane_mut(&mut self, idx: usize) -> Option<&mut Pane> {
        if idx == 0 { self.pane.as_mut() } else { None }
    }

    pub fn panes(&self) -> &[Pane] {
        match &self.pane {
            Some(pane) => std::slice::from_ref(pane),
            None => &[],
        }
    }

    pub fn panes_mut(&mut self) -> &mut [Pane] {
        match &mut self.pane {
            Some(pane) => std::slice::from_mut(pane),
            None => &mut [],
        }
    }

    pub fn pane_count(&self) -> usize {
        usize::from(self.pane.is_some())
    }

    pub fn is_empty(&self) -> bool {
        self.pane.is_none()
    }

    /// Set container price scale, applying it to ALL panels and retaining it for future panels.
    pub fn set_scale(&mut self, pct: Option<f32>) {
        self.scale = pct;
        for p in self.panes_mut() {
            apply_scale(&mut p.view, pct);
        }
    }

    /// Open a market manually.
    ///
    /// The terminal invariant is one market per `ChartEngine`; a multi-chart stack lives outside as
    /// a list of separate `ChartPanel` instances.
    pub fn open_manual(&mut self, core: CoreId, market: &str, epoch_ms: f64) {
        if self.find(core, market).is_some() {
            if let Some(p) = self.pane.as_mut() {
                p.source = PaneSource::Manual;
            }
            return;
        }
        let view = self.new_view(epoch_ms);
        self.pane = Some(Pane {
            core,
            market: market.to_string(),
            source: PaneSource::Manual,
            view,
            pinned: false,
        });
    }

    /// Apply an AddToChart detect to one chart by extending TTL or replacing this `ChartPanel` market.
    ///
    /// The external `AddChartStack`, not an internal tiled canvas, owns multiple charts.
    pub fn push_auto(
        &mut self,
        core: CoreId,
        market: &str,
        now_ms: f64,
        ttl_ms: f64,
        epoch_ms: f64,
    ) {
        match self.find(core, market) {
            Some(_) => {
                if let Some(p) = self.pane.as_mut() {
                    p.source = PaneSource::AddToChart {
                        born_ms: now_ms,
                        ttl_ms,
                    };
                }
            }
            None => {
                let view = self.new_view(epoch_ms);
                self.pane = Some(Pane {
                    core,
                    market: market.to_string(),
                    source: PaneSource::AddToChart {
                        born_ms: now_ms,
                        ttl_ms,
                    },
                    view,
                    pinned: false,
                });
            }
        }
    }

    /// Remove expired AddToChart panels and return their markets for owner refcount updates.
    pub fn prune_ttl(&mut self, now_ms: f64) -> Vec<(CoreId, String)> {
        // Through `deadline_ms`, like `next_ttl_deadline_ms`, rather than taking `ttl_ms` apart a
        // second time here: an infinite TTL then reads as "no deadline" in ONE place instead of
        // relying on every comparison against infinity happening to fall the right way.
        let remove = self
            .pane
            .as_ref()
            .is_some_and(|p| !p.pinned && p.source.deadline_ms().is_some_and(|d| now_ms >= d));
        if remove {
            if let Some(p) = self.pane.take() {
                return vec![(p.core, p.market)];
            }
        }
        Vec::new()
    }

    /// The smallest `key` over the panes a cap or a timer may act on.
    ///
    /// Pinned panes are excluded under requirement 2 — they answer neither question — so the two
    /// public accessors below differ only in what they ask each pane.
    fn min_unpinned(&self, key: impl Fn(&PaneSource) -> Option<f64>) -> Option<f64> {
        self.panes()
            .iter()
            .filter(|p| !p.pinned)
            .filter_map(|p| key(&p.source))
            .min_by(|a, b| a.total_cmp(b))
    }

    /// When the earliest pane closes itself, or `None` when none of them will.
    ///
    /// A pane held forever is filtered out by `deadline_ms`, which is what keeps the caller from
    /// arming a timer for `u64::MAX` milliseconds and parking a task for the life of the process.
    pub fn next_ttl_deadline_ms(&self) -> Option<f64> {
        self.min_unpinned(PaneSource::deadline_ms)
    }

    /// The stalest auto-added pane's last detect, or `None` when no pane may give way to a cap.
    ///
    /// The eviction counterpart of [`Self::next_ttl_deadline_ms`]: same pinned exclusion, but it
    /// answers for a pane held forever too, because such a pane still occupies a slot.
    pub fn stalest_detect_ms(&self) -> Option<f64> {
        self.min_unpinned(PaneSource::last_detect_ms)
    }

    /// Return whether panel `idx` can be pinned: an AddToChart pane, whether its TTL is finite or
    /// not, but never a Manual one or Main.
    pub fn is_pinnable(&self, idx: usize) -> bool {
        self.pane(idx)
            .is_some_and(|p| matches!(p.source, PaneSource::AddToChart { .. }))
    }

    pub fn is_pinned(&self, idx: usize) -> bool {
        self.pane(idx).is_some_and(|p| p.pinned)
    }

    /// Toggle pinning for panel `idx`, returning its new state or `None` for an invalid index.
    pub fn toggle_pin(&mut self, idx: usize) -> Option<bool> {
        let p = self.pane_mut(idx)?;
        p.pinned = !p.pinned;
        Some(p.pinned)
    }

    /// Remove a panel from the UI close button and return its `(core, market)` for deciding whether
    /// to release the order-book subscription. Returns `None` for an invalid index.
    pub fn remove_pane(&mut self, idx: usize) -> Option<(CoreId, String)> {
        if idx != 0 {
            return None;
        }
        let p = self.pane.take()?;
        Some((p.core, p.market))
    }

    /// Return whether another panel still uses `(core, market)` so its order book is not released.
    pub fn uses_market(&self, core: CoreId, market: &str) -> bool {
        self.panes()
            .iter()
            .any(|p| p.core == core && p.market == market)
    }

    /// Close ALL panels from a detached window's "close all charts" button and return their
    /// `(core, market)` pairs for releasing order-book subscriptions.
    pub fn clear_panes(&mut self) -> Vec<(CoreId, String)> {
        self.pane
            .take()
            .map(|p| vec![(p.core, p.market)])
            .unwrap_or_default()
    }

    /// Return visible panel layout as `(panel index, rectangle)` in physical `content` pixels.
    pub fn layout(&self, content: Rect) -> Vec<(usize, Rect)> {
        if self.pane.is_none() {
            return Vec::new();
        }
        vec![(0, content)]
    }
}

#[cfg(test)]
mod tests;
