//! Runtime configuration-constructor regressions.

use super::{AppConfig, BadgesConfig, ChartThemeSet, OrdersStyleSet};

/// Regression target: removing `ensure_server_group_configs` from
/// `AppConfig::build_plaintext_config` leaves the environment-backed core without durable
/// group-local Size, TP, or SL state.
#[test]
fn plaintext_config_materializes_its_server_group() {
    let config = AppConfig::build_plaintext_config(
        None,
        ChartThemeSet::default(),
        OrdersStyleSet::default(),
        BadgesConfig::default(),
        "synthetic".to_string(),
        "desk".to_string(),
        "BTCUSDT".to_string(),
        "synthetic".to_string(),
        true,
    );

    assert_eq!(
        config.groups,
        vec![super::GroupConfig::new("desk")],
        "the plaintext runtime must own concrete local controls for its server group"
    );
}
