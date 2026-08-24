//! Static contracts for the platform-specific chart shaders.

use super::support::*;

/// `trade_history_hover.rs:profit_text` — dropping the `record.quote` half of the guard would
/// print a profit amount with no resolved ticker, which is exactly the unlabeled figure this
/// design refuses to show. `moon-ui-gpui` has no `[lib]`, so this invariant can only be read as
/// text.
#[test]
fn trade_hover_profit_text_never_prints_an_amount_outside_a_resolved_quote() {
    let source = code_only(&read_src("panels/chart/trade_history_hover.rs"));
    let body = braced_body(&source, "fn profit_text(");
    assert!(
        body.contains("if let Some(profit) = record.profit")
            && body.contains("&& let Some(quote) = record.quote"),
        "profit_text must gate the profit amount on a resolved quote currency, or it prints an \
         amount with no ticker"
    );
}

/// All three shader copies of closed-trade-history geometry must bound the arrow branch
/// (`3.5 < shape < 5.5`) strictly BEFORE the open-ended warning-badge branch (`shape > 2.5`).
///
/// Nothing in the repository compiles a shader, so a tweak landing in one backend and forgotten
/// in the others is invisible until someone runs that platform — and this workspace only runs
/// the DX11 (HLSL) backend daily. A later shape id would silently render as the warning badge on
/// whichever backend drifted.
#[test]
fn every_backend_bounds_the_arrow_branch_before_the_open_ended_warning_branch() {
    const BACKENDS: &[(&str, &str, &str)] = &[
        (
            "chartdx/shaders/order_lines.hlsl",
            "if (i.shape > 3.5 && i.shape < 5.5) {",
            "if (i.shape > 2.5) {",
        ),
        (
            "chartdx/shaders/native_marker.wgsl",
            "if in.shape > 3.5 && in.shape < 5.5 {",
            "if in.shape > 2.5 {",
        ),
        (
            "chartdx/shaders/chart_native.metal",
            "if (in.shape > 3.5 && in.shape < 5.5) {",
            "if (in.shape > 2.5) {",
        ),
    ];
    for (path, arrow_branch, warning_branch) in BACKENDS {
        let source = code_only(&read_src(path));
        let arrow_at = source
            .find(arrow_branch)
            .unwrap_or_else(|| panic!("{path}: missing the bounded arrow branch"));
        let warning_at = source
            .find(warning_branch)
            .unwrap_or_else(|| panic!("{path}: missing the open-ended warning branch"));
        assert!(
            arrow_at < warning_at,
            "{path}: the arrow branch must be tested before the open-ended warning branch"
        );
    }
}

