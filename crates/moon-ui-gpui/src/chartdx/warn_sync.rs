//! Core-warning badges in the own-pass drawing layer: one amber gem per warning episode, drawn on
//! the plot's bottom edge at the episode's start. Mirrors `news_sync` — geometry is appended to the
//! SAME userdata marker buffer as orders, figures, and news, so badges cost no extra pass and no
//! extra buffer, and they scroll with the live edge every frame (a GPUI overlay would lag).
//!
//! The panel owns the SET (it reads the backend's warning episodes for this chart's server and
//! filters by the visible span — see `panels::chart::warn`); this module owns only the GPU shape,
//! reusing the news gem (`SHAPE_GEM`) in a single amber wedge. Marks carry ABSOLUTE Unix ms, rebased
//! on the pane epoch at build time.

use std::rc::Rc;

use moon_chart::news_marks::NewsMark;

use super::ChartDataState;

impl ChartDataState {
    /// Sets this panel's warning badges and the one under the cursor, invalidating userdata on a real
    /// change. Mirror of [`Self::set_news_marks`].
    pub(super) fn set_warn_marks(
        &mut self,
        marks: Rc<Vec<NewsMark>>,
        hovered: Option<usize>,
    ) -> bool {
        let same_marks = Rc::ptr_eq(&self.warn_marks, &marks) || self.warn_marks == marks;
        if same_marks && self.warn_hovered == hovered {
            return false;
        }
        self.warn_marks = marks;
        self.warn_hovered = hovered;
        let mut st = self.render.borrow_mut();
        for pr in &mut st.panes {
            pr.last_warn_sig = u64::MAX;
            pr.gpu_prepare_dirty = true;
        }
        st.needs_present = true;
        true
    }

    /// Userdata gate for the warning layer. Badge colour is a baked amber constant (not the theme
    /// neutral news falls back to), so only the device pixel ratio — sizes are baked in physical
    /// pixels — needs folding here. A mark or hover change already stamps panes dirty in
    /// [`Self::set_warn_marks`]. Never returns `u64::MAX` (the forced-dirty sentinel).
    pub(super) fn warn_sig(&self) -> u64 {
        // Distinct multiplier from `news_sig` so the two layers cannot alias to the same value.
        let sig = (self.last_ppp.to_bits() as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93);
        if sig == u64::MAX { 0 } else { sig }
    }

    /// Appends the warning-badge gems to the userdata marker buffer.
    pub(super) fn append_warn_geometry(
        &self,
        epoch_ms: f64,
        markers: &mut Vec<moon_chart::layers::MarkerInstance>,
    ) {
        if self.warn_marks.is_empty() {
            return;
        }
        // Book-only mode leaves a 1 px plot with no time axis to mark; skip, like the news layer.
        if self.orderbook_only {
            return;
        }
        moon_chart::build_news_geometry(
            &self.warn_marks,
            epoch_ms,
            self.warn_hovered,
            // Every warning gem carries its own amber colour, so this neutral fallback is unused;
            // passed for signature parity with the news layer.
            self.theme.label_neutral,
            self.last_ppp,
            markers,
        );
    }
}
