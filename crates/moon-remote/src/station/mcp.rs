//! One-button HTTPS setup over the existing pinned SSH administrator connection.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use moon_core::station_api::mcp::{AdminRequest, AdminState};

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
fn with_mcp(current: Option<&str>, origin: &str) -> Result<Option<String>> {
    let mut table: toml::Table = toml::from_str(current.context("station.toml is missing")?)?;
    if let Some(section) = table.get("mcp") {
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
