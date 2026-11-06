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
    /// Park the real camera before the diagnostic snapshots; no synthetic replacement renderer.
    #[cfg(windows)]
    pub(crate) fn firetest_park_recovery_camera(&self) -> Result<(), String> {
        let inner = self.inner.upgrade().ok_or("recovery chart was dropped")?;
        let mut data = inner.borrow_mut();
        let now = moon_core::util::now_unix_ms_i64() as f64;
        for pane in data.container.borrow_mut().panes_mut() {
            pane.view.park_at_live_edge(now);
        }
        data.mark_view_dirty();
        Ok(())
    }

    /// Request exactly one readback from the next completed native own-pass draw.
    #[cfg(windows)]
    pub(crate) fn firetest_request_recovery_image(&self) -> Result<(), String> {
        let inner = self.inner.upgrade().ok_or("recovery chart was dropped")?;
        let render = inner.borrow().render.clone();
        let mut render = render.borrow_mut();
        render.firetest_capture = Some(super::recovery_probe::Capture::Requested);
        render.needs_present = true;
        Ok(())
    }

    /// Poll a completed diagnostic capture without copying its pixel payload.
    #[cfg(windows)]
    pub(crate) fn firetest_recovery_image(
        &self,
    ) -> Result<Option<std::sync::Arc<super::recovery_probe::RecoveryImage>>, String> {
        let inner = self.inner.upgrade().ok_or("recovery chart was dropped")?;
        let render = inner.borrow().render.clone();
        let render = render.borrow();
        match &render.firetest_capture {
            Some(super::recovery_probe::Capture::Ready(image)) => image.clone().map(Some),
            _ => Ok(None),
        }
    }

    /// Report prepared candle/side-volume/horizontal-volume rows beside the image qualification.
    /// These are coverage metadata, not evidence of a successful GPU upload.
    #[cfg(windows)]
    pub(crate) fn firetest_recovery_layer_counts(&self) -> Option<(usize, usize, usize)> {
        let inner = self.inner.upgrade()?;
        let render = inner.borrow().render.clone();
        let render = render.borrow();
        let pane = render.panes.iter().find(|pane| pane.active)?;
        Some((
            pane.candle_rows.len(),
            pane.side_samples.len(),
            pane.hvol_rows.len(),
        ))
    }

    /// Count visible trades, candles and occupied sixteenths of the plot width at a stage boundary.
    /// A full CPU ring compressed into a few pixels is not the requested dense chart workload.
    pub(crate) fn firetest_visible_history(&self) -> Option<(usize, usize, u32)> {
        let inner = self.inner.upgrade()?;
        let render = inner.borrow().render.clone();
        let render = render.borrow();
        let pane = render.panes.iter().find(|pane| pane.active)?;
        let left = pane.view.view_time0;
        let width_ms = pane.view.bounds[2] / pane.view.time_to_px;
        if !width_ms.is_finite() || width_ms <= 0.0 {
            return None;
        }
        let right = left + width_ms;
        let mut columns = 0_u16;
        let ticks = pane
            .layers
            .tick_samples(f64::from(left), f64::from(right))
            .filter(|tick| tick.time_rel >= left && tick.time_rel < right)
            .inspect(|tick| {
                let column = ((tick.time_rel - left) / width_ms * 16.0) as u32;
                columns |= 1_u16 << column.min(15);
            })
            .count();
        let candles = pane
            .candle_rows
            .iter()
            .filter(|candle| candle.t_open_rel < right && candle.t_open_rel + candle.tf_rel > left)
            .count();
        Some((ticks, candles, columns.count_ones()))
    }

    /// Inspect the actual chart consumer after history preparation, without adding hot-path counters.
    pub(crate) fn firetest_history_probe(&self) -> Option<(usize, u64)> {
        let inner = self.inner.upgrade()?;
        let render = inner.borrow().render.clone();
        let render = render.borrow();
        let ticks = render
            .panes
            .iter()
            .filter(|pane| pane.active)
            .map(|pane| {
                pane.layers
                    .tick_samples(f64::NEG_INFINITY, f64::INFINITY)
                    .count()
            })
            .sum();
        Some((ticks, render.last_gpu_prepare_generation))
    }
    /// Whether the owning host currently includes this chart in its visible scene.
    /// Hidden tabs retain live handles but cannot contribute to a rendering measurement.
    pub(crate) fn is_scene_visible(&self) -> bool {
        self.inner
            .upgrade()
            .is_some_and(|inner| inner.borrow().scene_visible)
    }

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
