//! Durable per-delivery receipts, replayed over the coalesced notification snapshot.

use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{AutoReport, CardKey, NotifyFile};

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

/// Replay complete synchronized receipts. A torn final write was never acknowledged and is
/// ignored; malformed complete records fail the load instead of replaying accepted cards.
pub(crate) fn replay(path: &Path, file: &mut NotifyFile) -> anyhow::Result<u64> {
    let bytes = match std::fs::read(crate::config::paths::telegram_notify_receipts(path)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    let mut length = 0;
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if line.last() != Some(&b'\n') {
            break;
        }
        let receipt: Receipt = serde_json::from_slice(line)?;
        receipt.apply(file);
        length += line.len() as u64;
    }
    Ok(length)
}

/// Append one small receipt, overwriting any tail left by a failed append. The in-memory
/// cursor advances only after the file flush, matching the former per-message crash window.
pub(crate) fn append(path: &Path, length: u64, receipt: &Receipt) -> anyhow::Result<u64> {
    let path = crate::config::paths::telegram_notify_receipts(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    file.set_len(length)?;
    file.seek(SeekFrom::Start(length))?;
    let mut bytes = serde_json::to_vec(receipt)?;
    bytes.push(b'\n');
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(length + bytes.len() as u64)
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
