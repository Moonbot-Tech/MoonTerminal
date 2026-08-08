// Explicit imports on purpose: `use super::*` would pull in the parent's `gpui::*` re-export,
// whose `test` shadows the built-in attribute and makes `#[test]` expand recursively
// ("recursion limit reached").
use super::{Disturbance, Filters, RowBuffer};
use moon_core::applog::LogLine;

/// A line as a core writes one: no level, the stamp decides its place.
fn line(ts: &str, msg: &str) -> LogLine {
    LogLine {
        ts: ts.to_string(),
        level: log::Level::Info,
        target: String::new(),
        msg: msg.to_string(),
    }
}

/// A batch of `(ts, msg)` pairs.
fn batch(rows: &[(&str, &str)]) -> Vec<LogLine> {
    rows.iter().map(|(ts, msg)| line(ts, msg)).collect()
}

const ALL: Filters<'static> = Filters {
    errors_only: false,
    query: "",
    coin: None,
};

const ERRORS: Filters<'static> = Filters {
    errors_only: true,
    query: "",
    coin: None,
};

/// Messages currently visible, in display order.
fn shown(buf: &RowBuffer) -> Vec<&str> {
    (0..buf.visible())
        .filter_map(|ix| buf.at(ix))
        .map(|row| row.flat.as_str())
        .collect()
}

/// Ordinary arrival: rows land at the end and nothing already on screen moves.
#[test]
fn in_order_arrivals_only_append() {
    let mut buf = RowBuffer::default();
    buf.ingest(batch(&[("01", "a"), ("02", "b")]), 10, ALL);

    let moved = buf.ingest(batch(&[("03", "c")]), 10, ALL);

    assert_eq!(moved, Disturbance::AppendedOnly);
    assert_eq!(shown(&buf), ["a", "b", "c"]);
}

/// A row stamped earlier than the buffer's tail belongs ABOVE it, and the buffer stays sorted.
///
/// This is not an exotic case: cores stamp their own lines and their clocks disagree, so on the
/// aggregate source most batches carry rows that are already "late".
#[test]
fn a_late_row_is_spliced_into_its_place() {
    let mut buf = RowBuffer::default();
    buf.ingest(batch(&[("01", "a"), ("03", "c")]), 10, ALL);

    buf.ingest(batch(&[("02", "b"), ("04", "d")]), 10, ALL);

    assert_eq!(shown(&buf), ["a", "b", "c", "d"]);
}

/// Splicing above visible rows must be REPORTED, or a held selection silently slides onto lines the
/// user never picked and Ctrl+C copies them.
#[test]
fn a_splice_above_visible_rows_reports_movement() {
    let mut buf = RowBuffer::default();
    buf.ingest(batch(&[("05", "a"), ("06", "b")]), 10, ALL);

    assert_eq!(
        buf.ingest(batch(&[("01", "old")]), 10, ALL),
        Disturbance::Moved { from: 0 }
    );
}

/// Rows equal to the tail's timestamp go after it: an arrival never reorders what is on screen.
#[test]
fn an_equal_timestamp_does_not_count_as_movement() {
    let mut buf = RowBuffer::default();
    buf.ingest(batch(&[("01", "a"), ("02", "b")]), 10, ALL);

    let moved = buf.ingest(batch(&[("02", "same")]), 10, ALL);

    assert_eq!(moved, Disturbance::AppendedOnly);
    assert_eq!(shown(&buf), ["a", "b", "same"]);
}

/// Eviction rebases the visible list by arithmetic; the rows on screen stay the rows on screen.
///
/// A busy source sits at its cap, so this runs on every revision — re-running the filters over the
/// whole buffer here is exactly the per-revision full pass the incremental path exists to avoid.
#[test]
fn eviction_rebases_the_visible_list() {
    let mut buf = RowBuffer::default();
    buf.ingest(batch(&[("01", "a"), ("02", "b"), ("03", "c")]), 3, ALL);

    let moved = buf.ingest(batch(&[("04", "d"), ("05", "e")]), 3, ALL);

    assert_eq!(moved, Disturbance::Moved { from: 0 });
    assert_eq!(buf.total(), 3);
    assert_eq!(shown(&buf), ["c", "d", "e"]);
}

/// Eviction under a filter keeps the visible list pointing at the right rows.
///
/// The failure this catches is off-by-`dropped`: indices that survive the trim but are not rebased
/// address whatever now sits at that position.
#[test]
fn eviction_keeps_filtered_positions_correct() {
    let mut buf = RowBuffer::default();
    buf.ingest(
        batch(&[
            ("01", "plain one"),
            ("02", "error two"),
            ("03", "plain three"),
            ("04", "error four"),
        ]),
        4,
        ERRORS,
    );
    assert_eq!(shown(&buf), ["error two", "error four"]);

    buf.ingest(
        batch(&[("05", "error five"), ("06", "plain six")]),
        3,
        ERRORS,
    );

    assert_eq!(buf.total(), 3, "the cap holds");
    assert_eq!(
        shown(&buf),
        ["error four", "error five"],
        "the evicted error must go and the survivors must still be themselves"
    );
}

/// Lowering the cap with nothing new arriving still evicts — that is what resuming follow does.
#[test]
fn an_empty_ingest_still_applies_the_cap() {
    let mut buf = RowBuffer::default();
    buf.ingest(batch(&[("01", "a"), ("02", "b"), ("03", "c")]), 10, ALL);

    let moved = buf.ingest(Vec::new(), 2, ALL);

    assert_eq!(moved, Disturbance::Moved { from: 0 });
    assert_eq!(shown(&buf), ["b", "c"]);
}

