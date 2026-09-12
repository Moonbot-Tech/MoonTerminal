//! The set of sounds the player can name: the embedded eighteen plus whatever the user's folder
//! holds, resolved by stem or by Moonbot ordinal.
//!
//! One catalog is installed at a time. It is built off the UI thread (reading a folder is I/O)
//! and swapped in whole, so a caller never sees a half-scanned list — and until the first scan
//! lands, the embedded set answers, so a sound fired in the first second of a session still plays.
//!
//! Sound NUMBERS — what the core's own settings store — are pinned, never positional: Moonbot's
//! eighteen hold 1–18 in Moonbot's order, and a user's file holds a number only when its name
//! says so (`19_MySound.wav`). A file without one is reachable by name everywhere a name is
//! stored and simply has no number, so adding a file can never shift another file's number. The
//! core never plays a sound itself, so a number only ever means what this terminal's table says
//! it means; a number set from Moonbot's own dropdown that names one of ITS extra sounds falls
//! back to the default until a file here claims that number.

use std::sync::Arc;
use std::time::Duration;

use super::embedded::{DEFAULT_SOUND, MB_SOUNDS, SOUNDS};
use super::wav_duration;

/// Where an entry's bytes came from. The folder wins by name, so a user's `ding1.wav` plays
/// instead of the embedded one under the same stem and the same ordinal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Embedded,
    Folder,
}

/// One playable sound.
#[derive(Clone)]
pub(crate) struct Entry {
    /// Lowercase file stem, without any number prefix: what strategies, trade-sound settings and
    /// warn settings store.
    pub stem: String,
    /// The name as the file carries it (`BABYTOY`, `MySound`), for display.
    pub label: String,
    /// Moonbot's 1-based sound number, what the core's settings store. `None` for a file that
    /// claimed none.
    pub ordinal: Option<i32>,
    /// Validated PCM WAV bytes. Shared rather than borrowed because playback is asynchronous and
    /// the platform player keeps reading after the call returns; see the player's `CURRENT` slot.
    pub wav: Arc<[u8]>,
    pub duration: Duration,
    pub source: Source,
}

/// A file the scan looked at and refused, with the reason, for the settings page to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Rejected {
    pub name: String,
    pub reason: RejectReason,
}

/// Why a file was refused. An enum rather than a message so the settings page can word it in the
/// user's language; the I/O variant carries the OS text as detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RejectReason {
    /// Not a RIFF/WAVE file, or not plain PCM — the player decodes nothing else.
    NotPcmWav,
    /// Larger than the per-file ceiling, in MB.
    TooLarge(u64),
    /// Would push the whole set past its ceiling, in MB.
    OverTotal(u64),
    /// The file could not be read.
    Io(String),
    /// The number prefix names one of Moonbot's own 1–18.
    NumberReserved,
    /// Another file already holds this number.
    NumberTaken(String),
    /// A numbered file's name is already a sound — an embedded one, or an earlier file.
    NameTaken,
}

/// What the last scan found, for the settings page: the folder, what it took, what it refused.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ScanStats {
    /// The folder that was scanned, whether or not it exists.
    pub dir: std::path::PathBuf,
    /// Entries whose bytes come from a file in the folder, replacements of embedded ones included.
    pub folder_files: usize,
    /// Of those, the ones holding a number past Moonbot's eighteen — claimed by name
    /// (`19_x.wav`), or inherited from such a claim by a later `x.wav` replacing its bytes. A
    /// replacement of an embedded sound inherits 1–18 and is not counted.
    pub numbered: usize,
    pub rejected: Vec<Rejected>,
}

/// The installed sound set.
pub(crate) struct Catalog {
    /// Moonbot's eighteen in Moonbot's order, then the user's numbered files by number, then the
    /// rest by stem.
    entries: Vec<Entry>,
    /// Whether a folder scan has landed. Before it, a name the embedded set lacks is not yet
    /// "missing" — the user's folder may well hold it — so the notice waits for the scan.
    scanned: bool,
    stats: ScanStats,
}