/// `chartdx` price and volume shader contracts — removing one backend uniform, changing the
/// `VolumeStyle` layout, moving the volume draw after the 18-vertex candle draw, or restoring a
/// liquidation volume bar makes one platform silently render different chart pixels.
#[test]
fn chart_appearance_contracts_stay_identical_across_shader_backends() {
    const PRICE_BACKENDS: &[(&str, &[&str])] = &[
        (
            "chartdx/shaders/crosses.hlsl",
            &[
                "cbuffer PriceStyle",
                "return ps_last",
                "return ps_mark",
                "max(ps_m.x, 0.25)",
            ],
        ),
        (
            "chartdx/shaders/native_price.wgsl",
            &[
                "struct PriceStyle",
                "return ps.last",
                "return ps.mark",
                "max(ps.m.x, 0.25)",
            ],
        ),
        (
            "chartdx/shaders/chart_native.metal",
            &[
                "struct PriceStyle",
                "return ps.last",
                "return ps.mark",
                "max(ps.m.x, 0.25)",
            ],
        ),
    ];
    for (path, required) in PRICE_BACKENDS {
        let source = code_only(&read_src(path));
        for token in *required {
            assert!(
                source.contains(token),
                "{path}: price lines must use `{token}` from PriceStyle"
            );
        }
        let vertex = braced_body(&source, "price_line_vertex");
        for retired in ["0.82", "0.60", "0.36", "0.42", "0.72", "0.78", "* 0.85"] {
            assert!(
                !vertex.contains(retired),
                "{path}: retired hard-coded price appearance `{retired}` bypasses PriceStyle"
            );
        }
    }

    const VOLUME_BACKENDS: &[(&str, &[&str], &[&str])] = &[
        (
            "chartdx/shaders/candles.hlsl",
            &[
                "float4 vs_up;",
                "float4 vs_down;",
                "float4 vs_scale;",
                "float4 vs_m;",
                "float4 vs_m2;",
            ],
            &[
                "vs_m.x < 0.5",
                "vs_m.x >= 1.5",
                "sqrt(norm)",
                "cv_bounds.w * vs_m.y",
                "cv_bounds.y + cv_bounds.w - 1.0",
                "clamp(pxh.x, cv_bounds.x, cv_bounds.x + cv_bounds.z)",
                "clamp(px.x, cv_bounds.x, cv_bounds.x + cv_bounds.z)",
            ],
        ),
        (
            "chartdx/shaders/native_candles.wgsl",
            &[
                "up: vec4<f32>,",
                "down: vec4<f32>,",
                "scale: vec4<f32>,",
                "m: vec4<f32>,",
                "m2: vec4<f32>,",
            ],
            &[
                "vs.m.x < 0.5",
                "vs.m.x < 1.5",
                "sqrt(norm)",
                "cv.bounds.w * vs.m.y",
                "cv.bounds.y + cv.bounds.w - 1.0",
                "clamp(pxh.x, cv.bounds.x, cv.bounds.x + cv.bounds.z)",
                "clamp(px.x, cv.bounds.x, cv.bounds.x + cv.bounds.z)",
            ],
        ),
        (
            "chartdx/shaders/chart_native.metal",
            &[
                "float4 up;",
                "float4 down;",
                "float4 scale;",
                "float4 m;",
                "float4 m2;",
            ],
            &[
                "vs.m.x < 0.5",
                "vs.m.x < 1.5",
                "sqrt(norm)",
                "cv.bounds.w * vs.m.y",
                "cv.bounds.y + cv.bounds.w - 1.0",
                "clamp(pxh.x, cv.bounds.x, cv.bounds.x + cv.bounds.z)",
                "clamp(px.x, cv.bounds.x, cv.bounds.x + cv.bounds.z)",
            ],
        ),
    ];
    for (path, layout, geometry) in VOLUME_BACKENDS {
        let source = code_only(&read_src(path));
        let volume_style = braced_body(&source, "VolumeStyle");
        let mut last = 0;
        for member in *layout {
            let at = volume_style
                .find(member)
                .unwrap_or_else(|| panic!("{path}: VolumeStyle is missing `{member}`"));
            assert!(
                at >= last,
                "{path}: VolumeStyle member `{member}` is out of wire-layout order"
            );
            last = at;
        }
        for token in *geometry {
            assert!(
                source.contains(token),
                "{path}: volume geometry must retain `{token}`"
            );
        }
    }

    const DRAWS: &[(&str, &str, &str)] = &[
        (
            "chartdx/candles.rs",
            "context.DrawInstanced(6, bars, 0, 0);",
            "context.DrawInstanced(18, self.count, 0, 0);",
        ),
        (
            "chartdx/wgpu_backend/render.rs",
            "&pipelines.volume_bars",
            "&pipelines.candles",
        ),
        (
            "chartdx/metal_backend.rs",
            "&pipelines.volume_bars",
            "&pipelines.candles",
        ),
    ];
    for (path, volume_draw, candle_draw) in DRAWS {
        let source = code_only(&read_src(path));
        let volume_at = source
            .find(volume_draw)
            .unwrap_or_else(|| panic!("{path}: missing volume draw"));
        let candle_at = source
            .find(candle_draw)
            .unwrap_or_else(|| panic!("{path}: missing 18-vertex candle draw"));
        assert!(
            volume_at < candle_at,
            "{path}: volume band must draw before the 18-vertex candles"
        );
    }

    const TRADE_VOLUME_BACKENDS: &[(&str, &str)] = &[
        ("chartdx/shaders/crosses.hlsl", "c.side >= 2u"),
        ("chartdx/shaders/native_crosses.wgsl", "c.side >= 2u"),
        ("chartdx/shaders/chart_native.metal", "c.side >= 2"),
    ];
    for (path, liquidation_cull) in TRADE_VOLUME_BACKENDS {
        let source = code_only(&read_src(path));
        let volume = braced_body(&source, "volume_vertex");
        assert!(
            volume.contains(liquidation_cull),
            "{path}: per-trade volume must cull liquidations"
        );
    }
}

