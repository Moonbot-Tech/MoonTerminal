//! The station's bot seen from the terminal: hand the terminal's bot over, and read how the
//! station's bot is doing — from the station's control API (`super::api`), with whether the
//! station holds a token from the helper's `status`.

use std::time::{Duration, Instant};

use crate::error::StationError;
use crate::progress::{Progress, Step};
use anyhow::Context;
use moon_core::config::Secret;
use moon_core::station_api::{Access, Answer, BotStatus, Request, Status, TapeWindow};
use moon_core::telegram::TelegramStatus;

use super::{BotChange, admin_conn, api, current_helper_status, edit_config, with_telegram};
use crate::script::{self, STEP_TIMEOUT};
use crate::ssh::Target;

/// A recovered bot; credentials never appear in progress or error text.
#[derive(Clone)]
pub struct ReturnedBot {
    pub token: Secret,
    pub access: Access,
}

/// Typed, secret-free read-back failures for localized UI feedback.
#[derive(Debug)]
pub enum BotReturnError {
    /// The installed helper cannot export the bot yet.
    OldHelper,
    /// A connection, credential or pairing read failed before removal.
    ReadFailed,
}

impl std::fmt::Display for BotReturnError {
    /// Fixed diagnostics for non-UI callers, never remote credential output.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::OldHelper => "the helper cannot return the bot: run setup again first",
            Self::ReadFailed => "could not read the station bot; it has not been removed",
        })
    }
}

impl std::error::Error for BotReturnError {}

/// Read the token and the station's saved Access over admin SSH before any removal.
pub fn read_bot(target: &Target) -> anyhow::Result<ReturnedBot> {
    let conn = admin_conn(target).map_err(|_| BotReturnError::ReadFailed)?;
    let status = conn
        .run(&script::helper("status", &[]), &[], STEP_TIMEOUT)
        .map_err(|_| BotReturnError::ReadFailed)?;
    anyhow::ensure!(status.ok(), BotReturnError::ReadFailed);
    if script::value(&status.stdout_text(), "bot_return") != Some("yes") {
        return Err(BotReturnError::OldHelper.into());
    }
    let out = conn
        .run_secret(&script::helper("get-token", &[]), &[], STEP_TIMEOUT)
        .map_err(|_| BotReturnError::ReadFailed)?;
    let token = decode_token(out)?;
    let out = conn
        .run(&script::helper("get-pairing", &[]), &[], STEP_TIMEOUT)
        .map_err(|_| BotReturnError::ReadFailed)?;
    anyhow::ensure!(out.ok(), BotReturnError::ReadFailed);
    let mut access: Access =
        serde_json::from_slice(&out.stdout).map_err(|_| BotReturnError::ReadFailed)?;
    access.adopt_legacy_owner();
    access.check().map_err(|_| BotReturnError::ReadFailed)?;
    Ok(ReturnedBot { token, access })
}

/// Protect SSH buffers before inspecting them and never include their contents in errors.
fn decode_token(out: crate::ssh::SecretOutput) -> anyhow::Result<Secret> {
    anyhow::ensure!(out.status == Some(0), BotReturnError::ReadFailed);
    let text = std::str::from_utf8(&out.stdout).map_err(|_| BotReturnError::ReadFailed)?;
    let text = text.trim();
    anyhow::ensure!(
        !text.is_empty() && !text.contains(['\r', '\n']),
        BotReturnError::ReadFailed
    );
    Ok(Secret::new(text))
}

