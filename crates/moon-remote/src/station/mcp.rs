//! One-button AI access setup over the existing pinned SSH administrator connection: trusted
//! HTTPS for clients that open an address, and the OpenAI tunnel for ChatGPT.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use moon_core::config::Secret;
use moon_core::station_api::mcp::{
    AdminRequest, AdminState, TUNNEL_KEY_MISSING, TUNNEL_KEY_REJECTED, TUNNEL_NOT_FOUND,
    valid_tunnel_id,
};
use moon_core::station_api::{Answer, Request};

use super::{admin_conn, api, current_helper_status, edit_config};
use crate::script::{self, STEP_TIMEOUT};
use crate::ssh::Target;

/// Accept the known SSH destination as a public HTTPS host, never a path or shell fragment.
fn public_origin(host: &str) -> Result<(String, String)> {
    let host = host.trim_matches(['[', ']']);
    ensure!(
        !host.is_empty()
            && host.len() <= 253
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".:-".contains(&b)),
        "invalid public HTTPS host"
    );
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        let public = match ip {
            std::net::IpAddr::V4(ip) => {
                let [a, b, _, _] = ip.octets();
                !ip.is_private()
                    && !ip.is_loopback()
                    && !ip.is_link_local()
                    && !ip.is_documentation()
                    && a != 0
                    && a < 224
                    && !(a == 100 && (64..=127).contains(&b))
                    && !(a == 198 && (b == 18 || b == 19))
            }
            std::net::IpAddr::V6(ip) => {
                ip.segments()[0] & 0xe000 == 0x2000 && ip.segments()[..2] != [0x2001, 0x0db8]
            }
        };
        ensure!(
            public,
            "automatic HTTPS requires the station's public IP or public DNS name"
        );
        return Ok((
            host.into(),
            format!(
                "https://{}",
                if ip.is_ipv6() {
                    format!("[{ip}]")
                } else {
                    ip.to_string()
                }
            ),
        ));
    }
    ensure!(
        host.contains('.')
            && !host.contains(':')
            && host.split('.').all(|label| !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')),
        "a public IP or fully qualified DNS name is required"
    );
    ensure!(
        ![
            ".local",
            ".internal",
            ".localhost",
            ".invalid",
            ".test",
            ".example"
        ]
        .iter()
        .any(|suffix| host.ends_with(suffix)),
        "local names cannot receive public certificates"
    );
    Ok((
        host.to_ascii_lowercase(),
        format!("https://{}", host.to_ascii_lowercase()),
    ))
}

/// Preserve all unrelated station configuration and refuse to silently move an established audience.
/// A section that holds only the ChatGPT tunnel gains the public origin beside it.
fn with_mcp(current: Option<&str>, origin: &str) -> Result<Option<String>> {
    let mut table: toml::Table = toml::from_str(current.context("station.toml is missing")?)?;
    if let Some(section) = table.get_mut("mcp").and_then(toml::Value::as_table_mut) {
        if section.get("public_url").is_none() {
            section.insert("public_url".into(), toml::Value::String(origin.into()));
            return Ok(Some(toml::to_string_pretty(&table)?));
        }
        ensure!(
            section.get("public_url").and_then(toml::Value::as_str) == Some(origin)
                && section
                    .get("port")
                    .and_then(toml::Value::as_integer)
                    .unwrap_or(19791)
                    == 19791,
            "MCP already has another endpoint; changing it revokes clients and requires explicit reconfiguration"
        );
        return Ok(None);
    }
    table.insert(
        "mcp".into(),
        toml::Value::Table(toml::Table::from_iter([(
            "public_url".into(),
            toml::Value::String(origin.into()),
        )])),
    );
    Ok(Some(toml::to_string_pretty(&table)?))
}

