//! Outbound intents for the core's built-in Telegram reader.
//!
//! Maps each [`TelegramCmd`] onto the matching `MoonTelegram` method. A successful call means
//! queued, not completed. Log lines name only the method; they never carry a payload.

use moonproto::MoonClient;
use moonproto::state::{TelegramLoginMode, TelegramProxy};

use crate::feed::{CoreTelegramLoginMode, CoreTelegramProxy, TelegramCmd};
use crate::session::CoreId;

/// Execute one Telegram intent on the connected core.
///
/// Captures [`TelegramCmd::name`] before the match consumes the payload, then logs only that
/// name plus the moonproto error's Display. `Logout` logs at warn even on success: it resets the
/// shared local account for every MoonBot process on that machine.
///
/// Args:
///     client: Connected moonproto client.
///     core: Core whose command this is, for the log label.
///     cmd: The typed intent.
pub(super) fn handle(client: &MoonClient, core: CoreId, cmd: TelegramCmd) {
    let what = cmd.name();
    let is_logout = matches!(cmd, TelegramCmd::Logout);
    let tg = client.telegram();
    let result = match cmd {
        TelegramCmd::Refresh => tg.refresh(),
        TelegramCmd::SetEnabled(on) => tg.set_enabled(on),
        TelegramCmd::SetProxy(proxy) => tg.set_proxy(proxy.into()),
        TelegramCmd::SetLoginMode(mode) => tg.set_login_mode(mode.into()),
        TelegramCmd::SetPhone(phone) => tg.set_phone(phone),
        TelegramCmd::SetCode(code) => tg.set_code(code),
        TelegramCmd::SetPassword(password) => tg.set_password(password),
        TelegramCmd::SetEmail(email) => tg.set_email(email),
        TelegramCmd::SetEmailCode(email_code) => tg.set_email_code(email_code),
        TelegramCmd::Register {
            first_name,
            last_name,
        } => tg.register(first_name, last_name),
        TelegramCmd::ResendCode => tg.resend_code(),
        TelegramCmd::Logout => tg.logout(),
    };
    match result {
        Err(error) => log::warn!(
            "core {} telegram {what} failed: {error}",
            crate::feed::core_label(core)
        ),
        Ok(()) if is_logout => log::warn!(
            "core {} telegram logout sent — resets the shared Telegram account for every MoonBot \
             process on that machine",
            crate::feed::core_label(core)
        ),
        Ok(()) => log::info!(
            "core {} telegram {what} sent",
            crate::feed::core_label(core)
        ),
    }
}

impl From<CoreTelegramProxy> for TelegramProxy {
    fn from(proxy: CoreTelegramProxy) -> Self {
        match proxy {
            CoreTelegramProxy::None => Self::None,
            CoreTelegramProxy::Socks5 {
                host,
                port,
                user,
                password,
            } => Self::Socks5 {
                host,
                port,
                user,
                password,
            },
            CoreTelegramProxy::MtProto { host, port, secret } => {
                Self::MtProto { host, port, secret }
            }
        }
    }
}

impl From<CoreTelegramLoginMode> for TelegramLoginMode {
    fn from(mode: CoreTelegramLoginMode) -> Self {
        match mode {
            CoreTelegramLoginMode::Phone => Self::Phone,
            CoreTelegramLoginMode::Qr => Self::Qr,
        }
    }
}