impl Catalog {
    /// The embedded set alone: what plays until the first scan lands.
    pub(super) fn embedded() -> Self {
        let mut catalog = Self {
            entries: Vec::with_capacity(SOUNDS.len()),
            scanned: false,
            stats: ScanStats::default(),
        };
        for (i, (label, (stem, wav))) in MB_SOUNDS.iter().zip(SOUNDS).enumerate() {
            // The sibling test pins the two lists to the same set in the same order; a mismatch
            // here would be that test failing, not a runtime condition.
            let duration = wav_duration(wav).unwrap_or_default();
            catalog.entries.push(Entry {
                stem: (*stem).to_string(),
                label: (*label).to_string(),
                ordinal: Some(i as i32 + 1),
                wav: Arc::from(*wav),
                duration,
                source: Source::Embedded,
            });
        }
        catalog
    }

    /// The smallest number a file may claim: the one after Moonbot's eighteen.
    pub(crate) const FIRST_USER_ORDINAL: i32 = MB_SOUNDS.len() as i32 + 1;

    /// Adds a file's sound, or refuses it with the reason.
    ///
    /// An UNNUMBERED file under a known stem replaces that sound's bytes and keeps its number —
    /// how a user swaps `ding1` for their own. A NUMBERED file is a new sound and must be new on
    /// both counts: its number free and past Moonbot's eighteen, its name not yet a sound.
    pub(super) fn add(&mut self, entry: Entry) -> Result<(), RejectReason> {
        let existing = self.entries.iter().position(|e| e.stem == entry.stem);
        match (entry.ordinal, existing) {
            (None, Some(index)) => {
                let ordinal = self.entries[index].ordinal;
                self.entries[index] = Entry { ordinal, ..entry };
            }
            (None, None) => self.entries.push(entry),
            (Some(_), Some(_)) => return Err(RejectReason::NameTaken),
            (Some(n), None) => {
                if n < Self::FIRST_USER_ORDINAL {
                    return Err(RejectReason::NumberReserved);
                }
                if let Some(holder) = self.by_ordinal(n) {
                    return Err(RejectReason::NumberTaken(holder.label.clone()));
                }
                self.entries.push(entry);
            }
        }
        Ok(())
    }

    /// Orders the user's files — numbered ones by number, the rest by stem — after Moonbot's
    /// eighteen, so two scans of the same folder list them the same way, and counts what plays
    /// from the folder.
    pub(super) fn finish(&mut self, mut stats: ScanStats) {
        let mut extras = self
            .entries
            .split_off(MB_SOUNDS.len().min(self.entries.len()));
        extras.sort_by(|a, b| match (a.ordinal, b.ordinal) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.stem.cmp(&b.stem),
        });
        self.entries.extend(extras);
        let folder = || self.entries.iter().filter(|e| e.source == Source::Folder);
        stats.folder_files = folder().count();
        stats.numbered = folder()
            .filter(|e| e.ordinal.is_some_and(|n| n >= Self::FIRST_USER_ORDINAL))
            .count();
        self.scanned = true;
        self.stats = stats;
    }

    pub(crate) fn scanned(&self) -> bool {
        self.scanned
    }

    pub(crate) fn stats(&self) -> &ScanStats {
        &self.stats
    }

    /// The entry a stored name refers to: trimmed, ASCII case-insensitive, `.wav` tolerated, so
    /// `BABYTOY`, `babytoy` and `BABYTOY.wav` all reach the same file.
    pub(crate) fn find(&self, name: &str) -> Option<&Entry> {
        let low = name.trim().to_ascii_lowercase();
        let stem = low.strip_suffix(".wav").unwrap_or(&low);
        self.entries.iter().find(|e| e.stem == stem)
    }

    /// The entry Moonbot's 1-based sound number refers to, or `None` when no sound holds that
    /// number — past the eighteen and unclaimed by any file.
    pub(crate) fn by_ordinal(&self, ordinal: i32) -> Option<&Entry> {
        self.entries.iter().find(|e| e.ordinal == Some(ordinal))
    }

    /// The fallback entry. Embedded, so it is always present.
    pub(crate) fn default_entry(&self) -> Option<&Entry> {
        self.find(DEFAULT_SOUND)
    }

    /// Every entry in table order.
    pub(crate) fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Every numbered sound as `(ordinal, entry)`, in number order.
    pub(crate) fn ordinals(&self) -> impl Iterator<Item = (i32, &Entry)> {
        self.entries
            .iter()
            .filter_map(|entry| Some((entry.ordinal?, entry)))
    }
}