/// `station.toml` with `[mcp] tunnel_id` set, everything else kept: a public origin beside it
/// stays, so Claude's address and its clients survive adding ChatGPT. `None` when it already holds
/// this tunnel.
fn with_tunnel(current: Option<&str>, tunnel_id: &str) -> Result<Option<String>> {
    let mut table: toml::Table = toml::from_str(current.context("station.toml is missing")?)?;
    let section = table
        .entry("mcp")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .context("[mcp] in station.toml is not a table")?;
    if section.get("tunnel_id").and_then(toml::Value::as_str) == Some(tunnel_id) {
        return Ok(None);
    }
    section.insert("tunnel_id".into(), toml::Value::String(tunnel_id.into()));
    Ok(Some(toml::to_string_pretty(&table)?))
}

/// `station.toml` without the tunnel: `tunnel_id` leaves `[mcp]`, and the section goes with it
/// when no public origin is left in it. `None` when there was no tunnel.
fn without_tunnel(current: Option<&str>) -> Result<Option<String>> {
    let mut table: toml::Table = toml::from_str(current.context("station.toml is missing")?)?;
    let Some(section) = table.get_mut("mcp").and_then(toml::Value::as_table_mut) else {
        return Ok(None);
    };
    if section.remove("tunnel_id").is_none() {
        return Ok(None);
    }
    if section.get("public_url").is_none() {
        table.remove("mcp");
    }
    Ok(Some(toml::to_string_pretty(&table)?))
}

/// How long the ChatGPT setup waits for the restarted station's tunnel to reach OpenAI.
const TUNNEL_WAIT: Duration = Duration::from_secs(60);

/// Connect ChatGPT: the tunnel key goes into the station's encrypted credentials, the tunnel ID
/// into `[mcp]`, and the station restarts with its own tunnel client, which needs no inbound port.
/// Returns once the tunnel reached OpenAI or OpenAI refused it, so the owner sees which.
///
/// Args:
///     target: The station's administrator destination.
///     tunnel_id: The tunnel from OpenAI Platform → Tunnels.
///     key: An OpenAI API key with the Tunnels permission; never read back.
pub fn tunnel_setup(target: &Target, tunnel_id: &str, key: &Secret) -> Result<AdminState> {
    let tunnel_id = tunnel_id.trim();
    ensure!(
        valid_tunnel_id(tunnel_id),
        "this is not an OpenAI tunnel ID (tunnel_ and 32 letters or digits)"
    );
    let key = key.expose().trim();
    ensure!(
        !key.is_empty() && key.len() <= 1024 && key.bytes().all(|b| b.is_ascii_graphic()),
        "this does not look like an OpenAI API key"
    );
    let conn = admin_conn(target)?;
    let status = current_helper_status(&conn)?;
    ensure!(
        script::value(&status, "mcp_tunnel") == Some("yes"),
        "update the station before connecting ChatGPT"
    );
    script::checked(conn.run(
        &script::helper("put-tunnel-key", &[]),
        key.as_bytes(),
        STEP_TIMEOUT,
    )?)
    .context("store the OpenAI key on the station")?;
    edit_config(&conn, true, |current| with_tunnel(current, tunnel_id))?;
    // Restart whether or not the file changed: a new key reaches the station only at a start.
    script::checked(conn.run(&script::helper("restart", &[]), &[], STEP_TIMEOUT)?)?;
    let state = wait_for_tunnel(&conn)?;
    if state.enabled {
        return Ok(state);
    }
    match api::call(
        &conn,
        &Request::Mcp(AdminRequest::SetEnabled { enabled: true }),
    )? {
        Answer::Mcp(state) => Ok(state),
        other => anyhow::bail!("the station answered MCP administration with {other:?}"),
    }
}

