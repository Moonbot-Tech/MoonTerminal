//! Reading the user's sounds folder into a [`Catalog`]. Pure I/O over a path — nothing here
//! touches the installed catalog, so it runs on a background thread and is testable against a
//! temporary folder.
//!
//! A file wins over an embedded sound of the same name: a user who wants their own `ding1` drops
//! a `ding1.wav` in the folder; the embedded one keeps its number and loses its bytes. A file
//! named `19_MySound.wav` is the sound `MySound` holding number 19 — the way a sound gets a number
//! the core's settings can store; see the catalog for the rules.

use std::path::Path;
use std::sync::Arc;

use super::catalog::{Catalog, Entry, RejectReason, Rejected, ScanStats, Source};
use super::wav_duration;

/// Largest single sound accepted. Moonbot's longest ships at under 400 KB; a notification sound
/// past this is a mislabelled song, and the whole set is held in memory for the session.
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// Ceiling on everything the scan keeps.
const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

/// Reads `dir` on top of the embedded set. An absent folder is the common case and yields the
/// embedded catalog with `scanned` set; every refusal lands in the stats rather than in a log line
/// the user never opens.
pub(super) fn scan(dir: &Path) -> Catalog {
    let mut catalog = Catalog::embedded();
    let mut budget = Budget {
        used: 0,
        rejected: Vec::new(),
    };
    read_folder(dir, &mut catalog, &mut budget);
    catalog.finish(ScanStats {
        dir: dir.to_path_buf(),
        rejected: budget.rejected,
        ..ScanStats::default()
    });
    catalog
}

/// The running byte total and the refusals.
struct Budget {
    used: u64,
    rejected: Vec<Rejected>,
}

impl Budget {
    fn reject(&mut self, name: &str, reason: RejectReason) {
        self.rejected.push(Rejected {
            name: name.to_string(),
            reason,
        });
    }

    /// Whether a file of `len` bytes fits under both ceilings; a refusal is recorded here.
    fn admit(&mut self, name: &str, len: u64) -> bool {
        if len > MAX_FILE_BYTES {
            self.reject(name, RejectReason::TooLarge(MAX_FILE_BYTES / (1024 * 1024)));
            return false;
        }
        if self.used + len > MAX_TOTAL_BYTES {
            self.reject(
                name,
                RejectReason::OverTotal(MAX_TOTAL_BYTES / (1024 * 1024)),
            );
            return false;
        }
        self.used += len;
        true
    }
}

/// What a file name says about the sound.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct WavName {
    /// Lowercase name without the number prefix.
    pub(super) stem: String,
    /// The name as spelled, without the number prefix.
    pub(super) label: String,
    /// The number the prefix claims, if there is one: `19_MySound.wav` or `19-MySound.wav`.
    pub(super) ordinal: Option<i32>,
}

/// Reads a `.wav` file name in any spelling of the extension; `None` for anything else — a
/// readme in the folder is not a refusal.
///
/// A leading run of digits followed by `_` or `-` is the sound's number; the rest is its name.
/// `19_MySound.wav` claims 19. A number too large for `i32`, or nothing after the separator, reads
/// as an ordinary name rather than a claim, so a strangely named file still plays by name.
pub(super) fn wav_name(file_name: &str) -> Option<WavName> {
    let dot = file_name.len().checked_sub(".wav".len())?;
    let (name, ext) = file_name.split_at_checked(dot)?;
    if !ext.eq_ignore_ascii_case(".wav") {
        return None;
    }
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let digits = name.len() - name.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let claim = (digits > 0)
        .then(|| name.split_at_checked(digits))
        .flatten()
        .and_then(|(number, rest)| {
            let rest = rest.strip_prefix(['_', '-'])?.trim();
            let number = number.parse::<i32>().ok()?;
            (!rest.is_empty()).then_some((number, rest))
        });
    let (ordinal, label) = match claim {
        Some((n, rest)) => (Some(n), rest),
        None => (None, name),
    };
    Some(WavName {
        stem: label.to_ascii_lowercase(),
        label: label.to_string(),
        ordinal,
    })
}

/// Reads the loose `.wav` files of the folder, by name order so two runs agree. An unreadable or
/// absent folder reads as empty.
fn read_folder(dir: &Path, catalog: &mut Catalog, budget: &mut Budget) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<(String, std::path::PathBuf)> = read
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            let name = path.file_name()?.to_str()?.to_string();
            path.is_file().then_some((name, path))
        })
        .collect();
    names.sort();
    for (name, path) in names {
        let Some(wav) = wav_name(&name) else {
            continue;
        };
        let len = match std::fs::metadata(&path) {
            Ok(meta) => meta.len(),
            Err(e) => {
                budget.reject(&name, RejectReason::Io(e.to_string()));
                continue;
            }
        };
        if !budget.admit(&name, len) {
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) => {
                budget.reject(&name, RejectReason::Io(e.to_string()));
                continue;
            }
        };
        let Some(duration) = wav_duration(&bytes) else {
            budget.reject(&name, RejectReason::NotPcmWav);
            continue;
        };
        let entry = Entry {
            stem: wav.stem,
            label: wav.label,
            ordinal: wav.ordinal,
            wav: Arc::from(bytes),
            duration,
            source: Source::Folder,
        };
        if let Err(reason) = catalog.add(entry) {
            budget.reject(&name, reason);
        }
    }
}