/// `chartdx` `vol_band_h` — wrapping the pane-height * fraction product in
/// `min(..., cap_px)` again (or in only one backend) makes the height slider inert
/// on any pane taller than a few hundred pixels, and leaves Windows and macOS
/// drawing different band heights. Nothing in this repo compiles a shader, so only
/// this text contract can catch it.
///
/// `braced_body` returns comments too, and the new comments name the removed cap, so
/// a naive substring on "cap" or `vs_m2.x` would pass with the code deleted. Comments
/// are stripped first; the return is matched as a statement a comment cannot contain.
#[test]
fn every_backend_sizes_the_candle_volume_band_from_the_pane_fraction_only() {
    const BACKENDS: &[(&str, &str, &str)] = &[
        (
            "chartdx/shaders/candles.hlsl",
            "float vol_band_h()",
            "return cv_bounds.w * vs_m.y;",
        ),
        (
            "chartdx/shaders/native_candles.wgsl",
            "fn vol_band_h()",
            "return cv.bounds.w * vs.m.y;",
        ),
        (
            "chartdx/shaders/chart_native.metal",
            "float vol_band_h(",
            "return cv.bounds.w * vs.m.y;",
        ),
    ];
    for (path, signature, expected_return) in BACKENDS {
        let body = code_only(braced_body(&read_src(path), signature));
        assert!(
            body.contains(expected_return),
            "{path}: vol_band_h must return pane height times the height fraction, with no pixel ceiling"
        );
        assert!(
            !body.contains("min("),
            "{path}: vol_band_h must not reintroduce a fixed pixel ceiling"
        );
    }
}

/// Moving the visibility guard below `aligned_ticks_ms` makes every hidden time axis resolve,
/// sort, and discard a complete selected-zone DST grid on each chart text preparation.
#[test]
fn a_hidden_time_axis_skips_tick_generation() {
    let source = code_only(&read_src("chartdx/text/prepare.rs"));
    let prepare = braced_body(&source, "pub(crate) fn prepare_text(");
    let guard = prepare
        .find("if !time_axis_visible {\n                continue;\n            }")
        .expect("hidden time axes must leave the pane before tick generation");
    let generation = prepare
        .find("crate::chartdx::axes::aligned_ticks_ms(")
        .expect("visible time axes must generate selected-zone ticks");

    assert!(
        guard < generation,
        "the visibility guard must run before selected-zone tick generation"
    );
}

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

/// Both figure-settings frames float over the chart, so BOTH must swallow the input the chart would
/// otherwise act on — a wheel zooms, a left-press can move an order, a right-press opens the chart's
/// own menu. The guards therefore live in the one frame builder every host goes through.
///
/// The regression this exists for actually happened: the toolbar frame was written without them on
/// the belief that the tab strip does not overlap the plot, and a click on a colour swatch reached
/// the chart underneath. Adding a second frame that builds its own container, or dropping a guard
/// from `shell`, must fail here.
#[test]
fn every_figure_settings_frame_swallows_the_charts_input() {
    let src = read_src("figstyle/mod.rs");
    let shell = code_only(braced_body(
        &src,
        "fn shell<V: 'static>(id: &'static str, cx: &mut Context<V>) -> Stateful<Div>",
    ));
    for guard in [
        "on_mouse_down(MouseButton::Left",
        "on_mouse_down(MouseButton::Right",
        "on_mouse_up(MouseButton::Left",
        "on_mouse_up(MouseButton::Right",
        "on_mouse_move(",
        "on_scroll_wheel(",
    ] {
        assert!(
            shell.contains(guard),
            "the settings frame no longer stops {guard} before the chart sees it"
        );
    }
    assert_eq!(
        shell.matches("stop_propagation").count(),
        6,
        "every guard in the settings frame must stop propagation"
    );
    // One frame builder, so no host can grow a container that forgets the guards above.
    let code = code_only(&src);
    assert_eq!(
        code.matches(".absolute()").count(),
        1,
        "a figure-settings frame is built outside `shell`"
    );
}

