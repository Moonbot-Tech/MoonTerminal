//! Draft endpoint values and duplicate peers, independent of connection state.
//!
//! The address cell edits `ServerConfig::endpoint_override` (#616): empty follows the key, and the
//! key's own endpoint is then the field's placeholder ([`key_placeholder`]). Everything here reads
//! the draft only and never resolves a name, so it is safe on the UI thread.

use std::collections::HashMap;

use moon_core::config::{ServerConfig, parse_endpoint_override, target_from_key};

/// One row's endpoint as the address cell shows it.
#[derive(Clone)]
pub(super) struct EndpointCell {
    /// The endpoint the core would dial, `host:port`; empty for an unreadable key or an invalid
    /// override, which have none.
    pub(super) text: String,
    /// Another draft row dialing the same `host:port`, if any.
    pub(super) duplicate_name: Option<String>,
    /// The override field holds something that is not an address.
    pub(super) invalid: bool,
    /// Neither the key nor the override names a host, so the core dials the localhost fallback.
    pub(super) localhost_fallback: bool,
}

/// Resolve each draft row once and mark every duplicate, including inactive and hidden rows.
/// Returns cells in draft order; rows with no endpoint never count as duplicates.
///
/// Duplicates compare the TEXT the rows would dial, without DNS: a name and the address it
/// resolves to are not recognised as one core here, which keeps this pass off the resolver.
pub(super) fn endpoint_cells(servers: &[ServerConfig]) -> Vec<EndpointCell> {
    let cells: Vec<EndpointCell> = servers
        .iter()
        .map(|s| match parse_endpoint_override(&s.endpoint_override) {
            Err(_) => EndpointCell {
                text: String::new(),
                duplicate_name: None,
                invalid: true,
                localhost_fallback: false,
            },
            Ok(endpoint_override) => {
                let target = target_from_key(s.key.expose(), endpoint_override.as_ref());
                EndpointCell {
                    text: target.as_ref().map(|t| t.text()).unwrap_or_default(),
                    duplicate_name: None,
                    invalid: false,
                    localhost_fallback: target.is_some_and(|t| t.localhost_fallback),
                }
            }
        })
        .collect();
    let mut peers: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, cell) in cells.iter().enumerate() {
        if !cell.text.is_empty() {
            peers.entry(cell.text.as_str()).or_default().push(index);
        }
    }
    let duplicates: Vec<Option<String>> = cells
        .iter()
        .enumerate()
        .map(|(index, cell)| {
            peers
                .get(cell.text.as_str())
                .filter(|_| !cell.text.is_empty())
                .and_then(|indices| indices.iter().find(|&&other| other != index))
                .map(|&other| servers[other].name.clone())
        })
        .collect();
    cells
        .into_iter()
        .zip(duplicates)
        .map(|(cell, duplicate_name)| EndpointCell {
            duplicate_name,
            ..cell
        })
        .collect()
}

/// The key's own endpoint, shown as the address field's placeholder while the field is empty, so
/// the row always says which address is in effect. `None` for an unreadable or empty key.
pub(super) fn key_placeholder(key: &str) -> Option<String> {
    target_from_key(key, None).map(|target| target.text())
}

#[cfg(test)]
mod tests;
