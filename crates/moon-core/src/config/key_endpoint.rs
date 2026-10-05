//! Where a core is reached: the endpoint its key carries, with the Connections row's override laid
//! over it. One resolution for the Settings table, the live feed and the station link, so the
//! address a row shows is the address the feed dials.

use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};

use super::endpoint_override::{CoreHost, EndpointOverride};
use crate::feed::CoreEndpoint;

/// Port used when the key names none (a legacy export) or names zero.
const FALLBACK_PORT: u16 = 3000;

/// A core's connection target before any name is resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreTarget {
    /// Host to dial: the override's, else the key's address, else localhost.
    pub host: CoreHost,
    /// UDP port to dial: the override's, else the key's, else [`FALLBACK_PORT`].
    pub port: u16,
    /// Neither the key nor the override named a host, so [`Self::host`] is the localhost fallback.
    ///
    /// Kept as a fact rather than re-derived from `127.0.0.1`: a core really running on this
    /// machine may name localhost on purpose, and only a MISSING address deserves the warning that
    /// the Connections row shows for it.
    pub localhost_fallback: bool,
}

/// A [`CoreTarget`] with its host resolved to an address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    /// The address the rest of the terminal keys this core's machine by (core updates, warnings,
    /// Core Status grouping), plus the port.
    pub endpoint: CoreEndpoint,
    /// What MoonProto's `ClientConfig` is given as the server host.
    pub client_host: String,
}

impl CoreTarget {
    /// `host:port` as the Connections row shows it, IPv6 bracketed so the port stays unambiguous.
    pub fn text(&self) -> String {
        match &self.host {
            CoreHost::Ip(ip) => std::net::SocketAddr::new(*ip, self.port).to_string(),
            CoreHost::Name(name) => format!("{name}:{}", self.port),
        }
    }

    /// Resolve the host. A literal address returns at once; a name asks the system resolver, which
    /// BLOCKS — call it on a feed thread, never on the UI thread.
    ///
    /// A name with only IPv4 addresses is handed to MoonProto as the name itself, so MoonProto
    /// resolves it again whenever it rebinds its socket and follows a dynamic-DNS address change
    /// without a restart. Any other name is handed over as the chosen IP literal instead: MoonProto
    /// picks its socket family from a `:` in the host string (`socket_lifecycle.rs`, `bind_socket`),
    /// so a name resolving to IPv6 would bind an IPv4 socket and never connect, and a dual-stack
    /// name could resolve to a different family on its side than on ours.
    ///
    /// The returned [`ResolvedTarget::endpoint`] is fixed for the feed attempt that asked for it.
    /// After a dynamic-DNS change MoonProto's own re-resolution carries the traffic to the new
    /// address, while the machine identity built on this endpoint (Core Status grouping, core
    /// update lanes, warnings) keeps the old one until the core's next feed attempt.
    ///
    /// Errors:
    ///     The resolver's error, or `NotFound` when it returned no address at all.
    pub fn resolve(&self) -> std::io::Result<ResolvedTarget> {
        let name = match &self.host {
            CoreHost::Ip(ip) => {
                return Ok(ResolvedTarget {
                    endpoint: CoreEndpoint {
                        address: *ip,
                        port: self.port,
                    },
                    client_host: ip.to_string(),
                });
            }
            CoreHost::Name(name) => name,
        };
        let addresses: Vec<IpAddr> = (name.as_str(), self.port)
            .to_socket_addrs()?
            .map(|socket| socket.ip())
            .collect();
        let address = addresses
            .iter()
            .find(|ip| ip.is_ipv4())
            .or_else(|| addresses.first())
            .copied()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no address"))?;
        let client_host = if addresses.iter().all(IpAddr::is_ipv4) {
            name.clone()
        } else {
            address.to_string()
        };
        Ok(ResolvedTarget {
            endpoint: CoreEndpoint {
                address,
                port: self.port,
            },
            client_host,
        })
    }
}

/// Decode a pasted key and lay `endpoint_override` over its endpoint, without connecting.
///
/// Args:
///     key: The row's key.
///     endpoint_override: The row's parsed override, if any.
///
/// Returns:
///     `None` for an unreadable or empty key: there is nothing to connect with, whatever the
///     override says.
pub fn target_from_key(
    key: &str,
    endpoint_override: Option<&EndpointOverride>,
) -> Option<CoreTarget> {
    let info = moonproto::parse_key_info(key)?;
    Some(target_from_network(
        info.network.as_ref(),
        endpoint_override,
    ))
}

/// Lay `endpoint_override` over parsed key network metadata, for Settings and the live feed alike.
/// A missing address and a zero or absent port keep the established localhost and port-3000
/// defaults, now flagged through [`CoreTarget::localhost_fallback`].
pub(crate) fn target_from_network(
    network: Option<&moonproto::ImportedNetworkConfig>,
    endpoint_override: Option<&EndpointOverride>,
) -> CoreTarget {
    let host = endpoint_override
        .and_then(|o| o.host.clone())
        .or_else(|| network.and_then(|n| n.address).map(CoreHost::Ip));
    let port = endpoint_override.and_then(|o| o.port).unwrap_or_else(|| {
        network
            .map(|n| n.port)
            .filter(|port| *port != 0)
            .unwrap_or(FALLBACK_PORT)
    });
    CoreTarget {
        localhost_fallback: host.is_none(),
        host: host.unwrap_or(CoreHost::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST))),
        port,
    }
}

#[cfg(test)]
mod tests;
