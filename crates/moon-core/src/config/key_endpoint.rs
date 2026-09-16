//! Decode draft endpoints with the same defaults used by live connections.

use std::net::{IpAddr, Ipv4Addr};

use crate::feed::CoreEndpoint;

/// Decode a pasted key without connecting; unreadable or empty keys have no endpoint.
/// Legacy exports retain the live connection's localhost and port-3000 defaults.
pub fn endpoint_from_key(key: &str) -> Option<CoreEndpoint> {
    let info = moonproto::parse_key_info(key)?;
    Some(endpoint_from_network(info.network.as_ref()))
}

/// Resolve parsed network metadata for both Settings and the live feed.
/// Missing addresses and zero or absent ports retain the established connection defaults.
pub(crate) fn endpoint_from_network(
    network: Option<&moonproto::ImportedNetworkConfig>,
) -> CoreEndpoint {
    let address = network
        .and_then(|network| network.address)
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    let port = network
        .map(|network| network.port)
        .filter(|port| *port != 0)
        .unwrap_or(3000);
    CoreEndpoint { address, port }
}

#[cfg(test)]
mod tests;
