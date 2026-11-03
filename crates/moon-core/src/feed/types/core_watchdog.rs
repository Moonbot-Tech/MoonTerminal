//! Moon-core-owned mirror of the core's observers ("Защита от зависаний") and of the service
//! settings acknowledgements that answer every service-settings action.
//!
//! Field-for-field the shape of moonproto's `WatchDogState` / `WatchDogSettings` family, with no
//! moonproto import: the UI crate has no moonproto dependency. The mapping lives in
//! `feed::live::watchdog`.
//!
//! The observer block is ONE writer for four settings that the safe-share snapshot also carries
//! (`trading.orders_control.sign_orders`, `h_pos_report`, `h_pos_auto_sell`,
//! `trading.h_pos_black_list_text`): the projection of that snapshot leaves them out, so the
//! terminal never has two routes fighting over one field. See `feed::SpecialSettings`.
//!
//! Credentials never come back from the core. An edit carries them only when the user typed a new
//! one, and the hand-written `Debug` impls below never print them.

use std::fmt;
use std::net::Ipv4Addr;

/// Shortest key or secret the core accepts: "Supplied keys must contain more than five
/// characters" (moonproto `WatchDogEdit`).
pub const WATCHDOG_MIN_CREDENTIAL_CHARS: usize = 6;

/// The core's observer snapshot. Absent on a core older than MoonBot 7.72 L6 with service-settings
/// support — the store then holds `None` and the UI keeps every control unavailable.
#[derive(Clone, Default, PartialEq)]
pub struct CoreWatchdogState {
    /// Monotonic within one core session; moonproto already drops an older one.
    pub revision: u64,
    /// The observer controller exists on the core.
    pub available: bool,
    /// The core's licence permits editing observer ENTRIES. The settings above the table (sender
    /// address and flags) stay editable without it.
    pub can_edit: bool,
    /// The core's platform/exchange supports observers, independently of the licence.
    pub supported: bool,
    /// IPv4 of the observer server this bot reports to ("Control VDS IP"); empty or `0.0.0.0`
    /// disables sending.
    pub server_ip: String,
    /// Mark this bot's orders as its own; also required to send keepalives to the observer.
    pub sign_orders: bool,
    /// Telegram notice about a confirmed unaccounted position.
    pub report: bool,
    /// Close a confirmed unaccounted position; requires position control on a row.
    pub auto_sell: bool,
    /// Position-control exclusions, comma-separated — the same list as
    /// `trading.h_pos_black_list_text`.
    pub blacklist: String,
    /// The core's observer UDP listener is running.
    pub udp_active: bool,
    /// Why the listener is not running, empty when it is or when nothing failed.
    pub udp_error: String,
    pub entries: Vec<CoreWatchdogEntry>,
}

impl fmt::Debug for CoreWatchdogState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CoreWatchdogState")
            .field("revision", &self.revision)
            .field("available", &self.available)
            .field("can_edit", &self.can_edit)
            .field("supported", &self.supported)
            .field("entries", &self.entries.len())
            .finish_non_exhaustive()
    }
}

impl CoreWatchdogState {
    /// The edit that changes nothing: every row by its core-assigned id, no credentials.
    ///
    /// The one way an edit is started, as moonproto's `WatchDogState::edit` is: an edit built from
    /// anything but a fresh snapshot of THIS core session would carry ids the core no longer knows.
    pub fn edit(&self) -> WatchdogSettingsEdit {
        WatchdogSettingsEdit {
            server_ip: self.server_ip.clone(),
            sign_orders: self.sign_orders,
            report: self.report,
            auto_sell: self.auto_sell,
            blacklist: self.blacklist.clone(),
            entries: self
                .entries
                .iter()
                .map(|e| WatchdogEntryEdit {
                    id: e.id.clone(),
                    name: e.name.clone(),
                    api_key: None,
                    api_secret: None,
                    rsa: e.rsa,
                    orders: e.orders,
                    positions: e.positions,
                    ip: e.ip,
                    price: e.price,
                    time: e.time,
                })
                .collect(),
        }
    }

    /// The row the core knows under `id`, if it still does.
    pub fn entry(&self, id: &str) -> Option<&CoreWatchdogEntry> {
        if id.is_empty() {
            return None;
        }
        self.entries.iter().find(|e| e.id == id)
    }
}

