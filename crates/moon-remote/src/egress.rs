//! Binding an installed core's outgoing connections to one of the server's IPv4 addresses.
//!
//! MoonBot has no setting for the address it connects from: every exchange request leaves from
//! the server's primary address. On a server with several, two cores can still reach their
//! exchanges from addresses of their own — separate rate-limit buckets, per-key IP whitelists —
//! through the OS. The server side lives in `remote/moonbot-move.sh` (systemd `NFTSet=` plus an
//! nftables SNAT table of ours, see the script's header); this module carries what it reports
//! and the one step that changes a running core.
//!
//! The address the terminal connects to (the MoonProto key's) is a separate thing and is never
//! changed here: replies to connections the core accepted keep their source.

use std::net::Ipv4Addr;

use crate::core_move::command;
use crate::script::{self, APT_TIMEOUT, Privilege};
use crate::ssh::Conn;

/// The first systemd with `NFTSet=`: older servers are refused, with no fallback.
pub const MIN_SYSTEMD: u32 = 255;

/// What a core's outgoing address can be chosen from on one server: the script's `EGRESS_*`
/// lines, printed by both `check` and `info`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EgressOptions {
    /// The running systemd's major version; 0 when the server did not say.
    pub systemd: u32,
    /// The address the default route leaves from: the core's outgoing address when unbound.
    pub primary: Option<Ipv4Addr>,
    /// Every global address of the default route's interface, the primary one included.
    pub addresses: Vec<Ipv4Addr>,
}

impl EgressOptions {
    /// Read the `EGRESS_*` lines; anything unreadable is left out rather than guessed.
    pub fn parse(text: &str) -> Self {
        Self {
            systemd: script::value(text, "EGRESS_SYSTEMD")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            primary: script::value(text, "EGRESS_PRIMARY").and_then(|v| v.parse().ok()),
            addresses: text
                .lines()
                .filter_map(|line| line.strip_prefix("EGRESS_ADDRESS="))
                .filter_map(|v| v.trim().parse().ok())
                .collect(),
        }
    }

    /// Whether systemd on this server is new enough to bind a core at all.
    pub fn supported(&self) -> bool {
        self.systemd >= MIN_SYSTEMD
    }

    /// The addresses a core can be bound to besides the primary one: binding to the primary
    /// address changes nothing, so a choice offers "primary" (unbound) plus these.
    pub fn alternatives(&self) -> Vec<Ipv4Addr> {
        self.addresses
            .iter()
            .copied()
            .filter(|a| Some(*a) != self.primary)
            .collect()
    }
}

/// A located core's binding, from the script's `info` step.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EgressInfo {
    /// The recorded outgoing address; `None` for an unbound core.
    pub address: Option<Ipv4Addr>,
    /// Whether the server still has the recorded address on its main interface. A lost address
    /// keeps its rules, which drop the core's connections rather than let them out elsewhere.
    pub present: bool,
    /// Whether the running core sits in its address's set — false for a bound core means its
    /// connections are not translated right now.
    pub bound: bool,
    /// New connections the address has carried since the rules were last loaded, counted over
    /// every core bound to it; `None` when unread.
    pub connections: Option<u64>,
    /// What another address could be chosen from.
    pub options: EgressOptions,
}

impl EgressInfo {
    /// Read the `INFO_EGRESS*` and `EGRESS_*` lines of an `info` reply.
    pub fn parse(text: &str) -> Self {
        Self {
            address: script::value(text, "INFO_EGRESS").and_then(|v| v.parse().ok()),
            present: script::value(text, "INFO_EGRESS_PRESENT") == Some("1"),
            bound: script::value(text, "INFO_EGRESS_BOUND") == Some("1"),
            connections: script::value(text, "INFO_EGRESS_CONNECTIONS")
                .and_then(|v| v.parse().ok()),
            options: EgressOptions::parse(text),
        }
    }
}

/// The trailing option an install step takes to bind the core before its first start.
pub(crate) fn install_option(egress: Option<Ipv4Addr>) -> Option<String> {
    egress.map(|address| format!("egress={address}"))
}

/// The server refused an outgoing-address change, in the script's own words: an old systemd, an
/// address the server does not have, a core that came back outside its rules. Typed so a caller
/// tells it from a lost connection.
#[derive(Debug)]
pub struct EgressRefused(pub String);

impl std::fmt::Display for EgressRefused {
    /// The script's reason as it wrote it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EgressRefused {}

/// Bind the core on UDP `port` to `address`, or unbind it with `None`. A running core is restarted
/// there — connections already open keep their source until then; a stopped one stays stopped and
/// takes it on its next start. The script refuses an old systemd or an address the server does
/// not have before it changes anything, and fails when the restarted core is not in its address's
/// set, putting the previous binding back. Asked for what the core already has, it returns without
/// a restart.
///
/// Errors:
///     [`EgressRefused`] when the script said no; any other error is the connection's.
pub fn set(
    conn: &Conn,
    privilege: &Privilege,
    port: u16,
    address: Option<Ipv4Addr>,
) -> anyhow::Result<()> {
    let wanted = address.map_or_else(|| "off".to_owned(), |a| a.to_string());
    // nftables may be installed through apt on the first binding.
    let out = privilege.run_raw(
        conn,
        &command("egress", &[&port.to_string(), &wanted]),
        &[],
        APT_TIMEOUT,
    )?;
    if !out.ok() {
        // No exit status: the session went away mid-step, which is not the server's answer.
        anyhow::ensure!(
            out.status.is_some(),
            "the outgoing-address change ended without an exit status"
        );
        let stderr = out.stderr_text();
        let reason = stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("the outgoing-address change failed")
            .trim()
            .to_owned();
        return Err(EgressRefused(reason).into());
    }
    let text = out.stdout_text();
    anyhow::ensure!(
        script::value(&text, "EGRESS_OK") == Some(wanted.as_str()),
        "the outgoing-address change did not finish: {}",
        text.trim()
    );
    Ok(())
}

#[cfg(test)]
mod tests;
