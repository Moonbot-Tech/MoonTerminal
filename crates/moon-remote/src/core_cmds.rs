//! Locate and operate an installed MoonBot core through its MoonProto UDP port.

use std::net::Ipv4Addr;

use zeroize::Zeroizing;

use crate::core_move::command;
use crate::script::{self, Privilege, STEP_TIMEOUT};
use crate::ssh::{Conn, Output};

mod access;
mod wire;

pub use crate::core_new::{
    exchanges, retry_login, select_exchange, set_api_keys, setup_moonproto, status, wait_settled,
};
pub use access::{Login, open};
pub use wire::{
    Cmd, LocateResult, Located, LogLines, LogTail, Refusal, ServiceAction, ServiceFailed,
    ServiceInfo, gate, parse_info, parse_locate, parse_log, parse_service,
};

/// Resolve the instance directory and service unit that own `port` on the server.
pub fn locate(conn: &Conn, privilege: &Privilege, port: u16) -> anyhow::Result<LocateResult> {
    let port = port.to_string();
    let out = privilege.run(conn, &command("locate", &[&port]), &[], STEP_TIMEOUT)?;
    Ok(locate_reply(&out.stdout_text()))
}

/// Promote a lone stopped candidate only when its requested port was confirmed on the server.
fn locate_reply(text: &str) -> LocateResult {
    match parse_locate(text) {
        LocateResult::Candidates(mut candidates)
            if candidates.len() == 1 && candidates[0].port_known =>
        {
            let candidate = candidates.remove(0);
            LocateResult::Found(Located {
                ours: false,
                active: Some(false),
                ..candidate
            })
        }
        result => result,
    }
}

/// Resolve `port` and read its service state, uptime and current memory use.
pub fn info(conn: &Conn, privilege: &Privilege, port: u16) -> anyhow::Result<Option<ServiceInfo>> {
    let port = port.to_string();
    let out = privilege.run(conn, &command("info", &[&port]), &[], STEP_TIMEOUT)?;
    Ok(parse_info(&out.stdout_text()))
}

/// Change the located service state, optionally using a validated unit hint to start a stopped core.
pub fn service(
    conn: &Conn,
    privilege: &Privilege,
    action: ServiceAction,
    port: u16,
    unit_hint: Option<&str>,
) -> anyhow::Result<String> {
    let port = port.to_string();
    let mut args = vec![action.as_str(), &port];
    if let Some(unit) = unit_hint {
        args.push(unit);
    }
    let out = privilege.run_raw(conn, &command("service", &args), &[], STEP_TIMEOUT)?;
    service_output(out)
}

/// Turn a service command's raw output into its state or a typed failure carrying only its journal.
fn service_output(out: Output) -> anyhow::Result<String> {
    if !out.ok() {
        return Err(ServiceFailed {
            tail: out.stderr_text().lines().map(str::to_owned).collect(),
        }
        .into());
    }
    parse_service(&out.stdout_text())
}

/// Read at most the selected number of lines, using `dir_hint` only if location finds no core.
pub fn log_tail(
    conn: &Conn,
    privilege: &Privilege,
    port: u16,
    n: LogLines,
    dir_hint: Option<&str>,
) -> anyhow::Result<LogTail> {
    let port = port.to_string();
    let n = n.count().to_string();
    let mut args = vec![port.as_str(), n.as_str()];
    if let Some(dir) = dir_hint {
        args.push(dir);
    }
    let out = privilege.run_raw(conn, &command("log", &args), &[], STEP_TIMEOUT)?;
    anyhow::ensure!(out.ok(), "the core log could not be read");
    Ok(parse_log(&out.stdout_text()))
}

/// Move a marked instance's MoonProto port and its recorded firewall rule.
pub fn retarget(conn: &Conn, privilege: &Privilege, dir: &str, port: u16) -> anyhow::Result<()> {
    let port = port.to_string();
    let out = privilege.run(conn, &command("retarget", &[dir, &port]), &[], STEP_TIMEOUT)?;
    anyhow::ensure!(
        script::value(&out.stdout_text(), "RETARGET_OK") == Some(port.as_str()),
        "the server did not confirm the new MoonProto port"
    );
    Ok(())
}

/// A successful MoonProto setup, including a retarget failure that must not discard its key.
pub struct AddressChanged {
    /// The connection key returned by the core.
    pub key: Zeroizing<String>,
    /// Whether the installation marker and recorded firewall rule were moved.
    pub retarget: Result<(), String>,
}

/// Configure MoonProto at `host:port`, retarget its marker and firewall, and return its secret key.
pub fn set_moonproto_address(
    conn: &Conn,
    privilege: &Privilege,
    dir: &str,
    host: Ipv4Addr,
    port: u16,
) -> anyhow::Result<AddressChanged> {
    let key = crate::core_new::setup_moonproto(conn, privilege, dir, host, port)?;
    Ok(address_changed(key, retarget(conn, privilege, dir, port)))
}

/// Keep a successful setup's key beside a fixed, stdout-free retarget failure description.
fn address_changed(key: Zeroizing<String>, retarget: anyhow::Result<()>) -> AddressChanged {
    AddressChanged {
        key,
        retarget: retarget.map_err(|_| {
            "MoonProto was configured, but its marker and firewall rule could not be updated"
                .to_owned()
        }),
    }
}

/// Read the core's existing MoonProto connection key without exposing an unreadable reply in errors.
pub fn moonproto_key(
    conn: &Conn,
    privilege: &Privilege,
    dir: &str,
) -> anyhow::Result<Zeroizing<String>> {
    let out = privilege.run_raw(
        conn,
        &command("mb", &[dir, "--moonProtoKey"]),
        &[],
        STEP_TIMEOUT,
    )?;
    let stdout = Zeroizing::new(out.stdout_text());
    key_reply(out.ok(), &stdout, &out.stderr_text())
}

/// The connection key in a `--moonProtoKey` reply. MoonBot 7.73 prints the bare packet, unlike
/// the JSON its other commands answer with: a successful reply's last packet-shaped line is the
/// key, whatever else stdout carries. Anything else — a JSON reply with `key`, the core's refusal,
/// no reply at all — is read the way [`crate::core_new::mb`] reads a reply, which reports stderr
/// or the output's size. No error carries stdout: it is the secret.
fn key_reply(exited_ok: bool, stdout: &str, stderr: &str) -> anyhow::Result<Zeroizing<String>> {
    let base64 = |c: char| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=');
    let packet = stdout
        .lines()
        .map(str::trim)
        .rfind(|line| line.len() >= 64 && line.chars().all(base64));
    match packet {
        Some(packet) if exited_ok => Ok(Zeroizing::new(packet.to_owned())),
        _ => connection_key(crate::core_new::parse_reply(exited_ok, stdout, stderr)?),
    }
}

/// Move, trim and wipe the connection key; the SSH stdout buffer remains a residual copy.
fn connection_key(mut reply: serde_json::Value) -> anyhow::Result<Zeroizing<String>> {
    let key = match reply.get_mut("key").map(serde_json::Value::take) {
        Some(serde_json::Value::String(key)) => Zeroizing::new(key),
        _ => anyhow::bail!("the core did not hand back a connection key"),
    };
    let key = Zeroizing::new(key.trim().to_owned());
    anyhow::ensure!(
        !key.is_empty(),
        "the core did not hand back a connection key"
    );
    Ok(key)
}

#[cfg(test)]
mod script_tests;
#[cfg(test)]
mod tests;