/// All three backends draw the same five line styles.
///
/// A pattern lives in a shader, so a style added on one platform and forgotten on another is
/// invisible until someone runs that platform — and the DX11 build is the only one anyone here runs
/// daily. The five branches are `TPenStyle`: solid, dash, dot, dash-dot, dash-dot-dot.
///
/// The regression this exists for actually shipped: the shaders had three patterns for five kinds,
/// so a figure drawn in Moonbot as a dash came back dash-dot-dot.
#[test]
fn every_backend_draws_all_five_line_styles() {
    const BACKENDS: &[&str] = &[
        "chartdx/shaders/order_lines.hlsl",
        "chartdx/shaders/chart_native.metal",
        "chartdx/shaders/native_hline.wgsl",
        "chartdx/shaders/native_seg.wgsl",
    ];
    for rel in BACKENDS {
        let src = code_only(&read_src(rel));
        assert!(
            src.contains("pattern_on"),
            "{rel} does not go through the shared pattern function"
        );
        // One branch per style: four thresholds plus the fall-through.
        for cut in ["0.5", "1.5", "2.5", "3.5"] {
            assert!(
                src.contains(&format!("style < {cut}")),
                "{rel} is missing the style < {cut} branch — a line kind draws as another"
            );
        }
        // And the caller passes distance along the line, not a boolean test of its own.
        assert!(
            !src.contains("style >= 0.5"),
            "{rel} still tests the style inline instead of asking `pattern_on`"
        );
    }
}

/// All three backends must PIN a flagged segment to the plot.
///
/// The exit line a running position is managed by is drawn on the plot's edge once its price leaves
/// the visible band, so it stays visible and grabbable at any zoom. The flag rides a spare slot of
/// an instance every backend already uploads, so a backend that never learned the branch compiles,
/// uploads and draws exactly as before — and silently clips the line away again on that platform
/// alone, while the hit test on the same platform still grabs at the edge. Nothing else catches it:
/// no test here compiles a shader, and DX11 is the only backend anyone runs daily.
#[test]
fn every_backend_pins_a_flagged_segment_to_the_plot() {
    // Each backend spells the plot's top edge in its own syntax, and the bound is asserted in full:
    // the CPU mirror (`order_geometry::pin_line_y`) subtracts NOTHING from it, on purpose, because
    // the only thing available to inset by is the thickness — a user setting the highlight scales by
    // 1.7 — and neither the hit test nor the label column has that number. A backend that insets
    // moves the line away from where both of them look for it.
    const BACKENDS: &[(&str, &str)] = &[
        (
            "chartdx/shaders/order_lines.hlsl",
            "float lo = cv_bounds.y;",
        ),
        (
            "chartdx/shaders/chart_native.metal",
            "float lo = cv.bounds.y;",
        ),
        ("chartdx/shaders/native_seg.wgsl", "let lo = cv.bounds.y;"),
    ];
    for (path, low_bound) in BACKENDS {
        // Comments stripped, as the sibling style test does: every one of these shaders carries a
        // paragraph about the pin, and a test that greps prose passes on a deleted branch.
        let src = code_only(&read_src(path));
        assert!(
            src.contains("s.m.w >= 0.5"),
            "{path}: no pin branch — a flagged exit line is clipped away instead of held at the edge"
        );
        for endpoint in ["clamp(a.y", "clamp(b.y"] {
            assert!(
                src.contains(endpoint),
                "{path}: `{endpoint}` is not pinned — both ends must be, or a pinned line tilts                  instead of staying flat"
            );
        }
        assert!(
            src.contains(low_bound),
            "{path}: the pin must hold the line at the plot's own edge (`{low_bound}`), with no              inset the hit test and the labels cannot match"
        );
    }
}

/// All three backends must extend a RAY along its own direction.
///
/// The instance carries three extend modes, and two of them look alike from Rust: `1` moves the far
/// end in TIME while keeping its price — which is right for an order line and turns any sloped
/// figure horizontal — and `2` extrapolates through the second point. A backend that never learned
/// the second one draws a ray as a flat line to the right edge, at the origin's price. Nothing else
/// catches that: the geometry is built, uploaded and drawn, every test passes, and only the pixels
/// on that one platform are wrong — and DX11 is the only backend anyone here runs daily.
#[test]
fn every_backend_extends_a_ray_along_its_direction() {
    const BACKENDS: &[&str] = &[
        "chartdx/shaders/order_lines.hlsl",
        "chartdx/shaders/chart_native.metal",
        "chartdx/shaders/native_seg.wgsl",
    ];
    for path in BACKENDS {
        let src = read_src(path);
        assert!(
            src.contains("1.5"),
            "{path}: no ray branch — the extend mode is not distinguished from the order line's"
        );
        assert!(
            src.contains("normalize(d"),
            "{path}: a ray's far end must be pushed along the a->b direction"
        );
        assert!(
            src.contains("b_raw - a_raw"),
            "{path}: the direction must come from the UNSNAPPED points; a one-pixel round over the \
             ray's reach visibly tilts the line"
        );
    }
}

