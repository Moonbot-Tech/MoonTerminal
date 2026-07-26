//! Legacy-config constructor regressions.

use super::{config_from_legacy_enc, config_from_legacy_toml};

/// Regression target: removing group materialization from `config_from_legacy_enc` lets an older
/// encrypted config reach schema v17 without durable Size, TP, or SL state for its server group.
#[test]
fn encrypted_legacy_constructor_materializes_missing_server_groups() {
    let config = config_from_legacy_enc(
        br#"
            [[servers]]
            name = "alpha"
            key = "secret"
            group = "desk"
            market = "BTCUSDT"
        "#,
        None,
    )
    .expect("a valid decrypted legacy config must parse");

    assert_eq!(
        config.groups,
        vec![super::GroupConfig::new("desk")],
        "every migrated encrypted server group needs concrete local controls"
    );
}

/// Regression target: removing group materialization from `config_from_legacy_toml` persists the
/// oldest one-core migration without a `GroupConfig`, leaving its manual controls on fallbacks.
#[test]
fn plaintext_legacy_constructor_materializes_the_default_server_group() {
    let config = config_from_legacy_toml(
        r#"
            key = "secret"
            market = "ETHUSDT"
        "#,
        None,
    )
    .expect("a valid legacy plaintext config must parse");

    assert_eq!(
        config.groups,
        vec![super::GroupConfig::new(config.servers[0].group.clone())],
        "the oldest migration must own concrete controls for its server group"
    );
}
