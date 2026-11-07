//! Log redaction must happen before a diagnostic line enters any model-facing buffer.

use super::*;

/// Secret-labelled lines must be removed before storage without erasing useful order evidence.
#[test]
fn credential_lines_are_removed_but_order_diagnostics_remain_readable() {
    let cfg = AppConfig::headless(Vec::new());
    assert_eq!(
        redact("API_KEY=do-not-send", &cfg),
        "[credential-bearing line redacted]"
    );
    assert_eq!(
        redact("Authorization: Bearer do-not-send", &cfg),
        "[credential-bearing line redacted]"
    );
    assert_eq!(
        redact("Order 42 BTCUSDT cancelled: insufficient balance", &cfg),
        "Order 42 BTCUSDT cancelled: insufficient balance"
    );
}
