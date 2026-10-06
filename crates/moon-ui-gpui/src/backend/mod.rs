//! Methods for the application's shared backend state ([`Backend`]). The struct is declared in
//! `main.rs`, the crate root, so its private fields are visible to descendant modules. Methods in
//! this sibling module use `pub(crate)` because private items here would not be crate-wide.

mod alert_sound;
pub(crate) mod core_warn;
mod detect_sound;
mod favorites;
mod figures;
mod manual_trading;
mod open_request;
mod problem_sound;
mod quiet;
pub(crate) mod server_chart;
pub(crate) mod station;
pub(crate) mod telegram;
#[cfg(test)]
mod tests;
pub(crate) mod traces;
pub(crate) mod trade_sound;

pub(crate) use alert_sound::AlertLeg;
pub(crate) use favorites::FavLocal;
pub(crate) use manual_trading::{
    FIELD_USE_HOOK_STRATEGY, IgnoreSellLocal, MANUAL_STRATEGY_KIND, ManualOrderTerms, ManualSource,
    ManualStop, MsExitOverlay, PendingStop, SettleKey, hook_of, manual_strategy_id,
    strat_field_value,
};
pub(crate) use open_request::{ChartHistoryScope, OpenCompareRequest, OpenMainRequest};
pub(crate) use problem_sound::ProblemSoundState;

mod chart_consumers;
mod chart_refs;
mod header_coins;
mod main_targets;
mod notify;
mod report_adapters;
mod request_routing;
mod strategy_watch;
mod warn_slices;
mod warnings;
mod workspace_entities;
mod workspace_scope;
pub(crate) use workspace_scope::WorkspaceLiveIndex;

pub(crate) use strategy_watch::{PendingStrategyEditWatch, StrategyEditToast};
pub(crate) use warn_slices::PendingWarnSlice;

use crate::Backend;
