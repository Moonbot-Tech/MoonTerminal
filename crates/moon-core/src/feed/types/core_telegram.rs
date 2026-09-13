//! Moon-core-owned mirror of the core's built-in Telegram reader state.
//!
//! Field-for-field the same shape as moonproto's `TelegramState` family, but with no moonproto
//! import and no serde: the UI crate has no moonproto dependency, and these types never hit the
//! wire or a config file. The mapping from the protocol snapshot lives in the live feed, so the UI
//! layer stays transport-agnostic like the rest of `feed::types`.
//!
//! Debug on the state, service and auth-details structs is hand-written and omits account data, QR
//! tokens and proxy secrets. A derived `Debug` would put them into any `{:?}`.

use std::fmt;

/// Full core snapshot of the built-in Telegram reader. Missing optional fields are unavailable,
/// not unchanged.
///
/// The account and active proxy are shared by MoonBot processes on the core's machine. `enabled`
/// and saved proxy settings belong to this particular core. Debug output intentionally omits
/// account, QR and error details.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct CoreTelegramState {
    pub enabled: bool,
    /// Saved proxy kind: 0 = none, 1 = SOCKS5, 2 = MTProto.
    pub proxy_type: Option<u8>,
    pub proxy_host: Option<String>,
    pub proxy_port: Option<u16>,
    pub proxy_user: Option<String>,
    /// The core never returns the saved password or MTProto secret.
    pub proxy_password_set: bool,
    /// Core-to-service status, such as starting, connecting, disabled or offline.
    pub client_state: Option<String>,
    /// Pipe connection to the service, not a connection to Telegram itself.
    pub service_online: bool,
    /// The service supports recoverable remote login state.
    pub state_supported: bool,
    pub service_version: Option<String>,
    pub setup_error: Option<String>,
    /// Failure to reset an unfinished login or the local account state.
    pub client_error: Option<String>,
    /// Absent when disabled or disconnected from the service.
    pub service: Option<CoreTelegramService>,
}

impl fmt::Debug for CoreTelegramState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CoreTelegramState")
            .field("enabled", &self.enabled)
            .field("service_online", &self.service_online)
            .field("state_supported", &self.state_supported)
            .finish_non_exhaustive()
    }
}

/// Current TDLib state. State names remain strings so newer service states survive.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct CoreTelegramService {
    /// Only `ready` means authenticated. See the Telegram guide for all steps.
    pub auth_state: String,
    /// Replaced with each snapshot; belongs only to the current auth step.
    pub details: CoreTelegramAuthDetails,
    /// Telegram network status, independent of authentication.
    pub connection: String,
    /// Actual account phone after login, including QR login.
    pub phone: Option<String>,
    pub error: Option<CoreTelegramError>,
    /// Actually enabled service proxy; may differ from this core's saved settings.
    pub proxy: Option<CoreTelegramActiveProxy>,
    pub proxy_error: Option<CoreTelegramError>,
}

impl fmt::Debug for CoreTelegramService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CoreTelegramService")
            .finish_non_exhaustive()
    }
}

/// Step-specific input hints. Absent fields must not be carried over from old steps.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct CoreTelegramAuthDetails {
    /// Encode the entire `tg://login?token=...` string as QR; never log it.
    pub qr_link: Option<String>,
    pub phone: Option<String>,
    pub code_type: Option<CoreTelegramCodeType>,
    pub next_code_type: Option<CoreTelegramCodeType>,
    /// Unix seconds UTC; phone-code resend also needs `next_code_type`.
    pub resend_at: Option<i64>,
    pub password_hint: Option<String>,
    pub recovery_email_pattern: Option<String>,
    pub email_pattern: Option<String>,
    pub code_length: Option<i32>,
    pub terms: Option<String>,
    pub min_user_age: Option<i32>,
    pub show_popup: bool,
    pub support_email: Option<String>,
    pub support_subject: Option<String>,
}

impl fmt::Debug for CoreTelegramAuthDetails {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CoreTelegramAuthDetails")
            .finish_non_exhaustive()
    }
}

/// Delivery method for the current or next login code.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoreTelegramCodeType {
    /// telegram, sms, sms_word, sms_phrase, call, flash_call, missed_call,
    /// fragment or unsupported. Preserve unknown kinds in UI fallback handling.
    pub kind: String,
    pub length: Option<i32>,
    pub first_letter: Option<String>,
    pub first_word: Option<String>,
    pub pattern: Option<String>,
    pub prefix: Option<String>,
    pub url: Option<String>,
}

/// A service error to display next to the current step or proxy settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoreTelegramError {
    pub code: i32,
    pub message: String,
}

/// Proxy the service currently has enabled; may differ from this core's saved settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoreTelegramActiveProxy {
    /// none, socks5, mtproto or other.
    pub mode: String,
    pub host: Option<String>,
    pub port: Option<u16>,
}

/// Explicit choice of login method. Choosing Phone cancels an unfinished login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreTelegramLoginMode {
    Phone,
    Qr,
}

/// Complete saved proxy configuration, not a partial update.
///
/// An empty password or secret CLEARS it, not "keep the old value". The core never returns the
/// saved secret, so an edit must collect it again. Debug prints only the variant, never the
/// payload.
#[derive(Clone, PartialEq, Eq)]
pub enum CoreTelegramProxy {
    None,
    Socks5 {
        host: String,
        port: u16,
        user: String,
        password: String,
    },
    MtProto {
        host: String,
        port: u16,
        secret: String,
    },
}

impl fmt::Debug for CoreTelegramProxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::None => "None",
            Self::Socks5 { .. } => "Socks5 { .. }",
            Self::MtProto { .. } => "MtProto { .. }",
        })
    }
}

