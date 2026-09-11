//! Actual traded quote values under the cursor, drawn with retained native chart text.
//!
//! Two different facts share the block, and they are never blended:
//!
//! * the individual prints near the cursor's time column, per side, as a count and a min/max RANGE
//!   — overlapping trades stay independent prints and are never summed;
//! * the hovered candle's own bucket turnover, one aggregate figure, labelled with the period it
//!   covers. The chart has no per-side candle data, so this figure is never split into BUY/SELL and
//!   never presented as a trade.

use moon_chart::tick_volume::TickVolumeRange;
use rust_i18n::t;

use super::*;

/// Select only visible volume bands beneath the pointer, in the shaders' device-pixel coordinates.
/// Tick height matches `volume_vertex`; candle height follows the uploaded per-pane style.
fn hovered_volume_bands(
    bounds: [f32; 4],
    cursor: [f32; 2],
    tick_alpha: f32,
    candle_style: crate::chartdx::types::VolumeStyleGpu,
) -> [bool; 2] {
    if moon_chart::tick_volume::cursor_time(bounds, 0.0, 1.0, cursor).is_none() {
        return [false, false];
    }
    let base = bounds[1] + bounds[3] - 1.0;
    let inside = |height: f32| {
        height.is_finite() && height > 0.0 && (base - height..=base).contains(&cursor[1])
    };
    let ticks = moon_chart::tick_volume::tick_ranges_visible(tick_alpha)
        && inside((bounds[3] * 0.18).min(72.0));
    let candle = candle_style.m[0] >= 0.5
        && (moon_chart::tick_volume::tick_ranges_visible(candle_style.up[3])
            || moon_chart::tick_volume::tick_ranges_visible(candle_style.down[3]))
        && inside(bounds[3] * candle_style.m[1]);
    [ticks, candle]
}

/// The hovered candle's own turnover, as the readout states it.
///
/// One aggregate for the whole bucket. It carries its bucket width because the history tail mixes
/// coarser buckets into the series: an unlabelled amount would leave a one-minute total and a
/// one-day total indistinguishable. The amount may be a source figure or an estimate — see
/// [`moon_chart::VolumeSample::quote_volume`] — and the sample does not carry which, so neither
/// does this.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CandleVolume {
    quote: f32,
    tf_ms: f64,
}

impl CandleVolume {
    /// Accept a bucket only when its turnover is a real, positive amount.
    ///
    /// A bucket with nothing in it — or a source that reports no turnover — contributes no line
    /// rather than a bare zero, and a non-finite figure contributes none rather than an `inf` where
    /// the reader expects money. `collect_samples` floors the value at zero, which folds a NaN into
    /// zero but leaves an infinity intact, so this is the guard that stops one.
    fn new(quote: f32, tf_ms: f64) -> Option<Self> {
        (quote.is_finite() && quote > 0.0).then_some(Self { quote, tf_ms })
    }
}

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

/// The candle's own lines, appended after the tick rows and never mixed into them.
///
/// The heading above the tick rows says "nearby ticks", so this figure carries its own label rather
/// than inheriting one that would misdescribe it.
///
/// Stacked, it has its own heading and the amount goes on the next line, which is what lets a
/// candle-only readout narrow at all: without it the single inline line is the same width in all
/// three attempts and a narrow pane simply loses the block. The unit rides that heading only when
/// there is no tick heading above already carrying it.
fn candle_lines(
    candle: CandleVolume,
    unit: &str,
    locale: &str,
    stacked: bool,
    unit_above: bool,
    compact: bool,
) -> Vec<String> {
    let value = tick_amount(candle.quote, compact);
    let tf = moon_chart::volume_bars::bucket_label(candle.tf_ms);
    if !stacked {
        return vec![match tf {
            Some(tf) => t!(
                "tick_volume.candle_values",
                locale = locale,
                tf = tf,
                value = value,
                unit = unit
            )
            .to_string(),
            None => t!(
                "tick_volume.candle_values_untimed",
                locale = locale,
                value = value,
                unit = unit
            )
            .to_string(),
        }];
    }
    let head = match (unit_above, tf) {
        (true, Some(tf)) => t!("tick_volume.candle_head", locale = locale, tf = tf).to_string(),
        (true, None) => t!("tick_volume.candle_head_untimed", locale = locale).to_string(),
        (false, Some(tf)) => t!(
            "tick_volume.candle_head_unit",
            locale = locale,
            tf = tf,
            unit = unit
        )
        .to_string(),
        (false, None) => t!(
            "tick_volume.candle_head_unit_untimed",
            locale = locale,
            unit = unit
        )
        .to_string(),
    };
    vec![head, value]
}

