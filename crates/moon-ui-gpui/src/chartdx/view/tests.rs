use super::*;

/// `view.rs:cross_cull_margin_physical_px` — dropping a theme-scale multiplier or its floor
/// would cull a visible scaled trade glyph at the chart edge.
#[test]
fn cross_cull_margin_matches_shader_margin() {
    let mut view = ChartView::new(0.0);
    view.marker_half_px = 3.5;
    assert_eq!(cross_cull_margin_physical_px(&view, 1.0, 1.0), 8.0);
    assert_eq!(cross_cull_margin_physical_px(&view, 3.0, 1.0), 11.5);
    assert_eq!(cross_cull_margin_physical_px(&view, 3.5, 1.5), 19.375);
    assert_eq!(cross_cull_margin_physical_px(&view, 3.5, 0.1), 8.0);
    assert_eq!(cross_cull_margin_physical_px(&view, 20.0, 0.1), 15.0);
}
