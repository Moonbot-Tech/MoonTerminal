//! One whole day of a core's trading log, pulled from its server and kept beside the live copy.
//!
//! The live copy (`<date>_<label>.log`) holds only the hours this terminal was connected to the
//! core. A pulled day is the core's own file, complete, and it is stored under a name of its own:
//! spliced into the live copy it would repeat every hour both of them hold. Both are dated by the
//! core's own day — the live copy takes its date from the stream's timestamp, which is the core's
//! clock passed through unconverted — so a day's two files sit side by side whatever zone the
//! core's server keeps. Their clocks are not the same reading,
//! though: a pulled line carries the clock the core wrote it with, a live line the clock of the
//! stream that delivered it — a fraction of a second later, seconds on a busy day — with the core's
//! own clock left in front of the message. A reader that compares the two (the Report's trade log)
//! has to allow for both.
//!
//! The pulled file is rewritten into the live copy's own line format, so every reader of a log file
//! ([`super::pager::FilePager`], [`super::scan_file`]) takes it without a second parser.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use super::{one_line, redact, sanitize_label};
use crate::config::paths;

/// Ending of a pulled day's file name.
///
/// A source label never contains a dot ([`sanitize_label`]), so the live file of no core — whatever
/// it is called — can end like this, and a pulled day can never be taken for a live one.
pub(super) const SERVER_SUFFIX: &str = ".server.log";

/// Name of the file a pulled day is stored under, inside `logs/`.
///
/// It leads with the day, as the live files do, which is what makes [`super::purge_old`] retire it
/// on the same retention setting.
///
/// Args:
///     date: The server's calendar day, `YYYY-MM-DD`, as the core named its file.
///     label: Source label, the core's name.
///
/// Returns:
///     The bare file name, without a directory.
pub fn server_file_name(date: &str, label: &str) -> String {
    format!("{date}_{}{SERVER_SUFFIX}", sanitize_label(label))
}

/// Whether `name` is a day pulled from a server rather than a live copy.
pub fn is_server_file(name: &str) -> bool {
    name.ends_with(SERVER_SUFFIX)
}

/// What [`write_server_day`] stored.
#[derive(Debug, PartialEq, Eq)]
pub struct SavedDay {
    /// Bare name of the stored file inside `logs/`.
    pub file: String,
    /// Number of lines it holds.
    pub lines: usize,
}

/// Stores one pulled day in the terminal's log folder, replacing an earlier pull of the same day.
///
/// The day is written beside its final name and moved over it only when complete, so a pull that
/// fails half-way leaves the previous copy — or nothing — rather than a file that looks like a day
/// and ends at noon. This is an explicit request, so it does not consult the file-logging switch,
/// which governs what the terminal records on its own.
///
/// Args:
///     date: The server's calendar day, `YYYY-MM-DD`.
///     label: Source label, the core's name.
///     raw: The core's file as it was on the server.
///
/// Returns:
///     The stored file's name and line count, or the I/O error that stopped the write.
pub fn write_server_day(date: &str, label: &str, raw: &[u8]) -> std::io::Result<SavedDay> {
    write_server_day_in(&paths::logs_dir(), date, label, raw)
}

/// [`write_server_day`] into an explicit folder, which is what lets a test use its own.
///
/// The folder is created here. The live writer makes it on its first line, so a terminal that has
/// file logging switched off — or has not logged anything yet — has no `logs/` at all, and a pull
/// is the first thing to need it.
fn write_server_day_in(
    dir: &Path,
    date: &str,
    label: &str,
    raw: &[u8],
) -> std::io::Result<SavedDay> {
    let file = server_file_name(date, label);
    let part = dir.join(format!("{file}.part"));
    let written = (|| {
        std::fs::create_dir_all(dir)?;
        let mut out = BufWriter::new(File::create(&part)?);
        let lines = convert_day(raw, &mut out)?;
        out.flush()?;
        drop(out);
        std::fs::rename(&part, dir.join(&file))?;
        Ok(lines)
    })();
    match written {
        Ok(lines) => Ok(SavedDay { file, lines }),
        Err(error) => {
            let _ = std::fs::remove_file(&part);
            Err(error)
        }
    }
}

/// Rewrites a core's log into the live copy's line format and returns how many lines it wrote.
///
/// A core line is `hh:nn:ss  text` or `hh:nn:ss.zzz  text`. A line without a clock — the header a
/// fresh file starts with, or the continuation of a multi-line message — takes the clock of the
/// line before it, so it stays where it was written instead of sorting to midnight. Blank lines are
/// dropped. Machine addresses are redacted here, as the live writer does: a stored log gets shared.
fn convert_day(raw: &[u8], out: &mut impl Write) -> std::io::Result<usize> {
    let mut clock = String::from("00:00:00.000");
    let mut lines = 0;
    for line in raw.split(|byte| *byte == b'\n') {
        let line = String::from_utf8_lossy(line);
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let text = match split_clock(line) {
            Some((stamp, rest)) => {
                clock = stamp;
                rest
            }
            None => line,
        };
        let text = one_line(redact::addresses(text));
        writeln!(out, "{clock}\tINFO\t\t{text}")?;
        lines += 1;
    }
    Ok(lines)
}

/// Splits a core log line into its clock, normalised to `HH:MM:SS.mmm`, and the text after it.
///
/// Returns `None` for a line that does not start with a clock followed by white space (or by
/// nothing): a message that merely begins with digits and colons is text, not a time.
fn split_clock(line: &str) -> Option<(String, &str)> {
    let bytes = line.as_bytes();
    let digit = |at: usize| bytes.get(at).is_some_and(u8::is_ascii_digit);
    let colon = |at: usize| bytes.get(at) == Some(&b':');
    if !(digit(0)
        && digit(1)
        && colon(2)
        && digit(3)
        && digit(4)
        && colon(5)
        && digit(6)
        && digit(7))
    {
        return None;
    }
    // Every byte checked so far is ASCII, so these offsets are character boundaries.
    let (end, millis) = if bytes.get(8) == Some(&b'.') && digit(9) && digit(10) && digit(11) {
        (12, &line[9..12])
    } else {
        (8, "000")
    };
    let rest = &line[end..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    Some((format!("{}.{millis}", &line[..8]), rest.trim_start()))
}

#[cfg(test)]
mod tests;