/// A batch larger than the cap is trimmed BEFORE parsing; only the newest survive.
#[test]
fn an_oversized_batch_is_trimmed_before_parsing() {
    let mut buf = RowBuffer::default();

    buf.ingest(
        batch(&[("01", "a"), ("02", "b"), ("03", "c"), ("04", "d")]),
        2,
        ALL,
    );

    assert_eq!(shown(&buf), ["c", "d"]);
}

/// Filters decide what is visible, never what is held; the total keeps counting everything.
#[test]
fn filtering_hides_rows_without_dropping_them() {
    let mut buf = RowBuffer::default();

    buf.ingest(
        batch(&[("01", "just info"), ("02", "an error here")]),
        10,
        ERRORS,
    );

    assert_eq!(shown(&buf), ["an error here"]);
    assert_eq!(buf.total(), 2, "a hidden row is still buffered");
}

/// Refiltering re-runs the predicate over rows already parsed, in both directions.
#[test]
fn refilter_reconsiders_every_buffered_row() {
    let mut buf = RowBuffer::default();
    buf.ingest(
        batch(&[("01", "just info"), ("02", "an error here")]),
        10,
        ERRORS,
    );

    buf.refilter(ALL);
    assert_eq!(shown(&buf), ["just info", "an error here"]);

    buf.refilter(ERRORS);
    assert_eq!(shown(&buf), ["an error here"]);
}

/// A query matches the message or the source name, case-insensitively.
#[test]
fn a_query_matches_message_or_source() {
    let mut buf = RowBuffer::default();
    let mut rows = batch(&[("01", "nothing to see"), ("02", "MATCHED here")]);
    rows[0].target = "AlphaCore".to_string();
    buf.ingest(rows, 10, ALL);

    buf.refilter(Filters {
        errors_only: false,
        query: "matched",
        coin: None,
    });
    assert_eq!(shown(&buf), ["MATCHED here"]);

    buf.refilter(Filters {
        errors_only: false,
        query: "alphacore",
        coin: None,
    });
    assert_eq!(
        shown(&buf),
        ["nothing to see"],
        "the source column matches too"
    );
}

/// A coin named outright by one line lights up its bare mentions in the SAME batch.
#[test]
fn a_batch_learns_its_own_coin_names() {
    let mut buf = RowBuffer::default();

    buf.ingest(
        batch(&[
            ("01", "12:27:47  1kRATS: [1] (48186) Auto cancel buy order"),
            ("02", "Srv: NewOrder worker created: Market=1kRATS"),
        ]),
        10,
        ALL,
    );

    let bare = buf.at(1).expect("both rows are visible");
    assert_eq!(
        bare.coin
            .as_ref()
            .map(|(range, _)| &bare.flat[range.clone()]),
        Some("1kRATS"),
        "a name stated by an earlier line in the batch must be recognised"
    );
}

/// Clearing drops the rows AND what was learned from them, so the next source starts clean.
#[test]
fn clearing_forgets_the_source() {
    let mut buf = RowBuffer::default();
    buf.ingest(batch(&[("01", "a very wide row indeed")]), 10, ALL);
    assert!(buf.widest_chars() > 0);

    buf.clear();

    assert_eq!(buf.total(), 0);
    assert_eq!(buf.visible(), 0);
    assert_eq!(
        buf.widest_chars(),
        0,
        "a narrow source must not inherit the previous one's scroll width"
    );
}

/// The buffer sorts what it is given: a source handing over unordered lines must not corrupt it.
///
/// Sortedness is what every later splice trusts, so establishing it here rather than trusting the
/// caller is the difference between one bad batch and a buffer that is wrong from then on.
#[test]
fn an_unsorted_batch_is_ordered_on_the_way_in() {
    let mut buf = RowBuffer::default();

    buf.ingest(batch(&[("03", "c"), ("01", "a"), ("02", "b")]), 10, ALL);
    buf.ingest(batch(&[("05", "e"), ("04", "d")]), 10, ALL);

    assert_eq!(shown(&buf), ["a", "b", "c", "d", "e"]);
}

/// Movement is reported as the FIRST position affected, not as a bare flag.
///
/// On a multi-core source rows arrive late on most batches. A flag would clear the user's selection
/// several times a second; the position lets a selection above the splice survive.
#[test]
fn movement_reports_the_first_affected_position() {
    let mut buf = RowBuffer::default();
    buf.ingest(
        batch(&[("01", "a"), ("02", "b"), ("05", "c"), ("06", "d")]),
        10,
        ALL,
    );

    // Lands between "b" and "c": positions 0 and 1 keep addressing the rows they did.
    let moved = buf.ingest(batch(&[("03", "late")]), 10, ALL);

    assert_eq!(moved, Disturbance::Moved { from: 2 });
    assert_eq!(shown(&buf), ["a", "b", "late", "c", "d"]);
}

/// Evicting rows that were filtered out anyway leaves the visible list alone — and the selection.
#[test]
fn evicting_hidden_rows_does_not_disturb_the_visible_list() {
    let mut buf = RowBuffer::default();
    buf.ingest(
        batch(&[
            ("01", "plain one"),
            ("02", "plain two"),
            ("03", "an error here"),
        ]),
        3,
        ERRORS,
    );
    assert_eq!(shown(&buf), ["an error here"]);

    // "plain one" is evicted; nothing visible went with it.
    let moved = buf.ingest(batch(&[("04", "plain four")]), 3, ERRORS);

    assert_eq!(moved, Disturbance::AppendedOnly);
    assert_eq!(shown(&buf), ["an error here"]);
}
