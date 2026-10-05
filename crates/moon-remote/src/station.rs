//! The station's cores on a prepared server: each core's key into its own encrypted credential,
//! `station.toml` without a key, a restart. And its bot: the token into the credential
//! `telegram-token`, `[telegram]` into `station.toml`. And the terminal's window around a trade:
//! `[tape]` into `station.toml`, a reload.
//!
//! Each push rewrites `station.toml` from what the server holds (`edit_config`), and only while
//! the file is still the one read: several terminals may share one station.
//! Core pushes only add or update; only `remove_cores` removes named entries and credentials,
//! leaving their report data intact.
//!
//! Only to a server `setup` closed: the administrator must already be recorded in `hosts.toml`,
//! which `setup` writes only after its key login and sudo rule were verified. A key goes on the
//! SSH channel's stdin straight into `systemd-creds encrypt` — never argv, never a file.

use crate::error::StationError;
use crate::progress::{Progress, Step};
use anyhow::Context;
use moon_core::config::{Secret, TransportVersion};
use moon_core::station_api::{Answer, Request};
use sha2::{Digest, Sha256};

use crate::app_key;
use crate::hosts::Hosts;
use crate::script::{self, STEP_TIMEOUT};
use crate::ssh::Target;

mod session;
pub use session::AdminConn;

/// One station credential with its transport and hand-typed endpoint settings.
pub struct CoreKey {
    pub uid: u64,
    pub name: String,
    pub transport: Option<TransportVersion>,
    pub key: Secret,
    /// Hand-typed endpoint carried unchanged; empty follows the key.
    pub endpoint_override: String,
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

/// Reuse the administrator's pinned login for a short idle window after station use.
pub fn admin_conn(target: &Target) -> anyhow::Result<AdminConn> {
    let hosts = Hosts::load(&Hosts::path())?;
    let host = hosts
        .get(&target.addr())
        .ok_or_else(|| anyhow::anyhow!("{} was never set up here", target.addr()))?;
    let admin = host
        .admin
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("{}: the setup did not finish", target.addr()))?;
    let app = app_key::load_or_create()?;
    AdminConn::open(target, admin, app, &host.fingerprint)
}

/// Add or update `upsert` by uid, preserving other cores, their unknown fields and all sections.
/// `tape` seeds only a station without a window; malformed tables return an error.
pub fn merge_cores(
    current: Option<&str>,
    upsert: &[CoreKey],
    tape: Option<TapeWindow>,
) -> anyhow::Result<String> {
    let file: toml::Table = match current {
        Some(text) => toml::from_str(text).context("the server's station.toml")?,
        None => toml::Table::new(),
    };
    merge_core_table(file, upsert, tape)
}

/// Serialize uid-preserving credential metadata, including override updates/clears and the uid floor.
fn merge_core_table(
    mut file: toml::Table,
    upsert: &[CoreKey],
    tape: Option<TapeWindow>,
) -> anyhow::Result<String> {
    let retired = config_high_water(&file)?;
    let high_water = retired.max(upsert.iter().map(|core| core.uid).max().unwrap_or(0));
    let cores = file
        .entry("core")
        .or_insert_with(|| toml::Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("core in station.toml is not an array"))?;
    for core in upsert {
        let uid = i64::try_from(core.uid).context("core uid exceeds TOML's integer range")?;
        let position = cores
            .iter()
            .position(|entry| entry.get("uid").and_then(toml::Value::as_integer) == Some(uid));
        let entry = match position {
            Some(index) => &mut cores[index],
            None => {
                anyhow::ensure!(core.uid > retired, "station uid retired, refresh");
                cores.push(toml::Value::Table(toml::Table::new()));
                cores.last_mut().expect("just appended")
            }
        }
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("core in station.toml is not a table"))?;
        entry.insert("uid".into(), toml::Value::Integer(uid));
        entry.insert("name".into(), toml::Value::String(core.name.clone()));
        entry.insert("active".into(), toml::Value::Boolean(true));
        if core.endpoint_override.is_empty() {
            entry.remove("endpoint_override");
        } else {
            entry.insert(
                "endpoint_override".into(),
                toml::Value::String(core.endpoint_override.clone()),
            );
        }
        match core.transport {
            Some(transport) => {
                entry.insert("transport".into(), toml::Value::try_from(transport)?);
            }
            None => {
                entry.remove("transport");
            }
        }
    }
    set_high_water(&mut file, high_water)?;
    if !file.contains_key("tape") {
        if let Some(tape) = tape {
            file.insert("tape".into(), toml::Value::try_from(tape)?);
        }
    }
    Ok(toml::to_string_pretty(&file)?)
}

