//! Locate and operate an installed MoonBot core through its MoonProto UDP port.

use std::net::Ipv4Addr;

use zeroize::Zeroizing;

use crate::core_move::command;
use crate::core_new::mb;
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
    Ok(match parse_locate(&out.stdout_text()) {
        LocateResult::Candidates(mut candidates) if candidates.len() == 1 => {
            let candidate = candidates.remove(0);
            LocateResult::Found(Located {
                ours: false,
                active: Some(false),
                ..candidate
            })
        }
        result => result,
    })
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

/// Read at most the selected number of lines from the located core's newest log.
pub fn log_tail(
    conn: &Conn,
    privilege: &Privilege,
    port: u16,
    n: LogLines,
) -> anyhow::Result<LogTail> {
    let port = port.to_string();
    let n = n.count().to_string();
    let out = privilege.run_raw(conn, &command("log", &[&port, &n]), &[], STEP_TIMEOUT)?;
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

/// Configure MoonProto at `host:port`, retarget its marker and firewall, and return its secret key.
pub fn set_moonproto_address(
    conn: &Conn,
    privilege: &Privilege,
    dir: &str,
    host: Ipv4Addr,
    port: u16,
) -> anyhow::Result<Zeroizing<String>> {
    let key = crate::core_new::setup_moonproto(conn, privilege, dir, host, port)?;
    retarget(conn, privilege, dir, port)?;
    Ok(key)
}

/// Read the core's existing MoonProto connection key without exposing an unreadable reply in errors.
pub fn moonproto_key(
    conn: &Conn,
    privilege: &Privilege,
    dir: &str,
) -> anyhow::Result<Zeroizing<String>> {
    let reply = mb(conn, privilege, dir, &["--moonProtoKey"], &[])?;
    connection_key(reply)
}

/// Move, trim and wipe the connection key; the SSH stdout buffer remains a residual copy.
fn connection_key(mut reply: serde_json::Value) -> anyhow::Result<Zeroizing<String>> {
    let mut key = match reply.get_mut("key").map(serde_json::Value::take) {
        Some(serde_json::Value::String(key)) => Zeroizing::new(key),
        _ => anyhow::bail!("the core did not hand back a connection key"),
    };
    let leading = key.len() - key.trim_start().len();
    if leading > 0 {
        key.drain(..leading);
    }
    let trimmed = key.trim_end().len();
    key.truncate(trimmed);
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