/// Read first, stop and remove second; a failed read preserves the station's only bot copy.
pub fn return_bot(
    target: &Target,
    restore: bool,
    recovered: Option<ReturnedBot>,
    remember: &mut dyn FnMut(ReturnedBot),
    say: &mut dyn FnMut(Progress),
) -> anyhow::Result<Option<ReturnedBot>> {
    read_then_remove(
        restore,
        || {
            let returned = match recovered {
                Some(returned) => {
                    // A failed removal may have left the credential on the station. Its bot may
                    // have been replaced since: read that bot rather than restoring the old token
                    // with the new bot's grants. Only an absent credential uses our retained copy.
                    let conn = admin_conn(target).map_err(|_| BotReturnError::ReadFailed)?;
                    let status = conn
                        .run(&script::helper("status", &[]), &[], STEP_TIMEOUT)
                        .map_err(|_| BotReturnError::ReadFailed)?;
                    anyhow::ensure!(status.ok(), BotReturnError::ReadFailed);
                    let has_token = match script::value(&status.stdout_text(), "token") {
                        Some("yes") => true,
                        Some("no") => false,
                        _ => return Err(BotReturnError::ReadFailed.into()),
                    };
                    current_or_recovered(returned, has_token, || read_bot(target))?
                }
                None => read_bot(target)?,
            };
            Ok(returned)
        },
        remember,
        |returned, remember| {
            // Prevent the live station from rewriting pairing after it has been deleted.
            let conn = admin_conn(target)?;
            script::checked(conn.run(&script::helper("stop", &[]), &[], STEP_TIMEOUT)?)?;
            if let Some(returned) = returned {
                // A chat may have paired between the first read and stop. Read the final saved
                // Access before deleting it; on a partial-removal retry retain our copy if gone.
                let status = script::checked(conn.run(
                    &script::helper("status", &[]),
                    &[],
                    STEP_TIMEOUT,
                )?)?;
                if script::value(&status.stdout_text(), "pairing") == Some("yes") {
                    let out = conn
                        .run(&script::helper("get-pairing", &[]), &[], STEP_TIMEOUT)
                        .map_err(|_| BotReturnError::ReadFailed)?;
                    anyhow::ensure!(out.ok(), BotReturnError::ReadFailed);
                    let mut access: Access = serde_json::from_slice(&out.stdout)
                        .map_err(|_| BotReturnError::ReadFailed)?;
                    access.adopt_legacy_owner();
                    access.check().map_err(|_| BotReturnError::ReadFailed)?;
                    returned.access = access;
                    remember(returned.clone());
                }
            }
            super::push_telegram(target, None, &BotChange::default(), true, say)
        },
    )
}

/// The station's current bot takes precedence on retry; retain our snapshot only when its
/// credential has already been removed, so a partial removal remains recoverable.
fn current_or_recovered(
    recovered: ReturnedBot,
    has_token: bool,
    read: impl FnOnce() -> anyhow::Result<ReturnedBot>,
) -> anyhow::Result<ReturnedBot> {
    if has_token { read() } else { Ok(recovered) }
}

/// The ordering contract shared by production SSH execution and fixture tests.
fn read_then_remove(
    restore: bool,
    read: impl FnOnce() -> anyhow::Result<ReturnedBot>,
    remember: &mut dyn FnMut(ReturnedBot),
    remove: impl FnOnce(&mut Option<ReturnedBot>, &mut dyn FnMut(ReturnedBot)) -> anyhow::Result<()>,
) -> anyhow::Result<Option<ReturnedBot>> {
    let mut returned = if restore { Some(read()?) } else { None };
    if let Some(returned) = &returned {
        remember(returned.clone());
    }
    remove(&mut returned, remember)?;
    Ok(returned)
}

/// How long a handed-over bot may take to come up paired: the station restarts, checks its
/// replica, connects the bot — ~30 s on one vCPU (STATION.md §7.7).
const PAIRED_WITHIN: Duration = Duration::from_secs(90);
/// How often the station is asked while waiting.
const ASK_EVERY: Duration = Duration::from_secs(3);
/// Looks in a row that find the station stopped before a hand-over gives up (~15 s): systemd
/// restarts a crash after 5 s, so a stop that lasts is not a restart.
const STOPPED_LOOKS: u32 = 5;

/// How the station's bot is doing, read at one moment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BotState {
    /// The station is not running.
    pub stopped: bool,
    /// The station runs but its API does not answer yet: it is starting, or its binary predates
    /// the API and needs an update.
    pub no_api: bool,
    /// The station holds a bot token (its credential exists); `false` runs no bot.
    pub has_token: bool,
    /// The running bot as the station tells it; `None` while it runs none.
    pub bot: Option<BotStatus>,
    /// Until when the bot's pairing code is accepted, counted from this read.
    pub pairing_until: Option<Instant>,
    /// The paired chats, the owner and the viewers' grants, while a bot runs.
    pub access: Option<Access>,
    /// The window around a trade the station records with; `None` from a station older than it.
    pub tape: Option<TapeWindow>,
    /// The station's whole status as read, for its "Status"; `None` while it does not answer.
    /// Boxed: the state travels in every job's end.
    pub station: Option<Box<Status>>,
}

