//! The station's cores on a prepared server: each core's key into its own encrypted credential,
//! `station.toml` without a key, a restart. And its bot: the token into the credential
//! `telegram-token`, `[telegram]` into `station.toml`. And the terminal's window around a trade:
//! `[tape]` into `station.toml`, a reload.
//!
//! Each push rewrites `station.toml` from what the server holds (`edit_config`), and only while
//! the file is still the one read: several terminals may share one station.
//!
//! Only to a server `setup` closed: the administrator must already be recorded in `hosts.toml`,
//! which `setup` writes only after its key login and sudo rule were verified. A key goes on the
//! SSH channel's stdin straight into `systemd-creds encrypt` — never argv, never a file.

use anyhow::Context;
use moon_core::config::{Secret, TransportVersion};
use moon_core::station_api::{Answer, Request};
use serde::Serialize;
use sha2::{Digest, Sha256};

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

pub use moon_core::station_api::TapeWindow;

/// This terminal's window around a trade, from its `storage.toml` (Settings -> Storage): what a
/// new station starts with. Read only when the file exists: loading a missing one would write a
/// default into the terminal's folder.
pub fn terminal_tape() -> Option<TapeWindow> {
    if !moon_core::config::paths::storage_path().exists() {
        return None;
    }
    let cfg = moon_core::config::storage::load().trade_replay;
    Some(TapeWindow {
        margin_s: cfg.margin_s,
        long_position_min: cfg.long_position_min,
    })
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

/// `new` with the server's own sections carried over from `current`: `[telegram]` (set by the
/// bot's own command) and `[tape]` (set by hand, `push_tape`) — the window `new` brings is only
/// what a station without one starts with. The cores are the terminal's to push whole.
///
/// Args:
///     new: A freshly built `station.toml`, without `[telegram]`.
///     current: The server's `station.toml`, when it has one.
pub fn keep_server_sections(new: &str, current: Option<&str>) -> anyhow::Result<String> {
    let Some(current) = current else {
        return Ok(new.to_owned());
    };
    let current: toml::Table = toml::from_str(current).context("the server's station.toml")?;
    let mut merged: toml::Table = toml::from_str(new)?;
    let mut changed = false;
    for section in ["telegram", "tape"] {
        let Some(value) = current.get(section) else {
            continue;
        };
        merged.insert(section.to_owned(), value.clone());
        changed = true;
    }
    match changed {
        true => Ok(toml::to_string_pretty(&merged)?),
        false => Ok(new.to_owned()),
    }
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

/// `current` with its `[tape]` set to `tape`, or `None` when it already says exactly that — the
/// other sections (the cores, the bot) as they were.
///
/// Args:
///     current: The server's `station.toml`.
///     tape: The terminal's window around a trade.
pub fn with_tape(current: &str, tape: TapeWindow) -> anyhow::Result<Option<String>> {
    let mut file: toml::Table = toml::from_str(current).context("the server's station.toml")?;
    let wanted = toml::Value::try_from(tape)?;
    if file.get("tape") == Some(&wanted) {
        return Ok(None);
    }
    file.insert("tape".to_owned(), wanted);
    Ok(Some(toml::to_string_pretty(&file)?))
}

/// Set the station's window around a trade (the user's "Set" in the station's section): `[tape]`
/// in its `station.toml`, then a reload (SIGHUP) — the station applies the window without a
/// restart, its cores and bot stay connected. A station already on that window is not touched; a
/// stopped one takes it at its next start. A server without `station.toml` yet gets a window with
/// its first cores.
pub fn push_tape(
    target: &Target,
    tape: TapeWindow,
    say: &mut dyn FnMut(&str),
) -> anyhow::Result<()> {
    // On the steps the station records with, so what it reports back can equal what was sent.
    let tape = TapeWindow {
        margin_s: moon_core::config::storage::snap_trade_margin_s(tape.margin_s),
        long_position_min: moon_core::config::storage::clamp_long_position_min(
            tape.long_position_min,
        ),
    };
    let conn = admin_conn(target)?;
    let status = current_helper_status(&conn)?;
    if script::value(&status, "config") != Some("yes") {
        say("tape window: the station has no station.toml yet, it comes with the cores");
        return Ok(());
    }
    let wrote = edit_config(&conn, true, |current| match current {
        Some(current) => with_tape(current, tape),
        None => Ok(None),
    })?;
    let window = format!(
        "margin {} s, long position from {} min",
        tape.margin_s, tape.long_position_min
    );
    // Asked after the write: a station started meanwhile may have read the file before it.
    let status = script::checked(conn.run(&script::helper("status", &[]), &[], STEP_TIMEOUT)?)?
        .stdout_text();
    if script::value(&status, "active") != Some("active") {
        say(&format!(
            "tape window {}; the station is not running and takes it at its next start: {window}",
            if wrote {
                "written"
            } else {
                "already in its file"
            }
        ));
        return Ok(());
    }
    // A service without the control API cannot say what it records with.
    if script::value(&status, "api") != Some("yes") {
        script::checked(conn.run(&script::helper("reload", &[]), &[], STEP_TIMEOUT)?)?;
        say(&format!(
            "tape window written, the station asked to re-read it (its service does not report \
             the window: update it): {window}"
        ));
        return Ok(());
    }
    // The file may already say it while the station does not: an earlier re-read that failed.
    if wrote || reported_tape(&conn)? != Some(tape) {
        script::checked(conn.run(&script::helper("reload", &[]), &[], STEP_TIMEOUT)?)?;
    }
    confirm_tape(&conn, tape, &window, say)
}

/// The window the running station reports; `None` from a service older than the report.
fn reported_tape(conn: &Conn) -> anyhow::Result<Option<TapeWindow>> {
    match api::call(conn, &Request::Status)? {
        Answer::Status(status) => Ok(status.tape),
        other => anyhow::bail!("the station answered a status request with {other:?}"),
    }
}

/// How long a reloaded station has to report the new window: its main loop takes a reload within
/// 100 ms, the re-read of `station.toml` is quick.
const TAPE_APPLIED_WITHIN: std::time::Duration = std::time::Duration::from_secs(10);

/// Wait until the running station reports `tape` as the window it records with. A station whose
/// re-read failed (its log says why) keeps the old window: that is an error here, not a success.
/// A failed look is only one look until the deadline.
fn confirm_tape(
    conn: &Conn,
    tape: TapeWindow,
    window: &str,
    say: &mut dyn FnMut(&str),
) -> anyhow::Result<()> {
    let deadline = std::time::Instant::now() + TAPE_APPLIED_WITHIN;
    loop {
        let last_chance = std::time::Instant::now() >= deadline;
        match reported_tape(conn) {
            Ok(Some(applied)) if applied == tape => {
                say(&format!("tape window on the station: {window}"));
                return Ok(());
            }
            Ok(None) => {
                say(&format!(
                    "tape window written, the station asked to re-read it (its service does not \
                     report the window: update it): {window}"
                ));
                return Ok(());
            }
            Ok(Some(applied)) => anyhow::ensure!(
                !last_chance,
                "the station still records with margin {} s, long position from {} min: it did \
                 not take the new window (its journal says why)",
                applied.margin_s,
                applied.long_position_min
            ),
            Err(e) if last_chance => {
                return Err(e.context("the window is written; the station did not confirm it"));
            }
            Err(_) => {}
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// How many times a push reads `station.toml` again after another terminal changed it meanwhile.
const CONFIG_TRIES: usize = 3;

/// Rewrite the server's `station.toml` from what it holds now: `build` gets the current text
/// (`None` before the first push) and returns the new one, or `None` to leave the file as it is.
/// The helper writes only while the file is still the one read (its sha256), so what another
/// terminal wrote in between is read again and built on, not overwritten. Whether it wrote.
///
/// Args:
///     conn: The administrator's connection.
///     exists: Whether the server has the file, as its helper's `status` said.
///     build: The new file from the current one.
fn edit_config(
    conn: &Conn,
    mut exists: bool,
    mut build: impl FnMut(Option<&str>) -> anyhow::Result<Option<String>>,
) -> anyhow::Result<bool> {
    for _ in 0..CONFIG_TRIES {
        // The digest of the bytes as the helper hashes them, not of a decoded copy.
        let current = match exists {
            true => {
                let out = script::checked(conn.run(
                    &script::helper("get-config", &[]),
                    &[],
                    STEP_TIMEOUT,
                )?)
                .context("read the server's station.toml")?;
                Some((
                    out.stdout_text(),
                    format!("{:x}", Sha256::digest(&out.stdout)),
                ))
            }
            false => None,
        };
        let Some(new) = build(current.as_ref().map(|(text, _)| text.as_str()))? else {
            return Ok(false);
        };
        let base = current.map_or_else(|| "none".to_owned(), |(_, digest)| digest);
        let out = conn.run(
            &script::helper("put-config", &[&base]),
            new.as_bytes(),
            STEP_TIMEOUT,
        )?;
        if script::value(&out.stdout_text(), "config") == Some("changed") {
            exists = true;
            continue;
        }
        script::checked(out)?;
        return Ok(true);
    }
    anyhow::bail!("station.toml kept changing under this push: another terminal is writing it")
}

/// Whether a helper's `status` comes from this crate's helper: the line added last,
/// `config_cas=` (`put-config` writes only over the file it was given the digest of), is the
/// marker. An older helper answers "unknown command" halfway through a push, or overwrites what
/// another terminal wrote.
fn helper_is_current(status: &str) -> bool {
    script::value(status, "config_cas").is_some()
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
    if let Some(token) = token {
        anyhow::ensure!(!token.is_empty(), "the token is empty");
        run(script::helper("put-token", &[]), token.expose().as_bytes())
            .context("token credential")?;
        say("bot token: credential written");
    }
    edit_config(&conn, true, |current| {
        let current = current.ok_or_else(|| anyhow::anyhow!("the station has no station.toml"))?;
        with_telegram(current, change, off).map(Some)
    })?;
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
    let new = station_toml(cores, tape)?;
    let exists = script::value(&status, "config") == Some("yes");
    // A dry run of the merge before any credential is written; the real one below reads the file
    // again, as it is by then.
    if exists {
        let current = run(script::helper("get-config", &[]), &[])
            .context("read the server's station.toml")?;
        keep_server_sections(&new, Some(&current))?;
    }

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
    edit_config(&conn, exists, |current| {
        keep_server_sections(&new, current).map(Some)
    })?;
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
pub mod pull;

#[cfg(test)]
mod tests;
