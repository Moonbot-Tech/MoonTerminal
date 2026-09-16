//! Shared lookup of the configured core colour for detection cards and chart arrivals.

use moon_core::config::ServerConfig;
use moon_core::session::CoreId;

/// Return the configured RGB colour for `core`, or `None` when its server is absent.
/// Callers retain their own fallback because cards and flashing borders have different needs.
pub(crate) fn core_color(servers: &[ServerConfig], core: CoreId) -> Option<[u8; 3]> {
    servers
        .iter()
        .find(|server| server.id == core)
        .map(|server| server.color)
}

#[cfg(test)]
mod tests;