impl BotState {
    /// Whether the bot runs with at least one paired chat.
    pub fn paired(&self) -> bool {
        self.bot
            .as_ref()
            .is_some_and(|b| matches!(b.status, TelegramStatus::Paired { .. }))
    }

    /// Whether the bot polls Telegram: paired, or waiting for a chat to pair. Both are published
    /// only after a successful poll, so the token works and no other poller holds it.
    pub fn polling(&self) -> bool {
        self.paired()
            || self
                .bot
                .as_ref()
                .is_some_and(|b| b.status == TelegramStatus::Unpaired)
    }

    /// The state in a few words, for a progress line.
    pub fn summary(&self) -> String {
        match &self.bot {
            _ if self.stopped => "station stopped".into(),
            _ if self.no_api => "station not answering yet".into(),
            None => "no bot".into(),
            Some(bot) => format!("{:?}, Mini App {:?}", bot.status, bot.mini_app),
        }
    }

    /// The code to send as `/pair <code>`, while it is still accepted.
    pub fn pairing_code(&self) -> Option<&str> {
        let code = self.bot.as_ref()?.pairing.as_ref()?;
        self.pairing_until
            .is_some_and(|until| Instant::now() < until)
            .then_some(code.code.as_str())
    }
}

/// How the station's bot is doing now, with its chats.
pub fn bot_state(target: &Target) -> anyhow::Result<BotState> {
    let conn = admin_conn(target)?;
    let status = current_helper_status(&conn)?;
    let mut state = BotState {
        has_token: script::value(&status, "token") == Some("yes"),
        ..BotState::default()
    };
    if script::value(&status, "active") != Some("active") {
        state.stopped = true;
        return Ok(state);
    }
    if script::value(&status, "api") != Some("yes") {
        state.no_api = true;
        return Ok(state);
    }
    let read_at = Instant::now();
    let Answer::Status(station) = api::call(&conn, &Request::Status)? else {
        anyhow::bail!("the station answered a status request with something else");
    };
    if let Some(bot) = &station.bot {
        state.pairing_until = bot
            .pairing
            .as_ref()
            .map(|p| read_at + Duration::from_secs(p.expires_in_s));
        state.access = match api::call(&conn, &Request::AccessGet)? {
            Answer::Access(access) => Some(access),
            other => anyhow::bail!("the station answered an access request with {other:?}"),
        };
    }
    state.tape = station.tape;
    state.bot = station.bot.clone();
    state.station = Some(Box::new(station));
    Ok(state)
}

/// Give the station's bot `token`: a new bot, or another token for the one it runs. A different
/// token is a different bot, and chats paired with the old one are not its chats: they are dropped
/// FIRST, so the new bot never serves them even for a moment; then the token goes in over the old
/// one (a failure there leaves the old bot, unpaired — never no bot), and the wait until it polls.
pub fn set_token(
    target: &Target,
    token: &Secret,
    change: &BotChange,
    say: &mut dyn FnMut(Progress),
) -> anyhow::Result<BotState> {
    anyhow::ensure!(!token.is_empty(), "the token is empty");
    if bot_state(target)?.has_token {
        let conn = admin_conn(target)?;
        script::checked(conn.run(&script::helper("drop-pairing", &[]), &[], STEP_TIMEOUT)?)?;
        say(Progress::step(
            Step::ChatsDropped,
            "paired chats of the previous bot: dropped",
        ));
    }
    // Restarts the station: the new token and the empty pairing take effect together.
    super::push_telegram(target, Some(token), change, false, say)?;
    wait_polling(target, false, say)
}

