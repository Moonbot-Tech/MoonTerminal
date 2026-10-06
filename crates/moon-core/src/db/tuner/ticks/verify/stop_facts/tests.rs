use super::*;
use crate::db::tuner::ticks::tests::deal;

/// A print on the tape.
fn print(t_ms: i64, price: f64, side: Side) -> Tick {
    Tick {
        time_ms: t_ms as f64,
        price: price as f32,
        qty: 1.0,
        side,
    }
}

fn stop_params() -> ExitParams {
    ExitParams {
        stop_loss_pct: -3.0,
        ..ExitParams::default()
    }
}

/// A short the core stopped above its buy, as ALLINU 2026-09-22 printed it: the level between the
/// BID and the ASK.
fn short_stopped(reason: &str) -> Deal {
    Deal {
        is_short: true,
        buy_price: 0.024_513,
        close_ms: 20_000,
        sell_reason: reason.into(),
        ..deal()
    }
}

const ALLINU: &str = "StopLoss AutoActivated on price drop: BID = 0.025326 ASK: 0.025999 \
                      BidEMA: 0.025326 LastPrice = 0.025663 (strategy <S>); StopLoss fixed: 0.025334 ;";

#[test]
fn a_short_stopped_on_its_ask_alone_reads_as_the_stop_side() {
    let facts = quote_facts(&short_stopped(ALLINU), &[], &stop_params(), None);
    // Only the ASK is past a short's level: had the side been read as a long's, this would be
    // the other side's quote.
    assert_eq!(facts.quote, Some(QuoteSide::StopSideOnly));
}

#[test]
fn a_long_reads_its_bid_as_the_stop_side() {
    let long = Deal {
        buy_price: 100.0,
        sell_reason: "StopLoss AutoActivated on price drop: BID = 96.9 ASK: 97.2 BidEMA: 96.9 \
                      (strategy <S>); StopLoss fixed: 97.00 ;"
            .into(),
        ..deal()
    };
    let facts = quote_facts(&long, &[], &stop_params(), None);
    assert_eq!(facts.quote, Some(QuoteSide::StopSideOnly));
}

#[test]
fn a_quote_cut_off_by_the_stored_reason_is_not_read() {
    let cut = short_stopped("StopLoss AutoActivated on price drop: BID = 0.025326 ASK: 0.0259");
    assert_eq!(
        quote_facts(&cut, &[], &stop_params(), None),
        StopFacts::default()
    );
}

#[test]
fn a_trailing_stop_carries_no_quote() {
    let trailing = short_stopped("TrailingStop PeakPrice = 0.0240; BID = 0.025 ASK: 0.026 ;");
    assert_eq!(
        quote_facts(&trailing, &[], &stop_params(), None),
        StopFacts::default()
    );
}

/// The core's archived Exit line: the take, then the panic sell's jump past the short's level
/// at 20 s.
const JUMPED: [(i64, f64); 2] = [(12_000, 0.024), (20_000, 0.0262)];

#[test]
fn the_ticker_age_is_the_last_stop_side_print_at_the_quote() {
    let ticks = [
        // The short's own side (a taker buy at the ASK) at the quote: 5 s before the close.
        print(15_000, 0.025_999, Side::Buy),
        // At the quote, but on the other side.
        print(18_000, 0.025_999, Side::Sell),
        // On the stop's side, but not at the quote.
        print(19_000, 0.027, Side::Buy),
        // After the activation: the ticker could not have shown it.
        print(21_000, 0.025_999, Side::Buy),
    ];
    let facts = quote_facts(
        &short_stopped(ALLINU),
        &ticks,
        &stop_params(),
        Some(&JUMPED),
    );
    assert_eq!(facts.ticker_age_ms, Some(5_000));
    // Without the archived jump the close is all there is, and it trails the activation by the
    // sale: no age.
    let facts = quote_facts(&short_stopped(ALLINU), &ticks, &stop_params(), None);
    assert_eq!(facts.ticker_age_ms, None);
    assert_eq!(facts.quote, Some(QuoteSide::StopSideOnly));
}

#[test]
fn a_hole_in_the_lookback_leaves_the_age_unknown() {
    let ticks = [print(15_000, 0.025_999, Side::Buy)];
    let holed = Deal {
        gap: Some(TapeGap {
            from_ms: 16_000,
            to_ms: 18_000,
            fact_line: None,
            fact_stop: None,
        }),
        ..short_stopped(ALLINU)
    };
    let facts = quote_facts(&holed, &ticks, &stop_params(), Some(&JUMPED));
    assert_eq!(facts.ticker_age_ms, None);
    // A hole that ended before the lookback began says nothing about it.
    let earlier = Deal {
        gap: Some(TapeGap {
            from_ms: 1_000,
            to_ms: 20_000 - QUOTE_LOOKBACK_MS,
            fact_line: None,
            fact_stop: None,
        }),
        ..short_stopped(ALLINU)
    };
    let facts = quote_facts(&earlier, &ticks, &stop_params(), Some(&JUMPED));
    assert_eq!(facts.ticker_age_ms, Some(5_000));
}

#[test]
fn no_matching_print_within_the_lookback_leaves_the_age_unknown() {
    let ticks = [print(20_000 - QUOTE_LOOKBACK_MS - 1, 0.025_999, Side::Buy)];
    let facts = quote_facts(
        &short_stopped(ALLINU),
        &ticks,
        &stop_params(),
        Some(&JUMPED),
    );
    assert_eq!(facts.ticker_age_ms, None);
    assert_eq!(facts.quote, Some(QuoteSide::StopSideOnly));
}
