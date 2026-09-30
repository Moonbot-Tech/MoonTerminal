//! The station's cores on a prepared server: each core's key into its own encrypted credential,
//! `station.toml` without a key, a restart. And its bot: the token into the credential
//! `telegram-token`, `[telegram]` into `station.toml`.
//!
//! Only to a server `setup` closed: the administrator must already be recorded in `hosts.toml`,
//! which `setup` writes only after its key login and sudo rule were verified. A key goes on the
//! SSH channel's stdin straight into `systemd-creds encrypt` — never argv, never a file.

use anyhow::Context;
use moon_core::config::{Secret, TransportVersion};
use serde::Serialize;

use crate::app_key;
use crate::hosts::Hosts;
use crate::script::{self, STEP_TIMEOUT};
use crate::ssh::{Auth, Conn, Target};

/// One core for the station.
pub struct CoreKey {
    pub uid: u64,
    pub name: String,
    pub transport: Option<TransportVersion>,
    pub key: Secret,
}

/// The terminal's window around a trade (`[trade_replay]`), sent as the station's `[tape]`.
#[derive(Clone, Copy, Serialize)]
pub struct TapeWindow {
    pub margin_s: u32,
    pub long_position_min: u32,
}

/// `station.toml` as the station reads it: no key, the credential is `core-<uid>`.
#[derive(Serialize)]
struct StationFile<'a> {
    core: Vec<StationCore<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tape: Option<TapeWindow>,
}

#[derive(Serialize)]
struct StationCore<'a> {
    uid: u64,
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    transport: Option<TransportVersion>,
}

/// The station's `station.toml` for `cores`; without `tape` the station keeps its own window.
pub fn station_toml(cores: &[CoreKey], tape: Option<TapeWindow>) -> anyhow::Result<String> {
    let file = StationFile {
        core: cores
            .iter()
            .map(|c| StationCore {
                uid: c.uid,
                name: &c.name,
                transport: c.transport,
            })
            .collect(),
        tape,
    };
    Ok(toml::to_string_pretty(&file)?)
}

/// The administrator's connection to a server `setup` closed.
pub fn admin_conn(target: &Target) -> anyhow::Result<Conn> {
    let hosts = Hosts::load(&Hosts::path())?;
    let host = hosts
        .get(&target.addr())
        .ok_or_else(|| anyhow::anyhow!("{} was never set up here", target.addr()))?;
    let admin = host
        .admin
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("{}: the setup did not finish", target.addr()))?;
    let app = app_key::load_or_create()?;
    Ok(Conn::open(
        target,
        &Auth::Key {
            user: admin,
            key: &app,
        },
        Some(&host.fingerprint),
    )?)
}

/// `new` with the `[telegram]` of `current` carried over: the cores and the tape are the
/// terminal's to push whole, the bot's section is set by its own command.
///
/// Args:
///     new: A freshly built `station.toml`, without `[telegram]`.
///     current: The server's `station.toml`, when it has one.
pub fn keep_telegram(new: &str, current: Option<&str>) -> anyhow::Result<String> {
    let Some(current) = current else {
        return Ok(new.to_owned());
    };
    let current: toml::Table = toml::from_str(current).context("the server's station.toml")?;
    let Some(telegram) = current.get("telegram") else {
        return Ok(new.to_owned());
    };
    let mut merged: toml::Table = toml::from_str(new)?;
    merged.insert("telegram".to_owned(), telegram.clone());
    Ok(toml::to_string_pretty(&merged)?)
}

/// What `moon-remote telegram` changes in `[telegram]`; `None` keeps the server's value.
#[derive(Default)]
pub struct BotChange {
    pub mini_app: Option<bool>,
    pub zone: Option<String>,
    pub language: Option<String>,
}