/// Move the terminal's bot to the station: its token, its paired chats with their access, and
/// `[telegram]`; then restart the station and wait until its bot polls — paired when chats came
/// with it, offering a pairing code when none did.
///
/// The caller suspends its own bot BEFORE this — one token, one poller — and erases its token only
/// when this returns `Ok`; on an error its own bot is what should run again, and the station's is
/// taken off here first (token, chats and `[telegram]`). When that undo fails too the error carries
/// [`StationMayStillPoll`]: the terminal's bot must then stay down, or the two poll one token.
///
/// Args:
///     target: The station's server.
///     token: The bot token.
///     pairing: The chats the terminal's bot had paired; empty when it had none.
///     change: `[telegram]` for the station: Mini App, zone, language.
///     say: One line per step.
///
/// Returns:
///     The bot's state once it polls.
pub fn transfer_bot(
    target: &Target,
    token: &Secret,
    pairing: &Access,
    change: &BotChange,
    say: &mut dyn FnMut(Progress),
) -> anyhow::Result<BotState> {
    anyhow::ensure!(!token.is_empty(), "the token is empty");
    let conn = admin_conn(target)?;
    let run = |command: String, stdin: &[u8]| -> anyhow::Result<String> {
        Ok(script::checked(conn.run(&command, stdin, STEP_TIMEOUT)?)?.stdout_text())
    };
    let status = current_helper_status(&conn)?;
    anyhow::ensure!(
        script::value(&status, "config") == Some("yes"),
        StationError::ConfigMissing
    );
    // A bot already there has its own chats; handing another over would drop them.
    anyhow::ensure!(
        script::value(&status, "token") != Some("yes"),
        StationError::BotAlreadyPresent
    );
    let current =
        run(script::helper("get-config", &[]), &[]).context("read the server's station.toml")?;
    // Checked before anything is written; written below over whatever the file holds by then.
    with_telegram(&current, change, false)?;
    let pairing_json = serde_json::to_vec(pairing).context("encode telegram.json")?;

    let handed = (|| -> anyhow::Result<BotState> {
        run(script::helper("put-token", &[]), token.expose().as_bytes())
            .context("token credential")?;
        say(Progress::step(
            Step::TokenWritten,
            "bot token: credential written",
        ));
        // Stops the station: it owns the file and would overwrite it on a /pair.
        run(script::helper("put-pairing", &[]), &pairing_json).context("telegram.json")?;
        say(Progress::step(
            Step::ChatsTransferred,
            format!(
                "paired chats: {} handed over",
                pairing.authorized_chat_ids.len()
            ),
        ));
        edit_config(&conn, true, |now| {
            let now = now.ok_or_else(|| anyhow::anyhow!("the station has no station.toml"))?;
            with_telegram(now, change, false).map(Some)
        })?;
        run(script::helper("start", &[]), &[])?;
        say(Progress::step(
            Step::BotStarted,
            "station started with the bot",
        ));
        wait_polling(target, !pairing.authorized_chat_ids.is_empty(), say)
    })();
    let Err(e) = handed else {
        return handed;
    };
    say(Progress::step(
        Step::UndoHandover,
        "hand-over failed: taking the bot off the station",
    ));
    match take_bot_off(target, &current) {
        Ok(()) => {
            say(Progress::step(
                Step::BotRemoved,
                "the station's bot is off again: token and chats dropped",
            ));
            Err(e)
        }
        Err(undo) => Err(e
            .context(format!(
                "the hand-over failed AND taking the bot off the station failed ({undo:#})"
            ))
            .context(StationMayStillPoll)),
    }
}

/// Wait until the station's bot polls again after its transport restarted (a change of the
/// chats' permissions): paired or offering a code, either will do.
pub fn wait_bot(target: &Target, say: &mut dyn FnMut(Progress)) -> anyhow::Result<BotState> {
    wait_polling(target, false, say)
}

/// Wait until the station, restarted with its Mini App switched `on` or off, polls again and its
/// Mini App has settled: published or failed when on, stopped when off.
pub fn wait_mini_app(
    target: &Target,
    on: bool,
    say: &mut dyn FnMut(Progress),
) -> anyhow::Result<BotState> {
    use moon_core::telegram::runtime::mini_app::MiniAppStatus;
    wait_until(target, say, |state| {
        state.polling()
            && state.bot.as_ref().is_some_and(|bot| {
                bot.mini_app_on == on
                    && match on {
                        true => matches!(
                            bot.mini_app,
                            MiniAppStatus::Tunneling { .. } | MiniAppStatus::Failed { .. }
                        ),
                        false => bot.mini_app == MiniAppStatus::Stopped,
                    }
            })
    })
}

