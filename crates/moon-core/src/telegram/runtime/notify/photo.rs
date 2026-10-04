//! Deal charts in the outbox. The PNG is not in the notifications file — that file is rewritten
//! whole on every change, and a picture is hundreds of kilobytes — but in a folder beside it; the
//! row names it, and the file goes when nothing names it any more.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::NotifyStore;
use crate::telegram::api::{
    ApiError, BotApi, CAPTION_UTF16_LIMIT, Message, is_permanent_bad_request,
};
use crate::telegram::notify::{NotifyFile, Pending};
use crate::telegram::reply::utf16_len;

/// The folder beside the notifications file the charts wait in.
const CHART_DIR: &str = "telegram_charts";

/// How old a picture no row names must be before it is deleted. The drawing job writes the file
/// before the owner thread queues its row, so a fresh unnamed file is one on its way in.
const ORPHAN_AGE: Duration = Duration::from_secs(600);

/// The charts folder of the notifications file at `notifications`.
pub fn chart_spool_dir(notifications: &Path) -> PathBuf {
    notifications
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(CHART_DIR)
}

/// Queue a deal chart without saving: `photo` names its file in [`chart_spool_dir`], `caption` is
/// its HTML. A caption past Telegram's limit is dropped rather than cut inside a tag — the picture
/// still goes.
///
/// Returns:
///     `true`: the row is always appended.
pub fn push_photo(
    file: &mut NotifyFile,
    chat: i64,
    caption: String,
    cores: Option<Vec<u64>>,
    photo: String,
    now_utc: i64,
) -> bool {
    let caption = if utf16_len(&caption) <= CAPTION_UTF16_LIMIT {
        caption
    } else {
        log::warn!(
            "telegram chart for chat {chat}: caption of {} utf-16 units dropped, the limit is {CAPTION_UTF16_LIMIT}",
            utf16_len(&caption)
        );
        String::new()
    };
    let id = file.next_id;
    file.next_id = file.next_id.saturating_add(1);
    file.outbox.push(Pending {
        id,
        chat,
        html: caption,
        created_utc: now_utc,
        cores,
        photo: Some(photo),
        ..Pending::default()
    });
    true
}

/// The bytes of `row`'s picture, `None` when the row is no picture or its file is gone.
pub(super) fn read_photo(store_path: &Path, row: &Pending) -> Option<Result<Vec<u8>, ()>> {
    let name = row.photo.as_deref()?;
    Some(
        std::fs::read(chart_spool_dir(store_path).join(name)).map_err(|error| {
            log::warn!("telegram chart {name} unreadable, its caption goes alone: {error}");
        }),
    )
}

/// Send a chart row: the picture with its caption. A picture Telegram refuses for good, or whose
/// file is gone, leaves its caption as a plain message, so the trade is still told; with no caption
/// either, the row is dropped without a call.
pub(super) fn send_chart(
    api: &mut BotApi,
    chat: i64,
    png: Result<Vec<u8>, ()>,
    caption: &str,
) -> Result<Message, ApiError> {
    let refused = match png {
        Ok(png) => match api.send_photo(chat, &png, caption) {
            Err(error) if is_permanent_bad_request(&error) && !caption.is_empty() => {
                log::warn!(
                    "telegram chart for chat {chat} refused, its caption goes alone: {error}"
                );
                true
            }
            sent => return sent,
        },
        Err(()) => true,
    };
    if refused && caption.is_empty() {
        return Err(ApiError::Telegram {
            description: "Bad Request: the chart's picture is gone and it has no caption".into(),
            retry_after_secs: None,
        });
    }
    api.send_html(chat, caption)
}

impl NotifyStore {
    /// Delete the pictures no outbox row names any more, past [`ORPHAN_AGE`]. A file that cannot
    /// be listed or deleted is left for the next sweep.
    pub(super) fn sweep_charts(&self) {
        let dir = chart_spool_dir(&self.path);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        let named: BTreeSet<&str> = self
            .file
            .outbox
            .iter()
            .filter_map(|row| row.photo.as_deref())
            .collect();
        let now = SystemTime::now();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if named.contains(name) {
                continue;
            }
            let old = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|at| now.duration_since(at).ok())
                .is_some_and(|age| age >= ORPHAN_AGE);
            if !old {
                continue;
            }
            if let Err(error) = std::fs::remove_file(entry.path()) {
                log::debug!("telegram chart {name} not deleted: {error}");
            }
        }
    }

    /// Delete the picture of a row that just left the outbox, or of one never queued. A file
    /// another row still names stays.
    pub fn drop_chart(&self, photo: &str) {
        if self
            .file
            .outbox
            .iter()
            .any(|row| row.photo.as_deref() == Some(photo))
        {
            return;
        }
        let path = chart_spool_dir(&self.path).join(photo);
        match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                log::debug!("telegram chart {photo} not deleted: {error}");
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
