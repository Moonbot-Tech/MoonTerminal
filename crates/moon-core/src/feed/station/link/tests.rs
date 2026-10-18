//! Donor client configuration contracts without opening a transport.

use super::*;

/// Reverting to unscaled Compact would halve the pre-entry rows retained for every seed.
#[test]
fn donor_requests_twice_the_compact_history() {
    let config = donor_config(ClientConfig::new("127.0.0.1", 1, [0; 16], [0; 16]));
    assert_eq!(
        config.market_history,
        MarketHistorySizing::CompactBudgetPercent(200)
    );
}