/// One observed account on the core. Read-only state beside the editable fields.
#[derive(Clone, Default, PartialEq)]
pub struct CoreWatchdogEntry {
    /// Opaque, valid for this core session only; never reused after a reconnect.
    pub id: String,
    /// Observed-account label.
    pub name: String,
    /// What the core shows of the saved key — never the key itself.
    pub api_key_hint: String,
    pub key_set: bool,
    pub secret_set: bool,
    /// The observed account signs with RSA keys.
    pub rsa: bool,
    /// Watch buys by price distance and age.
    pub orders: bool,
    /// Watch unaccounted positions; requires core support and a licence.
    pub positions: bool,
    /// Cancel related buys when the observed bots stop sending heartbeats.
    pub ip: bool,
    /// Price distance in per cent at or below which a buy becomes a cancellation candidate; zero
    /// disables the check.
    pub price: f64,
    /// Seconds without an order update after which a buy becomes a candidate; zero disables it.
    pub time: i32,
    /// Calendar date the row was added on the core, as the core formats it.
    pub added: String,
    pub status: WatchdogStatus,
    /// Why the row is in [`WatchdogStatus::Error`], empty otherwise.
    pub error: String,
    /// How many observed bots are currently sending heartbeats.
    pub online_count: i32,
    /// Another row already observes the same account.
    pub duplicate: bool,
}

impl fmt::Debug for CoreWatchdogEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CoreWatchdogEntry")
            .field("id", &self.id)
            .field("status", &self.status)
            .field("online_count", &self.online_count)
            .finish_non_exhaustive()
    }
}

/// A row's state as the core reports it. `Unknown` keeps a code a newer core may add.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WatchdogStatus {
    #[default]
    Added,
    Error,
    Ready,
    Checking,
    Removing,
    Unknown(u8),
}

/// Upserts plus the observer-wide settings. Rows left out are NOT deleted on the core —
/// deletion is [`WatchdogCmd::Remove`].
#[derive(Clone, Default, PartialEq)]
pub struct WatchdogSettingsEdit {
    pub server_ip: String,
    pub sign_orders: bool,
    pub report: bool,
    pub auto_sell: bool,
    pub blacklist: String,
    pub entries: Vec<WatchdogEntryEdit>,
}

impl fmt::Debug for WatchdogSettingsEdit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WatchdogSettingsEdit")
            .field("sign_orders", &self.sign_orders)
            .field("report", &self.report)
            .field("auto_sell", &self.auto_sell)
            .field("entries", &self.entries)
            .finish_non_exhaustive()
    }
}

/// One row to add (empty `id`) or update.
#[derive(Clone, Default, PartialEq)]
pub struct WatchdogEntryEdit {
    pub id: String,
    pub name: String,
    /// `None` keeps the saved key. Never prefilled from the core: it does not return keys.
    pub api_key: Option<String>,
    /// `None` keeps the saved secret.
    pub api_secret: Option<String>,
    pub rsa: bool,
    pub orders: bool,
    pub positions: bool,
    pub ip: bool,
    pub price: f64,
    pub time: i32,
}

impl fmt::Debug for WatchdogEntryEdit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WatchdogEntryEdit")
            .field("id", &self.id)
            .field("api_key", &self.api_key.as_ref().map(|_| "<new>"))
            .field("api_secret", &self.api_secret.as_ref().map(|_| "<new>"))
            .finish_non_exhaustive()
    }
}

/// Why an edit cannot be sent. Checked before the send so a typo is named beside the control
/// rather than coming back as the core's refusal of the whole packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchdogEditError {
    /// The sender address is neither empty nor an IPv4 address.
    ServerIp,
    /// Row `index` carries a key shorter than [`WATCHDOG_MIN_CREDENTIAL_CHARS`].
    ApiKey { index: usize },
    /// Row `index` carries a secret shorter than [`WATCHDOG_MIN_CREDENTIAL_CHARS`].
    ApiSecret { index: usize },
    /// Row `index` is new and lacks a key or a secret: the core has nothing to keep for it.
    NewRowCredentials { index: usize },
    /// Row `index` has a negative or non-finite price distance.
    Price { index: usize },
    /// Row `index` has a negative age.
    Time { index: usize },
}

