use super::*;

fn span(t0_ms: f64, p0: f64, t1_ms: f64, p1: f64) -> RulerSpan {
    RulerSpan {
        pane: 0,
        t0_ms,
        p0,
        t1_ms,
        p1,
    }
}

fn unit(key: &str) -> String {
    t!(key).to_string()
}

/// The move is measured FROM the press TO the pointer, so a drag down reads negative — the sign is
/// the direction the user dragged, not which price is larger.
#[test]
fn the_move_line_is_signed_by_the_drag_direction() {
    let (m, s) = (
        unit("chart_labels.unit_minute"),
        unit("chart_labels.unit_second"),
    );
    assert_eq!(
        move_text(span(0.0, 100.0, 750_000.0, 101.25)),
        format!("+1.25% · 12{m} 30{s}")
    );
    assert_eq!(
        move_text(span(750_000.0, 101.25, 0.0, 100.0)),
        format!("-1.23% · 12{m} 30{s}"),
        "dragged leftward and down: duration is still positive"
    );
    // No positive base: the percentage is meaningless, the duration is not.
    assert_eq!(move_text(span(0.0, 0.0, 42_000.0, 5.0)), format!("42{s}"));
}

/// Two units at most, the larger first, rounded down — a measurement states what elapsed.
#[test]
fn a_duration_reads_in_its_two_largest_units() {
    let (d, h, m, s) = (
        unit("chart_labels.unit_day"),
        unit("chart_labels.unit_hour"),
        unit("chart_labels.unit_minute"),
        unit("chart_labels.unit_second"),
    );
    assert_eq!(duration_text(999.0), format!("0{s}"));
    assert_eq!(duration_text(59_999.0), format!("59{s}"));
    assert_eq!(duration_text(60_000.0), format!("1{m} 00{s}"));
    assert_eq!(
        duration_text(3_600_000.0 * 3.0 + 300_000.0),
        format!("3{h} 05{m}")
    );
    assert_eq!(
        duration_text(86_400_000.0 * 2.0 + 3_600_000.0 * 3.0),
        format!("2{d} 03{h}")
    );
    assert_eq!(duration_text(f64::NAN), format!("0{s}"));
}

fn readout(buy: f64, sell: f64, complete: bool) -> VolumeSpanReadout {
    VolumeSpanReadout {
        buy_quote: buy,
        sell_quote: sell,
        complete,
        ..VolumeSpanReadout::default()
    }
}

/// The split by side wins whenever the trade history covers the period; candles reaching the
/// period's start stand in when it does not, and say they are an estimate; then a partial split;
/// candles covering only the period's tail are the last resort.
#[test]
fn the_volume_line_prefers_a_whole_split_then_the_candles() {
    let (bv, sv, vol) = (
        unit("chart_labels.short.window_buy_volume"),
        unit("chart_labels.short.window_sell_volume"),
        unit("chart_labels.short.window_volume"),
    );
    let money = |v: f64| crate::chartdx::text::quote_turnover_label(v as f32, "USDT").unwrap();
    assert_eq!(
        volume_text(
            Some(readout(30_100.0, 18_100.0, true)),
            Some((99.0, true)),
            "USDT"
        ),
        Some(format!("{bv}{} · {sv}{}", money(30_100.0), money(18_100.0)))
    );
    assert_eq!(
        volume_text(
            Some(readout(10.0, 5.0, false)),
            Some((48_200.0, true)),
            "USDT"
        ),
        Some(format!("{vol}~{}", money(48_200.0)))
    );
    assert_eq!(
        volume_text(Some(readout(10.0, 5.0, false)), None, "USDT"),
        Some(format!("{bv}~{} · {sv}~{}", money(10.0), money(5.0)))
    );
    // Candles reaching only the period's tail rank below a partial split, and above nothing.
    assert_eq!(
        volume_text(
            Some(readout(10.0, 5.0, false)),
            Some((48_200.0, false)),
            "USDT"
        ),
        Some(format!("{bv}~{} · {sv}~{}", money(10.0), money(5.0)))
    );
    assert_eq!(
        volume_text(None, Some((48_200.0, false)), "USDT"),
        Some(format!("{vol}~{}", money(48_200.0)))
    );
    assert_eq!(
        volume_text(Some(readout(0.0, 0.0, false)), None, "USDT"),
        None
    );
    assert_eq!(volume_text(None, None, "USDT"), None);
    assert_eq!(
        volume_text(Some(readout(1.0, 1.0, true)), None, ""),
        None,
        "a unitless figure would read as a coin count"
    );
}

/// Ends snap to the NEAREST step — a tenth of a second at least, about half a percent of a longer
/// period — so neighbouring pointer positions share one read without the window growing past
/// the duration printed beside it.
#[test]
fn the_period_snaps_to_the_nearest_step_that_grows_with_it() {
    assert_eq!(quantized_period((1_520.0, 4_260.0)), Some((1_500, 4_300)));
    assert_eq!(quantized_period((1_500.0, 1_500.0)), None);
    assert_eq!(quantized_period((f64::NAN, 1.0)), None);
    // A one-hour period snaps to an 18 s step, never more than half a step off either end.
    let (from, to) = quantized_period((10_000.0, 3_610_000.0)).unwrap();
    assert_eq!((from % 18_000, to % 18_000), (0, 0));
    assert!((from - 10_000).abs() <= 9_000 && (to - 3_610_000).abs() <= 9_000);
}

fn view() -> ChartViewGpu {
    ChartViewGpu {
        bounds: [100.0, 50.0, 400.0, 200.0],
        time_to_px: 0.1,
        view_time0: 0.0,
        price_to_px: 2.0,
        view_price0: 100.0,
        ..ChartViewGpu::default()
    }
}

/// The band spans the two points and is cut at the plot's edges, never drawn over the axes.
#[test]
fn the_band_spans_both_ends_inside_the_plot() {
    // t 0 → x 100, t 1000 → x 200; price 100 → y 250, price 150 → y 150.
    assert_eq!(
        band_rect_px(&view(), 0.0, &span(1_000.0, 150.0, 0.0, 100.0)),
        Some([100.0, 150.0, 100.0, 100.0])
    );
    // Dragged past the right edge and above the top: clipped to the plot.
    assert_eq!(
        band_rect_px(&view(), 0.0, &span(3_000.0, 120.0, 9_000.0, 900.0)),
        Some([400.0, 50.0, 100.0, 160.0])
    );
    // Wholly left of the plot.
    assert_eq!(
        band_rect_px(&view(), 0.0, &span(-5_000.0, 120.0, -1_000.0, 130.0)),
        None
    );
    // A collapsed pane has no mapping.
    let flat = ChartViewGpu {
        time_to_px: 0.0,
        ..view()
    };
    assert_eq!(
        band_rect_px(&flat, 0.0, &span(0.0, 100.0, 1_000.0, 110.0)),
        None
    );
}
