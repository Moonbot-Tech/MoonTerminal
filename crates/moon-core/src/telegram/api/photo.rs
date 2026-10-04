//! `sendPhoto` with the picture uploaded in the request: a multipart body written by hand — three
//! text fields and one file are no reason for a multipart dependency.

use super::{ApiError, BotApi, Message};

/// Telegram's limit on a photo caption, in characters after entity parsing. The HTML is held to
/// it in UTF-16 units, tags included, which is never more than Telegram counts.
pub const CAPTION_UTF16_LIMIT: usize = 1024;

impl BotApi {
    /// Send `png` as a photo with an HTML caption.
    ///
    /// Args:
    ///     chat_id: Destination chat.
    ///     png: The picture's bytes.
    ///     caption_html: Telegram HTML, at most [`CAPTION_UTF16_LIMIT`] UTF-16 units; empty sends
    ///         the picture alone.
    ///
    /// Returns:
    ///     The echoed message on success.
    ///
    /// Errors:
    ///     The same classified failures as [`BotApi::send_html`].
    pub fn send_photo(
        &mut self,
        chat_id: i64,
        png: &[u8],
        caption_html: &str,
    ) -> Result<Message, ApiError> {
        let boundary = boundary(png);
        let body = multipart(&boundary, chat_id, caption_html, png);
        let content_type = format!("multipart/form-data; boundary={boundary}");
        self.post_with("sendPhoto", false, &|request| {
            request
                .header("Content-Type", content_type.as_str())
                .send(&body[..])
        })
    }
}

/// A boundary that does not occur in the picture: a PNG is binary, so the first candidate that
/// is not inside it is taken — the first one, all but always.
fn boundary(png: &[u8]) -> String {
    let seed = crate::util::now_unix_ms_i64() as u64 ^ (png.len() as u64).rotate_left(32);
    (0u64..)
        .map(|n| format!("moonterminal-chart-{:016x}", seed.wrapping_add(n)))
        .find(|candidate| !contains(png, candidate.as_bytes()))
        .unwrap_or_default()
}

/// Whether `needle` occurs in `hay`.
fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|window| window == needle)
}

/// The `sendPhoto` form: the chat, the caption with its parse mode, and the picture.
fn multipart(boundary: &str, chat_id: i64, caption_html: &str, png: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(png.len() + caption_html.len() + 512);
    let mut field = |name: &str, value: &str| {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    };
    field("chat_id", &chat_id.to_string());
    if !caption_html.is_empty() {
        field("caption", caption_html);
        field("parse_mode", "HTML");
    }
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"photo\"; filename=\"deal.png\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: image/png\r\n\r\n");
    body.extend_from_slice(png);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

#[cfg(test)]
mod tests;