impl WatchdogSettingsEdit {
    /// Check what the core and moonproto would refuse, row by row.
    ///
    /// moonproto refuses a negative or non-finite price/time locally and the core refuses an empty
    /// or short credential; a new row without both credentials would be saved with nothing to log
    /// in with, so it is refused here too.
    ///
    /// Returns:
    ///     The first problem in row order, the sender address first.
    pub fn validate(&self) -> Result<(), WatchdogEditError> {
        let ip = self.server_ip.trim();
        if !ip.is_empty() && ip.parse::<Ipv4Addr>().is_err() {
            return Err(WatchdogEditError::ServerIp);
        }
        let short = |v: &Option<String>| {
            v.as_ref()
                .is_some_and(|s| s.chars().count() < WATCHDOG_MIN_CREDENTIAL_CHARS)
        };
        for (index, row) in self.entries.iter().enumerate() {
            if row.id.is_empty() && (row.api_key.is_none() || row.api_secret.is_none()) {
                return Err(WatchdogEditError::NewRowCredentials { index });
            }
            if short(&row.api_key) {
                return Err(WatchdogEditError::ApiKey { index });
            }
            if short(&row.api_secret) {
                return Err(WatchdogEditError::ApiSecret { index });
            }
            if !row.price.is_finite() || row.price < 0.0 {
                return Err(WatchdogEditError::Price { index });
            }
            if row.time < 0 {
                return Err(WatchdogEditError::Time { index });
            }
        }
        Ok(())
    }
}

/// One user action for the core's observers, nested under a single `CoreCmd` arm.
///
/// Every variant but [`Self::Refresh`] is a LIVE action (`feed::live::stale`): moonproto forbids
/// replaying a configuration change or a cancellation after a reconnect, and row ids belong to the
/// session that issued them. Debug prints only [`Self::name`] — never a credential.
#[derive(Clone, PartialEq)]
pub enum WatchdogCmd {
    /// Ask for the current snapshot; answered through a `get` acknowledgement.
    Refresh,
    /// Upsert rows and save the observer-wide settings.
    Apply(WatchdogSettingsEdit),
    /// Delete one row by its core-assigned id.
    Remove(String),
    /// Ask the controller to cancel the tracked buys of the observed accounts.
    CancelBuys,
}

impl WatchdogCmd {
    /// Snake_case moonproto `MoonWatchDogs` method this command maps to.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Refresh => "refresh",
            Self::Apply(_) => "apply",
            Self::Remove(_) => "remove",
            Self::CancelBuys => "cancel_buys",
        }
    }

    /// The action an acknowledgement is tracked for, or `None` for a refresh: a refresh changes
    /// nothing, so nobody waits on its verdict.
    pub fn tracked_action(&self) -> Option<ServiceAction> {
        match self {
            Self::Refresh => None,
            Self::Apply(_) => Some(ServiceAction::WatchdogApply),
            Self::Remove(_) => Some(ServiceAction::WatchdogRemove),
            Self::CancelBuys => Some(ServiceAction::WatchdogCancelBuys),
        }
    }
}

impl fmt::Debug for WatchdogCmd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Which service block an acknowledgement belongs to. The core answers both through one command
/// (`ServiceSettingsApplied`), so one track per domain keeps a Telegram reply from settling an
/// observer request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceDomain {
    Watchdog,
    Telegram,
}

/// What this terminal asked a service block to do, for the banner that reports the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceAction {
    WatchdogApply,
    WatchdogRemove,
    WatchdogCancelBuys,
    /// `MoonTelegram::set_outgoing_bot` — own-bot switch and token.
    TelegramOutgoingBot,
    /// `MoonTelegram::generate_channel_pin`.
    TelegramChannelPin,
    /// `MoonTelegram::reset_channel`.
    TelegramResetChannel,
}

impl ServiceAction {
    /// The block whose acknowledgement answers this action.
    pub fn domain(self) -> ServiceDomain {
        match self {
            Self::WatchdogApply | Self::WatchdogRemove | Self::WatchdogCancelBuys => {
                ServiceDomain::Watchdog
            }
            Self::TelegramOutgoingBot | Self::TelegramChannelPin | Self::TelegramResetChannel => {
                ServiceDomain::Telegram
            }
        }
    }
}

impl ServiceTrack {
    /// Whether the request a surface queued when this track had registered `sent_before` requests
    /// has been answered — sent and settled, refused locally, or lost with the link.
    pub fn settled_since(&self, sent_before: u64) -> bool {
        self.sent != sent_before && self.pending.is_none()
    }
}