/// `current` with its `[telegram]` changed by `change`, or removed with `off`.
///
/// Args:
///     current: The server's `station.toml`.
///     change: The fields to set; the others keep the server's values.
///     off: Remove the section: the station runs no bot.
pub fn with_telegram(current: &str, change: &BotChange, off: bool) -> anyhow::Result<String> {
    let mut file: toml::Table = toml::from_str(current).context("the server's station.toml")?;
    if off {
        file.remove("telegram");
        return Ok(toml::to_string_pretty(&file)?);
    }
    let section = file
        .entry("telegram")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("[telegram] in station.toml is not a table"))?;
    if let Some(on) = change.mini_app {
        section.insert("mini_app".to_owned(), toml::Value::Boolean(on));
    }
    if let Some(zone) = &change.zone {
        section.insert("zone".to_owned(), toml::Value::String(zone.clone()));
    }
    if let Some(language) = &change.language {
        section.insert("language".to_owned(), toml::Value::String(language.clone()));
    }
    Ok(toml::to_string_pretty(&file)?)
}

/// Whether a helper's `status` comes from this crate's helper: the newest line it prints, `api=`
/// (with `ctl`, the station's control API), is the marker. An older helper answers "unknown
/// command" halfway through a push.
fn helper_is_current(status: &str) -> bool {
    script::value(status, "api").is_some()
}

/// The helper's `status`, after putting this crate's helper in place when the server's is older —
/// before anything else is written. The administrator's sudo needs no password (`setup`), so the
/// terminal updates the helper itself; a server set up before that is refused with the way out.
fn current_helper_status(conn: &crate::ssh::Conn) -> anyhow::Result<String> {
    let status = |conn: &crate::ssh::Conn| -> anyhow::Result<String> {
        Ok(
            script::checked(conn.run(&script::helper("status", &[]), &[], STEP_TIMEOUT)?)?
                .stdout_text(),
        )
    };
    // An old helper still answers `status`; one that fails is replaced the same way.
    match status(conn) {
        Ok(text) if helper_is_current(&text) => return Ok(text),
        _ => {}
    }
    let out = conn.run(
        &format!("sudo -n {}", script::bootstrap("helper", &[conn.user()])),
        script::HELPER.as_bytes(),
        STEP_TIMEOUT,
    )?;
    script::checked(out).context(
        "the server's moon-station-admin is older than this terminal and sudo there still asks \
         for a password: re-run the setup once to move the server over",
    )?;
    let text = status(conn)?;
    anyhow::ensure!(
        helper_is_current(&text),
        "the helper on the server was updated but still answers as the old one"
    );
    Ok(text)
}

/// Set the station's bot: its token when given, `[telegram]` changed by `change` (or removed
/// with `off`, the token with it), and the service restarted — a credential reaches it only at a
/// start. A bot without a token on the server is refused before anything is written.
pub fn push_telegram(
    target: &Target,
    token: Option<&Secret>,
    change: &BotChange,
    off: bool,
    say: &mut dyn FnMut(&str),
) -> anyhow::Result<()> {
    let conn = admin_conn(target)?;
    let run = |command: String, stdin: &[u8]| -> anyhow::Result<String> {
        Ok(script::checked(conn.run(&command, stdin, STEP_TIMEOUT)?)?.stdout_text())
    };
    let status = current_helper_status(&conn)?;
    let has_token = script::value(&status, "token") == Some("yes");
    anyhow::ensure!(
        off || token.is_some() || has_token,
        "the station has no bot token yet: give --token"
    );
    anyhow::ensure!(
        script::value(&status, "config") == Some("yes"),
        "the station has no station.toml yet: send its cores first"
    );
    let current =
        run(script::helper("get-config", &[]), &[]).context("read the server's station.toml")?;
    let config = with_telegram(&current, change, off)?;
    if let Some(token) = token {
        anyhow::ensure!(!token.is_empty(), "the token is empty");
        run(script::helper("put-token", &[]), token.expose().as_bytes())
            .context("token credential")?;
        say("bot token: credential written");
    }
    run(script::helper("put-config", &[]), config.as_bytes())?;
    if off {
        if has_token {
            run(script::helper("drop-token", &[]), &[])?;
            say("bot token: dropped");
        }
        // The chats belonged to that bot: the next one starts unpaired.
        run(script::helper("drop-pairing", &[]), &[])?;
        say("paired chats: dropped");
    }
    run(script::helper("start", &[]), &[])?;
    let status = run(script::helper("status", &[]), &[])?;
    for line in status.lines() {
        say(line);
    }
    Ok(())
}