/// Prepare must hand the price auto-fit the two bands SEPARATELY, through `fit_band`.
///
/// This is the one link in the chain no unit test can reach: `moon-ui-gpui` is a binary crate, so
/// the rule itself lives in `moon-chart` where it can be tested and only its CALL is here. The rule
/// exists because the two spans mean different things — trades and order lines are inside the
/// window, the last price and the order-book band merely have to stay on screen — and unioning them
/// before the fit destroys the distinction. A view panned off the data then sizes itself to a
/// reference that is not on screen: measured, a settled 2.4% window snapped to 1.2% against the book
/// band, and to 0.06% against the bare last price, in a single frame. Restoring the union compiles,
/// passes every unit test, and shows up only as the price scale jumping when the chart crosses the
/// live edge.
#[test]
fn the_price_fit_is_told_what_the_window_holds_and_what_only_has_to_be_visible() {
    let src = code_only(&read_src("chartdx/data_state/market.rs"));
    // Argument ORDER is pinned, not just the call: the two spans have the same type, and swapping
    // them type-checks while inverting the rule.
    assert!(
        src.contains("fit_band(window_data, reference, use_reference)"),
        "prepare no longer asks `fit_band` what the price fit should cover, in that order"
    );
    assert!(
        src.contains("let window_data =") && src.contains("let reference ="),
        "the reference band must be built apart from the window's own data"
    );
    // The candle band and the order band must be joined through the BOUNDED admission, never through
    // a plain union. `union_range` here compiles, type-checks and passes every unit test, and shows up
    // only as one order with a distant target squeezing every candle into a corner — with the
    // translucent entry-to-sell panic zone then covering the whole pane, which is what the user
    // reported as "something painted the chart red". The rule itself is unit-tested in `moon-chart`;
    // only its CALL can be pinned here.
    assert!(
        src.contains("admit_order_band(tick_price, pr.cached_order_price)"),
        "the order band is unioned into the candle band again, so one distant order target can squash \
         the candles to a hairline"
    );
    // And the fallback test is TWO things joined by OR: following the live edge, or never having had
    // window data. `&&` would strip the reference from every live pane; testing the view's current
    // scale instead of the data would switch the fallback off one frame after the first fit and
    // latch a data-less pane onto that first hairline band.
    assert!(
        src.contains("pane.view.follow || !pr.saw_window_data"),
        "a pane that has never had window data is not allowed to fall back to the reference band"
    );
    // The other link no unit test can reach: the ceiling on a future pan depends on the plot width
    // and the X scale, and both change without any pan (a resize, a Shift+MMB scale sync). Prepare
    // is the only place that sees all three, so it has to re-apply it or the live edge ends up off
    // the left of the plot with nothing on screen to navigate back by.
    assert!(
        src.contains("clamp_future_anchor("),
        "prepare no longer re-applies the future-pan ceiling"
    );
}

/// The platform pairs presses per WINDOW, from timing and cursor distance alone, and never asks
/// which element was hit. So a press landing on a chart can carry `click_count == 2` while press
/// one closed a DIFFERENT chart with its × — the stack reflow moves the next slot under the
/// cursor, and the default `LeftDouble` buy gesture fires on a chart the user only meant to close.
/// Every chart press must be counted through the panel's own `ClickSeries` first; a handler
/// reading `e.click_count` for anything but that call brings the accidental order straight back.
#[test]
fn chart_presses_are_counted_per_panel_before_a_gesture_reads_them() {
    let source = code_only(&read_src("panels/chart/render_input.rs"));
    for signature in [
        "pub(super) fn mouse_down_left(",
        "pub(super) fn mouse_down_right(",
        "pub(super) fn mouse_down_middle(",
    ] {
        let body = braced_body(&source, signature);
        // Substring, not a formatted call: `rustfmt` splits this call across lines as its argument
        // list grows, and a check that reads the arguments would pass or fail on line breaks.
        assert!(
            body.contains("press_count("),
            "{signature}: presses must be counted by this panel before a gesture matches them"
        );
        // Placement must sit behind the `Option` `press_count` returns, so a press that belongs to
        // a chart closing reaches no gesture at all — not even a single-press binding on the middle
        // button or a modifier, which never looks at a count.
        assert!(
            body.contains("clicks.is_some_and("),
            "{signature}: order placement must be gated on the counted press, not called outright"
        );
        let call = chain_between(body, "try_place_order_click(", ")", signature);
        assert!(
            !call.contains("e.click_count"),
            "{signature}: order placement must read this panel's own count, not the window's"
        );
    }
}