/// Return changed config and actually removed uids; absent identities are harmless retries.
pub fn without_cores(current: &str, uids: &[u64]) -> anyhow::Result<(String, Vec<u64>)> {
    let mut file: toml::Table = toml::from_str(current).context("the server's station.toml")?;
    let high_water = config_high_water(&file)?;
    set_high_water(&mut file, high_water)?;
    let cores = file
        .get_mut("core")
        .and_then(toml::Value::as_array_mut)
        .ok_or_else(|| anyhow::anyhow!("station.toml has no core array"))?;
    let mut removed = Vec::new();
    cores.retain(|entry| {
        entry
            .get("uid")
            .and_then(toml::Value::as_integer)
            .and_then(|uid| u64::try_from(uid).ok())
            .is_none_or(|uid| {
                if uids.contains(&uid) {
                    removed.push(uid);
                    false
                } else {
                    true
                }
            })
    });
    anyhow::ensure!(
        !cores.is_empty(),
        "removal would leave the station without cores"
    );
    Ok((toml::to_string_pretty(&file)?, removed))
}

/// Read the retirement floor and surviving entries before an edit can remove their evidence.
fn config_high_water(file: &toml::Table) -> anyhow::Result<u64> {
    let persisted = match file.get("core_uid_high_water") {
        None => 0,
        Some(value) => value
            .as_integer()
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| anyhow::anyhow!("invalid core_uid_high_water"))?,
    };
    let configured = file
        .get("core")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("uid").and_then(toml::Value::as_integer))
        .filter_map(|uid| u64::try_from(uid).ok())
        .max()
        .unwrap_or(0);
    Ok(persisted.max(configured))
}

