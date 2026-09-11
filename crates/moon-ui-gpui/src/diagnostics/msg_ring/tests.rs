use super::*;

fn rec(i: u64) -> Record {
    Record {
        at_ms: i,
        hwnd: 0xA0A90,
        msg: u32::try_from(i).unwrap(),
        wparam: 1,
        lparam: -1,
    }
}

#[test]
fn newest_first_and_bounded_by_what_was_written() {
    let ring: Ring<4> = Ring::new();
    assert!(ring.newest(8).is_empty());
    ring.record(rec(1));
    ring.record(rec(2));
    let got = ring.newest(8);
    assert_eq!(got.iter().map(|r| r.at_ms).collect::<Vec<_>>(), [2, 1]);
}

#[test]
fn wraps_and_keeps_only_the_last_n() {
    let ring: Ring<4> = Ring::new();
    for i in 1..=10 {
        ring.record(rec(i));
    }
    let got = ring.newest(8);
    assert_eq!(
        got.iter().map(|r| r.at_ms).collect::<Vec<_>>(),
        [10, 9, 8, 7],
        "a ring of four holds the last four, newest first"
    );
    assert_eq!(
        ring.newest(2).len(),
        2,
        "the limit trims the newest, not the oldest"
    );
    assert_eq!(ring.newest(2)[0].at_ms, 10);
}

#[test]
fn a_record_survives_the_round_trip_intact() {
    let ring: Ring<2> = Ring::new();
    let r = Record {
        at_ms: 77,
        hwnd: 0x50A82,
        msg: 0x003D,
        wparam: usize::MAX,
        lparam: isize::MIN,
    };
    ring.record(r);
    assert_eq!(ring.newest(1), [r]);
}

#[test]
fn format_prints_age_before_the_crash_and_the_message_name() {
    let line = format(&[rec(1_000)], 1_234);
    assert!(
        line.starts_with("  -234ms hwnd=0xA0A90 msg=0x03E8"),
        "{line}"
    );
    let getobject = format(
        &[Record {
            at_ms: 5,
            hwnd: 1,
            msg: 0x003D,
            wparam: 0,
            lparam: -4,
        }],
        5,
    );
    assert!(getobject.contains("WM_GETOBJECT"), "{getobject}");
    assert!(getobject.contains("-0ms"), "{getobject}");
    // A torn read can carry a timestamp past `now`; that is an age of zero, never a wrap-around.
    let future = format(&[rec(9_999)], 5);
    assert!(future.contains("-0ms"), "{future}");
}

#[test]
fn format_says_when_the_ring_is_empty() {
    assert_eq!(format(&[], 0), "  (no messages recorded)\n");
}

#[test]
fn message_names_cover_the_gpui_user_range_and_fall_back_to_none() {
    assert_eq!(message_name(0x0407), Some("WM_USER+7"));
    assert_eq!(message_name(0x0400 + 40), Some("WM_USER+n"));
    assert_eq!(message_name(0x8001), Some("WM_APP+n"));
    assert_eq!(message_name(0x0003), None);
}

#[test]
fn report_keeps_a_rare_message_the_routine_ones_pushed_past_the_verbatim_lines() {
    // Newest first: 32 frame-clock posts, then one WM_GETOBJECT, then more routine noise.
    let mut newest_first: Vec<Record> = (0..REPORT_LINES as u64)
        .map(|i| Record {
            at_ms: 1_000 - i,
            hwnd: 1,
            msg: 0x0409,
            wparam: 0,
            lparam: 0,
        })
        .collect();
    newest_first.push(Record {
        at_ms: 900,
        hwnd: 2,
        msg: 0x003D,
        wparam: 0,
        lparam: -4,
    });
    newest_first.extend((0..10).map(|i| Record {
        at_ms: 800 - i,
        hwnd: 1,
        msg: 0x0200,
        wparam: 0,
        lparam: 0,
    }));
    let out = report_from(&newest_first, 1_000);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), REPORT_LINES + 2, "{out}");
    assert!(
        lines[REPORT_LINES].contains("older, routine messages omitted"),
        "{out}"
    );
    assert!(
        lines[REPORT_LINES + 1].contains("-100ms hwnd=0x2 msg=0x003D WM_GETOBJECT"),
        "{out}"
    );
    assert!(
        !out.contains("WM_MOUSEMOVE"),
        "routine noise past the verbatim lines is dropped"
    );
}

#[test]
fn report_without_notable_older_messages_has_no_second_section() {
    let newest_first: Vec<Record> = (0..40u64)
        .map(|i| Record {
            at_ms: i,
            hwnd: 1,
            msg: 0x0403,
            wparam: 0,
            lparam: 0,
        })
        .collect();
    let out = report_from(&newest_first, 40);
    assert_eq!(out.lines().count(), REPORT_LINES, "{out}");
    assert!(is_routine(0x0403) && is_routine(0x0409) && !is_routine(0x0407));
}