/// Every per-tab chart setting must reach the tab through `apply_tab_setting`, never by writing the
/// global default in `layout.toml` directly.
///
/// The graphics popup did exactly that until the settings became per tab, and the shape of a
/// regression is a one-line convenience — "just set `b.layout.chart_graphics` here" — which still
/// looks correct on the source tab and silently changes every OTHER window's charts.
#[test]
fn the_chart_popups_write_their_settings_through_the_tab_spec() {
    for (module, global) in [
        ("chart_tabs/graphics_popup.rs", "b.layout.chart_graphics ="),
        ("chart_tabs/candle_popup.rs", "b.layout.candle_view ="),
    ] {
        let source = code_only(&read_src(module));
        assert!(
            source.contains("self.apply_tab_setting(StackSetting::"),
            "{module}: the popup must apply its setting to the target tab, not to a global"
        );
        assert!(
            !source.contains(global),
            "{module}: only the shared ⧉ walk may write the global default these tabs inherit"
        );
    }
}

/// A chart host shows ONE overlay at a time, and that has to be structural rather than remembered.
///
/// Both hosts used to carry a `bool` per popup, and nothing in the code kept them apart: two of the
/// six — the labels popup, which must keep `overlay_closable(false)` because its dropdown menus
/// paint in their own deferred layers, and the drawing-tool panel, whose dismiss layer sits under
/// the button row — simply stayed on screen under whatever opened next. The fix is one
/// `PopupSlot` per host, so the regression to catch is a new `..._popup_open: bool` field bringing
/// an independent flag back.
#[test]
fn a_chart_host_shows_one_overlay_at_a_time() {
    for module in [
        "chart_tabs/mod.rs",
        "chart_tabs/detached_host/mod.rs",
        "chart_tabs/settings.rs",
    ] {
        let source = code_only(&read_src(module));
        assert!(
            !source.contains("_popup_open: bool"),
            "{module}: chart overlays go through the one PopupSlot, never a flag of their own"
        );
    }
    // The slot's own guarantee: opening reports what it displaced, so the caller can settle it, and
    // hiding checks ownership so a late close report cannot shut the popup that replaced it.
    let slot = code_only(&read_src("chart_tabs/popup_slot.rs"));
    assert!(
        slot.contains("self.0.replace(popup).filter(|prev| *prev != popup)"),
        "popup_slot.rs: showing a popup must name the one it displaced"
    );
    assert!(
        slot.contains("if self.0 != Some(popup)"),
        "popup_slot.rs: hiding must be a no-op unless that popup is the one showing"
    );
    // Every popover on a host routes its open/close report through the slot rather than a setter of
    // its own, which is what makes the exclusion hold for all of them at once.
    for module in [
        "chart_tabs/common.rs",
        "chart_tabs/candle_popup.rs",
        "chart_tabs/graphics_popup.rs",
        "chart_tabs/labels_popup/mod.rs",
    ] {
        let source = code_only(&read_src(module));
        assert!(
            source.contains("report_chart_popup(ChartPopup::"),
            "{module}: the popover must report open and close to the shared slot"
        );
    }
}

