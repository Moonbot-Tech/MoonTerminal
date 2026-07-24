//! News marks in the own-pass drawing layer: the tag-coloured gem per news item, drawn on the plot's
//! bottom edge. Geometry is appended to the SAME userdata layers as orders and figures (see
//! `data_state::sync_orders_from_session`), so marks cost no extra pass and no extra buffer.
//!
//! The panel owns the SET and its colours (it reads the store, filters by coin, resolves tag colours
//! from the UI palette, and hit-tests the same list under the cursor — see `panels::chart::news`);
//! this module owns only its GPU shape. Marks carry ABSOLUTE Unix ms and are rebased on the pane
//! epoch at build time, because a pane's epoch changes when its market changes while the mark list
//! does not.
//!
//! Hover DOES reach this layer (the mark under the cursor grows upward from the axis), which means a
//! hover change rebuilds userdata through `sync_orders_*` — the same cost the order-line hover
//! already pays. The panel keeps that to a minimum: it publishes only when the nearest mark actually
//! changes, and only past the Delphi movement threshold.

use std::rc::Rc;

use moon_chart::news_marks::NewsMark;

use super::ChartDataState;

impl ChartDataState {
    /// Sets this panel's news marks and the mark under the cursor, invalidating panel userdata on a
    /// real change.
    pub(super) fn set_news_marks(
        &mut self,
        marks: Rc<Vec<NewsMark>>,
        hovered: Option<usize>,
    ) -> bool {
        // Compare by pointer first: the panel rebuilds the list only on a news revision change, so
        // the common case settles without touching the contents.
        let same_marks = Rc::ptr_eq(&self.news_marks, &marks) || self.news_marks == marks;
        if same_marks && self.news_hovered == hovered {
            return false;
        }
        self.news_marks = marks;
        self.news_hovered = hovered;
        let mut st = self.render.borrow_mut();
        for pr in &mut st.panes {
            pr.last_news_sig = u64::MAX;
            pr.gpu_prepare_dirty = true;
        }
        st.needs_present = true;
        true
    }

    /// Userdata gate for the news layer. Never returns `u64::MAX`, which panes reserve as the
    /// forced-dirty sentinel.
    ///
    /// A mark or hover change already stamps every pane dirty in [`Self::set_news_marks`], so this
    /// signature exists for the inputs that reach the instances WITHOUT going through that setter:
    /// the device pixel ratio (sizes are baked in physical pixels) and the theme's neutral colour
    /// (baked into every uncoloured gem). Without them a DPI change or a colour edit would leave the
    /// gems stale until the next news item arrived.
    pub(super) fn news_sig(&self) -> u64 {
        let c = self.theme.label_neutral;
        let color = ((c[0] as u64) << 16) | ((c[1] as u64) << 8) | c[2] as u64;
        let sig = (self.last_ppp.to_bits() as u64)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(color.wrapping_mul(0xBF58_476D_1CE4_E5B9));
        if sig == u64::MAX { 0 } else { sig }
    }

    /// Appends the news-mark gems to the userdata marker buffer.
    pub(super) fn append_news_geometry(
        &self,
        epoch_ms: f64,
        markers: &mut Vec<moon_chart::layers::MarkerInstance>,
    ) {
        if self.news_marks.is_empty() {
            return;
        }
        // Book-only (broom) mode leaves the plot 1 px wide, which is a 1 px window for the shader's
        // clip rather than a real plot: every gem would paint as an overlapping sliver over the
        // full-width order book. There is no time axis to mark in that mode, so skip the layer —
        // the panel's hit-test skips it for the same reason.
        if self.orderbook_only {
            return;
        }
        moon_chart::build_news_geometry(
            &self.news_marks,
            epoch_ms,
            self.news_hovered,
            // Untagged (or uncoloured) news falls back to the theme's neutral label colour — chart
            // chrome, never a literal.
            self.theme.label_neutral,
            self.last_ppp,
            markers,
        );
    }
}
