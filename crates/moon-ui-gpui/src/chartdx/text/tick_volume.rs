//! Actual per-trade quote values near the cursor, drawn with retained native chart text.

use moon_chart::tick_volume::TickVolumeRange;
use rust_i18n::t;

use super::*;

/// Complete text and measured line boxes; amounts are never truncated to fit a pane.
struct TickReadout {
    lines: Vec<(String, [f32; 2])>,
    width: f32,
    height: f32,
}

/// Prefer familiar grouped decimals; the last narrow fallback round-trips the original f32.
/// Unlike rounded SI suffixes, scientific notation keeps distinct min/max values distinct.
fn tick_amount(value: f32, compact: bool) -> String {
    let decimal = moon_core::util::fmt::group_decimal(&value.to_string());
    if compact {
        let scientific = format!("{value:e}");
        if scientific.len() < decimal.len() {
            return scientific;
        }
    }
    decimal
}

/// Keep the wide wording intact, then trade width for height without dropping a side or unit.
fn tick_lines(
    ranges: [Option<TickVolumeRange>; 2],
    unit: &str,
    locale: &str,
    stacked: bool,
    compact: bool,
) -> Vec<String> {
    let mut lines = vec![if stacked {
        t!("tick_volume.nearby_unit", locale = locale, unit = unit).to_string()
    } else {
        t!("tick_volume.nearby", locale = locale).to_string()
    }];
    for (side, range) in ["BUY", "SELL"].into_iter().zip(ranges) {
        let Some(range) = range else { continue };
        let lo = tick_amount(range.min, compact);
        let hi = tick_amount(range.max, compact);
        if stacked {
            lines.push(
                t!(
                    "tick_volume.side_count",
                    locale = locale,
                    side = side,
                    count = range.count.to_string()
                )
                .to_string(),
            );
            if range.min == range.max {
                lines.push(lo);
            } else {
                lines.push(t!("tick_volume.minimum", locale = locale, value = lo).to_string());
                lines.push(t!("tick_volume.maximum", locale = locale, value = hi).to_string());
            }
        } else {
            let value = if range.min == range.max {
                lo
            } else {
                format!("{lo} - {hi}")
            };
            lines.push(
                t!(
                    "tick_volume.side_values",
                    locale = locale,
                    side = side,
                    count = range.count.to_string(),
                    value = value,
                    unit = unit
                )
                .to_string(),
            );
        }
    }
    lines
}

/// Select a complete layout using shaped logical-pixel measurements at the user's font size.
/// A 200x180 logical-pixel plot fits both side ranges at the default 11.5px Geist Mono font,
/// including f32 extremes and six-digit counts in all three locales. Larger fonts or longer
/// quote symbols require more room; genuinely undersized plots still cannot show clipped money.
fn fit_tick_readout(
    ranges: [Option<TickVolumeRange>; 2],
    unit: &str,
    locale: &str,
    size: [f32; 2],
    mut measure: impl FnMut(&str) -> [f32; 2],
) -> Option<TickReadout> {
    if ranges.iter().all(Option::is_none) || unit.is_empty() {
        return None;
    }
    let pad = READOUT_PAD_X + READOUT_INSET;
    for (stacked, compact) in [(false, false), (true, false), (true, true)] {
        let lines: Vec<_> = tick_lines(ranges, unit, locale, stacked, compact)
            .into_iter()
            .map(|line| {
                let metrics = measure(&line);
                (line, metrics)
            })
            .collect();
        let width = lines.iter().map(|(_, m)| m[0]).fold(0.0f32, f32::max);
        let height = lines.iter().map(|(_, m)| m[1]).sum::<f32>();
        if width + 2.0 * pad <= size[0] && height + 2.0 * pad <= size[1] {
            return Some(TickReadout {
                lines,
                width,
                height,
            });
        }
    }
    None
}

/// Anchor above the native band where possible, keeping every padded line inside this pane.
fn tick_readout_origin(
    bounds: [f32; 4],
    sf: f32,
    cursor_x: f32,
    readout: &TickReadout,
) -> [f32; 2] {
    let [left, top, width, height] = bounds;
    let pad = READOUT_PAD_X + READOUT_INSET;
    // Native shaders use physical pixels, including the 72px ceiling and 1px floor inset.
    let band_top = top + height - 1.0 - (height * 0.18).min(72.0);
    [
        clamp_anchor(
            cursor_x / sf + CURSOR_BADGE_DX,
            left / sf + pad,
            (left + width) / sf - readout.width - pad,
        ),
        clamp_anchor(
            band_top / sf - readout.height - pad,
            top / sf + pad,
            (top + height) / sf - readout.height - pad,
        ),
    ]
}

impl RenderState {
    /// Describe every ordinary print within three logical pixels while hovering the tick band.
    /// A side with overlapping prints gets a count and range, never an arbitrary selected trade.
    /// Read the upcoming native ring because text preparation runs before GPU upload preparation.
    pub(super) fn draw_tick_volume_readout(
        &mut self,
        ctx: &mut GpuCanvasTextContext<'_>,
        idx: usize,
        cursor: CursorState,
        sf: f32,
        placed: &mut Vec<PlacedLabel>,
    ) -> anyhow::Result<()> {
        let pr = &self.panes[idx];
        let view = pr.view;
        if pr.quote.is_empty() {
            return Ok(());
        }
        let x_dev = self.slot_origin[0] + cursor.local[0];
        let y_dev = self.slot_origin[1] + cursor.local[1];
        let Some((from, to)) = moon_chart::tick_volume::cursor_column(
            view.bounds,
            view.view_time0,
            view.time_to_px,
            [x_dev, y_dev],
            sf,
            view.volume_alpha,
        ) else {
            return Ok(());
        };
        let [_, _, width, height] = view.bounds;
        // A small column tolerates the cached bitmap's subpixel pan phase. Its explicit nearby
        // wording avoids pretending it is an exact time or selecting one of overlapping prints.
        let ranges = pr.layers.nearby_tick_volumes(from, to);
        let unit = pr.quote.clone();
        let Some(readout) = fit_tick_readout(
            ranges,
            &unit,
            &rust_i18n::locale(),
            [width / sf, height / sf],
            |line| {
                let m = self.measure_label_text(ctx, line);
                [m.width.as_f32(), m.line_height.as_f32()]
            },
        ) else {
            return Ok(());
        };
        let text_w = readout.width;
        let [x, mut y] = tick_readout_origin(view.bounds, sf, x_dev, &readout);
        let ink = color(self.readout_label);
        for (line, metrics) in &readout.lines {
            let h = metrics[1];
            self.draw_label_text(ctx, line, x, y, 0.0, 0.0, ink)?;
            placed.push(PlacedLabel {
                x,
                y,
                ax: 0.0,
                ay: 0.0,
                w: text_w,
                h,
                solid: true,
            });
            y += h;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
