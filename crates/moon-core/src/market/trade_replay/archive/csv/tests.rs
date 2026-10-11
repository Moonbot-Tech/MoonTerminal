use std::io::Cursor;

use super::*;

/// Dropping the optional header support or maker inversion corrupts futures archive tapes.
#[test]
fn binance_futures_accepts_header_and_headerless_rows() {
    let rows = "2,12.5,3,20,21,2000,false\n1,11.5,2,10,11,1000,true\n";
    let header =
        "agg_trade_id,price,quantity,first_trade_id,last_trade_id,transact_time,is_buyer_maker\n";
    for input in [rows.to_owned(), format!("{header}{rows}")] {
        let mut out = Vec::new();
        assert_eq!(
            parse_binance_agg(Cursor::new(input), &Coverage::one((1000, 2000)), &mut out),
            Ok(2)
        );
        assert_eq!(
            out.iter().map(|t| t.time_ms).collect::<Vec<_>>(),
            vec![1000.0, 2000.0]
        );
        assert_eq!(
            (out[0].price, out[0].qty, out[0].side),
            (11.5, 2.0, Side::Sell)
        );
        assert_eq!(out[1].side, Side::Buy);
    }
}

/// Per-file unit inference loses mixed spot rows; capitalized maker flags must keep taker sides.
#[test]
fn binance_spot_detects_microseconds_per_row() {
    let input = "1,10,2,1,2,1735689600000123,True,True\n2,11,3,3,4,1735689600001,False,True\n";
    let mut out = Vec::new();
    assert_eq!(
        parse_binance_agg(
            Cursor::new(input),
            &Coverage::one((1735689600000, 1735689600001)),
            &mut out
        ),
        Ok(2)
    );
    assert_eq!((out[0].time_ms, out[0].side), (1735689600000.0, Side::Sell));
    assert_eq!((out[1].time_ms, out[1].side), (1735689600001.0, Side::Buy));
}

/// Keeping whole-day or zero-quantity rows would inflate tapes outside the requested holes.
#[test]
fn binance_filters_spans_and_zero_size() {
    let input = "1,10,2,1,1,999,true\n2,10,2,2,2,1000,true\n3,10,0,3,3,1500,false\n4,10,2,4,4,2000,false\n5,10,2,5,5,2001,false\n";
    let mut out = Vec::new();
    assert_eq!(
        parse_binance_agg(Cursor::new(input), &Coverage::one((1000, 2000)), &mut out),
        Ok(2)
    );
    assert_eq!(
        out.iter().map(|t| t.time_ms).collect::<Vec<_>>(),
        vec![1000.0, 2000.0]
    );
}

/// Ignoring bad rows silently claims exhaustive archive coverage while discarding prints.
#[test]
fn binance_malformed_data_is_corrupt_even_outside_spans() {
    for input in [
        "1,bad,2,1,1,1000,true\n",
        "1,10,NaN,1,1,1000,true\n",
        "1,10,2,1,1,1000,maybe\n",
        "1,10,2,1,1,1000\n",
        "agg_trade_id,price,quantity,first_trade_id,last_trade_id,transact_time,is_buyer_maker\nbad,10,2,1,1,1000,true\n",
    ] {
        assert!(matches!(
            parse_binance_agg(Cursor::new(input), &Coverage::none(), &mut Vec::new()),
            Err(ArchiveError::Corrupt(_))
        ));
    }
}

