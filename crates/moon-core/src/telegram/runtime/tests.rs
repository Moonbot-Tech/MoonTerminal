//! Runtime-only regression tests for the Telegram transport owner.

use super::*;

const RUNTIME_SOURCE: &str = include_str!("../runtime.rs");
const EMPTY_TOKEN_GUARD: &str =
    "        if config.token.is_empty() {\n            return None;\n        }\n";
const FIRST_SERVICE_CONSTRUCTION: &str = "        let alive = Arc::new(());";

/// `telegram/runtime.rs:TelegramService::start_localized` must keep the empty-token guard before
/// its first service construction; removing that guard would let a default installation create
/// Telegram background transport and reach network code although the feature is disabled.
#[test]
fn empty_token_guard_dominates_telegram_service_construction() {
    let guard_offset = RUNTIME_SOURCE
        .find(EMPTY_TOKEN_GUARD)
        .expect("the empty-token off switch must remain as a complete guard block");
    let construction_offset = RUNTIME_SOURCE
        .find(FIRST_SERVICE_CONSTRUCTION)
        .expect("start_localized must still identify its first service construction");

    assert!(
        guard_offset < construction_offset,
        "the empty-token guard must dominate service construction before any transport can start"
    );

    let config = TelegramConfig::default();
    assert!(
        TelegramService::start(&config).is_none(),
        "an empty credential must be the hard off switch rather than an inactive service handle"
    );
}
