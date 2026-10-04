use super::*;

const ENTRY_MS: i64 = 1_791_000_000_000;

fn tick(ms: i64, price: f32, side: Side) -> Tick {
    Tick {
        time_ms: ms as f64,
        price,
        qty: 1_000.0,
        side,
    }
}

/// Ten seconds of run-up, a dip at the entry and a climb to the exit 35 s later.
fn tape(n: usize) -> Vec<Tick> {
    let from = ENTRY_MS - 10_000;
    let span = 48_000.0;
    (0..n)
        .map(|i| {
            let ms = from + (span * i as f64 / n as f64) as i64;
            let wobble = ((i * 7919) % 100) as f32 / 100.0 - 0.5;
            let base = if ms < ENTRY_MS {
                0.0430 - 0.0006 * ((ms - from) as f32 / 10_000.0)
            } else {
                0.0424 + 0.0005 * ((ms - ENTRY_MS) as f32 / 38_000.0)
            };
            let side = if i % 3 == 0 { Side::Sell } else { Side::Buy };
            tick(ms, base * (1.0 + wobble * 0.004), side)
        })
        .collect()
}

fn chart<'a>(market: &'a str, ticks: &'a [Tick], lines: &'a [(i64, f64)]) -> DealChart<'a> {
    DealChart {
        market,
        base: "ACE",
        short: false,
        entry: (ENTRY_MS, 0.04236),
        exit: (ENTRY_MS + 35_000, 0.04282),
        stop: Some(0.04152),
        entry_line: lines,
        exit_line: &[],
        caption: "+2.17 USDT (+1.08%)  Auto Price Down",
        won: true,
        spent_usd: Some(200.0),
        day_usd: Some(15.39),
        ticks,
        window: (ENTRY_MS - 11_667, ENTRY_MS + 38_000),
        zone: chrono_tz::Europe::Moscow,
    }
}

#[test]
fn a_trade_draws_a_png_of_the_frame_size() {
    let ticks = tape(1_500);
    let lines = [(ENTRY_MS - 9_000, 0.0428), (ENTRY_MS - 4_000, 0.04236)];
    let png = render(&chart("币安人生USDT", &ticks, &lines)).expect("drawn");
    assert_eq!(
        &png[..8],
        &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']
    );
    assert_eq!(&png[16..20], &(WIDTH as u32).to_be_bytes());
    assert_eq!(&png[20..24], &(HEIGHT as u32).to_be_bytes());
    assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
    // The developer's eyes on the frame: `MOON_DEAL_CHART_OUT=path cargo test …`.
    if let Ok(path) = std::env::var("MOON_DEAL_CHART_OUT") {
        std::fs::write(path, &png).expect("write the picture");
    }
}

/// A market that did not print in the window still gets its trade drawn: the frame is the
/// trade's, the fills give the prices.
#[test]
fn a_silent_market_still_draws_the_trade() {
    assert!(render(&chart("LRCUSDT", &[], &[])).is_some());
}

#[test]
fn an_empty_window_or_no_price_draws_nothing() {
    let mut c = chart("ACEUSDT", &[], &[]);
    c.window = (ENTRY_MS, ENTRY_MS);
    assert!(render(&c).is_none());
    let mut c = chart("ACEUSDT", &[], &[]);
    (c.entry, c.exit, c.stop) = ((ENTRY_MS, 0.0), (ENTRY_MS, 0.0), None);
    assert!(render(&c).is_none());
}

#[test]
fn the_price_scale_steps_round_and_holds_every_price() {
    let s = PriceScale::fit([0.01054, 0.01055, 0.01108].into_iter()).unwrap();
    assert!(s.low <= 0.01054 && s.high >= 0.01108);
    let intervals = (s.high - s.low) / s.step;
    assert!((intervals - intervals.round()).abs() < 1e-6);
    assert!((5.0..=14.0).contains(&intervals.round()), "{intervals}");
    let mantissa = s.step / 10f64.powf(s.step.log10().floor());
    assert!(
        [1.0, 2.0, 2.5, 5.0]
            .iter()
            .any(|m| (mantissa - m).abs() < 1e-6),
        "{}",
        s.step
    );
    // One flat price still gets a band.
    let flat = PriceScale::fit([2.5].into_iter()).unwrap();
    assert!(flat.high > flat.low);
    assert!(PriceScale::fit(std::iter::empty()).is_none());
}

