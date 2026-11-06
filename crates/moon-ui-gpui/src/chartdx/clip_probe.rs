//! One-shot FireTest coverage of the real userdata pass, including its installed scissor.
//! The normal chart stays on screen; diagnostic geometry is removed by the next base blit.

use super::{ChartViewGpu, gpu, recovery_probe, userdata::UserDataLayer};
use gpui::RawGpuAccess;
use moon_chart::layers::{MarkerInstance, SEG_EXTEND_EDGE, SegInstance};

/// Draw adversarial geometry under the caller's actual scissor, then inspect native pixels.
/// No scissor is set here: replacing the production clip with the pane must fail this probe.
pub(super) fn check(
    view: &ChartViewGpu,
    book: &ChartViewGpu,
    pane: [f32; 4],
    gpu: &RawGpuAccess,
) -> Result<(), String> {
    let (device, context, target) =
        gpu::borrow_d3d(gpu).ok_or("clip probe has no native target")?;
    let clip = [pane[0], pane[1], pane[0] + pane[2], pane[1] + pane[3]];
    let capture = || {
        recovery_probe::capture(&device, &context, &target, clip, gpu.device_generation())
            .map_err(|error| error.to_string())
    };
    let before = capture()?;
    let [left, top, width, height] = view.bounds;
    if width < 40.0 || height < 40.0 {
        return Err("clip probe needs a visible plot".into());
    }
    let right = left + width;
    let bottom = top + height;
    let book_right = book.bounds[0] + book.bounds[2];
    let time = |x: f32| view.view_time0 + (x - left) / view.time_to_px;
    let price = |y: f32| view.view_price0 + (bottom - y) / view.price_to_px;
    let arrow_y = (top + height * 0.35).round();
    let line_y = (top + height * 0.7).round();
    let mut layer = UserDataLayer::new();
    let arrows = [
        pane[0] + 8.0,
        left - 3.0,
        left + width * 0.5,
        right - 3.0,
        right + 12.0,
    ]
    .map(|x| MarkerInstance::arrow(time(x), price(arrow_y), 8.0, 6.0, 4.0, 1, [1., 0., 1., 1.]));
    let segment = |x0, y0, x1, y1, extend| SegInstance {
        t0_rel: time(x0),
        p0: price(y0),
        t1_rel: time(x1),
        p1: price(y1),
        thickness: 3.0,
        pattern: 0.0,
        extend,
        clamp: 0.0,
        color: [0., 1., 1., 1.],
    };
    let segments = [
        segment(pane[0], line_y, pane[0] + pane[2], line_y, 0.0),
        segment(
            left + width * 0.25,
            pane[1],
            left + width * 0.25,
            pane[1] + pane[3],
            0.0,
        ),
        segment(
            left + 2.0,
            line_y + 10.0,
            left + 4.0,
            line_y + 10.0,
            SEG_EXTEND_EDGE,
        ),
    ];
    layer.set(&[], &[], &segments, &arrows);
    layer.prepare(&device, &context, gpu);
    layer.render_lines(view, &context, &target, gpu);
    let after = capture()?;
    let mut plot_changes = 0;
    let mut arrow_pixels = 0;
    let mut book_line_changes = 0;
    for y in 0..before.height {
        for x in 0..before.width {
            let offset = ((y * before.width + x) * 4) as usize;
            if before.pixels[offset..offset + 4] == after.pixels[offset..offset + 4] {
                continue;
            }
            let px = pane[0].floor().max(0.0) + x as f32 + 0.5;
            let py = pane[1].floor().max(0.0) + y as f32 + 0.5;
            // Independent oracle: the measured plot/book layout, never content_clip().
            if px < left.floor()
                || px >= book_right.max(right).ceil()
                || py < top.floor()
                || py >= bottom.ceil()
            {
                return Err(format!(
                    "chart geometry painted outside content at ({px},{py})"
                ));
            }
            let arrow_pixel = after.pixels[offset] > 0
                && after.pixels[offset + 1] == 0
                && after.pixels[offset + 2] > 0;
            if px >= right && arrow_pixel {
                return Err(format!("trade arrow painted in book at ({px},{py})"));
            }
            if px >= left && px < right {
                plot_changes += 1;
                if arrow_pixel {
                    arrow_pixels += 1;
                }
            }
            if px >= right && (py - (line_y + 10.0)).abs() < 2.0 {
                book_line_changes += 1;
            }
        }
    }
    if plot_changes < 20 {
        return Err("clip probe drew no visible geometry".into());
    }
    if arrow_pixels == 0 {
        return Err("clip probe lost the visible trade arrows".into());
    }
    if book.bounds[2] > 10.0 && book_line_changes < 5 {
        return Err("clip probe lost the order-line continuation into the book".into());
    }
    Ok(())
}
