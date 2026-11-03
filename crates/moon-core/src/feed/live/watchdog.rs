//! The core's observers ("Защита от зависаний") over moonproto's service-settings API. What every
//! service action shares — the acknowledgement, the dropped-request report — is in
//! `feed::live::service`.
//!
//! Outbound: each [`WatchdogCmd`] maps onto one `MoonWatchDogs` method. A successful call means
//! queued locally — the core's verdict arrives as `ServiceSettingsApplied`, matched by request id.
//! Inbound: the retained snapshot is projected into moonproto-free types.
//! Log lines name the method and the request id, never a credential.

use moonproto::MoonClient;
use moonproto::state::{
    WatchDogEdit, WatchDogEntry, WatchDogSettings, WatchDogState, WatchDogStatus,
};

use crate::feed::{
    CoreWatchdogEntry, CoreWatchdogState, FeedMsg, WatchdogCmd, WatchdogEntryEdit,
    WatchdogSettingsEdit, WatchdogStatus,
};
use crate::session::CoreId;

/// Execute one observer action on the connected core.
///
/// Args:
///     client: Connected moonproto client.
///     core: Core whose command this is, for the log label.
///     cmd: The typed action.
///
/// Returns:
///     The `ServiceRequest` the store tracks the answer by, or `None` for a refresh, whose answer
///     only updates the snapshot.
pub(super) fn handle(client: &MoonClient, core: CoreId, cmd: WatchdogCmd) -> Option<FeedMsg> {
    let what = cmd.name();
    let action = cmd.tracked_action();
    let wd = client.watchdogs();
    let result = match cmd {
        WatchdogCmd::Refresh => wd.refresh(),
        WatchdogCmd::Apply(edit) => wd.apply(&settings_to_proto(edit)),
        WatchdogCmd::Remove(id) => wd.remove(&id),
        WatchdogCmd::CancelBuys => wd.cancel_buys(),
    };
    let sent = match result {
        Ok(request_id) => {
            log::info!(
                "core {} watchdog {what} sent as request {request_id}",
                crate::feed::core_label(core)
            );
            Ok(request_id)
        }
        Err(error) => {
            log::warn!(
                "core {} watchdog {what} failed: {error}",
                crate::feed::core_label(core)
            );
            Err(error.to_string())
        }
    };
    action.map(|action| FeedMsg::ServiceRequest { action, sent })
}

/// Project moonproto's observer snapshot.
pub(super) fn watchdog_from_proto(state: &WatchDogState) -> CoreWatchdogState {
    CoreWatchdogState {
        revision: state.revision,
        available: state.available,
        can_edit: state.can_edit,
        supported: state.supported,
        server_ip: state.server_ip.clone(),
        sign_orders: state.sign_orders,
        report: state.report,
        auto_sell: state.auto_sell,
        blacklist: state.blacklist.clone(),
        udp_active: state.udp_active,
        udp_error: state.udp_error.clone(),
        entries: state.entries.iter().map(entry_from_proto).collect(),
    }
}

/// Project one observer row.
fn entry_from_proto(e: &WatchDogEntry) -> CoreWatchdogEntry {
    CoreWatchdogEntry {
        id: e.id.clone(),
        name: e.name.clone(),
        api_key_hint: e.api_key_hint.clone(),
        key_set: e.key_set,
        secret_set: e.secret_set,
        rsa: e.rsa,
        orders: e.orders,
        positions: e.positions,
        ip: e.ip,
        price: e.price,
        time: e.time,
        added: e.added.clone(),
        status: match e.status {
            WatchDogStatus::Added => WatchdogStatus::Added,
            WatchDogStatus::Error => WatchdogStatus::Error,
            WatchDogStatus::Ready => WatchdogStatus::Ready,
            WatchDogStatus::Checking => WatchdogStatus::Checking,
            WatchDogStatus::Removing => WatchdogStatus::Removing,
            WatchDogStatus::Unknown(code) => WatchdogStatus::Unknown(code),
        },
        error: e.error.clone(),
        online_count: e.online_count,
        duplicate: e.duplicate,
    }
}

/// The wire edit for one terminal edit. Field for field; credentials pass through as given, so
/// `None` keeps the saved value on the core.
fn settings_to_proto(edit: WatchdogSettingsEdit) -> WatchDogSettings {
    WatchDogSettings {
        server_ip: edit.server_ip.trim().to_string(),
        sign_orders: edit.sign_orders,
        report: edit.report,
        auto_sell: edit.auto_sell,
        blacklist: edit.blacklist,
        entries: edit.entries.into_iter().map(entry_to_proto).collect(),
    }
}

/// The wire upsert for one row.
fn entry_to_proto(e: WatchdogEntryEdit) -> WatchDogEdit {
    WatchDogEdit {
        id: e.id,
        name: e.name,
        api_key: e.api_key,
        api_secret: e.api_secret,
        rsa: e.rsa,
        orders: e.orders,
        positions: e.positions,
        ip: e.ip,
        price: e.price,
        time: e.time,
    }
}

#[cfg(test)]
mod tests;