/// Make `cores` the station's whole set: credentials for these, none for any other, the matching
/// `station.toml`, and the service (re)started. The bot's `[telegram]` stays as it was.
pub fn push_cores(
    target: &Target,
    cores: &[CoreKey],
    tape: Option<TapeWindow>,
    say: &mut dyn FnMut(&str),
) -> anyhow::Result<()> {
    anyhow::ensure!(!cores.is_empty(), "no cores to send");
    let mut seen = std::collections::HashSet::new();
    for core in cores {
        anyhow::ensure!(core.uid != 0, "core {:?} has no uid", core.name);
        anyhow::ensure!(seen.insert(core.uid), "uid {} twice", core.uid);
        anyhow::ensure!(!core.key.is_empty(), "core {:?} has no key", core.name);
    }
    let conn = admin_conn(target)?;
    let run = |command: String, stdin: &[u8]| -> anyhow::Result<String> {
        Ok(script::checked(conn.run(&command, stdin, STEP_TIMEOUT)?)?.stdout_text())
    };
    // Read before anything is written: an old helper or an unreadable file stops the push here,
    // not between the credentials and the configuration. No file yet is the first push.
    let status = current_helper_status(&conn)?;
    let current = match script::value(&status, "config") {
        Some("yes") => Some(
            run(script::helper("get-config", &[]), &[])
                .context("read the server's station.toml")?,
        ),
        _ => None,
    };
    let config = keep_telegram(&station_toml(cores, tape)?, current.as_deref())?;

    for core in cores {
        let uid = core.uid.to_string();
        run(
            script::helper("put-cred", &[&uid]),
            core.key.expose().as_bytes(),
        )
        .with_context(|| format!("credential of {}", core.name))?;
        say(&format!(
            "core {} ({}): credential written",
            core.name, core.uid
        ));
    }
    let status = run(script::helper("status", &[]), &[])?;
    let wanted: std::collections::HashSet<String> =
        cores.iter().map(|c| format!("core-{}", c.uid)).collect();
    for stale in script::value(&status, "creds")
        .unwrap_or_default()
        .split_whitespace()
        .filter(|name| !wanted.contains(*name))
    {
        let uid = stale.trim_start_matches("core-");
        run(script::helper("drop-cred", &[uid]), &[])?;
        say(&format!("{stale}: dropped, not in the set"));
    }
    run(script::helper("put-config", &[]), config.as_bytes())?;
    run(script::helper("start", &[]), &[])?;
    let status = run(script::helper("status", &[]), &[])?;
    for line in status.lines() {
        say(line);
    }
    Ok(())
}

/// Put the terminal's USDT valuation cache (`snapshot`, a consistent copy of its
/// `valuation.sqlite`) on a new station before it first starts: its cached rates spare the new
/// station years of minute rates asked from the exchanges again (hours on one vCPU); the values
/// themselves the station re-derives from its own replica. Sent gzipped; the service is left
/// stopped for the caller to start.
pub fn put_valuation(
    target: &Target,
    snapshot: &std::path::Path,
    say: &mut dyn FnMut(&str),
) -> anyhow::Result<()> {
    use std::io::Write;
    let plain = std::fs::read(snapshot).with_context(|| format!("read {}", snapshot.display()))?;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&plain)?;
    let packed = gz.finish()?;
    say(&format!(
        "valuation cache: {} MB, {} MB to send",
        plain.len() / 1_000_000,
        packed.len() / 1_000_000
    ));
    let conn = admin_conn(target)?;
    // A station already valuing on its own (a server set up anew over a running one) keeps it.
    if script::value(&current_helper_status(&conn)?, "valuation") == Some("yes") {
        say("valuation cache: the station has its own, kept");
        return Ok(());
    }
    let out = script::checked(conn.run(
        &script::helper("put-valuation", &[]),
        &packed,
        script::APT_TIMEOUT,
    )?)?;
    for line in out.stdout_text().lines() {
        say(line);
    }
    Ok(())
}

pub mod api;
pub mod bot;

#[cfg(test)]
mod tests;
