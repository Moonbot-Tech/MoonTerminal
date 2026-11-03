//! Outbound intents for the core's built-in Telegram reader and its outgoing bot.
//!
//! Maps each [`TelegramCmd`] onto the matching `MoonTelegram` method. A successful call means
//! queued, not completed: the reader and login intents are observed through the next snapshot, the
//! outgoing-bot and channel actions through their `ServiceSettingsApplied` acknowledgement, matched
//! by request id. Log lines name only the method; they never carry a payload.

use moonproto::MoonClient;
use moonproto::state::{TelegramLoginMode, TelegramProxy};

use crate::feed::{CoreTelegramLoginMode, CoreTelegramProxy, FeedMsg, ServiceAction, TelegramCmd};
use crate::session::CoreId;

/// Execute one Telegram intent on the connected core.
///
/// Captures [`TelegramCmd::name`] before the match consumes the payload, then logs only that
/// name plus the moonproto error's Display — never a token, a PIN or a login secret. `Logout` logs
/// at warn even on success: it resets the shared local account for every MoonBot process on that
/// machine.
///
/// Args:
///     client: Connected moonproto client.
///     core: Core whose command this is, for the log label.
///     cmd: The typed intent.
///
/// Returns:
///     For an outgoing-bot or channel action, the `ServiceRequest` the store tracks the core's
///     acknowledgement by; `None` for the reader and login intents, observed through the snapshot.
pub(super) fn handle(client: &MoonClient, core: CoreId, cmd: TelegramCmd) -> Option<FeedMsg> {
    let what = cmd.name();
    let is_logout = matches!(cmd, TelegramCmd::Logout);
    let tg = client.telegram();
    let result = match cmd {
        TelegramCmd::SetOutgoingBot { own_bot, token } => {
            return Some(tracked(
                core,
                what,
                ServiceAction::TelegramOutgoingBot,
                tg.set_outgoing_bot(own_bot, token.as_deref()),
            ));
        }
        TelegramCmd::GenerateChannelPin => {
            return Some(tracked(
                core,
                what,
                ServiceAction::TelegramChannelPin,
                tg.generate_channel_pin(),
            ));
        }
        TelegramCmd::ResetChannel => {
            return Some(tracked(
                core,
                what,
                ServiceAction::TelegramResetChannel,
                tg.reset_channel(),
            ));
        }
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
    None
}

/// Log a tracked action's local outcome and wrap it for the store.
///
/// Args:
///     core: Core the action went to, for the log label.
///     what: The moonproto method name.
///     action: The tracked action, which names the service block its answer settles.
///     result: The request id moonproto queued it as, or why it did not.
///
/// Returns:
///     The `ServiceRequest` carrying the request id, or the local refusal.
fn tracked(
    core: CoreId,
    what: &str,
    action: ServiceAction,
    result: Result<u64, moonproto::MoonClientError>,
) -> FeedMsg {
    let sent = match result {
        Ok(request_id) => {
            log::info!(
                "core {} telegram {what} sent as request {request_id}",
                crate::feed::core_label(core)
            );
            Ok(request_id)
        }
        Err(error) => {
            log::warn!(
                "core {} telegram {what} failed: {error}",
                crate::feed::core_label(core)
            );
            Err(error.to_string())
        }
    };
    FeedMsg::ServiceRequest { action, sent }
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
