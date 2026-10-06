//! Weak chart-data handles and order-render observations.

use super::*;

#[derive(Clone)]
pub struct ChartDataHandle {
    pub(super) inner: Weak<RefCell<ChartDataState>>,
}

#[derive(Clone, Copy, Debug)]
pub struct OrderRenderProbe {
    pub order_lines_rev: u64,
    pub order_lines_sync_ms: f64,
    pub gpu_rev: u64,
    pub gpu_ms: f64,
    pub present_rev: u64,
    pub present_ms: f64,
}

impl PartialEq for ChartDataHandle {
    fn eq(&self, other: &Self) -> bool {
        self.inner.ptr_eq(&other.inner)
    }
}

impl ChartDataHandle {
    pub fn is_alive(&self) -> bool {
        self.inner.strong_count() > 0
    }

    /// Whether this engine draws a closed interval rather than the live market.
    ///
    /// Trade windows are historical; they do not count toward "all live charts closed" and must
    /// not stamp the idle auto-return clock.
    pub fn is_historical(&self) -> bool {
        self.inner
            .upgrade()
            .is_some_and(|inner| inner.borrow().historical)
    }

    pub fn sync_orders_if_visible(&self, session: &SessionManager, force: bool) -> bool {
        let Some(inner) = self.inner.upgrade() else {
            return false;
        };
        inner.borrow_mut().sync_orders_if_visible(session, force)
    }

    pub fn set_firetest_text_labels(&self, count: usize) -> bool {
        let Some(inner) = self.inner.upgrade() else {
            return false;
        };
        let mut data = inner.borrow_mut();
        let render = data.render.clone();
        let changed = render.borrow_mut().set_firetest_text_labels(count);
        if changed {
            data.mark_view_dirty();
        }
        changed
    }

    pub fn set_firetest_force_present(&self, enabled: bool) -> bool {
        let Some(inner) = self.inner.upgrade() else {
            return false;
        };
        let render = inner.borrow().render.clone();
        render.borrow_mut().set_firetest_force_present(enabled)
    }

    /// Start or clear the arrival border flash on this chart for a measurement stage.
    ///
    /// The same state a real arrival sets, reached without one: a live detect is not something a
    /// run can schedule, so measuring the flash's cost by waiting for one measures the market's
    /// mood instead. `accent` is the palette token, exactly as `ChartPanel::set_arrival_pulse`
    /// passes it, so the measured flash is the one the user sees and not a stand-in.
    ///
    /// Returns whether the chart is still alive and took the stamp.
    pub fn set_firetest_arrival_flash(&self, at: Option<Instant>, accent: u32) -> bool {
        let Some(inner) = self.inner.upgrade() else {
            return false;
        };
        let render = inner.borrow().render.clone();
        // The measured flash is the pulsing one: a held stroke costs nothing after it settles.
        render
            .borrow_mut()
            .set_arrival_pulse(at, types::accent_rgb4(accent), false);
        true
    }

    pub fn order_render_probe(
        &self,
        core: CoreId,
        market: &str,
    ) -> Option<crate::chartdx::OrderRenderProbe> {
        let inner = self.inner.upgrade()?;
        let render = inner.borrow().render.clone();
        render
            .borrow()
            .panes
            .iter()
            .find(|pane| pane.core == Some(core) && pane.market == market)
            .map(|pane| OrderRenderProbe {
                order_lines_rev: pane.last_order_lines_rev,
                order_lines_sync_ms: pane.last_order_lines_sync_ms,
                gpu_rev: pane.last_order_gpu_rev,
                gpu_ms: pane.last_order_gpu_ms,
                present_rev: pane.last_order_present_rev,
                present_ms: pane.last_order_present_ms,
            })
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub fn camera_shift_hz(&self) -> Option<f32> {
        let inner = self.inner.upgrade()?;
        let render = inner.borrow().render.clone();
        Some(render.borrow_mut().camera_shift_hz())
    }
}