/// One user action for the core's built-in Telegram reader.
///
/// Nested under a single `CoreCmd` arm so the live-feed drain stays one match arm. Debug prints
/// only [`Self::name`] — the snake_case MoonProto method — never a phone, code, password, email
/// or proxy secret.
#[derive(Clone, PartialEq, Eq)]
pub enum TelegramCmd {
    Refresh,
    SetEnabled(bool),
    SetProxy(CoreTelegramProxy),
    SetLoginMode(CoreTelegramLoginMode),
    SetPhone(String),
    SetCode(String),
    SetPassword(String),
    SetEmail(String),
    SetEmailCode(String),
    Register {
        first_name: String,
        last_name: String,
    },
    ResendCode,
    Logout,
}

impl TelegramCmd {
    /// Snake_case MoonProto `MoonTelegram` method this command maps to.
    ///
    /// Stable and payload-free so a log line can name the action without printing the secret the
    /// variant carries.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Refresh => "refresh",
            Self::SetEnabled(_) => "set_enabled",
            Self::SetProxy(_) => "set_proxy",
            Self::SetLoginMode(_) => "set_login_mode",
            Self::SetPhone(_) => "set_phone",
            Self::SetCode(_) => "set_code",
            Self::SetPassword(_) => "set_password",
            Self::SetEmail(_) => "set_email",
            Self::SetEmailCode(_) => "set_email_code",
            Self::Register { .. } => "register",
            Self::ResendCode => "resend_code",
            Self::Logout => "logout",
        }
    }
}

impl fmt::Debug for TelegramCmd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// UI-facing auth step derived from the protocol's open `auth_state` string.
///
/// Total by design: MoonProto ships new auth states without a version bump, so anything this build
/// has not heard of is [`Self::Unknown`] rather than a parse failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStep {
    /// `wait_phone` — collect an international phone number or switch to QR.
    WaitPhone,
    /// `wait_qr_confirmation` — render `details.qr_link` as QR.
    WaitQrConfirmation,
    /// `wait_code` — submit the login code; resend is gated by [`resend_state`].
    WaitCode,
    /// `wait_password` — submit the two-factor password.
    WaitPassword,
    /// `wait_email_address` — submit a recovery email address.
    WaitEmailAddress,
    /// `wait_email_code` — submit the email verification code.
    WaitEmailCode,
    /// `wait_registration` — register only after the shown terms are accepted.
    WaitRegistration,
    /// `ready` — authenticated. The only string that means logged in.
    Ready,
    /// `wait_premium_purchase` — an external Telegram step is required.
    WaitPremiumPurchase,
    /// `starting`, `wait_tdlib_parameters`, `logging_out`, `closing` or `closed`.
    Transitional,
    /// Empty, unknown, or a future protocol string. Do not guess an input.
    Unknown,
}

/// Map a protocol `auth_state` string onto [`AuthStep`].
///
/// Case-sensitive on the exact protocol strings (`"READY"` is [`AuthStep::Unknown`]). Never panics:
/// every input, including `""` and any future value, has a variant.
pub fn auth_step(auth_state: &str) -> AuthStep {
    match auth_state {
        "wait_phone" => AuthStep::WaitPhone,
        "wait_qr_confirmation" => AuthStep::WaitQrConfirmation,
        "wait_code" => AuthStep::WaitCode,
        "wait_password" => AuthStep::WaitPassword,
        "wait_email_address" => AuthStep::WaitEmailAddress,
        "wait_email_code" => AuthStep::WaitEmailCode,
        "wait_registration" => AuthStep::WaitRegistration,
        "ready" => AuthStep::Ready,
        "wait_premium_purchase" => AuthStep::WaitPremiumPurchase,
        "starting" | "wait_tdlib_parameters" | "logging_out" | "closing" | "closed" => {
            AuthStep::Transitional
        }
        _ => AuthStep::Unknown,
    }
}

/// Whether the Settings -> Telegram auth-step panel is actionable.
///
/// All five must hold: a live core connection, the reader enabled, the service pipe up, recoverable
/// login state supported, and a received `service` snapshot. Missing any one of them is not
/// actionable; a retained snapshot still renders muted rather than inferring a step.
pub fn auth_controls_visible(core_live: bool, s: &CoreTelegramState) -> bool {
    core_live && s.enabled && s.service_online && s.state_supported && s.service.is_some()
}

/// Whether phone-code (or email-code) resend is offered, and how soon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResendState {
    /// `next_code_type` or `resend_at` is missing: a missing optional field means unavailable.
    Unavailable,
    /// `resend_at` is still in the future; `secs_left` is the whole seconds remaining.
    Wait { secs_left: u64 },
    /// Both fields are present and `resend_at` is now or in the past.
    Ready,
}

/// Gate `resend_code()` from the current auth details and a Unix-seconds clock.
///
/// Both `next_code_type` and `resend_at` must be present; a missing optional field means
/// unavailable, even when the other field is set. `resend_at` in the future yields [`ResendState::Wait`];
/// `resend_at == now` or in the past yields [`ResendState::Ready`].
pub fn resend_state(details: &CoreTelegramAuthDetails, now_unix_secs: i64) -> ResendState {
    match (&details.next_code_type, details.resend_at) {
        (None, _) | (_, None) => ResendState::Unavailable,
        (Some(_), Some(at)) if at > now_unix_secs => ResendState::Wait {
            secs_left: (at - now_unix_secs) as u64,
        },
        (Some(_), Some(_)) => ResendState::Ready,
    }
}

#[cfg(test)]
mod tests;
