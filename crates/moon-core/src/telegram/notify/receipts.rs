//! Durable per-delivery receipts, replayed over the coalesced notification snapshot.
//!
//! The sidecar is part of notification state: copy and restore it together with the snapshot.

use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{AutoReport, CardKey, NotifyFile};

/// New owner transactions and legacy delivery receipts share one ordered durable cursor.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Entry {
    Update {
        sequence: u64,
        update: super::updates::Update,
    },
    Receipt(Receipt),
}

impl Entry {
    /// Read the cursor independently of the transaction's payload kind.
    fn sequence(&self) -> u64 {
        match self {
            Self::Update { sequence, .. } => *sequence,
            Self::Receipt(row) => row.sequence,
        }
    }

    /// Apply only entries newer than the published checkpoint.
    fn apply(self, file: &mut NotifyFile) -> anyhow::Result<Option<String>> {
        match self {
            Self::Update { sequence, update } => {
                if sequence > file.ack_sequence {
                    update.apply(file)?;
                    file.ack_sequence = sequence;
                }
                Ok(None)
            }
            Self::Receipt(receipt) => Ok(receipt.apply(file).1),
        }
    }
}

/// Receipt I/O has its own mutex, so flushing a delivered card never holds the owner store.
#[derive(Debug)]
pub(crate) struct Journal {
    pub bytes: u64,
    pub sequence: u64,
}

/// One accepted delivery or permanent drop, synchronized before the sender forgets its id.
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Receipt {
    pub sequence: u64,
    pub id: u64,
    pub chat: i64,
    pub replaced: Option<(AutoReport, i64)>,
    pub card: Option<(CardKey, i64)>,
}

impl Receipt {
    /// Apply the old ack's ledger mutations, then remove only its queued id.
    /// Replaying a snapshot's receipts again is harmless because of the persisted cursor.
    pub(crate) fn apply(&self, file: &mut NotifyFile) -> (Option<i64>, Option<String>) {
        if self.sequence <= file.ack_sequence {
            return (None, None);
        }
        let photo = file.outbox.remove_id(self.id).and_then(|row| row.photo);
        if let Some((key, message)) = self.card {
            let wait = file
                .chats
                .get_mut(&self.chat)
                .and_then(|entry| entry.ledger.cards.get_mut(&key.core))
                .and_then(|rows| rows.get_mut(&key.rec_id));
            if let Some(wait) = wait {
                wait.message = Some(message);
            }
        }
        let previous = self
            .replaced
            .filter(|(kind, _)| kind.replaces_previous())
            .and_then(|(kind, message)| {
                let entry = file.chats.get_mut(&self.chat)?;
                if !entry.settings.reports.on(kind) {
                    return None;
                }
                entry
                    .ledger
                    .reports
                    .slot_mut(kind)
                    .message
                    .replace(message)
                    .filter(|old| *old != message)
            });
        file.ack_sequence = self.sequence;
        (previous, photo)
    }
}

/// Replay a contiguous journal over its snapshot. A discontinuous journal belongs to stale
/// state and is discarded whole; a corrupt or torn tail leaves the valid prefix replayable.
pub(crate) fn replay(path: &Path, file: &mut NotifyFile) -> anyhow::Result<u64> {
    let bytes = match std::fs::read(crate::config::paths::telegram_notify_receipts(path)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    let mut length = 0;
    let mut receipts: Vec<Entry> = Vec::new();
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if line.last() != Some(&b'\n') {
            log::warn!("telegram receipts: ignoring torn tail at byte {length}");
            break;
        }
        let receipt: Entry = match serde_json::from_slice(line) {
            Ok(receipt) => receipt,
            Err(error) => {
                log::warn!("telegram receipts: ignoring corrupt tail at byte {length}: {error}");
                break;
            }
        };
        let contiguous = match receipts.last() {
            Some(previous) => previous.sequence().checked_add(1) == Some(receipt.sequence()),
            None => {
                receipt.sequence() != 0 && receipt.sequence() <= file.ack_sequence.saturating_add(1)
            }
        };
        if !contiguous {
            log::warn!("telegram receipts: discarding discontinuous journal at byte {length}");
            clear(path)?;
            return Ok(0);
        }
        receipts.push(receipt);
        length += line.len() as u64;
    }
    // Keeping an older checkpointed prefix would leave a sequence gap when the next append
    // starts at the snapshot cursor. This also covers corruption inside that old prefix.
    if receipts
        .last()
        .is_some_and(|receipt| receipt.sequence() < file.ack_sequence)
    {
        clear(path)?;
        return Ok(0);
    }
    for receipt in receipts {
        let photo = receipt.apply(file)?;
        if let Some(photo) = photo {
            crate::telegram::runtime::drop_chart(path, file, &photo);
        }
    }
    Ok(length)
}

/// Append one small receipt, overwriting any tail left by a failed append. The in-memory
/// cursor advances only after the file flush, matching the former per-message crash window.
pub(crate) fn append(path: &Path, length: u64, receipt: &Receipt) -> anyhow::Result<u64> {
    append_with(path, length, receipt, sync_parent)
}

/// Append with an injectable directory sync so creation failures can be exercised on every OS.
fn append_with(
    path: &Path,
    length: u64,
    receipt: &impl Serialize,
    sync_directory: impl FnOnce(&Path) -> std::io::Result<()>,
) -> anyhow::Result<u64> {
    let path = crate::config::paths::telegram_notify_receipts(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)?;
    file.set_len(length)?;
    file.seek(SeekFrom::Start(length))?;
    let mut bytes = serde_json::to_vec(receipt)?;
    bytes.push(b'\n');
    file.write_all(&bytes)?;
    file.sync_all()?;
    // Zero is also retained after a failed first append, so a retry repeats directory sync.
    if length == 0 {
        sync_directory(&path)?;
    }
    Ok(length + bytes.len() as u64)
}

/// Synchronize one atomic owner mutation using the same torn-tail recovery as delivery receipts.
pub(crate) fn append_update(
    path: &Path,
    length: u64,
    sequence: u64,
    update: &super::updates::Update,
) -> anyhow::Result<u64> {
    /// Borrowed envelope avoids copying a notification delta during its durable append.
    #[derive(Serialize)]
    struct Transaction<'a> {
        sequence: u64,
        update: &'a super::updates::Update,
    }
    append_with(path, length, &Transaction { sequence, update }, sync_parent)
}

/// Persist the journal's directory entry on Unix; other platforms need no directory handle.
fn sync_parent(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::File::open(parent)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Retire receipts only after the snapshot carrying their cursor has been atomically saved.
/// A crash between those two writes simply replays and skips the already checkpointed prefix.
pub(crate) fn clear(path: &Path) -> anyhow::Result<()> {
    crate::config::write_file_atomic(
        &crate::config::paths::telegram_notify_receipts(path),
        b"",
        "notification receipts",
    )
}

#[cfg(test)]
mod tests;
