//! Manual-trading settings, order terms, and core command helpers for [`Backend`].

#[cfg(test)]
mod tests;

mod exits;
mod ownership;
mod panic;
mod selectors;
mod sizing;
mod slots;
mod stops;
mod strategy_state;
mod terms;
mod types;

pub(crate) use terms::{ManualOrderTerms, hook_of, manual_strategy_id, strat_field_value};
pub(crate) use types::{
    FIELD_USE_HOOK_STRATEGY, IGNORE_SELL_LOCAL_TTL, IgnoreSellLocal, MANUAL_STRATEGY_KIND,
    ManualSource, ManualStop, MsExitOverlay, PendingStop, SettleKey,
};
