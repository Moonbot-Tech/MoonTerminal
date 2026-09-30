//! The station's bot seen from the terminal: hand the terminal's bot over, and read how the
//! station's bot is doing.
//!
//! Until the station has its API (`docs-internal/STATION.md` §4.5) the only place it tells about
//! its bot is its journal: `telegram: bot …` on every change and `send /pair <code>` while no chat
//! is paired. The helper's `tg-state` hands over those lines of the running process only.

use std::time::{Duration, Instant};

use anyhow::Context;
use moon_core::config::Secret;
use moon_core::config::telegram_access::TelegramChatAccess;
use serde::Serialize;

use super::{BotChange, admin_conn, current_helper_status, with_telegram};
use crate::script::{self, STEP_TIMEOUT};
use crate::ssh::Target;

/// How long a handed-over bot may take to come up paired: the station restarts, checks its
/// replica, connects the bot — ~30 s on one vCPU (STATION.md §7.7).
const PAIRED_WITHIN: Duration = Duration::from_secs(90);
/// How often the journal is asked while waiting.
const ASK_EVERY: Duration = Duration::from_secs(3);
/// Looks in a row that find the station stopped before a hand-over gives up (~15 s): systemd
/// restarts a crash after 5 s, so a stop that lasts is not a restart.
const STOPPED_LOOKS: u32 = 5;

/// The terminal's pairing, as the station keeps it in `telegram.json`.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Pairing {
    pub authorized_chat_ids: Vec<i64>,
    pub owner_chat_id: Option<i64>,
    pub chat_access: Vec<TelegramChatAccess>,
}

/// What the station's journal says about its bot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BotState {
    /// The station is not running.
    pub stopped: bool,
    /// The last `telegram: bot …` line, without the prefix: `Paired { chat_count: 1 }, Mini App …`.
    pub status: Option<String>,
    /// The code to send as `/pair <code>`, while no chat is paired.
    pub pairing_code: Option<String>,
    /// The station holds a bot token (its credential exists); `false` runs no bot.
    pub has_token: bool,
}

impl BotState {
    /// Whether the bot runs with at least one paired chat.
    pub fn paired(&self) -> bool {
        self.status
            .as_deref()
            .is_some_and(|s| s.starts_with("Paired"))
    }

    /// Whether the bot polls Telegram: paired, or waiting for a chat to pair. Both are published
    /// only after a successful poll, so the token works and no other poller holds it.
    pub fn polling(&self) -> bool {
        self.paired()
            || self
                .status
                .as_deref()
                .is_some_and(|s| s.starts_with("Unpaired"))
    }

    /// Read the helper's `tg-state` answer.
    pub fn parse(text: &str) -> Self {
        let mut state = Self::default();
        for line in text.lines() {
            if line.trim() == "tg=stopped" {
                state.stopped = true;
            } else if let Some(rest) = line.split("telegram: bot ").nth(1) {
                state.status = Some(rest.trim().to_owned());
                // A status line after a code is newer: a bot that paired no longer offers one.
                if rest.starts_with("Paired") {
                    state.pairing_code = None;
                }
            } else if let Some(rest) = line.split("send /pair ").nth(1) {
                state.pairing_code = rest.split_whitespace().next().map(str::to_owned);
            }
        }
        state
    }
}