#[test]
fn a_hot_tape_is_cut_to_four_prints_a_column() {
    let ticks = tape(200_000);
    let refs: Vec<&Tick> = ticks.iter().collect();
    let plot = Plot {
        t0: ticks[0].time_ms as i64,
        t1: ticks[ticks.len() - 1].time_ms as i64,
        low: 0.0,
        high: 1.0,
    };
    let kept = column_extremes(&refs, &plot);
    let columns = (PLOT_RIGHT - PLOT_LEFT).ceil() as usize + 1;
    assert!(kept.len() <= columns * 4, "{} crosses", kept.len());
    assert!(std::ptr::eq(kept[0], refs[0]));
    assert!(std::ptr::eq(*kept.last().unwrap(), *refs.last().unwrap()));
    let lowest = refs.iter().map(|t| t.price).fold(f32::MAX, f32::min);
    assert!(kept.iter().any(|t| t.price == lowest));
}

#[test]
fn an_order_line_starts_where_it_rested_when_the_window_opened() {
    let moves = [(10, 1.0), (20, 2.0), (50, 3.0), (90, 4.0)];
    assert_eq!(clip_line(&moves, 30, 60), vec![(30, 2.0), (50, 3.0)]);
}

#[test]
fn numbers_read_as_written() {
    assert_eq!(format::signed_money(-0.004), "0.00");
    assert_eq!(format::signed_money(3.581), "+3.58");
    assert_eq!(format::signed_money(-150.4), "-150");
    assert_eq!(format::signed_percent(-0.001), "0.00");
    assert_eq!(format::volume(999_960.0), "1.0M");
    assert_eq!(format::volume(5_700.0), "5.7K");
    assert_eq!(format::volume(950.0), "950");
    assert_eq!(format::volume(0.3), "0.300");
    assert_eq!(format::volume(0.0312), "0.0312");
    assert_eq!(format::volume(4.25), "4.25");
    assert_eq!(format::volume(99.96), "100");
    assert_eq!(format::volume(0.9996), "1.00");
    assert_eq!(format::exact_price(64_123.45, 1), "64123.45");
    assert_eq!(format::exact_price(0.00001234, 5), "0.00001234");
    assert_eq!(format::signed_money(99.996), "+100");
    assert_eq!(format::money(99.994), "99.99");
    assert_eq!(format::step_decimals(0.0001), 5);
    assert_eq!(format::step_decimals(0.5), 2);
    assert_eq!(format::exact_price(0.01055, 5), "0.01055");
    assert_eq!(format::exact_price(0.0105, 5), "0.01050");
    assert_eq!(format::exact_price(0.0123456, 4), "0.0123456");
}

#[test]
fn the_caption_and_its_colour_follow_one_rounding() {
    assert_eq!(
        caption(2.17, Some(1.08), "Auto Price Down"),
        "2.17 USDT (+1.08%)  Auto Price Down"
    );
    assert_eq!(caption(-0.004, Some(-0.001), ""), "0.00 USDT (0.00%)");
    assert_eq!(caption(-150.4, None, "Manual"), "-150 USDT  Manual");
    assert!(won(-0.004));
    assert!(!won(-0.006));
}

/// A pump of many times its low keeps the axis above zero, and the range names what was drawn.
#[test]
fn the_axis_stays_above_zero_and_the_range_is_the_markets() {
    let s = PriceScale::fit([0.01, 0.2].into_iter()).unwrap();
    assert!(s.low >= 0.0);
    assert!((s.range_percent() - 1_900.0).abs() < 1e-6);
}

/// Two chips a tick apart stand a chip's height apart, and never leave the plot.
#[test]
fn chips_stay_apart_and_inside() {
    let half = CHIP_HEIGHT / 2.0;
    assert_eq!(chip_place(500.0, Some(510.0)), 510.0 - CHIP_HEIGHT);
    assert_eq!(
        chip_place(PLOT_BOTTOM, Some(PLOT_BOTTOM - half)),
        PLOT_BOTTOM - half - CHIP_HEIGHT
    );
    assert_eq!(chip_place(PLOT_TOP - 100.0, None), PLOT_TOP + half);
    // A price past the edge is first brought inside, then kept off its neighbour.
    let other = PLOT_BOTTOM - half;
    assert_eq!(
        chip_place(PLOT_BOTTOM + 50.0, Some(other)),
        other - CHIP_HEIGHT
    );
}

/// A day-long trade still draws, and its clock names the date.
#[test]
fn a_long_window_draws() {
    assert_eq!(clock_pattern(21_600), "%m-%d %H:%M");
    assert_eq!(clock_pattern(300), "%H:%M");
    assert_eq!(clock_pattern(5), "%H:%M:%S");
    let ticks = tape(500);
    let mut c = chart("ACEUSDT", &ticks, &[]);
    c.window = (ENTRY_MS - 600_000, ENTRY_MS + 2 * 86_400_000);
    c.exit.0 = ENTRY_MS + 2 * 86_400_000 - 3_000;
    assert!(render(&c).is_some());
}
