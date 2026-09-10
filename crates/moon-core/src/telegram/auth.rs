//! Pairing codes and authorized-chat checks.
//!
//! Authorized chat ids arrive from config already decrypted. This module never writes them back;
//! a successful pair returns [`PairingAccepted`] so the application can persist. Pairing codes
//! live only in process memory.

use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Alphabet for a six-character pairing code.
///
/// Thirty-two symbols (5 bits) so a uniform `byte % 32` map from `getrandom` has no bias.
/// Ambiguous `0/O` and `1/I` are omitted because the user types the code from Settings.
const PAIRING_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
/// Pairing codes are six characters from [`PAIRING_ALPHABET`].
const PAIRING_CODE_LEN: usize = 6;
/// How long a displayed pairing code remains valid.
///
/// Ten minutes covers opening Settings, switching to Telegram, and typing the code, without
/// leaving a guessable secret sitting in memory for the rest of the session.
const PAIRING_TTL: Duration = Duration::from_secs(10 * 60);

/// In-memory authorization and pairing state for one bot worker.
pub struct Authorization {
    authorized: HashSet<i64>,
    pairing: Option<PendingPairing>,
}

/// Chat that just presented a correct, unused pairing code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairingAccepted {
    /// Telegram chat id now authorized to issue commands.
    pub chat_id: i64,
}

/// Why a pairing attempt was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthReject {
    /// Pairing code missing, expired, or not the unused code currently displayed.
    BadPairingCode,
    /// `getrandom` failed while issuing a code.
    Entropy,
}

/// One unconsumed Settings pairing code and its expiry.
struct PendingPairing {
    code: String,
    expires_at: Instant,
}

impl Authorization {
    /// Build from the encrypted authorized-chat list supplied by config.
    ///
    /// Args:
    ///     chat_ids: Already-decrypted chat ids. Duplicates collapse.
    ///
    /// Returns:
    ///     State with those chats authorized and no live pairing code.
    pub fn from_authorized_chats(chat_ids: impl IntoIterator<Item = i64>) -> Self {
        Self {
            authorized: chat_ids.into_iter().collect(),
            pairing: None,
        }
    }

    /// Whether `chat_id` may issue commands other than `/pair`.
    pub fn is_authorized(&self, chat_id: i64) -> bool {
        self.authorized.contains(&chat_id)
    }

    /// Number of authorized chats, for [`super::TelegramStatus::Paired`].
    pub fn chat_count(&self) -> usize {
        self.authorized.len()
    }

    /// Revoke a candidate chat while its encrypted configuration write is pending.
    pub fn revoke_chat(&mut self, chat_id: i64) {
        self.authorized.remove(&chat_id);
    }

    /// Publish the persisted set without reviving consumed codes.
    pub fn sync_persisted_chats(&mut self, chat_ids: &[i64]) {
        self.authorized = chat_ids.iter().copied().collect();
    }

    /// Forget every authorized chat and every in-memory secret. Settings reset uses this;
    /// persistence is the caller's job.
    pub fn reset_pairing(&mut self) {
        self.authorized.clear();
        self.pairing = None;
    }

    /// Issue a fresh six-character one-use pairing code that expires at `now + PAIRING_TTL`.
    ///
    /// Replaces any unused previous code so Settings always displays the live value.
    ///
    /// Args:
    ///     now: Caller-supplied clock so expiry is deterministic.
    ///
    /// Returns:
    ///     The code to show in Settings, or [`AuthReject::Entropy`] if `getrandom` failed.
    pub fn issue_pairing_code(&mut self, now: Instant) -> Result<String, AuthReject> {
        let code = random_pairing_code()?;
        self.pairing = Some(PendingPairing {
            code: code.clone(),
            expires_at: now + PAIRING_TTL,
        });
        Ok(code)
    }

    /// Bind `chat_id` if `code` is the current unused pairing code and has not expired.
    ///
    /// The code is consumed only on success so a typo does not burn the Settings value.
    /// This method does not persist; the caller stores [`PairingAccepted::chat_id`].
    ///
    /// Args:
    ///     chat_id: Telegram chat that sent `/pair`.
    ///     code: User-typed code; compared case-insensitively against the issued alphabet.
    ///     now: Caller-supplied clock.
    ///
    /// Returns:
    ///     [`PairingAccepted`] after the chat is inserted, or a pairing reject.
    pub fn pair(
        &mut self,
        chat_id: i64,
        code: &str,
        now: Instant,
    ) -> Result<PairingAccepted, AuthReject> {
        let Some(pending) = self.pairing.as_ref() else {
            return Err(AuthReject::BadPairingCode);
        };
        if now >= pending.expires_at {
            self.pairing = None;
            return Err(AuthReject::BadPairingCode);
        }
        let typed = normalize_pairing_code(code);
        if typed != pending.code {
            return Err(AuthReject::BadPairingCode);
        }
        self.pairing = None;
        self.authorized.insert(chat_id);
        Ok(PairingAccepted { chat_id })
    }
}

/// Normalize user input to the pairing alphabet (uppercase, no surrounding space).
fn normalize_pairing_code(code: &str) -> String {
    code.trim().to_ascii_uppercase()
}

/// Six cryptographically random characters from [`PAIRING_ALPHABET`].
fn random_pairing_code() -> Result<String, AuthReject> {
    let mut bytes = [0u8; PAIRING_CODE_LEN];
    getrandom::getrandom(&mut bytes).map_err(|_| AuthReject::Entropy)?;
    let mut code = String::with_capacity(PAIRING_CODE_LEN);
    for byte in bytes {
        // 256 is divisible by 32, so this is uniform over the alphabet.
        code.push(PAIRING_ALPHABET[(byte as usize) % PAIRING_ALPHABET.len()] as char);
    }
    Ok(code)
}