/// A failed hand-over whose undo failed too: the station may still poll the token, so the
/// terminal's own bot must stay down until the user takes the bot off the station.
#[derive(Debug)]
pub struct StationMayStillPoll;

impl std::fmt::Display for StationMayStillPoll {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the station may still poll the bot token")
    }
}

/// Wait until the station's bot polls: `Paired` when chats came with it, `Unpaired` (a pairing
/// code offered) when none did. Each look is a fresh connection, and a failed look is only one
/// look: a dropped connection must not end a hand-over the station has already taken.
fn wait_polling(
    target: &Target,
    with_chats: bool,
    say: &mut dyn FnMut(Progress),
) -> anyhow::Result<BotState> {
    wait_until(target, say, |state| match with_chats {
        true => state.paired(),
        false => state.polling(),
    })
}

/// Ask the station every few seconds until `done` holds, for up to [`PAIRED_WITHIN`]; a station
/// that stays stopped, or the deadline, ends it with an error.
fn wait_until(
    target: &Target,
    say: &mut dyn FnMut(Progress),
    done: impl Fn(&BotState) -> bool,
) -> anyhow::Result<BotState> {
    let deadline = Instant::now() + PAIRED_WITHIN;
    let mut last: Option<BotState> = None;
    let mut last_error = None;
    let mut stopped_looks = 0;
    loop {
        std::thread::sleep(ASK_EVERY);
        match look(target) {
            Ok(state) => {
                if done(&state) {
                    say(Progress::step(
                        Step::BotReady,
                        format!("bot: {}", state.summary()),
                    ));
                    return Ok(state);
                }
                stopped_looks = if state.stopped { stopped_looks + 1 } else { 0 };
                anyhow::ensure!(stopped_looks < STOPPED_LOOKS, StationError::BotStopped);
                last = Some(state);
            }
            Err(e) => last_error = Some(format!("{e:#}")),
        }
        if Instant::now() >= deadline {
            let seen = last
                .map(|s| s.summary())
                .or(last_error)
                .unwrap_or_else(|| "nothing".into());
            return Err(anyhow::anyhow!(StationError::BotNotReady).context(format!("last: {seen}")));
        }
    }
}

fn look(target: &Target) -> anyhow::Result<BotState> {
    // `bot_state`, not the API alone: the answer also says whether the station holds a token.
    bot_state(target)
}

/// Undo a hand-over on a fresh connection, every step tried whatever the others did: `[telegram]`
/// as it was (the rest of the file as it is now), no token, no chats, and the station running
/// again.
fn take_bot_off(target: &Target, config_before: &str) -> anyhow::Result<()> {
    let conn = admin_conn(target)?;
    let run = |command: String, stdin: &[u8]| -> anyhow::Result<()> {
        script::checked(conn.run(&command, stdin, STEP_TIMEOUT)?)?;
        Ok(())
    };
    let restore = || -> anyhow::Result<()> {
        let before: toml::Table = toml::from_str(config_before)?;
        edit_config(&conn, true, |now| {
            let now = now.ok_or_else(|| anyhow::anyhow!("the station has no station.toml"))?;
            let mut file: toml::Table = toml::from_str(now)?;
            match before.get("telegram") {
                Some(telegram) => file.insert("telegram".to_owned(), telegram.clone()),
                None => file.remove("telegram"),
            };
            Ok(Some(toml::to_string_pretty(&file)?))
        })?;
        Ok(())
    };
    let failed: Vec<String> = [
        restore(),
        run(script::helper("drop-token", &[]), &[]),
        run(script::helper("drop-pairing", &[]), &[]),
        run(script::helper("start", &[]), &[]),
    ]
    .into_iter()
    .filter_map(|r| r.err().map(|e| format!("{e:#}")))
    .collect();
    anyhow::ensure!(failed.is_empty(), "{}", failed.join("; "));
    Ok(())
}

#[cfg(test)]
mod tests;
