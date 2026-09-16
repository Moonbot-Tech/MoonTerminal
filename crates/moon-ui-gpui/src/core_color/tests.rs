//! Core identity regressions for the shared detection and arrival-flash colour lookup.

use super::core_color;
use moon_core::config::ServerConfig;

/// Taking the first server instead of matching the core paints another connection's flash.
#[test]
fn flash_core_color_follows_identity_not_server_order() {
    let first: ServerConfig = serde_json::from_str(r#"{"id":91,"color":[255,0,0]}"#).unwrap();
    let second: ServerConfig = serde_json::from_str(r#"{"id":7,"color":[0,255,0]}"#).unwrap();
    let mut servers = vec![first, second];
    assert_eq!(core_color(&servers, 7), Some([0, 255, 0]));
    servers.reverse();
    assert_eq!(core_color(&servers, 7), Some([0, 255, 0]));
    assert_eq!(core_color(&servers, 8), None);
}