/// Keep the wide wording intact, then trade width for height without dropping a side or unit.
fn tick_lines(
    ranges: [Option<TickVolumeRange>; 2],
    candle: Option<CandleVolume>,
    unit: &str,
    locale: &str,
    stacked: bool,
    compact: bool,
) -> Vec<String> {
    let has_ticks = ranges.iter().any(Option::is_some);
    let mut lines = Vec::new();
    if has_ticks {
        lines.push(if stacked {
            t!("tick_volume.nearby_unit", locale = locale, unit = unit).to_string()
        } else {
            t!("tick_volume.nearby", locale = locale).to_string()
        });
    }
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
    if let Some(candle) = candle {
        lines.extend(candle_lines(
            candle, unit, locale, stacked, has_ticks, compact,
        ));
    }
    lines
}

/// Select a complete layout using shaped logical-pixel measurements at the user's font size.
/// A 200x180 logical-pixel plot fits both side ranges at the default 13px Geist Mono readout size,
/// including f32 extremes and six-digit counts in all three locales. Larger fonts, a candle line or
/// longer quote symbols require more room; genuinely undersized plots still cannot show clipped
/// money — the block is dropped whole rather than truncated.
fn fit_tick_readout(
    ranges: [Option<TickVolumeRange>; 2],
    candle: Option<CandleVolume>,
    unit: &str,
    locale: &str,
    size: [f32; 2],
    mut measure: impl FnMut(&str) -> [f32; 2],
) -> Option<TickReadout> {
    if (ranges.iter().all(Option::is_none) && candle.is_none()) || unit.is_empty() {
        return None;
    }
    let pad = READOUT_PAD_X + READOUT_INSET;
    for (stacked, compact) in [(false, false), (true, false), (true, true)] {
        let lines: Vec<_> = tick_lines(ranges, candle, unit, locale, stacked, compact)
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
    /// Describe trades only while the pointer is over their visible bottom-volume band.
    ///
    /// Every ordinary print within three logical pixels of the cursor's time column is reported per
    /// side as a count and a range, never an arbitrary selected trade; the tick rows follow the
    /// native band being drawn, since that ring is what they describe. Beside them, the candle the
    /// pointer is over contributes its own bucket turnover only inside its configured volume band.
    /// Moving back onto the candle plot hides both readouts without changing the crosshair labels.
    ///
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
        let [hover_ticks, hover_candle] = hovered_volume_bands(
            view.bounds,
            [x_dev, y_dev],
            view.volume_alpha,
            pr.volume_style,
        );
        if !hover_ticks && !hover_candle {
            return Ok(());
        }
        let Some((from, to)) = moon_chart::tick_volume::cursor_column(
            view.bounds,
            view.view_time0,
            view.time_to_px,
            [x_dev, y_dev],
            sf,
        ) else {
            return Ok(());
        };
        let [_, _, width, height] = view.bounds;
        // A small column tolerates the cached bitmap's subpixel pan phase. Its explicit nearby
        // wording avoids pretending it is an exact time or selecting one of overlapping prints.
        let ranges = if hover_ticks {
            pr.layers.nearby_tick_volumes(from, to)
        } else {
            [None, None]
        };
        // The candle's own turnover, from the samples the bottom band is scaled from, so the
        // readout and the bar under the pointer cannot disagree. Those samples are RETAINED across
        // a plain pan, exactly as the band's own statistics are, so the figure survives the gesture
        // that never refills the history buffer. `t_open_ms` is absolute while the view's times are
        // relative to the chart epoch, hence the epoch added back here.
        let candle = moon_chart::tick_volume::cursor_time(
            view.bounds,
            view.view_time0,
            view.time_to_px,
            [x_dev, y_dev],
        )
        .filter(|_| hover_candle)
        .and_then(|at| {
            moon_chart::volume_bars::sample_at(&pr.volume_samples, pr.epoch_ms + at as f64)
        })
        .and_then(|sample| CandleVolume::new(sample.quote_volume, sample.tf_ms));
        let unit = pr.quote.clone();
        let Some(readout) = fit_tick_readout(
            ranges,
            candle,
            &unit,
            &rust_i18n::locale(),
            [width / sf, height / sf],
            |line| {
                let m = self.measure_readout_text(ctx, line);
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
            self.draw_readout_text(ctx, line, x, y, 0.0, 0.0, ink)?;
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
