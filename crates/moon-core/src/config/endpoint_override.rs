//! A core's address and port typed by hand in the Connections row, overriding its key (#616).
//!
//! The key is the only place a core's endpoint used to come from, so a core reached from the
//! terminal's own LAN, a key exported with a wrong or missing address, or a public IP the ISP moved
//! all meant re-exporting the key in MoonBot. The override covers those cases the way
//! `ServerConfig::transport` covers the transport mode: an empty field follows the key, a filled one
//! wins over it, and pasting a new key never touches it.
//!
//! The field is stored as the user typed it (`ServerConfig::endpoint_override`), exactly as the key
//! is: an invalid entry must survive Save so the row can keep showing it in red, and the feed then
//! reports it as a connection fault instead of silently falling back to the key's address.

use std::net::{IpAddr, Ipv6Addr, SocketAddr};

/// A core host: an IP address, or a name the terminal resolves when it connects.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CoreHost {
    /// A literal IPv4 or IPv6 address.
    Ip(IpAddr),
    /// A DNS name, lowercased: names are case-insensitive, and duplicate detection compares them.
    Name(String),
}

impl std::fmt::Display for CoreHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ip(ip) => ip.fmt(f),
            Self::Name(name) => f.write_str(name),
        }
    }
}

/// The parts of a core's endpoint the user typed. A part left out still comes from the key, so
/// `:5020` moves only the port (router port forwarding) and `192.168.1.5` only the host.
///
/// At least one part is always present: an entry with neither is `None` from
/// [`parse_endpoint_override`], never an empty override.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EndpointOverride {
    /// Host to connect to instead of the key's address.
    pub host: Option<CoreHost>,
    /// UDP port to connect to instead of the key's, never zero.
    pub port: Option<u16>,
}

/// The override field holds something that is not `host`, `host:port`, `[ipv6]:port` or `:port`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InvalidEndpoint;

/// Parse the override field of one Connections row.
///
/// Accepted forms: an IPv4 or IPv6 address, a host name, either with `:port` (IPv6 bracketed, as in
/// `[2001:db8::1]:5020`), or `:port` alone. Surrounding whitespace is ignored.
///
/// Args:
///     text: The field as typed.
///
/// Returns:
///     `Ok(None)` for an empty field (follow the key), the parsed override otherwise.
///
/// Errors:
///     [`InvalidEndpoint`] when the text is none of the accepted forms: a zero or out-of-range port,
///     an unbracketed IPv6 address with a port, or a host name with characters DNS does not allow.
pub fn parse_endpoint_override(text: &str) -> Result<Option<EndpointOverride>, InvalidEndpoint> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    // A bare address first: `::1` would otherwise split into an empty host and port `1`.
    if let Ok(ip) = text.parse::<IpAddr>() {
        return Ok(Some(EndpointOverride {
            host: Some(CoreHost::Ip(ip)),
            port: None,
        }));
    }
    if let Ok(socket) = text.parse::<SocketAddr>() {
        if socket.port() == 0 {
            return Err(InvalidEndpoint);
        }
        return Ok(Some(EndpointOverride {
            host: Some(CoreHost::Ip(socket.ip())),
            port: Some(socket.port()),
        }));
    }
    if let Some(inner) = text.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
        let ip = inner.parse::<Ipv6Addr>().map_err(|_| InvalidEndpoint)?;
        return Ok(Some(EndpointOverride {
            host: Some(CoreHost::Ip(IpAddr::V6(ip))),
            port: None,
        }));
    }
    let (host, port) = match text.rsplit_once(':') {
        Some((host, port)) => (host, Some(parse_port(port)?)),
        None => (text, None),
    };
    if host.is_empty() {
        return Ok(Some(EndpointOverride { host: None, port }));
    }
    if !is_host_name(host) {
        return Err(InvalidEndpoint);
    }
    Ok(Some(EndpointOverride {
        host: Some(CoreHost::Name(host.to_ascii_lowercase())),
        port,
    }))
}

/// The override a core hands the STATION: its own, only where the user ticked it for the station
/// (`ServerConfig::endpoint_to_station`); otherwise empty, so the station dials the key's address.
/// The one definition every station-side view reads — the push (`config::read_core_keys`) and the
/// terminal's address matching against the station's cores — so they cannot disagree about which
/// address the station has.
///
/// Args:
///     endpoint_override: The row's override as stored.
///     to_station: Whether the row is ticked for the station.
///
/// Returns:
///     The override text for the station, or `""` when the station follows the key.
pub fn station_endpoint(endpoint_override: &str, to_station: bool) -> &str {
    if to_station { endpoint_override } else { "" }
}

/// Parse a port: ASCII digits only (`u16::from_str` would also take a leading `+`), 1..=65535.
fn parse_port(text: &str) -> Result<u16, InvalidEndpoint> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(InvalidEndpoint);
    }
    match text.parse::<u16>() {
        Ok(port) if port != 0 => Ok(port),
        _ => Err(InvalidEndpoint),
    }
}

/// Whether `host` is a DNS name: dot-separated labels of 1..=63 letters, digits and inner hyphens,
/// 253 characters at most.
///
/// The last label must not be all digits. Without that rule a mistyped address such as
/// `192.168.1.300` or `192.168.1` would pass as a NAME, and the system resolver reads the short
/// numeric forms as an address of its own (`192.168.1` is `192.168.0.1` to `inet_aton`), so a typo
/// would connect somewhere else instead of turning the field red.
fn is_host_name(host: &str) -> bool {
    if host.len() > 253 {
        return false;
    }
    let labels: Vec<&str> = host.split('.').collect();
    let label_ok = |label: &&str| {
        (1..=63).contains(&label.len())
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    };
    labels.iter().all(label_ok)
        && labels
            .last()
            .is_some_and(|last| !last.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests;
