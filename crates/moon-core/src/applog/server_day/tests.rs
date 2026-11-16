use super::*;

/// Runs the conversion and returns the stored text with its line count.
fn converted(raw: &str) -> (String, usize) {
    let mut out = Vec::new();
    let lines = convert_day(raw.as_bytes(), &mut out).unwrap();
    (String::from_utf8(out).unwrap(), lines)
}

/// A pulled line must parse exactly like a live one, or the Log tab shows it as one unsplit blob
/// and the trade-log scan sorts it to midnight.
#[test]
fn a_core_line_becomes_the_live_line_format() {
    let (text, lines) = converted("12:34:56  ACE (4411) buy placed\r\n12:34:57.250  next\n");
    assert_eq!(
        text,
        "12:34:56.000\tINFO\t\tACE (4411) buy placed\n12:34:57.250\tINFO\t\tnext\n"
    );
    assert_eq!(lines, 2);
    let parsed = super::super::parse_file_line("2026-01-02", text.lines().next().unwrap());
    assert_eq!(parsed.ts, "2026-01-02 12:34:56.000");
    assert_eq!(parsed.msg, "ACE (4411) buy placed");
}

/// A header or a continuation line has no clock; sorting it to midnight would move it away from the
/// message it belongs to.
#[test]
fn a_line_without_a_clock_keeps_the_clock_before_it() {
    let (text, lines) = converted(
        "   --- Exchange: X ---\n08:00:00  first\n  continued here\n\n   \n12:00 not a clock\n",
    );
    assert_eq!(
        text,
        "00:00:00.000\tINFO\t\t   --- Exchange: X ---\n\
         08:00:00.000\tINFO\t\tfirst\n\
         08:00:00.000\tINFO\t\t  continued here\n\
         08:00:00.000\tINFO\t\t12:00 not a clock\n"
    );
    assert_eq!(lines, 4);
}

/// Text that only starts like a time is a message; cutting it would lose its first word.
#[test]
fn only_a_clock_followed_by_space_is_split_off() {
    assert_eq!(
        split_clock("01:02:03 text"),
        Some(("01:02:03.000".into(), "text"))
    );
    assert_eq!(
        split_clock("01:02:03.045\ttext"),
        Some(("01:02:03.045".into(), "text"))
    );
    assert_eq!(split_clock("01:02:03"), Some(("01:02:03.000".into(), "")));
    assert_eq!(split_clock("01:02:03x text"), None);
    assert_eq!(split_clock("01:02:03.4 text"), None);
    assert_eq!(split_clock("1:02:03 text"), None);
    assert_eq!(split_clock("привет"), None);
}

/// A stored log gets shared: the server's own address must not survive the pull.
#[test]
fn machine_addresses_are_redacted_on_the_way_to_disk() {
    let (text, _) = converted("01:02:03  connect to 203.0.113.7:5101 failed\n");
    assert!(!text.contains("203.0.113.7"), "{text}");
    assert!(text.starts_with("01:02:03.000\tINFO\t\tconnect to "));
}

/// The pulled name must never collide with a live one, or a pull would overwrite the live copy and
/// the file list could not tell them apart.
#[test]
fn the_pulled_name_is_distinct_from_every_live_name() {
    let pulled = server_file_name("2026-01-02", "Core A/1");
    assert_eq!(pulled, "2026-01-02_Core_A_1.server.log");
    assert!(is_server_file(&pulled));
    for label in ["Core A/1", "server", "Core_A_1.server", "x.server.log"] {
        let live = super::super::file_name("2026-01-02", label);
        assert!(!is_server_file(&live), "{live}");
    }
}

/// A pull that fails half-way must leave the earlier copy, and a finished one must replace it.
#[test]
fn a_day_is_replaced_whole_and_leaves_no_partial_file() {
    let dir = std::env::temp_dir().join(format!("moon-server-day-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let first = write_server_day_in(&dir, "2026-01-02", "CORE_A", b"01:00:00  one\n").unwrap();
    assert_eq!(
        first,
        SavedDay {
            file: "2026-01-02_CORE_A.server.log".into(),
            lines: 1
        }
    );
    let second = write_server_day_in(
        &dir,
        "2026-01-02",
        "CORE_A",
        b"01:00:00  one\n02:00:00  two\n",
    )
    .unwrap();
    assert_eq!(second.lines, 2);
    let stored = std::fs::read_to_string(dir.join(&second.file)).unwrap();
    assert_eq!(
        stored,
        "01:00:00.000\tINFO\t\tone\n02:00:00.000\tINFO\t\ttwo\n"
    );
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["2026-01-02_CORE_A.server.log"]);

    let _ = std::fs::remove_dir_all(&dir);
}

/// A terminal with file logging off has no log folder until a pull needs one; failing there would
/// make the pull work only for users who also record the live log.
#[test]
fn the_log_folder_is_created_by_the_first_pull() {
    let root = std::env::temp_dir().join(format!("moon-server-day-new-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let logs = root.join("data").join("logs");

    let saved = write_server_day_in(&logs, "2026-01-02", "CORE_A", b"01:00:00  one\n").unwrap();
    assert!(logs.join(&saved.file).is_file());

    // A folder that cannot exist — its parent is a file — is an error, and leaves nothing behind.
    let blocked = logs.join(&saved.file).join("logs");
    assert!(write_server_day_in(&blocked, "2026-01-02", "CORE_A", b"x\n").is_err());
    assert_eq!(std::fs::read_dir(&logs).unwrap().count(), 1);
    let _ = std::fs::remove_dir_all(&root);
}
