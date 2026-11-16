use std::io::Cursor;

use super::*;

/// A stored day with one line per second from midnight, `lines` long.
fn day(lines: usize) -> Vec<u8> {
    (0..lines)
        .map(|at| {
            format!(
                "{:02}:{:02}:{:02}.000\tINFO\t\tline-{at}\n",
                at / 3600,
                at / 60 % 60,
                at % 60
            )
        })
        .collect::<String>()
        .into_bytes()
}

/// The window read through the index, as line numbers taken from the message text.
fn window(bytes: &[u8], first: usize, max: usize) -> Vec<String> {
    let index = build_index(Cursor::new(bytes));
    read_window(Cursor::new(bytes), &index, first, max)
        .into_iter()
        .map(|line| line.rsplit('\t').next().unwrap().to_string())
        .collect()
}

/// Line found for a typed time, on a day of `lines` one-second lines.
fn found(bytes: &[u8], clock: &str) -> usize {
    let index = build_index(Cursor::new(bytes));
    find_clock(Cursor::new(bytes), &index, &parse_clock(clock).unwrap())
}

/// A window that starts mid-stride, or straddles a mark, must return exactly the asked lines — an
/// off-by-one here shows the wrong page under a correct range label.
#[test]
fn a_window_is_exactly_the_asked_lines() {
    let bytes = day(2600);
    assert_eq!(build_index(Cursor::new(&bytes)).total, 2600);
    assert_eq!(window(&bytes, 0, 2), ["line-0", "line-1"]);
    assert_eq!(
        window(&bytes, 999, 3),
        ["line-999", "line-1000", "line-1001"]
    );
    assert_eq!(window(&bytes, 2000, 1), ["line-2000"]);
    assert_eq!(window(&bytes, 2598, 50), ["line-2598", "line-2599"]);
    assert!(window(&bytes, 2600, 50).is_empty());
    assert!(window(&bytes, 9000, 50).is_empty());
    assert!(window(&[], 0, 50).is_empty());
}

/// A last line without a line break is still a line, and a CRLF file must not leave `\r` in the text.
#[test]
fn an_unterminated_last_line_and_crlf_are_read_clean() {
    let bytes = b"00:00:01.000\tINFO\t\tone\r\n00:00:02.000\tINFO\t\ttwo".to_vec();
    assert_eq!(build_index(Cursor::new(&bytes)).total, 2);
    assert_eq!(window(&bytes, 0, 10), ["one", "two"]);
}

/// The time jump must land on the first line of that second, whichever side of a mark it is on.
#[test]
fn a_time_lands_on_its_first_line() {
    let bytes = day(4000);
    assert_eq!(found(&bytes, "0:00"), 0);
    assert_eq!(found(&bytes, "00:00:07"), 7);
    // 00:16:40 is line 1000 — exactly on a mark.
    assert_eq!(found(&bytes, "0:16:40"), 1000);
    assert_eq!(found(&bytes, "0:16:39"), 999);
    assert_eq!(found(&bytes, "0:16:41"), 1001);
    assert_eq!(found(&bytes, "1"), 3600);
    // Past the end of the day: nothing is at or after it.
    assert_eq!(found(&bytes, "23:00"), 4000);
}

/// A second that fills more than a whole stride must still resolve to its FIRST line, not to the
/// last mark inside it.
#[test]
fn a_crowded_second_resolves_to_its_first_line() {
    let mut text = String::from("08:00:00.000\tINFO\t\tbefore\n");
    for at in 0..2500 {
        text.push_str(&format!("09:00:00.{:03}\tINFO\t\tburst-{at}\n", at % 1000));
    }
    text.push_str("10:00:00.000\tINFO\t\tafter\n");
    let bytes = text.into_bytes();
    assert_eq!(found(&bytes, "9:00"), 1);
    assert_eq!(found(&bytes, "9:30"), 2501);
    assert_eq!(found(&bytes, "8:00"), 0);
}

/// A line without a clock must neither stop the search nor be returned as the answer.
#[test]
fn lines_without_a_clock_are_stepped_over() {
    let bytes =
        b"header without a clock\n07:00:00.000\tINFO\t\ta\nfree text\n09:00:00.000\tINFO\t\tb\n"
            .to_vec();
    assert_eq!(found(&bytes, "6:00"), 1);
    assert_eq!(found(&bytes, "8:00"), 3);
}

/// A typo must move nothing; a short form must mean the start of that hour or minute.
#[test]
fn only_a_time_of_day_is_accepted() {
    assert_eq!(parse_clock("7").as_ref(), Some(b"07:00:00"));
    assert_eq!(parse_clock(" 7:5 ").as_ref(), Some(b"07:05:00"));
    assert_eq!(parse_clock("23:59:59").as_ref(), Some(b"23:59:59"));
    for bad in [
        "",
        "  ",
        "24",
        "7:60",
        "7:00:60",
        "7:",
        "a",
        "7:00:00:00",
        "123",
        "-1",
    ] {
        assert_eq!(parse_clock(bad), None, "{bad:?}");
    }
}
