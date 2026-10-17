//! Synthetic exchange identity for offline unknown-venue regression fixtures.

use std::collections::HashMap;

use crate::feed::ExchangeId;
use crate::market::{MarketDataSource, MarketStore};
use crate::session::CoreId;

/// Platform ordinal deliberately absent from the venue directory.
pub const CODE: u8 = 200;
/// Display caption reported by the synthetic core.
pub const CAPTION: &str = "EX-XX";
/// Synthetic core name for diagnostics.
pub const CORE_NAME: &str = "CORE_X";
/// Stable exchange identity used by recorder and replay storage.
pub const EXCHANGE_KEY: &str = "200:00000000";

/// Build an addressable source whose core serves an exchange outside the directory.
pub fn source(core: CoreId) -> MarketDataSource {
    let source = MarketDataSource::new(MarketStore::shared(0.0));
    source.set_provider_map(&HashMap::from([(core, core)]));
    source.set_provider_exchanges(&HashMap::from([(core, ExchangeId::new(CODE))]));
    source.set_core_venues(&HashMap::from([(
        core,
        crate::venue::CoreVenue::identify(CODE, "", Some(CAPTION)),
    )]));
    source
}

#[cfg(test)]
mod tests;
