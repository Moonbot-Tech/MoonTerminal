use super::*;

/// A trade as the retained ring holds one: a SELL is spelled by the quantity's sign bit.
fn trade(ms: i64, price: f32, qty: f32) -> TradeHistoryRow {
    TradeHistoryRow {
        time: MoonTime::from_unix_millis(ms),
        price,
        qty,
    }
}

/// A five-second aggregate, which carries value and no quantity.
fn mini(ms: i64, buy: f32, sell: f32, cnt: i32) -> MiniCandle {
    MiniCandle {
        time: MoonTime::from_unix_millis(ms),
        cnt,
        min_price: 1.0,
        max_price: 2.0,
        buy_vol: buy,
        sell_vol: sell,
    }
}

/// A sale must land on the selling side, and its amount must be positive.
///
/// Breakage: the ring spells a sale as a NEGATIVE quantity, so folding the raw value in would
/// subtract from `Sv` — a chart that sold ten thousand would print a negative sell volume, and the
/// bar beside it would run backwards.
#[test]
fn a_sale_is_counted_on_the_selling_side_as_a_positive_amount() {
    let mut out = VolumeSpanReadout {
        base_exact: true,
        ..VolumeSpanReadout::default()
    };
    out.add_trade(trade(0, 100.0, 2.0));
    out.add_trade(trade(1, 100.0, -3.0));

    assert_eq!(out.buy_quote, 200.0);
    assert_eq!(out.sell_quote, 300.0);
    assert_eq!(out.buy_base, 2.0);
    assert_eq!(out.sell_base, 3.0);
    assert_eq!(out.trades, 2);
    assert!(out.base_exact);
}

/// The total and the share must be derived from the same two halves.
///
/// Breakage: this is the whole reason traded amounts moved out of the window readout — a total from
/// one source and halves from another let `Bv + Sv` differ from `Vol` on the same caption block.
#[test]
fn the_total_and_the_share_come_from_the_halves() {
    let mut out = VolumeSpanReadout::default();
    out.add_trade(trade(0, 10.0, 3.0));
    out.add_trade(trade(1, 10.0, -1.0));

    assert_eq!(out.total_quote(), out.buy_quote + out.sell_quote);
    assert_eq!(out.buy_share_pct(), Some(75.0));
}

/// A market that has not traded has no share to state.
///
/// Breakage: dividing by a zero total yields NaN, and a NaN reaching the caption cache makes the
/// configuration stop equalling itself — every revision then re-formats and repaints the pane.
#[test]
fn a_silent_market_states_no_share() {
    assert_eq!(VolumeSpanReadout::default().buy_share_pct(), None);
}

/// A mini-candle contributes value but leaves the coin amount unknown.
///
/// Breakage: a mini-candle holds `price × quantity` only. Treating that as a quantity would state a
/// coin figure inflated by the price — thousands of coins where there were units — and nothing
/// downstream could tell it was invented.
#[test]
fn a_mini_candle_marks_the_coin_amount_inexact() {
    let mut out = VolumeSpanReadout {
        base_exact: true,
        ..VolumeSpanReadout::default()
    };
    out.add_trade(trade(0, 100.0, 1.0));
    out.add_mini(mini(0, 500.0, 250.0, 7));

    assert_eq!(out.buy_quote, 600.0);
    assert_eq!(out.sell_quote, 250.0);
    // The trade's own coin amount survives; the aggregate adds none.
    assert_eq!(out.buy_base, 1.0);
    assert_eq!(out.trades, 8);
    assert!(!out.base_exact);
}

/// A hand-built empty span asks a question the history cannot answer.
#[test]
fn an_empty_span_is_refused() {
    assert!(!VolumeSpan::Millis(0).is_useful());
    assert!(!VolumeSpan::Trades(0).is_useful());
    assert!(VolumeSpan::Millis(60_000).is_useful());
    assert!(VolumeSpan::Trades(1).is_useful());
}

/// The cache must not grow by one entry per coin ever charted.
///
/// Breakage: without pruning this is a leak that looks like a cache — a terminal that walks a
/// hundred coins an hour keeps every one of them, under every span it was ever read with.
#[test]
fn stale_entries_are_pruned_and_empty_spans_drop_out() {
    let mut book = VolumeBook::default();
    let span = VolumeSpan::Millis(60_000);
    let entry = |ms: i64| SpanEntry {
        read_ms: ms,
        readout: VolumeSpanReadout::default(),
    };
    let markets = book.spans.entry((1, span)).or_default();
    markets.insert("OLDUSDT".to_string(), entry(0));
    markets.insert("NEWUSDT".to_string(), entry(SPAN_KEEP_MS));

    book.prune(SPAN_KEEP_MS + 1);

    let markets = book.spans.get(&(1, span)).expect("span kept");
    assert!(!markets.contains_key("OLDUSDT"));
    assert!(markets.contains_key("NEWUSDT"));

    book.prune(SPAN_KEEP_MS * 4);
    assert!(book.spans.is_empty(), "a span with no markets left must go");
}

/// Replacing a client must take its figures with it.
///
/// Breakage: the entries describe retained history that belongs to the client slot that just went
/// away; keeping them would caption a freshly reconnected core with the previous session's volume
/// for as long as the TTL lasts.
#[test]
fn forgetting_a_core_drops_only_its_own_entries() {
    let mut book = VolumeBook::default();
    let span = VolumeSpan::Trades(500);
    for core in [1, 2] {
        book.spans.entry((core, span)).or_default().insert(
            "BTCUSDT".to_string(),
            SpanEntry {
                read_ms: 0,
                readout: VolumeSpanReadout::default(),
            },
        );
    }

    book.forget_core(1);

    assert!(!book.spans.contains_key(&(1, span)));
    assert!(book.spans.contains_key(&(2, span)));
}

/// A period the sides could not cover still prints a WHOLE total when the candle ring states one.
///
/// Breakage: the sides come from the mini-candles, which reach some thirty hours; the 5-minute
/// candle ring reaches a day and more and is where `Vol 24ч` came from before the sides existed.
/// Without this the day's volume became a marked fraction of itself — a figure the terminal has,
/// printed as one it does not.
#[test]
fn the_deep_total_answers_where_the_sides_fall_short() {
    let partial = VolumeSpanReadout {
        buy_quote: 300.0,
        sell_quote: 200.0,
        complete: false,
        total_quote_candles: Some(9_000.0),
        ..VolumeSpanReadout::default()
    };
    assert_eq!(partial.total_quote_stated(), (9_000.0, true));
    // The SIDES stay incomplete: the candle ring carries no split, so it cannot vouch for them.
    assert_eq!(partial.total_quote(), 500.0);
    assert!(!partial.complete);

    // Covered by the sides themselves: their own sum answers, and the candles are not consulted.
    let whole = VolumeSpanReadout {
        buy_quote: 300.0,
        sell_quote: 200.0,
        complete: true,
        total_quote_candles: Some(9_000.0),
        ..VolumeSpanReadout::default()
    };
    assert_eq!(whole.total_quote_stated(), (500.0, true));

    // Nothing deeper to ask: a partial sum is stated as partial rather than dressed up.
    let bare = VolumeSpanReadout {
        buy_quote: 300.0,
        sell_quote: 200.0,
        complete: false,
        ..VolumeSpanReadout::default()
    };
    assert_eq!(bare.total_quote_stated(), (500.0, false));
}