/// How the station's bot is doing now.
pub fn bot_state(target: &Target) -> anyhow::Result<BotState> {
    let conn = admin_conn(target)?;
    let status = current_helper_status(&conn)?;
    let out = script::checked(conn.run(&script::helper("tg-state", &[]), &[], STEP_TIMEOUT)?)?;
    let mut state = BotState::parse(&out.stdout_text());
    state.has_token = script::value(&status, "token") == Some("yes");
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
    say: &mut dyn FnMut(&str),
) -> anyhow::Result<BotState> {
    anyhow::ensure!(!token.is_empty(), "the token is empty");
    if bot_state(target)?.has_token {
        let conn = admin_conn(target)?;
        script::checked(conn.run(&script::helper("drop-pairing", &[]), &[], STEP_TIMEOUT)?)?;
        say("paired chats of the previous bot: dropped");
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
    pairing: &Pairing,
    change: &BotChange,
    say: &mut dyn FnMut(&str),
) -> anyhow::Result<BotState> {
    anyhow::ensure!(!token.is_empty(), "the token is empty");
    let conn = admin_conn(target)?;
    let run = |command: String, stdin: &[u8]| -> anyhow::Result<String> {
        Ok(script::checked(conn.run(&command, stdin, STEP_TIMEOUT)?)?.stdout_text())
    };
    let status = current_helper_status(&conn)?;
    anyhow::ensure!(
        script::value(&status, "config") == Some("yes"),
        "the station has no station.toml yet: send its cores first"
    );
    // A bot already there has its own chats; handing another over would drop them.
    anyhow::ensure!(
        script::value(&status, "token") != Some("yes"),
        "the station already runs a bot: take it off before handing another one over"
    );
    let current =
        run(script::helper("get-config", &[]), &[]).context("read the server's station.toml")?;
    let config = with_telegram(&current, change, false)?;
    let pairing_json = serde_json::to_vec(pairing).context("encode telegram.json")?;

    let handed = (|| -> anyhow::Result<BotState> {
        run(script::helper("put-token", &[]), token.expose().as_bytes())
            .context("token credential")?;
        say("bot token: credential written");
        // Stops the station: it owns the file and would overwrite it on a /pair.
        run(script::helper("put-pairing", &[]), &pairing_json).context("telegram.json")?;
        say(&format!(
            "paired chats: {} handed over",
            pairing.authorized_chat_ids.len()
        ));
        run(script::helper("put-config", &[]), config.as_bytes())?;
        run(script::helper("start", &[]), &[])?;
        say("station started with the bot");
        wait_polling(target, !pairing.authorized_chat_ids.is_empty(), say)
    })();
    let Err(e) = handed else {
        return handed;
    };
    say("hand-over failed: taking the bot off the station");
    match take_bot_off(target, &current) {
        Ok(()) => {
            say("the station's bot is off again: token and chats dropped");
            Err(e)
        }
        Err(undo) => Err(e
            .context(format!(
                "the hand-over failed AND taking the bot off the station failed ({undo:#})"
            ))
            .context(StationMayStillPoll)),
    }
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
    say: &mut dyn FnMut(&str),
) -> anyhow::Result<BotState> {
    let deadline = Instant::now() + PAIRED_WITHIN;
    let mut last: Option<BotState> = None;
    let mut last_error = None;
    let mut stopped_looks = 0;
    loop {
        std::thread::sleep(ASK_EVERY);
        match look(target) {
            Ok(state) => {
                let polls = match with_chats {
                    true => state.paired(),
                    false => state.polling(),
                };
                if polls {
                    say(&format!(
                        "bot: {}",
                        state.status.as_deref().unwrap_or_default()
                    ));
                    return Ok(state);
                }
                stopped_looks = if state.stopped { stopped_looks + 1 } else { 0 };
                anyhow::ensure!(
                    stopped_looks < STOPPED_LOOKS,
                    "the station is not running (it stopped after the bot was handed over)"
                );
                last = Some(state);
            }
            Err(e) => last_error = Some(format!("{e:#}")),
        }
        if Instant::now() >= deadline {
            let seen = last
                .and_then(|s| s.status)
                .or(last_error)
                .unwrap_or_else(|| "nothing".into());
            anyhow::bail!(
                "the station's bot did not start polling within {} s (last: {seen})",
                PAIRED_WITHIN.as_secs()
            );
        }
    }
}

fn look(target: &Target) -> anyhow::Result<BotState> {
    // `bot_state`, not the journal alone: the answer also says whether the station holds a token.
    bot_state(target)
}

/// Undo a hand-over on a fresh connection, every step tried whatever the others did: the
/// configuration as it was, no token, no chats, and the station running again.
fn take_bot_off(target: &Target, config_before: &str) -> anyhow::Result<()> {
    let conn = admin_conn(target)?;
    let run = |command: String, stdin: &[u8]| -> anyhow::Result<()> {
        script::checked(conn.run(&command, stdin, STEP_TIMEOUT)?)?;
        Ok(())
    };
    let failed: Vec<String> = [
        run(script::helper("put-config", &[]), config_before.as_bytes()),
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