/// The durable trade-history query must read the PANEL's effective graphics settings, must NOT be
/// narrowed by the trade-kind checkboxes, and must skip the round trip when both are clear.
///
/// Three rules at one call site. Reading `layout.chart_graphics` would load one tab's answer into
/// another's chart now that the popup is per tab. Narrowing the SQL by the checkboxes is the subtler
/// error: the row cap is applied AFTER the predicate, so hiding one kind frees slots under it and
/// silently changes which trades the history holds — `ChartTradeRecord::emulator` is carried per row
/// precisely so the drawing filter, and only it, answers to those boxes. And with both boxes clear
/// nothing would be drawn from the set at all, which is the one case worth not reading.
#[test]
fn the_durable_history_query_reads_the_panel_s_own_graphics_settings() {
    let source = code_only(&read_src("panels/chart/report_trades.rs"));
    assert!(
        source.contains("self.effective_chart_graphics(cx)"),
        "the history query must resolve this panel's override rather than the global default"
    );
    assert!(
        !source.contains("layout.chart_graphics"),
        "the history query must not reach past the panel to the global default"
    );
    assert!(
        !source.contains("f.emulator = None") && !source.contains(".emulator = Some("),
        "the trade-kind checkboxes must not narrow the durable query — only the drawing filter \
         may answer to them"
    );
    assert!(
        source.contains("if !draws_any_kind {"),
        "both boxes clear must skip the read, or the panel pays for a set nobody draws"
    );
    let drawing = code_only(&read_src("chartdx/trade_history_sync.rs"));
    assert!(
        drawing.contains("trade_kind_visible(&self.chart_graphics, record.emulator)"),
        "the drawing filter is where the trade-kind checkboxes apply"
    );
}

/// A panel that stops showing its history target must drop it.
///
/// Every emptying path matters, but TTL expiry most of all: it is the only AUTOMATIC one, it is what
/// a retained COMPRESS slot does on a quiet market, and a target left behind there goes on starting a
/// durable read on every report generation for the rest of the session.
#[test]
fn an_emptied_chart_panel_drops_its_trade_history_target() {
    let refs = code_only(&read_src("panels/chart/refs.rs"));
    let ttl = braced_body(&refs, "pub(super) fn arm_ttl_timer(");
    assert!(
        ttl.contains("this.clear_history_target_if_unused(cx)"),
        "TTL expiry must drop the history target of a slot that now shows nothing"
    );
    let panel = code_only(&read_src("panels/chart/mod.rs"));
    for signature in ["fn remove_pane(", "pub fn close_all_panes("] {
        let body = braced_body(&panel, signature);
        assert!(
            body.contains("self.clear_history_target_if_unused(cx)"),
            "{signature}: closing a market must drop the history target it leaves behind"
        );
    }
}

/// Turning the LAST trade kind back on must re-run the durable query, because the read was skipped
/// entirely while nothing was drawn and those rows are not in memory.
///
/// Ticking one box while the other is already on costs no read — the checkboxes filter at drawing
/// time — so this fires on the one transition that needs data, and on nothing else. TWO paths reach
/// a panel: its own override through the setter, and — for a panel that follows the global default —
/// a ⧉ press in another group window, which rewrites that default without ever walking this stack
/// and is heard only through the backend observer.
#[test]
fn a_trade_kind_change_re_runs_the_durable_history_query() {
    let source = code_only(&read_src("panels/chart/mod.rs"));
    let body = braced_body(&source, "pub fn set_chart_graphics(");
    assert!(
        body.contains("self.requery_trade_history_on_trade_kinds(cx)"),
        "set_chart_graphics must re-run the history query, or a re-ticked flag shows nothing"
    );
    // BOTH constructors carry a copy of this observer — `new_main` for Main panes and `new_addto`
    // for numbered AddToChart/Custom ones — and the second copy is precisely the ⧉ target. Counting
    // the branches instead of inspecting the first match is what makes this test see the copy that
    // was missed when the re-query was first hooked up.
    let branches = source
        .matches("if settings_sig != this.settings_sig {")
        .count();
    assert_eq!(
        branches, 2,
        "both panel constructors must observe the settings signature"
    );
    let requeries = source
        .matches("this.requery_trade_history_on_trade_kinds(cx)")
        .count();
    assert_eq!(
        requeries, branches,
        "a panel following the global default hears a graphics change only in its observer, so \
         every copy of that branch must re-run the query"
    );
}

