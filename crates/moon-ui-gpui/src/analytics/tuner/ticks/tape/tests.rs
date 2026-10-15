use super::*;

fn tick(time_ms: f64, price: f32, qty: f32, side: Side) -> Tick {
    Tick {
        time_ms,
        price,
        qty,
        side,
    }
}

/// What the model reads of a print — time, price, side — comes back unchanged.
fn as_read(t: &Tick) -> (f64, f32, bool) {
    (t.time_ms, t.price, matches!(t.side, Side::Sell))
}

#[test]
fn a_tape_packs_to_eight_bytes_a_print_and_comes_back_as_the_model_reads_it() {
    let ticks = vec![
        tick(1_790_129_435_428.0, 2.799, 12.0, Side::Sell),
        tick(1_790_129_435_428.0, 2.799, 3.0, Side::Buy),
        tick(1_790_129_445_561.0, 2.580_848_7, 7.5, Side::Sell),
        tick(1_790_129_805_561.0, 0.000_012_34, 1.0, Side::Buy),
    ];
    let tape = PackedTape::pack(ticks.clone());
    assert!(matches!(tape, PackedTape::Packed { .. }));
    assert_eq!(tape.len(), 4);
    assert_eq!(tape.bytes(), 4 * 8);
    let back = tape.unpack();
    assert_eq!(
        back.iter().map(as_read).collect::<Vec<_>>(),
        ticks.iter().map(as_read).collect::<Vec<_>>()
    );
    assert!(
        back.iter().all(|t| t.qty == 0.0),
        "the quantity is not kept"
    );
}

#[test]
fn a_tape_that_would_lose_something_is_kept_as_it_came() {
    let plain = |ticks: Vec<Tick>| {
        let tape = PackedTape::pack(ticks.clone());
        assert!(matches!(tape, PackedTape::Plain(_)), "{ticks:?}");
        assert_eq!(tape.bytes(), ticks.len() * std::mem::size_of::<Tick>());
        let full = |t: &Tick| (as_read(t), t.qty);
        assert_eq!(
            tape.unpack().iter().map(full).collect::<Vec<_>>(),
            ticks.iter().map(full).collect::<Vec<_>>()
        );
    };
    // A fractional millisecond.
    plain(vec![
        tick(1_000.0, 1.0, 1.0, Side::Buy),
        tick(1_000.5, 1.0, 1.0, Side::Buy),
    ]);
    // A span past a u32 of milliseconds.
    plain(vec![
        tick(0.0, 1.0, 1.0, Side::Buy),
        tick(f64::from(u32::MAX) + 1.0, 1.0, 1.0, Side::Buy),
    ]);
    // A print before the first: its offset would be negative. (Order past the first print is
    // kept as it came either way; the worker answers sorted.)
    plain(vec![
        tick(2_000.0, 1.0, 1.0, Side::Buy),
        tick(1_000.0, 1.0, 1.0, Side::Buy),
    ]);
    // A price the sign bit cannot carry.
    plain(vec![tick(1_000.0, 0.0, 1.0, Side::Sell)]);
}

#[test]
fn an_empty_tape_holds_nothing() {
    let tape = PackedTape::pack(Vec::new());
    assert_eq!(tape.len(), 0);
    assert_eq!(tape.bytes(), 0);
    assert!(tape.unpack().is_empty());
}

/// A shared cutoff in sample preparation would lose the long row's answered 90 s exit.
/// Preparing the trade picture alone must use the same trail as preparing variant columns.
#[test]
fn samples_and_single_pictures_keep_each_rows_own_exit_horizon() {
    use moon_core::db::tuner::ticks::Deltas;
    use moon_core::db::tuner::ticks::ModelSettings;
    use moon_core::db::tuner::ticks::search::variant_tally;

    let pending = |uid, trail_ms| PendingDeal {
        deal: Deal {
            report_uid: uid,
            core_uid: 1,
            core_name: "CORE_A".into(),
            strategy_id: 1,
            kind: "PumpsDetection".into(),
            coin: "XALPHA".into(),
            buy_ms: 1_000,
            close_ms: 2_000,
            buy_price: 100.0,
            sell_price: 100.2,
            spent: 1_000.0,
            is_short: false,
            sell_reason: String::new(),
            fact_pnl: 2.0,
            sizing: None,
            pnl_pct: false,
            profit: None,
            deltas: Deltas::default(),
            tick: None,
            pre_spike_ask: None,
            archived_take: None,
            fact_modifier: None,
            hook_depth_pct: None,
            hook_stated_take_pct: None,
            strike_detect: None,
            step_lag_ms: 0.0,
            round_trip_ms: None,
            stop_anchor: None,
            delta_track: None,
            bars: None,
            own_entry: None,
            buy_set_ms: None,
            corridor: None,
            entry_placed: None,
            gap: None,
        },
        tape: PackedTape::pack(vec![
            tick(1_000.0, 100.0, 1.0, Side::Buy),
            tick(2_000.0, 100.2, 1.0, Side::Buy),
            tick(62_000.0, 100.2, 1.0, Side::Buy),
            tick(92_000.0, 101.0, 1.0, Side::Buy),
            tick(182_000.0, 101.0, 1.0, Side::Buy),
            tick(182_001.0, 102.0, 1.0, Side::Buy),
        ]),
        entry_line: None,
        trail_ms,
        own: Arc::new(HashMap::new()),
    };
    let short = pending(1, 60_000);
    let long = pending(2, 180_000);
    let picture = long.clone().prepare();
    let deals = prepare_sample(vec![short, long]);
    assert_eq!(deals[0].ticks.last().expect("short tape").time_ms, 62_000.0);
    assert_eq!(deals[1].ticks.last().expect("long tape").time_ms, 182_000.0);
    assert_eq!(picture.ticks.len(), deals[1].ticks.len());
    let score = variant_tally(
        &deals,
        &HashMap::new(),
        "PumpsDetection",
        &[("SellPrice".into(), "1".into())],
        ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
    );
    assert_eq!((score.tally.n, score.open), (1, 1));
}
