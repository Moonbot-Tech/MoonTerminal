//! Draft endpoint values and duplicate peers, independent of connection state.

use std::collections::HashMap;

use moon_core::config::{ServerConfig, endpoint_from_key};
use moon_core::feed::CoreEndpoint;

/// One cell's endpoint and another draft row sharing it, if any.
pub(super) struct EndpointCell {
    pub(super) text: String,
    pub(super) duplicate_name: Option<String>,
}

/// Decode each draft key once and mark every duplicate, including inactive and hidden rows.
/// Returns cells in draft order; unknown endpoints never count as duplicates.
pub(super) fn endpoint_cells(servers: &[ServerConfig]) -> Vec<EndpointCell> {
    let endpoints: Vec<_> = servers
        .iter()
        .map(|s| endpoint_from_key(s.key.expose()))
        .collect();
    let mut peers: HashMap<CoreEndpoint, Vec<usize>> = HashMap::new();
    for (index, endpoint) in endpoints.iter().enumerate() {
        if let Some(endpoint) = endpoint {
            peers.entry(*endpoint).or_default().push(index);
        }
    }
    endpoints
        .iter()
        .enumerate()
        .map(|(index, endpoint)| EndpointCell {
            text: endpoint_text(*endpoint),
            duplicate_name: endpoint
                .and_then(|endpoint| peers.get(&endpoint))
                .and_then(|indices| indices.iter().find(|&&other| other != index))
                .map(|&other| servers[other].name.clone()),
        })
        .collect()
}

/// Format the full socket address, bracketing IPv6 to keep its port unambiguous.
fn endpoint_text(endpoint: Option<CoreEndpoint>) -> String {
    endpoint
        .map(|endpoint| std::net::SocketAddr::new(endpoint.address, endpoint.port).to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