/// Every path that puts a market on an AddToChart panel must also point that panel's durable trade
/// history at it.
///
/// The closed-trade arrows come from that history, and for a long time only Main ever asked for one:
/// the stack tiles showed a market with no history target, so their trade layer was empty and the
/// graphics popup's trade-kind checkboxes filtered a set that had never been loaded. `add_coin` has
/// three call paths — a live chart whose TTL is extended, a retained COMPRESS slot taken over by a
/// new detection, and a freshly created panel — and a fourth added later would reintroduce the gap.
#[test]
fn every_addtochart_market_gets_a_trade_history_target() {
    let source = code_only(&read_src("chart_tabs/add_stack.rs"));
    let helper = braced_body(&source, "fn show_market_with_history(");
    assert!(
        helper.contains(".add_coin(") && helper.contains(".track_history_scope("),
        "the helper must pair showing a market with requesting its durable history"
    );
    // Any receiver name, not just `panel.`: the crate writes this call as `p.`, `s.` and `panel.`
    // in different files, and a fourth path in another style would slip past a literal match.
    let outside = source.replace(helper, "");
    assert!(
        !outside.contains(".add_coin("),
        "a market may reach an AddToChart panel only through show_market_with_history, or its \
         chart draws no closed trades"
    );
    // A tile is showing the live edge; the FOCUSING entry point would jump it to the newest closed
    // trade, and `show_time_range` leaves that view manual for good.
    assert!(
        !helper.contains("apply_history_scope"),
        "a tile must request history without focusing, or a detect arrival tears it off the live edge"
    );
}

/// Protects the Moonbot shape of the order-move gestures: a CLICK on a price, on every button.
///
/// Two edits break it silently. Dropping the call from one button's press handler leaves that
/// button's binding recognised in the settings and dead on the chart — the defect this feature was
/// reported for. And routing the gesture back through `try_start_order_drag` makes it grab a line
/// when the press happens to land on one and move the whole side when it does not: two different
/// live trades behind the same hand movement, chosen by a few pixels.
#[test]
fn move_gestures_stay_a_click_on_every_button() {
    let source = code_only(&read_src("panels/chart/render_input.rs"));
    for button in ["Left", "Middle", "Right"] {
        assert!(
            source.contains(&format!("try_move_orders_click(TradeMouseButton::{button}")),
            "the {button} button must offer its press to the move gestures"
        );
    }
    let trade = code_only(&read_src("panels/chart/trade.rs"));
    let drag = braced_body(&trade, "pub(super) fn try_start_order_drag(");
    assert!(
        !drag.contains("gesture_matches") && !drag.contains("move_gestures"),
        "dragging is the plain left grab; the gestures are the bulk move-to-price"
    );
    // The command itself: the terminal must not compute a destination per order.
    let click = braced_body(&trade, "pub(super) fn try_move_orders_click(");
    assert!(
        click.contains("resolve_move_gesture") && click.contains("move_orders_to_price"),
        "the gesture resolves against the bindings and sends one bulk command"
    );
}

/// Removing `engine.rs:ChartEngine::set_follow`'s `self.historical ||` guard must fail: the
/// global Live flag re-anchors a closed-trade window to now and leaves its old candles off-screen.
#[test]
fn historical_panels_make_the_engine_ignore_the_global_follow_flag() {
    let engine = read_src("chartdx/engine.rs");
    let follow = code_only(braced_body(&engine, "pub fn set_follow("));
    assert!(
        follow.contains("if self.historical || self.follow == follow {"),
        "set_follow must return before the live-reset body when the engine is historical"
    );
    let panel = read_src("panels/chart/mod.rs");
    let historical = code_only(braced_body(&panel, "pub fn new_historical("));
    assert!(
        historical.contains("panel.chart.set_historical(true);"),
        "new_historical must tell its engine that the panel shows a closed interval"
    );
}

/// Removing `window.rs`'s `normalized_scale` call must fail: a hand-edited non-preset scale
/// alters a trade chart while the dropdown misleadingly labels the persisted setting Auto.
#[test]
fn trade_windows_normalize_their_persisted_scale_before_applying_it() {
    let window = code_only(&read_src("trade_window/window.rs"));
    assert!(
        window.contains("normalized_scale(owner.read(pcx).layout.trade_window_scale)"),
        "the persisted trade-window scale must be normalized before set_scale receives it"
    );
}

/// `report_trades.rs:ChartPanel::load_history_scope` must only publish loaded markers after a
/// Report coin click; a viewport action would move or rescale the reader's chart unrequested.
#[test]
fn report_coin_history_load_does_not_change_the_viewport() {
    let source = code_only(&read_src("panels/chart/report_trades.rs"));
    let load_history = braced_body(&source, "fn load_history_scope(");
    let successful_load = braced_body(load_history, "Ok(history) => {");
    for forbidden in [
        "this.chart.show_time_range(",
        "this.chart.center_time_range(",
        "this.mark_input_changed(cx);",
    ] {
        assert!(
            !successful_load.contains(forbidden),
            "a Report coin click must not change the viewport through `{forbidden}`"
        );
    }
}