/// One acknowledgement as the core sent it, for any service block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreServiceResult {
    pub domain: ServiceDomain,
    /// The request id this answers; `None` when the core sent one this build cannot read, which
    /// then answers nothing this terminal is waiting for.
    pub request_id: Option<u64>,
    /// The change took effect in memory, even if saving it failed.
    pub applied: bool,
    /// Empty on success; otherwise the core's own words, shown as sent.
    pub error: String,
}

/// How the last tracked request of one service block ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceVerdict {
    /// Applied and saved.
    Applied,
    /// Applied in memory, but the core could not save it; the text is the core's.
    AppliedNotSaved(String),
    /// The core refused it; the text is the core's, possibly empty.
    Refused(String),
    /// It never left this terminal: moonproto refused it locally, or it was dropped because the
    /// link was not up. The text is a technical token for the log line beside it.
    NotSent(String),
    /// The link dropped before the core answered. Nothing is replayed.
    Lost,
}

impl ServiceVerdict {
    /// The verdict an acknowledgement states.
    pub fn from_result(result: &CoreServiceResult) -> Self {
        match (result.applied, result.error.is_empty()) {
            (true, true) => Self::Applied,
            (true, false) => Self::AppliedNotSaved(result.error.clone()),
            (false, _) => Self::Refused(result.error.clone()),
        }
    }
}

/// A tracked request the core has not answered yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServicePending {
    pub request_id: u64,
    pub action: ServiceAction,
}

/// The newest tracked request of one service block, and how the one before it ended.
///
/// Only the NEWEST request is waited on: the core answers each one, but a banner reports the last
/// thing the user did, and an older answer arriving after a newer request must not settle it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceTrack {
    pub pending: Option<ServicePending>,
    /// The last settled request and its verdict.
    pub outcome: Option<(ServiceAction, ServiceVerdict)>,
    /// How many requests this track has registered, sent or not. A surface that queued one notes
    /// this count first and waits for it to move with nothing pending: the outcome in place before
    /// the move answers an EARLIER request, possibly with the same words.
    pub sent: u64,
}

impl ServiceTrack {
    /// Note that `action` left the terminal as `request_id`, or that it could not.
    ///
    /// Returns:
    ///     Whether the track changed.
    pub fn note_sent(&mut self, action: ServiceAction, sent: Result<u64, String>) -> bool {
        self.sent = self.sent.wrapping_add(1);
        match sent {
            Ok(request_id) => {
                self.pending = Some(ServicePending { request_id, action });
                // The previous verdict described an earlier request; the banner now waits.
                self.outcome = None;
            }
            Err(error) => {
                self.pending = None;
                self.outcome = Some((action, ServiceVerdict::NotSent(error)));
            }
        }
        true
    }

    /// Settle the pending request if `result` answers it.
    ///
    /// Returns:
    ///     Whether the track changed — `false` for an answer to someone else's request, or to a
    ///     refresh, both of which only update the snapshot.
    pub fn note_result(&mut self, result: &CoreServiceResult) -> bool {
        let Some(pending) = self.pending else {
            return false;
        };
        if result.request_id != Some(pending.request_id) {
            return false;
        }
        self.pending = None;
        self.outcome = Some((pending.action, ServiceVerdict::from_result(result)));
        true
    }

    /// The link went down: a pending request will not be answered and is not replayed.
    ///
    /// Returns:
    ///     Whether the track changed.
    pub fn note_lost(&mut self) -> bool {
        let Some(pending) = self.pending.take() else {
            return false;
        };
        self.outcome = Some((pending.action, ServiceVerdict::Lost));
        true
    }
}

/// One tracked track per service block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceTracks {
    pub watchdog: ServiceTrack,
    pub telegram: ServiceTrack,
}

impl ServiceTracks {
    /// The track of one block.
    pub fn get(&self, domain: ServiceDomain) -> &ServiceTrack {
        match domain {
            ServiceDomain::Watchdog => &self.watchdog,
            ServiceDomain::Telegram => &self.telegram,
        }
    }

    /// The track of one block, for an update.
    pub fn get_mut(&mut self, domain: ServiceDomain) -> &mut ServiceTrack {
        match domain {
            ServiceDomain::Watchdog => &mut self.watchdog,
            ServiceDomain::Telegram => &mut self.telegram,
        }
    }
}

#[cfg(test)]
mod tests;
