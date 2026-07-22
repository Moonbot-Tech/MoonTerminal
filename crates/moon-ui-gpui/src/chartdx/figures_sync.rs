//! Chart figures in the own-pass drawing layer. `ChartDataState` accesses the backend's shared
//! figure store and panel interactions such as drawing previews, hover, and selection. Geometry is
//! appended to userdata layers after `build_order_geometry`; see
//! `data_state::sync_orders_from_session`.

use std::cell::RefCell;
use std::rc::Rc;

use moon_core::figures::{Figure, FigureStore};
use moon_core::session::CoreId;

use super::ChartDataState;

/// Interactive figure state for one chart panel. Figure data lives in the shared store; this value
/// contains only panel-specific presentation state such as the draft preview, hover, and selection.
/// Any change increments `rev` and triggers a userdata rebuild.
#[derive(Default, Clone, PartialEq)]
pub(crate) struct FigureVisual {
    /// Drawing mode. Figures are visible and interactive only while the pencil is enabled.
    pub draw_mode: bool,
    /// Core and market identifying the chart whose interactions are represented. A `ChartEngine`
    /// represents one current core and market, and this key restricts previews and highlights to it.
    pub key: Option<(CoreId, String)>,
    /// Figure currently being drawn, previewed as a dashed shape following the cursor.
    pub draft: Option<Figure>,
    /// Hovered figure to highlight.
    pub hovered: Option<u64>,
    /// Selected figure to highlight and display control nodes for.
    pub selected: Option<u64>,
}

impl ChartDataState {
    /// Attaches the backend's shared figure store when the panel is created.
    pub(super) fn set_figures_store(&mut self, store: Rc<RefCell<FigureStore>>) {
        self.figures = Some(store);
        self.figure_visual_rev = self.figure_visual_rev.wrapping_add(1);
    }

    /// Sets figure preview, hover, and selection state, invalidating panel userdata on change.
    pub(super) fn set_figure_visual(&mut self, visual: FigureVisual) -> bool {
        if self.figure_visual == visual {
            return false;
        }
        self.figure_visual = visual;
        self.figure_visual_rev = self.figure_visual_rev.wrapping_add(1);
        let mut st = self.render.borrow_mut();
        for pr in &mut st.panes {
            pr.last_figures_sig = u64::MAX;
            pr.gpu_prepare_dirty = true;
        }
        st.needs_present = true;
        true
    }

    /// Combines store and interaction revisions to gate userdata rebuilds. Never returns
    /// `u64::MAX`, which panels reserve as the forced-dirty sentinel.
    pub(super) fn figures_sig(&self) -> u64 {
        let store_rev = self.figures.as_ref().map(|f| f.borrow().rev()).unwrap_or(0);
        let sig = store_rev
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(self.figure_visual_rev);
        if sig == u64::MAX { 0 } else { sig }
    }

    /// Appends figure geometry for a core and market to userdata buffers. Figures are visible only
    /// in pencil drawing mode; otherwise this layer is not built.
    pub(super) fn append_figure_geometry(
        &self,
        core: CoreId,
        market: &str,
        epoch_ms: f64,
        hlines: &mut Vec<moon_chart::layers::LineInstance>,
        segs: &mut Vec<moon_chart::layers::SegInstance>,
        markers: &mut Vec<moon_chart::layers::MarkerInstance>,
    ) {
        if !self.figure_visual.draw_mode {
            return;
        }
        let Some(store) = self.figures.as_ref() else {
            return;
        };
        let store = store.borrow();
        // Treat local and server-provided figures as one fully interactive set.
        let figures = store.figures(core, market);
        // Apply preview, hover, and selection only to the matching panel key. In a stack of
        // different markets, the draft appears only in the panel containing the cursor.
        let v = &self.figure_visual;
        let mine = v
            .key
            .as_ref()
            .is_some_and(|(c, m)| *c == core && m == market);
        let draft = if mine { v.draft.as_ref() } else { None };
        if figures.is_empty() && draft.is_none() {
            return;
        }
        moon_chart::build_figure_geometry(
            figures,
            draft,
            if mine { v.hovered } else { None },
            if mine { v.selected } else { None },
            epoch_ms,
            hlines,
            segs,
            markers,
        );
    }
}
