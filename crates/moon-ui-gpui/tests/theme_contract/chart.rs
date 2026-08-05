//! Static contracts for the platform-specific chart shaders.

use super::support::*;

/// A plausible regression is changing any grid fragment back to its interpolated vertex `px`
/// varying, which makes horizontal lines stop at the quad's diagonal seam. Removing the explicit
/// half-pixel snap is another regression: a division exactly between pixel centers can disappear
/// under the strict half-pixel test. Both edits must fail this assertion before users see broken
/// or missing price-grid lines.
#[test]
fn grid_fragments_use_raster_positions_with_one_pixel_snapping() {
    const SHADERS: &[(&str, &str, &str)] = &[
        (
            "chartdx/shaders/grid.hlsl",
            "float2 fragment_px = i.pos.xy;",
            "i.px",
        ),
        (
            "chartdx/shaders/native_grid.wgsl",
            "let fragment_px = in.pos.xy;",
            "in.px",
        ),
        (
            "chartdx/shaders/chart_native.metal",
            "float2 fragment_px = in.position.xy;",
            "in.px",
        ),
    ];

    for (path, raster_position, interpolated_position) in SHADERS {
        let source = read_src(path);
        let grid = source
            .split_once("struct GridOut")
            .unwrap_or_else(|| panic!("{path}: expected GridOut"))
            .1
            .split_once("struct CursorOut")
            .map_or_else(|| source.as_str(), |(grid, _)| grid);

        assert!(
            grid.contains(raster_position),
            "{path}: grid fragments must derive coordinates from rasterizer-generated position"
        );
        assert!(
            !grid.contains(interpolated_position),
            "{path}: interpolated grid coordinates recreate the diagonal quad seam"
        );
        assert!(
            grid.contains("floor(line_x) + 0.5") && grid.contains("floor(line_y) + 0.5"),
            "{path}: grid divisions must snap to one pixel center before the half-pixel test"
        );
        assert!(
            grid.contains("abs(fragment_px.x - snapped_x) < GRID_LINE_HALF_PX")
                && grid.contains("abs(fragment_px.y - snapped_y) < GRID_LINE_HALF_PX"),
            "{path}: grid hit tests must use the snapped one-pixel centers"
        );
    }
}

/// The zone layer — order zones and figure fills — must be drawn AFTER the grid and BEFORE the
/// candles in every backend's base pass.
///
/// The grid fragment paints the plot's background across its whole rect (`alpha = g_bg_alpha`, 1
/// unless a photo backdrop supplies the background instead — and that mode is off by default), so
/// it erases whatever was drawn between the background layer and itself. Zones sat there, and were
/// invisible on screen in the shipped configuration: nothing else catches this — the geometry is
/// built, uploaded and drawn, and every unit test, every review and a green build all pass while
/// the pixels are overwritten a microsecond later. Moving the draw back above the grid must fail
/// here.
#[test]
fn the_zone_layer_is_drawn_after_the_grid_that_would_paint_over_it() {
    const PASSES: &[(&str, &str, &str, &str, &str)] = &[
        (
            "chartdx/backend.rs",
            "pub fn render_base_d3d(",
            "self.grid.render(",
            "self.userdata.render_zones(",
            "self.candles.render(",
        ),
        (
            "chartdx/wgpu_backend/render.rs",
            "fn draw_base_layers(",
            "&pipelines.grid",
            "&pipelines.zone",
            "&pipelines.candles",
        ),
        (
            "chartdx/metal_backend.rs",
            "fn draw_base_layers(",
            "&pipelines.grid",
            "&pipelines.zone",
            "&pipelines.candles",
        ),
    ];

    for (path, signature, grid_draw, zone_draw, candle_draw) in PASSES {
        let source = read_src(path);
        // Comments are stripped first: these very call sites are documented with the reason for
        // the order, and a marker quoted in prose would move the boundary this test measures.
        let body = code_only(braced_body(&source, signature));
        let grid_at = body
            .find(grid_draw)
            .unwrap_or_else(|| panic!("{path}: expected `{grid_draw}` in the base pass"));
        assert!(
            body.contains(zone_draw),
            "{path}: expected `{zone_draw}` in the base pass"
        );
        // Tested as "NO zone draw before the grid", not "the first one is after it": a second draw
        // added above the grid is exactly the regression, and comparing first hits would miss it.
        assert!(
            !body[..grid_at].contains(zone_draw),
            "{path}: the grid paints the plot background over its whole rect, so a zone drawn \
             before it never reaches the screen"
        );
        // The other end of the sandwich: a band tints the plot, it does not bury the price action,
        // and "after the grid" alone would be satisfied by drawing it over the candles.
        let candles_at = body
            .find(candle_draw)
            .unwrap_or_else(|| panic!("{path}: expected `{candle_draw}` in the base pass"));
        assert!(
            !body[candles_at..].contains(zone_draw),
            "{path}: a zone drawn after the candles covers the price action it should sit under"
        );
        // Both halves above are measured against markers, so the order of the markers themselves
        // has to be pinned too: candles → zone → grid satisfies each half on its own.
        assert!(
            grid_at < candles_at,
            "{path}: the grid must be drawn before the candles for the sandwich to mean anything"
        );
    }
}
