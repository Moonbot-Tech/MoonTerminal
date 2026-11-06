//! Real GPUI text preparation must settle the current readout without owing another frame.

use std::{cell::RefCell, rc::Rc};

use gpui::{
    Context, GpuCanvasDrawContext, GpuCanvasDriver, GpuCanvasHandle, GpuCanvasTextContext,
    GpuFrameDecision, GpuFrameInfo, IntoElement, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, gpu_canvas, px,
};
use moon_core::config::ChartTheme;

use crate::chartdx::{ChartEngine, PaneRender, RenderState, hits::CursorState};

/// Keep the production frame/text path while excluding backend-specific resource allocation.
struct TextCanvas(Rc<RefCell<RenderState>>);

impl GpuCanvasDriver for TextCanvas {
    /// Use the real scheduling decision after the current frame's text was measured.
    fn frame(&mut self, info: GpuFrameInfo) -> GpuFrameDecision {
        self.0.borrow_mut().frame(info)
    }

    /// Shape through the window's real text context, including cursor plate dimensions.
    fn prepare_text(&mut self, ctx: &mut GpuCanvasTextContext<'_>) -> anyhow::Result<()> {
        self.0.borrow_mut().prepare_text(ctx).map(|_| ())
    }

    /// Plate geometry is checked on the CPU; hardware pixels have separate recovery coverage.
    fn draw(&mut self, _: &mut GpuCanvasDrawContext<'_>) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Host the retained canvas in an ordinary GPUI test window.
struct TextView(GpuCanvasHandle);

impl Render for TextView {
    /// Give the chart the same geometry used by its fixed test pane.
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpu_canvas(self.0.clone()).w(px(320.)).h(px(200.))
    }
}

/// Restoring prepare_text's needs_present assignment causes an unnecessary second present.
#[test]
fn measured_readout_is_current_without_an_extra_frame() {
    let _locale = crate::test_locale::force("en");
    let chart = ChartEngine::new(0., ChartTheme::default());
    {
        let mut state = chart.state.borrow_mut();
        let mut pane = PaneRender::new();
        pane.active = true;
        pane.pane_bounds = [0., 0., 320., 200.];
        pane.view.bounds = [50., 0., 200., 200.];
        pane.view.price_to_px = 10.;
        pane.view.view_price0 = 100.;
        pane.view.time_to_px = 1.;
        pane.price_axis_pos = crate::persistence::chart_persist::PriceAxisPos::Right;
        state.panes.push(pane);
        state.cursor = Some(CursorState {
            pane: 0,
            local: [10., 100.],
        });
    }
    let handle = GpuCanvasHandle::new(TextCanvas(chart.state.clone()));
    let mut app = TestAppContext::single();
    let window = app.add_window(move |_, _| TextView(handle));
    let mut visual = VisualTestContext::from_window(window.into(), &app);
    visual.simulate_frame();
    let mut state = chart.state.borrow_mut();
    let pane = &state.panes[0];
    assert!(
        pane.readout_price_width > 0.,
        "the native callback must shape the cursor label"
    );
    assert_eq!(pane.readout_rects.len(), 1);
    assert!(
        pane.readout_rects[0].dst[2] >= pane.readout_price_width + 10.,
        "the plate must contain the label and its insets"
    );
    assert_eq!(
        pane.readout_rects[0].dst[3],
        pane.readout_price_line_h + 5.,
        "the current frame already has the measured plate height"
    );
    assert!(matches!(
        state.frame(super::frame_info()),
        GpuFrameDecision::Skip
    ));
}
