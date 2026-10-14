//! Native AppKit text rasterization of the shared header, at the final messenger resolution.

use anyhow::{Context as _, ensure};
use objc2::{AnyThread as _, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{
    NSAttributedStringNSStringDrawing as _, NSBitmapImageRep, NSColor, NSDeviceRGBColorSpace,
    NSFont, NSFontAttributeName, NSFontWeightRegular, NSFontWeightSemibold,
    NSForegroundColorAttributeName, NSGraphicsContext,
};
use objc2_foundation::{NSAttributedString, NSDictionary, NSPoint, NSString};

use super::{ShotInputs, capture_mac, header, ink, resize};
use header::{Gaps, Measured, RunStyle, StripField};

/// A measured native run; the same attributed string is used for measurement and painting.
struct Run {
    text: Retained<NSAttributedString>,
    font: Retained<NSFont>,
    width: i32,
}

/// A whole header field, whose priority and leading gap are decided by the shared layout.
struct Field {
    runs: Vec<Run>,
    measured: Measured,
}

/// Restore the previous AppKit drawing context on success, failure, and unwind.
struct ContextGuard;

impl Drop for ContextGuard {
    /// Pair the context save even when native encoding fails.
    fn drop(&mut self) {
        NSGraphicsContext::restoreGraphicsState_class();
    }
}

/// Normalize the photographed body once, rasterize its header, and return a clipboard PNG.
pub(super) fn compose(png: &[u8], inputs: &ShotInputs, when_ms: i64) -> anyhow::Result<Vec<u8>> {
    let body = image::load_from_memory_with_format(png, image::ImageFormat::Png)?.to_rgb8();
    let body = resize::normalize(resize::RgbFrame {
        width: body.width(),
        height: body.height(),
        rgb: body.into_raw(),
    });
    ensure!(body.width > 0 && body.height > 0, "empty chart image");
    let base = resize::font_px(body.width);
    let strip_h = resize::strip_height(base);
    let strip = header::header_strip(&header::HeaderInputs {
        coin: inputs.coin.clone(),
        venue: inputs.venue.clone(),
        when_ms,
        zone: crate::chartdx::axes::display_zone(),
        tf_min: inputs.tf_min,
        scale_pct: inputs.scale_pct,
        time_scale_s: inputs.time_scale_s,
        delta_3h: inputs.delta_3h,
        delta_1h: inputs.delta_1h,
        delta_15m: inputs.delta_15m,
    });
    let palette = ink::palette(inputs.bg, inputs.text);
    let bitmap = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(), std::ptr::null_mut(), body.width as isize, strip_h as isize,
            8, 4, true, false, NSDeviceRGBColorSpace, (body.width * 4) as isize, 32,
        )
    }.context("could not allocate the header bitmap")?;
    let stride = usize::try_from(bitmap.bytesPerRow())?;
    ensure!(
        stride >= body.width as usize * 4 && !bitmap.bitmapData().is_null(),
        "invalid header bitmap storage"
    );
    // AppKit allocated these exact scanlines; no borrowed Rust memory survives a native draw.
    let pixels =
        unsafe { std::slice::from_raw_parts_mut(bitmap.bitmapData(), stride * strip_h as usize) };
    for y in 0..strip_h as usize {
        let colour = if y + resize::HAIRLINE_PX as usize >= strip_h as usize {
            palette.hairline
        } else {
            palette.band
        };
        for pixel in pixels[y * stride..y * stride + body.width as usize * 4].chunks_exact_mut(4) {
            pixel[..3].copy_from_slice(&colour);
            pixel[3] = 255;
        }
    }
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&bitmap)
        .context("no header drawing context")?;
    NSGraphicsContext::saveGraphicsState_class();
    let guard = ContextGuard;
    NSGraphicsContext::setCurrentContext(Some(&context));
    context.setShouldAntialias(true);
    let head = measure(&strip.head, base, &palette);
    let tail = measure(&strip.tail, base, &palette);
    let gaps = Gaps {
        field: resize::field_gap(base) as i32,
        group: resize::group_gap(base) as i32,
    };
    let sized_head: Vec<_> = head.iter().map(|field| field.measured).collect();
    let sized_tail: Vec<_> = tail.iter().map(|field| field.measured).collect();
    let fitted = header::fit_tail(
        &sized_head,
        &sized_tail,
        gaps,
        body.width as i32 - 2 * gaps.field,
    );
    let drawn: Vec<_> = head.iter().chain(tail.iter().take(fitted)).collect();
    let sized: Vec<_> = drawn.iter().map(|field| field.measured).collect();
    let mut x = header::centred_start_x(&sized, gaps, body.width as i32, gaps.field);
    let lead = unsafe {
        NSFont::systemFontOfSize_weight(resize::lead_px(base) as f64, NSFontWeightSemibold)
    };
    let baseline = strip_h as f64 - resize::strip_pad(base) as f64 - lead.ascender();
    for (index, field) in drawn.iter().enumerate() {
        if index > 0 {
            x += header::lead_width(field.measured.lead_gap, gaps);
        }
        for run in &field.runs {
            run.text
                .drawAtPoint(NSPoint::new(x as f64, baseline + run.font.descender()));
            x += run.width;
        }
    }
    context.flushGraphics();
    drop(guard);
    let header_png = capture_mac::png(&bitmap)?;
    let header =
        image::load_from_memory_with_format(&header_png, image::ImageFormat::Png)?.to_rgb8();
    ensure!(
        header.width() == body.width && header.height() == strip_h,
        "header dimensions changed"
    );
    let mut rgb = header.into_raw();
    rgb.extend_from_slice(&body.rgb);
    let composed = image::RgbImage::from_raw(body.width, body.height + strip_h, rgb)
        .context("inconsistent composed image")?;
    let mut result = std::io::Cursor::new(Vec::new());
    composed.write_to(&mut result, image::ImageFormat::Png)?;
    Ok(result.into_inner())
}

/// Measure complete fields with system font fallback, preserving shared role colours and weights.
fn measure(fields: &[StripField], base: u32, palette: &ink::Palette) -> Vec<Field> {
    fields
        .iter()
        .map(|field| {
            let runs: Vec<_> = field
                .runs
                .iter()
                .map(|run| {
                    let (size, weight, colour) = unsafe {
                        match run.style {
                            RunStyle::Lead => {
                                (resize::lead_px(base), NSFontWeightSemibold, palette.lead)
                            }
                            RunStyle::Primary => (base, NSFontWeightSemibold, palette.primary),
                            RunStyle::Secondary => (base, NSFontWeightRegular, palette.secondary),
                        }
                    };
                    let font = NSFont::systemFontOfSize_weight(size as f64, weight);
                    let colour = NSColor::colorWithSRGBRed_green_blue_alpha(
                        colour[0] as f64 / 255.0,
                        colour[1] as f64 / 255.0,
                        colour[2] as f64 / 255.0,
                        1.0,
                    );
                    let attributes = unsafe {
                        NSDictionary::from_slices(
                            &[NSFontAttributeName, NSForegroundColorAttributeName],
                            &[&*font as &AnyObject, &*colour as &AnyObject],
                        )
                    };
                    let text = unsafe {
                        NSAttributedString::new_with_attributes(
                            &NSString::from_str(&run.text),
                            &attributes,
                        )
                    };
                    let width = text.size().width.ceil() as i32;
                    Run { text, font, width }
                })
                .collect();
            let width = runs.iter().map(|run| run.width).sum();
            Field {
                runs,
                measured: Measured {
                    width,
                    lead_gap: field.lead_gap,
                },
            }
        })
        .collect()
}
