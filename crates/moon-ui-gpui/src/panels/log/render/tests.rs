// Explicit imports on purpose: `use super::*` would pull in the parent's `gpui::*`
// re-export, whose `test` shadows the built-in attribute and makes `#[test]` expand
// recursively ("recursion limit reached").
use super::{LineView, classify};
use moon_core::applog::LogLine;
use std::collections::HashSet;

/// A multi-line message is flattened before its coin range is computed.
#[test]
fn multiline_message_flattens_and_keeps_the_coin_range_valid() {
    let line = LogLine::core(0, "SPK order failed\r\n  at retry 2".to_string());
    let known = HashSet::from(["SPK".to_string()]);
    let view = LineView::from_parts(&line, classify(&line), &known);

    assert!(
        !view.flat.contains('\n') && !view.flat.contains('\r'),
        "raw break survived into the log row: {:?}",
        view.flat
    );
    let (range, base) = view
        .coin
        .expect("SPK is a known base and should be detected");
    assert_eq!(&view.flat[range], "SPK");
    assert_eq!(base, "SPK");
}