/// Fixed indices or input-order trust would corrupt Bybit archives with reordered or extra columns.
#[test]
fn bybit_header_names_sort_rows_and_prefer_home_notional() {
    for input in [
        "timestamp,symbol,side,size,price,tickDirection,trdMatchID,grossValue,homeNotional,foreignNotional\n2.0006,SYNUSDT,Sell,99,12,MinusTick,b,1,3,36\n1.0004,SYNUSDT,Buy,88,11,PlusTick,a,1,2,22\n",
        "timestamp,symbol,side,size,price,tickDirection,trdMatchID,grossValue,homeNotional,foreignNotional,RPI\n2.0006,SYNUSDT,Sell,99,12,MinusTick,b,1,3,36,0\n1.0004,SYNUSDT,Buy,88,11,PlusTick,a,1,2,22,0\n",
        "side,homeNotional,RPI,price,timestamp,symbol,size\nSell,3,0,12,2.0006,SYNUSDT,99\nBuy,2,0,11,1.0004,SYNUSDT,88\n",
    ] {
        let mut out = Vec::new();
        assert_eq!(
            parse_bybit_trading(Cursor::new(input), &Coverage::one((1000, 2001)), &mut out),
            Ok(2)
        );
        assert_eq!(
            (out[0].time_ms, out[0].price, out[0].qty, out[0].side),
            (1000.0, 11.0, 2.0, Side::Buy)
        );
        assert_eq!(
            (out[1].time_ms, out[1].price, out[1].qty, out[1].side),
            (2001.0, 12.0, 3.0, Side::Sell)
        );
    }
}

/// Removing the size fallback rejects older Bybit headers; filtering still excludes zero/outside rows.
#[test]
fn bybit_size_fallback_filters_missing_spans() {
    let input = "timestamp,side,size,price\n0.999,Buy,4,10\n1,Buy,2,11\n1.5,Sell,0,12\n2,Sell,3,13\n2.001,Sell,5,14\n";
    let mut out = Vec::new();
    assert_eq!(
        parse_bybit_trading(Cursor::new(input), &Coverage::one((1000, 2000)), &mut out),
        Ok(2)
    );
    assert_eq!(
        out.iter().map(|t| (t.time_ms, t.qty)).collect::<Vec<_>>(),
        vec![(1000.0, 2.0), (2000.0, 3.0)]
    );
}

/// Silently accepting truncated rows or invalid times turns malformed Bybit files into held tape.
#[test]
fn bybit_malformed_rows_and_missing_columns_are_corrupt() {
    for input in [
        "timestamp,side,size\n1,Buy,2\n",
        "timestamp,side,size,price\nNaN,Buy,2,10\n",
        "timestamp,side,size,price\n1,Unknown,2,10\n",
        "timestamp,side,size,price\n1,Buy,2\n",
        "timestamp,side,size,price,homeNotional\n1,Buy,2,10,bad\n",
    ] {
        assert!(matches!(
            parse_bybit_trading(
                Cursor::new(input),
                &Coverage::one((0, 3000)),
                &mut Vec::new()
            ),
            Err(ArchiveError::Corrupt(_))
        ));
    }
}

/// BOMs and blank tails must not poison a day, and sorting must preserve earlier harvests.
#[test]
fn bom_blank_lines_and_append_sort_preserve_previous_ticks() {
    let prior = Tick {
        time_ms: 9000.0,
        price: 1.0,
        qty: 1.0,
        side: Side::Buy,
    };
    for bybit in [false, true] {
        let input = if bybit {
            "\u{feff}timestamp,side,size,price\n2,Sell,3,12\n1,Buy,2,11\n \n\n"
        } else {
            "\u{feff}aggId,price,qty,firstId,lastId,timestamp,isBuyerMaker\n2,12,3,2,2,2000,false\n1,11,2,1,1,1000,true\n \n\n"
        };
        let mut out = vec![prior];
        let spans = Coverage::one((1000, 2000));
        let result = if bybit {
            parse_bybit_trading(Cursor::new(input), &spans, &mut out)
        } else {
            parse_binance_agg(Cursor::new(input), &spans, &mut out)
        };
        assert_eq!(result, Ok(2));
        assert_eq!(
            out.iter().map(|tick| tick.time_ms).collect::<Vec<_>>(),
            vec![9000.0, 1000.0, 2000.0]
        );
    }
}
