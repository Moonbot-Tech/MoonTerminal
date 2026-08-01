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
