//! FireTest's own log channel.
//!
//! Every stage line goes to both the app log and `firetest.log`. The file is what CI and the
//! developer read afterwards, so it must survive a process exit from inside `evaluate_and_exit`:
//! each line is appended and flushed on its own rather than buffered in a handle nobody closes.

use std::io::Write;

/// Record a normal FireTest line in the app log and in `firetest.log`.
pub(super) fn firetest_info(line: &str) {
    log::info!("{line}");
    write_firetest_line(line);
}

/// Record a failing FireTest line in the app log and in `firetest.log`.
pub(super) fn firetest_error(line: &str) {
    log::error!("{line}");
    write_firetest_line(line);
}

/// Append one line to `firetest.log`, ignoring an unavailable file: a diagnostic run must not die
/// because its own log could not be opened.
fn write_firetest_line(line: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("firetest.log")
    {
        let _ = writeln!(f, "{line}");
    }
}