/// The restarted station's MCP status once its tunnel reached OpenAI or OpenAI refused it; the
/// last status read when neither happened in time.
fn wait_for_tunnel(conn: &super::AdminConn) -> Result<AdminState> {
    let deadline = Instant::now() + TUNNEL_WAIT;
    let mut last = None;
    loop {
        // While it starts the station answers nothing; that is waited out, not reported.
        if let Ok(Answer::Mcp(state)) = api::call(conn, &Request::Mcp(AdminRequest::Status)) {
            let settled = state.tunnel.as_ref().is_some_and(|tunnel| {
                tunnel.online
                    || matches!(
                        tunnel.problem.as_deref(),
                        Some(TUNNEL_KEY_REJECTED | TUNNEL_NOT_FOUND | TUNNEL_KEY_MISSING)
                    )
            });
            if settled {
                return Ok(state);
            }
            last = Some(state);
        }
        if Instant::now() >= deadline {
            return last.context("the station did not come back after the restart");
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// Disconnect ChatGPT: the tunnel leaves `[mcp]`, its key leaves the station, and the station
/// restarts without it. ChatGPT accounts paired through it keep their grants until revoked, so
/// reconnecting the same tunnel needs no new codes.
pub fn tunnel_remove(target: &Target) -> Result<AdminState> {
    let conn = admin_conn(target)?;
    current_helper_status(&conn)?;
    edit_config(&conn, true, without_tunnel)?;
    script::checked(conn.run(&script::helper("drop-tunnel-key", &[]), &[], STEP_TIMEOUT)?)?;
    script::checked(conn.run(&script::helper("restart", &[]), &[], STEP_TIMEOUT)?)?;
    let deadline = Instant::now() + TUNNEL_WAIT;
    loop {
        match api::call(&conn, &Request::Mcp(AdminRequest::Status)) {
            Ok(Answer::Mcp(state)) => return Ok(state),
            _ if Instant::now() >= deadline => {
                anyhow::bail!("the station did not come back after the restart")
            }
            _ => std::thread::sleep(Duration::from_secs(2)),
        }
    }
}

/// Install the trusted certificate manager, restart the station with its account feed, then probe
/// discovery from the terminal with normal TLS verification before enabling new authorizations.
pub fn setup(target: &Target) -> Result<AdminState> {
    let (host, origin) = public_origin(&target.host)?;
    let conn = admin_conn(target)?;
    current_helper_status(&conn)?;
    // Validate conflicts before installing anything; edit_config repeats this check under its CAS.
    let current =
        script::checked(conn.run(&script::helper("get-config", &[]), &[], STEP_TIMEOUT)?)?
            .stdout_text();
    with_mcp(Some(&current), &origin)?;
    script::checked(conn.run(
        &format!("sudo -n {}", script::bootstrap("helper", &[conn.user()])),
        script::HELPER.as_bytes(),
        STEP_TIMEOUT,
    )?)?;
    let status = script::checked(conn.run(&script::helper("status", &[]), &[], STEP_TIMEOUT)?)?
        .stdout_text();
    ensure!(
        script::value(&status, "mcp_https") == Some("yes"),
        "update the station before enabling MCP"
    );
    let command = format!(
        "sudo -n sh -c {} moon-station-https {}",
        script::sh_quote(include_str!("../../remote/moon-station-https.sh")),
        script::sh_quote(&host)
    );
    script::checked(conn.run(&command, &[], Duration::from_secs(240))?)
        .context("install Station HTTPS")?;
    edit_config(&conn, true, |current| with_mcp(current, &origin))?;
    // A retry must recover even if an earlier run saved config but failed before restarting.
    script::checked(conn.run(&script::helper("restart", &[]), &[], STEP_TIMEOUT)?)?;
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .max_redirects(0)
        .build()
        .new_agent();
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let checked = (|| -> Result<()> {
            let mut response = agent
                .get(format!("{origin}/.well-known/oauth-protected-resource/mcp"))
                .call()?;
            ensure!(response.status().is_success(), "HTTPS discovery failed");
            let metadata: serde_json::Value =
                response.body_mut().with_config().limit(16384).read_json()?;
            ensure!(
                metadata["resource"] == format!("{origin}/mcp"),
                "HTTPS endpoint does not identify this Station MCP"
            );
            Ok(())
        })();
        match checked {
            Ok(()) => break,
            Err(error) if Instant::now() >= deadline => return Err(error.context("HTTPS is not reachable with a trusted certificate. Check provider TCP 443 firewall/NAT; retry setup after correcting it")),
            Err(_) => std::thread::sleep(Duration::from_secs(2)),
        }
    }
    api::mcp_admin(target, AdminRequest::SetEnabled { enabled: true })
}

#[cfg(test)]
mod tests;
