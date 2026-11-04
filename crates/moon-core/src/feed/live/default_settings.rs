//! The settings a core holds before anybody has changed one, through the live conversions.
//!
//! A synthetic core has no wire to read them from, and the terminal's core settings — the popup and
//! the expert window — draw nothing until they arrive. `moonproto` knows what a fresh core holds;
//! running that through the same two projections the live feed uses keeps a synthetic core from
//! ever showing a field, or a value, a real one could not.

use moonproto::ClientSettingsCommand;
use moonproto::shared_config::SharedConfig;

use super::{convert, shared_config};
use crate::feed::{ClientSettings, CoreConfig};

/// The compact settings and the full configuration of a core nobody has configured yet.
///
/// Returns:
///     What `FeedMsg::ClientSettings` and `FeedMsg::CoreConfig` carry on a live core.
pub(in crate::feed) fn default_core_settings() -> (ClientSettings, CoreConfig) {
    (
        convert::client_settings_from_proto(&ClientSettingsCommand::default()),
        shared_config::core_config_from_proto(&SharedConfig::default()),
    )
}
