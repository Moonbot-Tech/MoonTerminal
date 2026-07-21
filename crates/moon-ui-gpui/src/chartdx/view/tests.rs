use super::*;

#[test]
fn cross_cull_margin_matches_shader_margin() {
    let mut view = ChartView::new(0.0);
    view.marker_half_px = 3.5;
    assert_eq!(cross_cull_margin_physical_px(&view, 1.0), 8.0);
    assert_eq!(cross_cull_margin_physical_px(&view, 3.0), 11.5);
}