/// Insert the TOML-bounded allocation floor into the table the caller will persist with its cores.
fn set_high_water(file: &mut toml::Table, high_water: u64) -> anyhow::Result<()> {
    let high_water =
        i64::try_from(high_water).context("station uid history exceeds TOML's integer range")?;
    file.insert(
        "core_uid_high_water".into(),
        toml::Value::Integer(high_water),
    );
    Ok(())
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
    say: &mut dyn FnMut(Progress),
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
        say(Progress::step(
            Step::TapePending,
            "tape window: the station has no station.toml yet, it comes with the cores",
        ));
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
        say(Progress::step(
            Step::TapeWritten,
            format!(
                "tape window {}; the station is not running and takes it at its next start: {window}",
                if wrote {
                    "written"
                } else {
                    "already in its file"
                }
            ),
        ));
        return Ok(());
    }
    // A service without the control API cannot say what it records with.
    if script::value(&status, "api") != Some("yes") {
        script::checked(conn.run(&script::helper("reload", &[]), &[], STEP_TIMEOUT)?)?;
        say(Progress::step(
            Step::TapeReload,
            format!(
                "tape window written, the station asked to re-read it (its service does not report \
             the window: update it): {window}"
            ),
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
fn reported_tape(conn: &AdminConn) -> anyhow::Result<Option<TapeWindow>> {
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
    conn: &AdminConn,
    tape: TapeWindow,
    window: &str,
    say: &mut dyn FnMut(Progress),
) -> anyhow::Result<()> {
    let deadline = std::time::Instant::now() + TAPE_APPLIED_WITHIN;
    loop {
        let last_chance = std::time::Instant::now() >= deadline;
        match reported_tape(conn) {
            Ok(Some(applied)) if applied == tape => {
                say(Progress::step(
                    Step::TapeApplied,
                    format!("tape window on the station: {window}"),
                ));
                return Ok(());
            }
            Ok(None) => {
                say(Progress::step(
                    Step::TapeReload,
                    format!(
                        "tape window written, the station asked to re-read it (its service does not \
                     report the window: update it): {window}"
                    ),
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

/// `current` with `[update] auto` set to `on`, or `None` when it already says exactly that — the
/// other sections as they were. An absent switch is on, so a file without one already says `on`.
///
/// Args:
///     current: The server's `station.toml`.
///     on: Whether the station updates itself from the release.
pub fn with_auto_update(current: &str, on: bool) -> anyhow::Result<Option<String>> {
    let mut file: toml::Table = toml::from_str(current).context("the server's station.toml")?;
    let section = file
        .entry("update")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("[update] in station.toml is not a table"))?;
    let now = section
        .get("auto")
        .and_then(toml::Value::as_bool)
        .unwrap_or(true);
    if now == on {
        return Ok(None);
    }
    section.insert("auto".to_owned(), toml::Value::Boolean(on));
    Ok(Some(toml::to_string_pretty(&file)?))
}

/// Switch the station's own updates from the release (the user's switch in the station's
/// section): `[update] auto` in its `station.toml`, then a reload (SIGHUP), the way `push_tape`
/// sets the window — the cores and the bot stay connected. A stopped station takes it at its next
/// start; a server without `station.toml` yet is refused, there is nothing to switch.
pub fn push_auto_update(
    target: &Target,
    on: bool,
    say: &mut dyn FnMut(Progress),
) -> anyhow::Result<()> {
    let conn = admin_conn(target)?;
    let status = current_helper_status(&conn)?;
    anyhow::ensure!(
        script::value(&status, "config") == Some("yes"),
        "the station has no station.toml yet: send its cores first"
    );
    let wrote = edit_config(&conn, true, |current| match current {
        Some(current) => with_auto_update(current, on),
        None => Ok(None),
    })?;
    let status = script::checked(conn.run(&script::helper("status", &[]), &[], STEP_TIMEOUT)?)?
        .stdout_text();
    if script::value(&status, "active") != Some("active")
        || script::value(&status, "api") != Some("yes")
    {
        if script::value(&status, "active") == Some("active") {
            script::checked(conn.run(&script::helper("reload", &[]), &[], STEP_TIMEOUT)?)?;
        }
        say(Progress::step(
            Step::AutoUpdateWritten,
            format!("auto-update {}: written to station.toml", on_off(on)),
        ));
        return Ok(());
    }
    if wrote || reported_auto_update(&conn)? != Some(on) {
        script::checked(conn.run(&script::helper("reload", &[]), &[], STEP_TIMEOUT)?)?;
    }
    let deadline = std::time::Instant::now() + TAPE_APPLIED_WITHIN;
    loop {
        let last_chance = std::time::Instant::now() >= deadline;
        match reported_auto_update(&conn) {
            Ok(Some(applied)) if applied == on => {
                say(Progress::step(
                    Step::AutoUpdateApplied,
                    format!("auto-update on the station: {}", on_off(on)),
                ));
                return Ok(());
            }
            // A service older than the switch: written, it takes it once updated.
            Ok(None) => {
                say(Progress::step(
                    Step::AutoUpdateWritten,
                    format!("auto-update {}: written to station.toml", on_off(on)),
                ));
                return Ok(());
            }
            Ok(Some(_)) => anyhow::ensure!(
                !last_chance,
                "the station did not take the auto-update switch (its journal says why)"
            ),
            Err(e) if last_chance => {
                return Err(e.context("the switch is written; the station did not confirm it"));
            }
            Err(_) => {}
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// `on` or `off`, for a progress diagnostic.
fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// The switch the running station reports; `None` from a service older than it.
fn reported_auto_update(conn: &AdminConn) -> anyhow::Result<Option<bool>> {
    match api::call(conn, &Request::Status)? {
        Answer::Status(status) => Ok(status.auto_update),
        other => anyhow::bail!("the station answered a status request with {other:?}"),
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
    conn: &AdminConn,
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

/// Require guarded removal, bot preservation and an explicit installed-binary capability reading.
/// Both yes and no are current: an old binary must not cause repeated helper refreshes.
fn helper_is_current(status: &str) -> bool {
    script::value(status, "bot_return") == Some("yes")
        && script::value(status, "remove_station") == Some("yes")
        && script::value(status, "removal_guard") == Some("yes")
        && script::value(status, "bot_settings_merge") == Some("yes")
        && matches!(
            script::value(status, "core_endpoint_override"),
            Some("yes" | "no")
        )
}

/// The helper's `status`, after putting this crate's helper in place when the server's is older —
/// before anything else is written. The administrator's sudo needs no password (`setup`), so the
/// terminal updates the helper itself; a server set up before that is refused with the way out.
fn current_helper_status(conn: &AdminConn) -> anyhow::Result<String> {
    let status = |conn: &AdminConn| -> anyhow::Result<String> {
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
    script::checked(out).context(StationError::HelperTooOld)?;
    let text = status(conn)?;
    anyhow::ensure!(helper_is_current(&text), StationError::HelperTooOld);
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
    say: &mut dyn FnMut(Progress),
) -> anyhow::Result<()> {
    let conn = admin_conn(target)?;
    let run = |command: String, stdin: &[u8]| -> anyhow::Result<String> {
        Ok(script::checked(conn.run(&command, stdin, STEP_TIMEOUT)?)?.stdout_text())
    };
    let status = current_helper_status(&conn)?;
    let has_token = script::value(&status, "token") == Some("yes");
    anyhow::ensure!(
        off || token.is_some() || has_token,
        StationError::BotTokenMissing
    );
    anyhow::ensure!(
        script::value(&status, "config") == Some("yes"),
        StationError::ConfigMissing
    );
    if let Some(token) = token {
        anyhow::ensure!(!token.is_empty(), "the token is empty");
        run(script::helper("put-token", &[]), token.expose().as_bytes())
            .context("token credential")?;
        say(Progress::step(
            Step::TokenWritten,
            "bot token: credential written",
        ));
    }
    edit_config(&conn, true, |current| {
        let current = current.ok_or_else(|| anyhow::anyhow!("the station has no station.toml"))?;
        with_telegram(current, change, off).map(Some)
    })?;
    if off {
        if has_token {
            run(script::helper("drop-token", &[]), &[])?;
            say(Progress::step(Step::TokenDropped, "bot token: dropped"));
        }
        // The chats belonged to that bot: the next one starts unpaired.
        run(script::helper("drop-pairing", &[]), &[])?;
        say(Progress::step(Step::ChatsDropped, "paired chats: dropped"));
    }
    run(script::helper("start", &[]), &[])?;
    let status = run(script::helper("status", &[]), &[])?;
    for line in status.lines() {
        say(Progress::Diagnostic(line.to_owned()));
    }
    say(Progress::step(Step::Status, "station state read"));
    Ok(())
}

/// Allocate fresh station identities for CLI adds to an existing station, then restart.
/// A failed config write leaves unused credentials but never removes another core's key.
pub fn push_cores(
    target: &Target,
    cores: &[CoreKey],
    tape: Option<TapeWindow>,
    say: &mut dyn FnMut(Progress),
) -> anyhow::Result<()> {
    validate_cores(cores)?;
    let conn = admin_conn(target)?;
    let helper = current_helper_status(&conn)?;
    let cores = if script::value(&helper, "config") == Some("yes") {
        let status = match api::call(&conn, &Request::Status)? {
            Answer::Status(status) => status,
            _ => anyhow::bail!("the station did not answer status"),
        };
        let listing = status
            .cores
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("the station cannot list cores; update it first"))?;
        let current =
            script::checked(conn.run(&script::helper("get-config", &[]), &[], STEP_TIMEOUT)?)?
                .stdout_text();
        let file: toml::Table = toml::from_str(&current).context("the server's station.toml")?;
        let floor = config_high_water(&file)?.max(status.core_uid_high_water.unwrap_or(0));
        allocate_cli_cores(cores, listing, floor)?
    } else {
        cores
            .iter()
            .map(|core| CoreKey {
                uid: core.uid,
                name: core.name.clone(),
                transport: core.transport,
                key: core.key.clone(),
                endpoint_override: core.endpoint_override.clone(),
            })
            .collect()
    };
    push_cores_with_add_flags(target, &cores, &vec![true; cores.len()], tape, say)
}

/// Keep CLI picks add-only, rejecting the peer's effective addresses and allocating above known uids.
/// The push revalidates the floor before writing, so a concurrent allocation requires a retry.
fn allocate_cli_cores(
    cores: &[CoreKey],
    listing: &[moon_core::station_api::ListedCore],
    floor: u64,
) -> anyhow::Result<Vec<CoreKey>> {
    validate_cores(cores)?;
    let mut largest = cores
        .iter()
        .map(|core| core.uid)
        .chain(listing.iter().map(|core| core.uid))
        .max()
        .unwrap_or(0)
        .max(floor);
    cores
        .iter()
        .map(|core| {
            let endpoint_override = if listing.iter().any(|core| core.endpoint_override.is_some()) {
                core.endpoint_override.as_str()
            } else {
                ""
            };
            if let Some(address) =
                moon_core::station_api::core_address(core.key.expose(), endpoint_override)
            {
                anyhow::ensure!(
                    !listing
                        .iter()
                        .any(|listed| listed.address.as_ref() == Some(&address)),
                    "core already on the station; use the terminal to update it"
                );
            }
            largest = largest
                .checked_add(1)
                .filter(|uid| *uid <= i64::MAX as u64)
                .ok_or_else(|| anyhow::anyhow!("station uid range exhausted"))?;
            Ok(CoreKey {
                uid: largest,
                name: core.name.clone(),
                transport: core.transport,
                key: core.key.clone(),
                endpoint_override: core.endpoint_override.clone(),
            })
        })
        .collect()
}

/// Refuse invalid picks before any helper operation, including the CLI's allocation preflight.
fn validate_cores(cores: &[CoreKey]) -> anyhow::Result<()> {
    anyhow::ensure!(!cores.is_empty(), StationError::NoCorePicked);
    let mut seen = std::collections::HashSet::new();
    for core in cores {
        anyhow::ensure!(core.uid != 0, "core {:?} has no uid", core.name);
        anyhow::ensure!(seen.insert(core.uid), "uid {} twice", core.uid);
        anyhow::ensure!(
            !core.key.is_empty(),
            StationError::CoreWithoutKey(core.name.clone())
        );
    }
    Ok(())
}

/// Push reconciled cores, refusing occupied or retired uids before writing any credential.
/// The installed binary's capability controls override fields even before its first start;
/// older stations receive the original key-only configuration.
/// CLI picks and fresh installs mark every picked core as an add.
pub fn push_cores_with_add_flags(
    target: &Target,
    cores: &[CoreKey],
    adds: &[bool],
    tape: Option<TapeWindow>,
    say: &mut dyn FnMut(Progress),
) -> anyhow::Result<()> {
    validate_cores(cores)?;
    let conn = admin_conn(target)?;
    let run = |command: String, stdin: &[u8]| -> anyhow::Result<String> {
        Ok(script::checked(conn.run(&command, stdin, STEP_TIMEOUT)?)?.stdout_text())
    };
    // Read before anything is written: an old helper or an unreadable file stops the push here,
    // not between the credentials and the configuration. No file yet is the first push.
    let status = current_helper_status(&conn)?;
    let cores = peer_cores(cores, &status);
    let cores = cores.as_slice();
    let exists = script::value(&status, "config") == Some("yes");
    // Existing history needs a live reading before an add; stopped/unreadable stations fail closed.
    let high_water = if exists && adds.contains(&true) {
        match api::call(&conn, &Request::Status)? {
            Answer::Status(status) => status.core_uid_high_water,
            _ => anyhow::bail!("the station did not answer status"),
        }
    } else {
        None
    };
    // A dry run of the merge before any credential is written; the real one below reads the file
    // again, as it is by then.
    let current = if exists {
        Some(
            run(script::helper("get-config", &[]), &[])
                .context("read the server's station.toml")?,
        )
    } else {
        None
    };
    merge_cores_with_add_flags(current.as_deref(), cores, adds, tape, high_water)?;
    for core in cores {
        let uid = core.uid.to_string();
        run(
            script::helper("put-cred", &[&uid]),
            core.key.expose().as_bytes(),
        )
        .with_context(|| format!("credential of {}", core.name))?;
        say(Progress::step(
            Step::CoreWritten,
            format!("core {} ({}): credential written", core.name, core.uid),
        ));
    }
    edit_config(&conn, exists, |current| {
        merge_cores_with_add_flags(current, cores, adds, tape, high_water).map(Some)
    })?;
    restart_and_report(&conn, say)
}

/// Suppress unknown TOML fields for old binaries; helper upgrades alone cannot enable overrides.
fn peer_cores(cores: &[CoreKey], status: &str) -> Vec<CoreKey> {
    let supports = script::value(status, "core_endpoint_override") == Some("yes");
    cores
        .iter()
        .map(|core| CoreKey {
            uid: core.uid,
            name: core.name.clone(),
            transport: core.transport,
            key: core.key.clone(),
            endpoint_override: if supports {
                core.endpoint_override.clone()
            } else {
                String::new()
            },
        })
        .collect()
}

/// Reject stale adds, retired uids and missing updates before any credential write.
fn merge_cores_with_add_flags(
    current: Option<&str>,
    cores: &[CoreKey],
    adds: &[bool],
    tape: Option<TapeWindow>,
    high_water: Option<u64>,
) -> anyhow::Result<String> {
    anyhow::ensure!(
        cores.len() == adds.len(),
        "core add flags do not match selected cores"
    );
    let mut file: toml::Table = match current {
        Some(text) => toml::from_str(text).context("the server's station.toml")?,
        None => toml::Table::new(),
    };
    let high_water = config_high_water(&file)?.max(high_water.unwrap_or(0));
    let entries = file.get("core").and_then(toml::Value::as_array);
    for (core, add) in cores.iter().zip(adds) {
        let exists = entries.is_some_and(|entries| {
            entries.iter().any(|entry| {
                entry
                    .get("uid")
                    .and_then(toml::Value::as_integer)
                    .and_then(|uid| u64::try_from(uid).ok())
                    == Some(core.uid)
            })
        });
        anyhow::ensure!(*add != exists, "station changed, refresh");
        anyhow::ensure!(
            !add || core.uid > high_water,
            "station uid retired, refresh"
        );
    }
    set_high_water(&mut file, high_water)?;
    merge_core_table(file, cores, tape)
}

/// Remove exactly the named config entries, then their credentials, leaving report data intact.
/// Empty or zero uids and removing the last core are refused; config failures keep credentials.
/// An absent uid is an idempotent credential cleanup, which can race a concurrent add from
/// another terminal. The terminal UI guards its removal selection with `removal_matches`.
pub fn remove_cores(
    target: &Target,
    uids: &[u64],
    say: &mut dyn FnMut(Progress),
) -> anyhow::Result<()> {
    anyhow::ensure!(!uids.is_empty(), "no cores selected for removal");
    anyhow::ensure!(!uids.contains(&0), "uid 0 is not a uid");
    let conn = admin_conn(target)?;
    let status = current_helper_status(&conn)?;
    let exists = script::value(&status, "config") == Some("yes");
    let committed = std::cell::Cell::new(false);
    let cleanup = commit_cores_config(
        || {
            let changed = edit_config(&conn, exists, |current| {
                let current =
                    current.ok_or_else(|| anyhow::anyhow!("the station has no station.toml"))?;
                let (config, actual) = without_cores(current, uids)?;
                Ok((!actual.is_empty()).then_some(config))
            })?;
            committed.set(true);
            Ok(changed)
        },
        || {
            let mut dropped = std::collections::HashSet::new();
            for uid in uids {
                if !dropped.insert(*uid) {
                    continue;
                }
                script::checked(conn.run(
                    &script::helper("drop-cred", &[&uid.to_string()]),
                    &[],
                    STEP_TIMEOUT,
                )?)?;
                say(Progress::step(
                    Step::CoreDropped,
                    format!("core-{uid}: explicitly removed"),
                ));
            }
            Ok(())
        },
    );
    if !committed.get() {
        return cleanup;
    }
    finish_cores_cleanup(cleanup, || restart_and_report(&conn, say))
}

/// Always restart after cleanup, preserving a cleanup failure as the primary error.
fn finish_cores_cleanup(
    cleanup: anyhow::Result<()>,
    restart: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let restarted = restart();
    cleanup.and(restarted)
}

/// Start after a core edit and publish the helper status through the same progress tail.
fn restart_and_report(conn: &AdminConn, say: &mut dyn FnMut(Progress)) -> anyhow::Result<()> {
    script::checked(conn.run(&script::helper("start", &[]), &[], STEP_TIMEOUT)?)?;
    let status = script::checked(conn.run(&script::helper("status", &[]), &[], STEP_TIMEOUT)?)?
        .stdout_text();
    for line in status.lines() {
        say(Progress::Diagnostic(line.to_owned()));
    }
    say(Progress::step(Step::Status, "station state read"));
    Ok(())
}

/// Commit the removal before credential cleanup; any write failure keeps every old credential.
/// A cleanup failure is returned after the config is safely committed.
fn commit_cores_config(
    write_config: impl FnOnce() -> anyhow::Result<bool>,
    drop_credentials: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    write_config()?;
    drop_credentials()
}

/// Update the station from the latest release (the Settings' "Update the service"): the helper's
/// `update-from-release` has the installed binary find the release and download it, checked
/// against its immutable digest, then installs it as `update` does — a restart that must stay up,
/// or the previous binary back. The same the bot's "Update" starts through its request file.
pub fn update_from_release(target: &Target, say: &mut dyn FnMut(Progress)) -> anyhow::Result<()> {
    let conn = admin_conn(target)?;
    let status = current_helper_status(&conn)?;
    anyhow::ensure!(
        script::value(&status, "bin") != Some("none"),
        "no station is installed on {} yet",
        target.addr()
    );
    // The connection, not the update, failed — an error, or an end without an exit status: the
    // update goes on without it (the helper detaches the restart), and what the server says it
    // did is the answer, read through a new one.
    let out = match conn.run(
        &script::helper("update-from-release", &[]),
        &[],
        script::APT_TIMEOUT,
    ) {
        Ok(out) if out.status.is_some() => out,
        dropped => {
            let e = match dropped {
                Ok(out) => anyhow::anyhow!(
                    "the connection ended before the update did: {}",
                    out.stdout_text().trim()
                ),
                Err(e) => e,
            };
            let verdict = admin_conn(target).ok().and_then(|conn| {
                let out = conn
                    .run(&script::helper("status", &[]), &[], STEP_TIMEOUT)
                    .ok()?;
                script::value(&out.stdout_text(), "last_update").map(str::to_owned)
            });
            return Err(match verdict {
                Some(verdict) => e.context(format!("the server's last update says: {verdict}")),
                None => e,
            });
        }
    };
    let text = script::checked(out)?.stdout_text();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        say(Progress::Diagnostic(line.to_owned()));
    }
    say(Progress::step(Step::Update, "update request finished"));
    // Not a failure, and not an update either: said in words, not only as the helper's token.
    if let Some(none) = text.lines().find(|l| l.starts_with("update=none")) {
        say(Progress::step(
            if none.contains("unversioned") {
                Step::Unversioned
            } else {
                Step::NoNewRelease
            },
            match none.contains("unversioned") {
                true => "nothing installed: this station build is not from a release",
                false => "nothing installed: no release newer than the station carries its binary",
            },
        ));
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
    say: &mut dyn FnMut(Progress),
) -> anyhow::Result<()> {
    use std::io::Write;
    let plain = std::fs::read(snapshot).with_context(|| format!("read {}", snapshot.display()))?;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&plain)?;
    let packed = gz.finish()?;
    say(Progress::step(
        Step::ValuationSend,
        format!(
            "valuation cache: {} MB, {} MB to send",
            plain.len() / 1_000_000,
            packed.len() / 1_000_000
        ),
    ));
    let conn = admin_conn(target)?;
    // A station already valuing on its own (a server set up anew over a running one) keeps it.
    if script::value(&current_helper_status(&conn)?, "valuation") == Some("yes") {
        say(Progress::step(
            Step::ValuationKept,
            "valuation cache: the station has its own, kept",
        ));
        return Ok(());
    }
    let out = script::checked(conn.run(
        &script::helper("put-valuation", &[]),
        &packed,
        script::APT_TIMEOUT,
    )?)?;
    for line in out.stdout_text().lines() {
        say(Progress::Diagnostic(line.to_owned()));
    }
    say(Progress::step(
        Step::ValuationWritten,
        "valuation cache written",
    ));
    Ok(())
}

pub mod access;
pub mod api;
pub mod bot;
pub mod pull;

#[cfg(test)]
mod tests;
