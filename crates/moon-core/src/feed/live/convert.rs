//! Pure moonproto-to-terminal snapshot projections (license/client-settings/lev/runtime),
//! targeted edits to retained settings snapshots, and order-row (`OrderRow`) construction.

use std::collections::HashMap;
use std::sync::Arc;

use moonproto::state::{
    Order, OrderTraceChartPoint, OrderTraceLine, TelegramActiveProxy, TelegramAuthDetails,
    TelegramCodeType, TelegramError, TelegramServiceState, TelegramState,
};
use moonproto::{Event, MoonClient};

use crate::config::TakeProfitMode;
use crate::feed::strategies::strat_kind_name;
use crate::feed::{
    ApiKeyExpiry, ClientSettings, ClientSettingsEdit, ConnFault, ConnFaultKind, CoreIdentityFacts,
    CoreInitStep, CoreProblem, CoreProblemCategory, CoreProblems, CoreStartupState,
    CoreStartupStatus, CoreSysStatus, CoreTelegramActiveProxy, CoreTelegramAuthDetails,
    CoreTelegramCodeType, CoreTelegramError, CoreTelegramService, CoreTelegramState,
    EngineActionKind, EngineActionResult, LicenseState, NewsSnapshot, OrderRow, OrderTrace,
    OrderTracePoint, ProfitState, RuntimeState, WalletKind,
};

mod client_edit;
mod identity;
mod news_trace;
mod orders;
mod settings;
mod wallet;

pub(super) use client_edit::apply_client_settings_edit;
pub(super) use identity::{
    api_key_expiry_from_proto, bind_fault, conn_fault_from_proto, endpoint_fault, key_fault,
    stall_fault,
};
pub(super) use news_trace::{
    license_state_from_proto, news_snapshot_from_proto, problems_from_proto, telegram_from_proto,
};
pub(super) use orders::build_order_rows;
pub use orders::{percentage_stop_price, percentage_take_price};
pub(super) use settings::{
    client_settings_from_proto, profit_state_from_proto, runtime_state_from_proto,
    settings_event_snapshot, snapshot_when, startup_status_from_proto, sys_status_from_proto,
};
pub(super) use wallet::{engine_action_result, folders_from_proto, strategies_publish_sig};

#[cfg(test)]
use identity::api_days_and_unlimited;
#[cfg(test)]
use news_trace::{problems_supported, wire_text};
#[cfg(test)]
use orders::{overlay_captured_rows, stop_distance_pct, stop_loss_line_price};
#[cfg(test)]
use wallet::display_folder_paths;

#[cfg(test)]
mod tests;
