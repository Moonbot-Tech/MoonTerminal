//! Reading a day-sized log file one screen at a time.
//!
//! A day of one core's log is tens of megabytes and hundreds of thousands of lines, while the Log
//! tab shows a few thousand. Reading only the tail left the morning of a day on disk and out of
//! reach. A [`FilePager`] walks the file once, when
//! it is opened, remembering where every [`STRIDE`]-th line starts and what its clock reads; after
//! that any window — by line number or by time of day — is a seek and a short read, and only the
//! lines of that window are parsed.

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;

use super::{LogLine, parse_file_line};
use crate::config::paths;

/// Lines between two remembered positions: the most a window read has to skip, and the most a time
/// lookup has to scan.
const STRIDE: usize = 1000;

/// Length of the `HH:MM:SS` clock a stored line starts with.
const CLOCK_LEN: usize = 8;

/// A remembered position inside the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Mark {
    /// Byte offset of the line this mark stands on.
    offset: u64,
    /// That line's `HH:MM:SS`, or the clock of the mark before it when the line has none.
    clock: [u8; CLOCK_LEN],
}

/// The positions remembered for one file and how many lines it had when it was walked.
#[derive(Debug, Default, PartialEq, Eq)]
struct Index {
    marks: Vec<Mark>,
    total: usize,
}

/// A log file opened for windowed reading.
pub struct FilePager {
    path: PathBuf,
    /// Day the file's lines belong to, from the leading `YYYY-MM-DD` of its name.
    date: String,
    index: Index,
}

impl FilePager {
    /// Walks `filename` (a bare name inside `logs/`) once and remembers where its lines are.
    ///
    /// A missing or unreadable file opens as an empty pager, which is what the Log tab shows for a
    /// file retention removed between the listing and the click.
    pub fn open(filename: &str) -> Self {
        let path = paths::logs_dir().join(filename);
        let index = File::open(&path)
            .ok()
            .map(|file| build_index(BufReader::new(file)))
            .unwrap_or_default();
        Self {
            path,
            date: filename.get(0..10).unwrap_or("").to_string(),
            index,
        }
    }

    /// Day the file's lines belong to, `YYYY-MM-DD`, or empty when its name carries none.
    pub fn date(&self) -> &str {
        &self.date
    }

    /// Number of lines the file had when it was opened. A live file that grew since keeps this
    /// count: a file is a snapshot, the live source is what follows a tail.
    pub fn total(&self) -> usize {
        self.index.total
    }

    /// Parses up to `max` lines starting at line `first` (zero-based).
    pub fn read(&self, first: usize, max: usize) -> Vec<LogLine> {
        let Ok(file) = File::open(&self.path) else {
            return Vec::new();
        };
        read_window(BufReader::new(file), &self.index, first, max)
            .iter()
            .map(|line| parse_file_line(&self.date, line))
            .collect()
    }

    /// First line at or after a time of day, or the line count when the day ends before it.
    ///
    /// Args:
    ///     clock: `HH:MM:SS`, as [`parse_clock`] normalises what the user typed.
    pub fn line_at(&self, clock: &[u8; CLOCK_LEN]) -> usize {
        let Ok(file) = File::open(&self.path) else {
            return 0;
        };
        find_clock(BufReader::new(file), &self.index, clock)
    }
}

/// Normalises a typed time of day — `H`, `HH`, `H:MM`, `HH:MM` or `HH:MM:SS` — to `HH:MM:SS`.
///
/// Returns `None` for anything that is not a time of day, so a typo moves nothing.
pub fn parse_clock(text: &str) -> Option<[u8; CLOCK_LEN]> {
    let mut parts = text.trim().split(':');
    let mut field = |limit: u32| -> Option<u32> {
        match parts.next() {
            None => Some(0),
            Some(part) if (1..=2).contains(&part.len()) => {
                part.parse::<u32>().ok().filter(|value| *value < limit)
            }
            Some(_) => None,
        }
    };
    let (hours, minutes, seconds) = (field(24)?, field(60)?, field(60)?);
    if parts.next().is_some() {
        return None;
    }
    let mut clock = [0u8; CLOCK_LEN];
    clock.copy_from_slice(format!("{hours:02}:{minutes:02}:{seconds:02}").as_bytes());
    Some(clock)
}

/// The `HH:MM:SS` a stored line starts with, when it starts with one.
fn line_clock(line: &[u8]) -> Option<[u8; CLOCK_LEN]> {
    let clock: [u8; CLOCK_LEN] = line.get(..CLOCK_LEN)?.try_into().ok()?;
    let shaped = clock.iter().enumerate().all(|(at, byte)| match at {
        2 | 5 => *byte == b':',
        _ => byte.is_ascii_digit(),
    });
    shaped.then_some(clock)
}

/// Walks the whole file once, marking every [`STRIDE`]-th line.
///
/// Reads into one reused buffer rather than a `String` per line: this pass touches every byte of a
/// day and nothing of it is kept.
fn build_index(mut reader: impl BufRead) -> Index {
    let mut index = Index::default();
    let mut offset = 0u64;
    let mut last_clock = *b"00:00:00";
    let mut line = Vec::new();
    loop {
        line.clear();
        let Ok(read) = reader.read_until(b'\n', &mut line) else {
            break; // A damaged tail ends the file where it stopped being readable.
        };
        if read == 0 {
            break;
        }
        if let Some(clock) = line_clock(&line) {
            last_clock = clock;
        }
        if index.total % STRIDE == 0 {
            index.marks.push(Mark {
                offset,
                clock: last_clock,
            });
        }
        index.total += 1;
        offset += read as u64;
    }
    index
}

/// Positions `reader` at the start of line `first` and returns the lines of the window as text.
fn read_window(
    mut reader: impl BufRead + Seek,
    index: &Index,
    first: usize,
    max: usize,
) -> Vec<String> {
    let first = first.min(index.total);
    let wanted = max.min(index.total - first);
    let Some(mark) = index.marks.get(first / STRIDE) else {
        return Vec::new();
    };
    if wanted == 0 || reader.seek(SeekFrom::Start(mark.offset)).is_err() {
        return Vec::new();
    }
    let mut lines = Vec::with_capacity(wanted);
    let mut skip = first % STRIDE;
    let mut line = Vec::new();
    while lines.len() < wanted {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(read) if read > 0 => {}
            _ => break,
        }
        if skip > 0 {
            skip -= 1;
            continue;
        }
        let text = String::from_utf8_lossy(&line);
        lines.push(text.trim_end_matches(['\n', '\r']).to_string());
    }
    lines
}

/// Finds the first line whose clock is at or after `clock`.
///
/// The marks before the answer all read earlier than `clock`, so the answer is either inside the
/// last such block or is the first line of the block after it; one block is the most that is read.
fn find_clock(mut reader: impl BufRead + Seek, index: &Index, clock: &[u8; CLOCK_LEN]) -> usize {
    let earlier = index.marks.partition_point(|mark| mark.clock < *clock);
    let Some(block) = earlier.checked_sub(1) else {
        return 0;
    };
    let next_block = (earlier * STRIDE).min(index.total);
    if reader
        .seek(SeekFrom::Start(index.marks[block].offset))
        .is_err()
    {
        return next_block;
    }
    let mut line = Vec::new();
    for at in block * STRIDE..next_block {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(read) if read > 0 => {}
            _ => break,
        }
        if line_clock(&line).is_some_and(|found| found >= *clock) {
            return at;
        }
    }
    next_block
}

#[cfg(test)]
mod tests;
