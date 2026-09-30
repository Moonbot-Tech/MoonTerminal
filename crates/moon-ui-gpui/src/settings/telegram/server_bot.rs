//! The station on a Linux server, in the Telegram bot's segment: its install form, and once it runs
//! the bot's sections on top (the bot lives on the server: its token, access, chats with their
//! cores, the Mini App — read and changed through the station's control API) and the station's
//! own actions below.
//!
//! Unlike the rest of the Telegram tab nothing here waits for Save — every button is work on the
//! server, done at once (as the Storage tab does); the server's chats are edited in a draft of
//! their own and sent by "Apply on the server". The terminal keeps nothing secret for it: the
//! provider's login is used for the setup only, the administrator (`moon`) logs in by the
//! terminal's own key (`moon_remote::app_key`), and the core keys — every active core's — are read
//! from `servers.enc` inside the job; a Save that changes the cores sends them again
//! (`backend::station::cores_differ`).
//!
//! The job itself, its progress and the bot's hand-over belong to the backend
//! (`backend::station`): they outlive this window. This module builds jobs and shows the state.

use std::path::PathBuf;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::config::{Secret, TelegramConfig};
use moon_core::station_api::Access;
use moon_core::telegram::TelegramStatus;
use moon_core::telegram::runtime::mini_app::MiniAppStatus;
use moon_remote::setup::{FirstAccess, Setup};
use moon_remote::ssh::Target;
use moon_remote::station::BotChange;
use moon_remote::station::bot::BotState;
use moon_ui::{
    MoonButton, MoonCheckbox, MoonGroupBox, MoonInput, MoonInputState, MoonPalette, h_flex,
    rgba_from, v_flex,
};
use rust_i18n::t;
use zeroize::Zeroizing;

use super::super::SettingsView;
use super::access::{ChatEd, ChatsOf};
use crate::backend::station::job::{self, BotPlan, Job};
use crate::design;

/// Field width in unscaled pixels, as the token field of the terminal's bot.
const FIELD_W: f32 = 260.0;
/// Label column width in unscaled pixels.
const LABEL_W: f32 = 150.0;
/// The SSH port when the address names none.
const SSH_PORT: u16 = 22;

/// The station's editors.
pub(in crate::settings) struct ServerBotEd {
    host: Entity<MoonInputState>,
    login: Entity<MoonInputState>,
    password: Entity<MoonInputState>,
    key_path: Entity<MoonInputState>,
    passphrase: Entity<MoonInputState>,
    old_admin: Entity<MoonInputState>,
    bin: Entity<MoonInputState>,
    /// A token for the bot on the server: a new bot, or another one in place of it.
    server_token: Entity<MoonInputState>,
    /// First login by a key file rather than a password.
    by_key: bool,
    /// The Mini App on the server for a bot set up from here, set by the user; `None` follows the
    /// terminal's own Mini App switch.
    mini_app: Option<bool>,
    /// "Also a bot in the terminal", set by the user; `None` follows whether the terminal has one.
    local_bot: Option<bool>,
    /// The server this terminal already set up, from `remote/hosts.toml`.
    known: Option<Target>,
    /// The progress lines' scroll: kept at the newest line.
    lines_scroll: ScrollHandle,
    /// The backend's line counter at the last render: a newer one scrolls to the newest line.
    seen_line_seq: u64,
    /// The bot's state was asked for once this window: the first render of a known station does.
    bot_asked: bool,
    /// The backend's job end last seen: a newer one clears the secrets typed for it.
    seen_finished: u64,
    /// The backend's hand-over erase last seen: a newer one clears the bot's token input.
    seen_erased: u64,
    /// The server's chat editor, wired to `access_draft` by the tab's `build`.
    pub(super) chats: ChatEd,
    /// The server's chats as last read.
    access_seen: Option<Access>,
    /// The server's chats the draft was taken from: what "Apply on the server" replaces, and the
    /// station refuses if its chats are no longer these.
    pub(super) access_base: Option<Access>,
    /// The server's chats as edited here; `None` until read.
    pub(super) access_draft: Option<TelegramConfig>,
    /// The station's window around a trade, as read and as edited.
    pub(super) tape: super::server_tape::TapeEd,
}

impl ServerBotEd {
    /// The server the station runs on, when this terminal set one up.
    pub(in crate::settings) fn known(&self) -> Option<&Target> {
        self.known.as_ref()
    }
}

fn input<T: 'static>(
    window: &mut Window,
    cx: &mut Context<T>,
    masked: bool,
) -> Entity<MoonInputState> {
    let state = cx.new(|cx| MoonInputState::new(window, cx).masked(masked));
    if masked {
        state.update(cx, |st, c| st.set_masked(true, window, c));
    }
    state
}

/// Build the station's editors.
///
/// Args:
///     known: The server this terminal already set up ([`known_server`]); `None` shows the form.
pub(in crate::settings) fn build<T: 'static>(
    window: &mut Window,
    cx: &mut Context<T>,
    known: Option<Target>,
) -> ServerBotEd {
    let login = input(window, cx, false);
    login.update(cx, |st, c| st.set_value("root", window, c));
    ServerBotEd {
        host: input(window, cx, false),
        login,
        password: input(window, cx, true),
        key_path: input(window, cx, false),
        passphrase: input(window, cx, true),
        old_admin: input(window, cx, true),
        bin: input(window, cx, false),
        server_token: input(window, cx, true),
        by_key: false,
        mini_app: None,
        local_bot: None,
        known,
        lines_scroll: ScrollHandle::new(),
        seen_line_seq: 0,
        bot_asked: false,
        seen_finished: 0,
        seen_erased: 0,
        chats: ChatEd::new(window, cx),
        access_seen: None,
        access_base: None,
        access_draft: None,
        tape: Default::default(),
    }
}

/// The first server this terminal set up and closed.
pub(in crate::settings) fn known_server() -> Option<Target> {
    crate::backend::station::known_target()
}

/// `host` or `host:port`; an IPv6 address goes in brackets when it names a port.
fn parse_target(text: &str) -> Option<Target> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let (host, port) = match text.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') || host.ends_with(']') => {
            (host.trim_matches(['[', ']']), port.parse().ok()?)
        }
        _ => (text, SSH_PORT),
    };
    (!host.is_empty()).then(|| Target {
        host: host.to_owned(),
        port,
    })
}

fn text(state: &Entity<MoonInputState>, cx: &App) -> String {
    state.read(cx).value().trim().to_owned()
}

fn secret(state: &Entity<MoonInputState>, cx: &App) -> Option<Zeroizing<String>> {
    let value = state.read(cx).value().to_string();
    (!value.is_empty()).then(|| Zeroizing::new(value))
}

/// The station's bot in words.
fn bot_words(bot: &BotState) -> String {
    let status = bot.bot.as_ref().map(|b| &b.status);
    if bot.stopped {
        t!("telegram.server.bot_stopped").to_string()
    } else if bot.no_api {
        t!("telegram.server.bot_no_api").to_string()
    } else if !bot.has_token || status.is_none() {
        t!("telegram.server.bot_none").to_string()
    } else {
        match status {
            Some(TelegramStatus::Paired { chat_count }) => {
                t!("telegram.server.bot_paired", count = chat_count).to_string()
            }
            Some(TelegramStatus::Unpaired) => t!("telegram.server.bot_unpaired").to_string(),
            Some(TelegramStatus::Conflict) => t!("telegram.server.bot_conflict").to_string(),
            _ => t!("telegram.server.bot_starting").to_string(),
        }
    }
}

/// A Telegram configuration holding only `access`: what the chat editor works on.
fn draft_of(access: &Access) -> TelegramConfig {
    let mut telegram = TelegramConfig::default();
    access.apply_to(&mut telegram);
    telegram
}

impl SettingsView {
    /// After each job end: forget the secrets typed for it, empty the bot's token input after a
    /// hand-over erased the token, re-read which server is set up, and keep the newest progress
    /// line in view.
    pub(in crate::settings) fn server_bot_sync(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (finished, erased, line_seq, bot_unknown, local_token, new_access, bot_gone) = {
            let b = self.backend.read(cx);
            let draft = b.preview.as_ref().unwrap_or(&b.config);
            let access = b.station.bot.as_ref().and_then(|s| s.access.as_ref());
            (
                b.station.finished,
                b.station.erased,
                b.station.line_seq,
                b.station.bot.is_none() && b.station.bot_error.is_none(),
                !draft.telegram.token.is_empty(),
                // Cloned only when it differs from the last read.
                (access != self.telegram.server.access_seen.as_ref()).then(|| access.cloned()),
                // Known to run no bot: its chats are gone, not merely unread for a moment.
                b.station.bot.as_ref().is_some_and(|s| !s.has_token),
            )
        };
        let ed = &mut self.telegram.server;
        // The server's chats read anew. An untouched draft follows them; an edited one keeps the
        // base it was edited from, so "Apply" is refused ("discard the edits") when the server's
        // chats moved meanwhile — a chat paired since is never dropped. A read without chats (the
        // station restarting) keeps the edit; only a station known to run no bot clears it.
        if let Some(access) = new_access {
            let edited = ed.access_draft.as_ref().map(Access::of) != ed.access_base;
            match &access {
                // Untouched, or just applied (the station now holds exactly the draft).
                Some(read)
                    if !edited
                        || ed.access_draft.as_ref().map(Access::of).as_ref() == Some(read) =>
                {
                    ed.access_draft = Some(draft_of(read));
                    ed.access_base = Some(read.clone());
                }
                Some(_) => {}
                None if bot_gone => {
                    ed.access_draft = None;
                    ed.access_base = None;
                    ed.chats.close();
                }
                None => {}
            }
            ed.access_seen = access;
        }
        if line_seq != ed.seen_line_seq {
            ed.seen_line_seq = line_seq;
            ed.lines_scroll.scroll_to_bottom();
        }
        self.server_tape_sync(cx);
        let ed = &mut self.telegram.server;
        // "Also a bot in the terminal" is fixed on first sight of a station: following the draft
        // token live would hide the block while its token is being retyped.
        if ed.known.is_some() && ed.local_bot.is_none() {
            ed.local_bot = Some(local_token);
        }
        // A window opened straight on this tab never "activated" it: the first render asks.
        let ask = self.active == super::super::Tab::Telegram
            && ed.known.is_some()
            && bot_unknown
            && !ed.bot_asked;
        ed.bot_asked |= ask;
        if finished != ed.seen_finished {
            ed.seen_finished = finished;
            ed.known = known_server();
            for state in [
                &ed.password,
                &ed.passphrase,
                &ed.old_admin,
                &ed.server_token,
            ] {
                state.update(cx, |st, c| st.set_value("", window, c));
            }
        }
        // Only a hand-over empties the bot's token field: a token typed there and not saved yet
        // is the user's, whatever other job ended.
        if ask {
            self.backend.update(cx, |b, bcx| b.station_refresh_bot(bcx));
        }
        let ed = &mut self.telegram.server;
        if erased != ed.seen_erased {
            ed.seen_erased = erased;
            ed.local_bot = Some(false);
            self.telegram
                .token
                .update(cx, |st, c| st.set_value("", window, c));
        }
    }

    /// `[telegram]` for the station: the Mini App switch when given (`None` keeps the server's),
    /// the terminal's own display zone and language.
    fn server_bot_change(&self, mini_app: Option<bool>, cx: &App) -> BotChange {
        let zone =
            moon_core::util::display_time::zone_or_utc(self.backend.read(cx).header_clock_zone());
        BotChange {
            mini_app,
            zone: Some(zone.name().to_owned()),
            language: Some(rust_i18n::locale().to_string()),
        }
    }

    /// The Mini App for a bot set up on the server from here: the user's choice, or the
    /// terminal's own switch.
    fn server_mini_choice(&self, cx: &App) -> bool {
        self.telegram
            .server
            .mini_app
            .unwrap_or_else(|| self.backend.read(cx).config.telegram.mini_app_enabled)
    }

    /// The terminal's bot handed to the station: its saved token and chats. `None` when the
    /// terminal has no saved bot.
    fn server_bot_transfer(&self, cx: &App) -> Option<BotPlan> {
        let saved = &self.backend.read(cx).config.telegram;
        if saved.token.is_empty() {
            return None;
        }
        Some(BotPlan::Transfer {
            token: saved.token.clone(),
            pairing: Access::of(saved),
            change: self.server_bot_change(Some(self.server_mini_choice(cx)), cx),
        })
    }

    /// Every active core with a uid: the station gets them all.
    fn server_bot_cores(&self, cx: &App) -> Vec<u64> {
        self.backend
            .read(cx)
            .config
            .servers
            .iter()
            .filter(|s| s.active && s.uid != 0)
            .map(|s| s.uid)
            .collect()
    }

    /// Hand `job` to the backend, or show why it cannot start.
    pub(super) fn server_bot_run(&mut self, job: Result<Job, String>, cx: &mut Context<Self>) {
        match job {
            Ok(job) => {
                let hand_over = matches!(
                    job,
                    Job::Install {
                        bot: BotPlan::Transfer { .. },
                        ..
                    } | Job::Bot {
                        bot: BotPlan::Transfer { .. },
                        ..
                    }
                );
                self.backend
                    .update(cx, |b, bcx| b.station_start(job, hand_over, bcx));
            }
            Err(reason) => {
                self.backend.update(cx, |b, bcx| {
                    b.station.outcome = Some(Err(reason));
                    b.station.revision = b.station.revision.wrapping_add(1);
                    bcx.notify();
                });
            }
        }
        cx.notify();
    }

    /// How the user gets in the first time, from the form.
    fn server_bot_first_access(&self, cx: &App) -> Result<FirstAccess, String> {
        let ed = &self.telegram.server;
        let key = ed
            .by_key
            .then(|| (text(&ed.key_path, cx), secret(&ed.passphrase, cx)));
        job::first_access(text(&ed.login, cx), secret(&ed.password, cx), key)
            .map_err(|e| format!("{e:#}"))
    }

    /// "Install": prepare the server, install the station, send every active core, move the bot.
    fn server_bot_install_job(&self, cx: &App) -> Result<Job, String> {
        let ed = &self.telegram.server;
        let target = parse_target(&text(&ed.host, cx))
            .ok_or_else(|| t!("telegram.server.need_host").to_string())?;
        let bin = text(&ed.bin, cx);
        if bin.is_empty() {
            return Err(t!("telegram.server.need_bin").to_string());
        }
        // The terminal's bot always moves with the install (decided 2026-09-30).
        let bot = self.server_bot_transfer(cx).unwrap_or(BotPlan::Keep);
        Ok(Job::Install {
            setup: Setup {
                target,
                first: self.server_bot_first_access(cx)?,
                legacy_admin_password: secret(&ed.old_admin, cx),
                station_bin: Some(PathBuf::from(bin)),
            },
            cores: self.server_bot_cores(cx),
            bot,
        })
    }

    /// "Set up again" on a known server: the administrator comes in by the terminal's key, so no
    /// provider login is needed. A server that refuses that key cannot be reached from here:
    /// "Forget the server", then install it anew from the form.
    fn server_bot_resetup_job(&self, target: Target, cx: &App) -> Result<Job, String> {
        let ed = &self.telegram.server;
        let bin = text(&ed.bin, cx);
        Ok(Job::Resetup {
            setup: Setup {
                target,
                first: FirstAccess::Password {
                    user: "root".into(),
                    password: Zeroizing::new(String::new()),
                },
                legacy_admin_password: secret(&ed.old_admin, cx),
                station_bin: (!bin.is_empty()).then(|| PathBuf::from(bin)),
            },
        })
    }

    /// Drop the known server from `remote/hosts.toml`: after a reinstall it presents a new host
    /// key, which the pin would refuse. Nothing on the server changes.
    fn server_bot_forget(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.telegram.server.known.clone() else {
            return;
        };
        let path = moon_remote::hosts::Hosts::path();
        let forgotten = moon_remote::hosts::Hosts::load(&path).and_then(|mut hosts| {
            hosts.forget(&target.addr());
            hosts.save(&path)
        });
        let outcome = match forgotten {
            Ok(()) => {
                self.telegram.server.known = known_server();
                Ok(t!("telegram.server.forgotten", addr = target.addr()).to_string())
            }
            Err(e) => Err(format!("{e:#}")),
        };
        let ed = &mut self.telegram.server;
        ed.local_bot = None;
        ed.bot_asked = false;
        ed.access_seen = None;
        ed.access_base = None;
        ed.access_draft = None;
        ed.chats.close();
        self.backend.update(cx, |b, bcx| {
            b.station_forgotten(outcome);
            bcx.notify();
        });
        cx.notify();
    }

    /// Whether the terminal also runs a bot of its own next to the server's.
    pub(in crate::settings) fn server_bot_local_on(&self, cx: &App) -> bool {
        self.telegram.server.local_bot.unwrap_or_else(|| {
            let b = self.backend.read(cx);
            !b.preview
                .as_ref()
                .unwrap_or(&b.config)
                .telegram
                .token
                .is_empty()
        })
    }

    /// "Also a bot in the terminal": opens the terminal's own bot below; off empties its token in
    /// the draft, and Save stops it.
    pub(in crate::settings) fn server_bot_local_toggle(
        &self,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        MoonCheckbox::new("server-local-bot")
            .checked(self.server_bot_local_on(cx))
            .label(t!("telegram.server.local_bot").to_string())
            .description(t!("telegram.server.local_bot_hint").to_string())
            .on_change(cx.listener(|this, v: &bool, window, cx| {
                this.telegram.server.local_bot = Some(*v);
                if !*v {
                    // The token field writes the draft on Change.
                    this.telegram
                        .token
                        .update(cx, |st, c| st.set_value("", window, c));
                }
                cx.notify();
            }))
    }

    /// The bot on the server, on top of the bot's segment: how it is, its token.
    pub(in crate::settings) fn server_bot_block(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let b = self.backend.read(cx);
        let busy = b.station.busy();
        let bot = b.station.bot.clone();
        let bot_error = b.station.bot_error.clone();
        let has_local = !b.config.telegram.token.is_empty();
        let addr = self
            .telegram
            .server
            .known
            .as_ref()
            .map(Target::addr)
            .unwrap_or_default();
        let has_token = bot.as_ref().is_some_and(|b| b.has_token);
        let words = match (&bot_error, &bot) {
            (Some(error), _) => {
                t!("telegram.server.bot_unreadable", error = error.clone()).to_string()
            }
            (None, Some(bot)) => bot_words(bot),
            (None, None) => t!("telegram.server.bot_unknown").to_string(),
        };
        let words_color = match bot_error {
            Some(_) => rgba_from(p.red_text, 1.0),
            None => rgba_from(p.text, 1.0),
        };
        // Known to have no bot: only then is moving the terminal's bot offered.
        let server_without_bot = bot.as_ref().is_some_and(|b| !b.has_token);
        let button = |id: &'static str, label: String| {
            MoonButton::new(id)
                .padding_x(12.0)
                .label(label)
                .disabled(busy)
        };
        MoonGroupBox::new("telegram-server-bot")
            .title(t!("telegram.server.bot_title", addr = addr).to_string())
            .padding(14.0)
            .gap(10.0)
            .child(div().text_color(words_color).child(words))
            .child(self.server_bot_field(
                "server-token",
                t!("telegram.token").to_string(),
                &self.telegram.server.server_token,
                true,
                cx,
            ))
            .child(self.server_bot_hint("telegram.server.token_hint", cx))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 8.0))
                    .child(
                        button(
                            "server-token-set",
                            match has_token {
                                true => t!("telegram.server.token_replace").to_string(),
                                false => t!("telegram.server.token_set").to_string(),
                            },
                        )
                        .primary()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let token = text(&this.telegram.server.server_token, cx);
                            // One token, one poller: the terminal's own bot keeps its token.
                            let local = {
                                let b = this.backend.read(cx);
                                [Some(&b.config), b.preview.as_ref()]
                                    .into_iter()
                                    .flatten()
                                    .any(|c| c.telegram.token.expose() == token.as_str())
                            };
                            // A replaced token keeps the server's Mini App; a first one takes
                            // the choice made here.
                            let mini_app = (!has_token).then(|| this.server_mini_choice(cx));
                            let job = match (known_server(), token.is_empty()) {
                                (_, true) => {
                                    Err(t!("telegram.server.need_server_token").to_string())
                                }
                                (None, _) => return,
                                (Some(_), false) if local => {
                                    Err(t!("telegram.server.token_is_local").to_string())
                                }
                                (Some(target), false) => Ok(Job::ServerToken {
                                    target,
                                    token: Secret::new(token),
                                    change: this.server_bot_change(mini_app, cx),
                                }),
                            };
                            this.server_bot_run(job, cx);
                        }))
                        .render(),
                    )
                    .when(has_local && server_without_bot, |row| {
                        row.child(
                            button(
                                "server-move-bot",
                                t!("telegram.server.move_bot").to_string(),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                let job = match (known_server(), this.server_bot_transfer(cx)) {
                                    (Some(target), Some(bot)) => Ok(Job::Bot { target, bot }),
                                    _ => return,
                                };
                                this.server_bot_run(job, cx);
                            }))
                            .render(),
                        )
                    })
                    .when(has_token, |row| {
                        row.child(
                            button("server-bot-off", t!("telegram.server.bot_off").to_string())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(target) = known_server() {
                                        this.server_bot_run(Ok(Job::BotOff { target }), cx);
                                    }
                                }))
                                .render(),
                        )
                    })
                    .child(
                        button(
                            "server-bot-refresh",
                            t!("telegram.server.refresh").to_string(),
                        )
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.backend.update(cx, |b, bcx| b.station_refresh_bot(bcx));
                        }))
                        .render(),
                    ),
            )
            .when(!has_token, |s| {
                s.child(self.server_bot_mini_choice("server-mini-new", cx))
            })
    }

    /// The Mini App switch for a bot set up on the server from here (the install, a first token).
    fn server_bot_mini_choice(&self, id: &'static str, cx: &Context<Self>) -> MoonCheckbox {
        MoonCheckbox::new(id)
            .checked(self.server_mini_choice(cx))
            .label(t!("telegram.server.mini_app").to_string())
            .description(t!("telegram.server.mini_app_hint").to_string())
            .on_change(cx.listener(|this, v: &bool, _, cx| {
                this.telegram.server.mini_app = Some(*v);
                cx.notify();
            }))
    }

    /// The running bot's own sections, as the terminal's bot has them: access with its pairing
    /// code, the chats with their cores, the Mini App. None until the station has told its chats.
    pub(in crate::settings) fn server_bot_sections(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let b = self.backend.read(cx);
        let Some(state) = b.station.bot.as_ref().filter(|s| s.access.is_some()) else {
            return Vec::new();
        };
        let Some(bot) = state.bot.as_ref() else {
            return Vec::new();
        };
        let p = MoonPalette::active(cx);
        let muted = rgba_from(p.text_muted, 1.0);
        let busy = b.station.busy();
        let ed = &self.telegram.server;
        let seen = state.access.clone().unwrap_or_default();
        let edited = ed.access_draft.as_ref().map(Access::of) != ed.access_base;
        let paired = match seen.authorized_chat_ids.len() {
            0 => t!("telegram.paired_none").to_string(),
            count => t!("telegram.paired_count", count = count).to_string(),
        };
        let code = state.pairing_code().map(str::to_owned);

        let access =
            MoonGroupBox::new("telegram-server-access")
                .title(t!("telegram.server.access_title").to_string())
                .padding(14.0)
                .gap(10.0)
                .child(div().text_color(muted).child(paired))
                .child(
                    div()
                        .text_color(muted)
                        .child(t!("telegram.access_hint").to_string()),
                )
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap(design::ui_px(cx, 8.0))
                        .child(
                            MoonButton::new("server-pair")
                                .primary()
                                .padding_x(12.0)
                                .label(t!("telegram.pair_new").to_string())
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(target) = known_server() {
                                        this.server_bot_run(Ok(Job::PairIssue { target }), cx);
                                    }
                                }))
                                .render(),
                        )
                        .child(
                            MoonButton::new("server-pair-reset")
                                .ghost()
                                .padding_x(12.0)
                                .label(t!("telegram.pair_reset").to_string())
                                .disabled(busy || seen.authorized_chat_ids.is_empty())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    // All of them, as last read: nothing to keep from a draft.
                                    let base = this.telegram.server.access_seen.clone();
                                    if let (Some(target), Some(base)) = (known_server(), base) {
                                        this.server_bot_run(
                                            Ok(Job::Access {
                                                target,
                                                base,
                                                access: Access::default(),
                                            }),
                                            cx,
                                        );
                                    }
                                }))
                                .render(),
                        ),
                )
                .when_some(code, |s, code| {
                    s.child(
                        v_flex()
                            .gap(design::ui_px(cx, 8.0))
                            .child(div().font_family(design::mono()).child(
                                t!("telegram.server.bot_pair", code = code.clone()).to_string(),
                            ))
                            .child(
                                MoonButton::new("server-copy-pair")
                                    .label(t!("telegram.pair_copy").to_string())
                                    .padding_x(12.0)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        // Judged at the click: nothing repaints when the code expires.
                                        let code = this
                                            .backend
                                            .read(cx)
                                            .station
                                            .bot
                                            .as_ref()
                                            .and_then(|s| s.pairing_code().map(str::to_owned));
                                        if let Some(code) = code {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                format!("/pair {code}"),
                                            ));
                                            this.status = Some((
                                                super::super::StatusMsg::Key("settings.copied"),
                                                false,
                                            ));
                                        }
                                        cx.notify();
                                    }))
                                    .render(),
                            ),
                    )
                });

        let apply_row = v_flex()
            .gap(design::ui_px(cx, 8.0))
            .child(
                div()
                    .text_color(muted)
                    .child(t!("telegram.server.access_apply_hint").to_string()),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 8.0))
                    .child(
                        MoonButton::new("server-access-apply")
                            .primary()
                            .padding_x(12.0)
                            .label(t!("telegram.server.access_apply").to_string())
                            .disabled(busy || !edited)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let ed = &this.telegram.server;
                                let edit = ed.access_base.clone().zip(ed.access_draft.as_ref());
                                if let (Some(target), Some((base, draft))) = (known_server(), edit)
                                {
                                    let access = Access::of(draft);
                                    this.server_bot_run(
                                        Ok(Job::Access {
                                            target,
                                            base,
                                            access,
                                        }),
                                        cx,
                                    );
                                }
                            }))
                            .render(),
                    )
                    .child(
                        MoonButton::new("server-access-revert")
                            .ghost()
                            .padding_x(12.0)
                            .label(t!("telegram.server.access_revert").to_string())
                            .disabled(!edited)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let ed = &mut this.telegram.server;
                                ed.access_draft = ed.access_seen.as_ref().map(draft_of);
                                ed.access_base = ed.access_seen.clone();
                                ed.chats.close();
                                cx.notify();
                            }))
                            .render(),
                    ),
            )
            .into_any_element();

        let mini_words = match &bot.mini_app {
            _ if !bot.mini_app_on => t!("telegram.mini_disabled"),
            MiniAppStatus::Tunneling { .. } => t!("telegram.mini_ready"),
            MiniAppStatus::Starting | MiniAppStatus::Listening { .. } => {
                t!("telegram.mini_starting")
            }
            MiniAppStatus::Failed { .. } => t!("telegram.mini_failed"),
            MiniAppStatus::Stopped => t!("telegram.stopped"),
        }
        .to_string();
        let mini_url = match &bot.mini_app {
            MiniAppStatus::Tunneling { url, .. } if bot.mini_app_on => Some(url.clone()),
            _ => None,
        };
        let mini = MoonGroupBox::new("telegram-server-mini")
            .title(t!("telegram.section_mini_app").to_string())
            .padding(14.0)
            .gap(10.0)
            .child(
                MoonCheckbox::new("server-mini")
                    .checked(bot.mini_app_on)
                    .disabled(busy)
                    .label(t!("telegram.server.mini_app").to_string())
                    .description(t!("telegram.server.mini_app_hint").to_string())
                    .on_change(cx.listener(|this, v: &bool, _, cx| {
                        if let Some(target) = known_server() {
                            this.server_bot_run(Ok(Job::MiniApp { target, on: *v }), cx);
                        }
                    })),
            )
            .child(div().text_color(rgba_from(p.text, 1.0)).child(mini_words))
            .when_some(mini_url, |s, url| {
                s.child(
                    MoonButton::new("server-copy-url")
                        .label(t!("telegram.mini_copy_url").to_string())
                        .ghost()
                        .padding_x(12.0)
                        .tooltip(url.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                            this.status =
                                Some((super::super::StatusMsg::Key("settings.copied"), false));
                            cx.notify();
                        }))
                        .render(),
                )
            });

        vec![
            access.into_any_element(),
            self.telegram_chat_access(ChatsOf::Station, apply_row, cx)
                .into_any_element(),
            mini.into_any_element(),
        ]
    }

    /// The station section: the install form, or the station's own actions, and the last job.
    pub(in crate::settings) fn server_bot_section(&self, cx: &Context<Self>) -> impl IntoElement {
        let st = &self.backend.read(cx).station;
        let show_progress = st.outcome.is_some() || !st.lines.is_empty() || st.busy();
        let section = MoonGroupBox::new("telegram-station-section")
            .title(t!("telegram.server.section").to_string())
            .padding(14.0)
            .gap(10.0)
            .child(self.server_bot_hint("telegram.server.intro", cx));
        let section = match self.telegram.server.known.clone() {
            Some(target) => self.server_bot_known(section, &target, cx),
            None => self.server_bot_new(section, cx),
        };
        section.when(show_progress, |s| s.child(self.server_bot_progress(cx)))
    }

    fn server_bot_field(
        &self,
        id: &'static str,
        label: String,
        state: &Entity<MoonInputState>,
        masked: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let field = MoonInput::new(id).state(state).size(design::INPUT_SIZE);
        let field = if masked { field.mask_toggle() } else { field };
        h_flex()
            .flex_wrap()
            .gap(design::ui_px(cx, 10.0))
            .items_center()
            .child(
                div()
                    .w(design::font_w_px(cx, LABEL_W))
                    .text_color(rgba_from(p.text_soft, 1.0))
                    .child(label),
            )
            .child(div().w(design::font_w_px(cx, FIELD_W)).child(field))
    }

    fn server_bot_hint(&self, key: &str, cx: &Context<Self>) -> impl IntoElement {
        div()
            .text_color(rgba_from(MoonPalette::active(cx).text_muted, 1.0))
            .child(t!(key).to_string())
    }

    /// The old administrator password, asked only after a server set up by an older version said
    /// it still has one.
    fn server_bot_old_admin(&self, section: MoonGroupBox, cx: &Context<Self>) -> MoonGroupBox {
        let needs = self.backend.read(cx).station.needs_old_admin;
        section.when(needs, |s| {
            s.child(self.server_bot_field(
                "server-old-admin",
                t!("telegram.server.old_admin").to_string(),
                &self.telegram.server.old_admin,
                true,
                cx,
            ))
        })
    }

    /// A server not set up yet: what the provider gave, the service file, the bot, "Install".
    fn server_bot_new(&self, section: MoonGroupBox, cx: &Context<Self>) -> MoonGroupBox {
        let ed = &self.telegram.server;
        let busy = self.backend.read(cx).station.busy();
        let has_bot = !self.backend.read(cx).config.telegram.token.is_empty();
        let section = section
            .child(self.server_bot_field(
                "server-host",
                t!("telegram.server.host").to_string(),
                &ed.host,
                false,
                cx,
            ))
            .child(self.server_bot_field(
                "server-login",
                t!("telegram.server.login").to_string(),
                &ed.login,
                false,
                cx,
            ))
            .child(
                MoonCheckbox::new("server-by-key")
                    .checked(ed.by_key)
                    .label(t!("telegram.server.by_key").to_string())
                    .on_change(cx.listener(|this, v: &bool, _, cx| {
                        this.telegram.server.by_key = *v;
                        cx.notify();
                    })),
            )
            .when(ed.by_key, |s| {
                s.child(self.server_bot_field(
                    "server-key",
                    t!("telegram.server.key_path").to_string(),
                    &ed.key_path,
                    false,
                    cx,
                ))
                .child(self.server_bot_field(
                    "server-passphrase",
                    t!("telegram.server.passphrase").to_string(),
                    &ed.passphrase,
                    true,
                    cx,
                ))
                .child(self.server_bot_hint("telegram.server.key_hint", cx))
            })
            .child(self.server_bot_field(
                "server-password",
                if ed.by_key {
                    t!("telegram.server.sudo_password").to_string()
                } else {
                    t!("telegram.server.password").to_string()
                },
                &ed.password,
                true,
                cx,
            ))
            .child(self.server_bot_hint("telegram.server.first_login_hint", cx));
        self.server_bot_old_admin(section, cx)
            .child(self.server_bot_field(
                "server-bin",
                t!("telegram.server.bin").to_string(),
                &ed.bin,
                false,
                cx,
            ))
            .child(self.server_bot_hint("telegram.server.bin_hint", cx))
            .when(has_bot, |s| {
                s.child(self.server_bot_hint("telegram.server.transfer_hint", cx))
            })
            .child(self.server_bot_mini_choice("server-mini-install", cx))
            .child(self.server_bot_hint("telegram.server.cores_hint", cx))
            .child(
                h_flex().child(
                    MoonButton::new("server-install")
                        .primary()
                        .padding_x(12.0)
                        .label(t!("telegram.server.install").to_string())
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            let job = this.server_bot_install_job(cx);
                            this.server_bot_run(job, cx);
                        }))
                        .render(),
                ),
            )
    }

    /// A server this terminal set up: the station's own actions (its bot is on top of the
    /// segment).
    fn server_bot_known(
        &self,
        section: MoonGroupBox,
        target: &Target,
        cx: &Context<Self>,
    ) -> MoonGroupBox {
        let p = MoonPalette::active(cx);
        let busy = self.backend.read(cx).station.busy();
        let button = |id: &'static str, label: String| {
            MoonButton::new(id)
                .padding_x(12.0)
                .label(label)
                .disabled(busy)
        };
        let on = |make: fn(&Self, Target, &App) -> Result<Job, String>| {
            let target = target.clone();
            cx.listener(move |this: &mut Self, _: &ClickEvent, _, cx| {
                let job = make(this, target.clone(), cx);
                this.server_bot_run(job, cx);
            })
        };
        let actions = h_flex()
            .flex_wrap()
            .gap(design::ui_px(cx, 8.0))
            .child(
                button("server-status", t!("telegram.server.status").to_string())
                    .on_click(on(|_, target, _| Ok(Job::Status { target })))
                    .render(),
            )
            .child(
                button("server-update", t!("telegram.server.update").to_string())
                    .on_click(on(|this, target, cx| {
                        let bin = text(&this.telegram.server.bin, cx);
                        if bin.is_empty() {
                            return Err(t!("telegram.server.need_bin").to_string());
                        }
                        Ok(Job::Update {
                            target,
                            bin: PathBuf::from(bin),
                        })
                    }))
                    .render(),
            )
            .child(
                button("server-resetup", t!("telegram.server.resetup").to_string())
                    .ghost()
                    .tooltip(t!("telegram.server.resetup_hint").to_string())
                    .on_click(on(|this, target, cx| {
                        this.server_bot_resetup_job(target, cx)
                    }))
                    .render(),
            )
            .child(
                button("server-logs", t!("telegram.server.logs").to_string())
                    .ghost()
                    .on_click(on(|_, target, _| Ok(Job::Logs { target })))
                    .render(),
            )
            .child(
                button("server-forget", t!("telegram.server.forget").to_string())
                    .disabled(self.backend.read(cx).station.running)
                    .ghost()
                    .tooltip(t!("telegram.server.forget_hint").to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.server_bot_forget(cx)))
                    .render(),
            );
        let section = section.child(
            div()
                .text_color(rgba_from(p.text, 1.0))
                .child(t!("telegram.server.known", addr = target.addr()).to_string()),
        );
        self.server_bot_old_admin(section, cx)
            .child(self.server_bot_field(
                "server-bin",
                t!("telegram.server.bin").to_string(),
                &self.telegram.server.bin,
                false,
                cx,
            ))
            .child(actions)
            .child(self.server_tape_block(target, cx))
    }

    fn server_bot_progress(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let st = &self.backend.read(cx).station;
        let outcome = st.outcome.as_ref().map(|o| match o {
            Ok(text) => (text.clone(), rgba_from(p.text, 1.0)),
            Err(text) => (text.clone(), rgba_from(p.red_text, 1.0)),
        });
        let mut lines = v_flex()
            .id("server-bot-lines")
            .max_h(design::ui_px(cx, 260.0))
            .overflow_y_scroll()
            .track_scroll(&self.telegram.server.lines_scroll)
            .font_family(design::mono());
        for line in &st.lines {
            lines = lines.child(div().child(line.clone()));
        }
        v_flex()
            .gap(design::ui_px(cx, 8.0))
            .when(st.busy(), |s| {
                s.child(div().child(t!("telegram.server.running").to_string()))
            })
            .when_some(outcome, |s, (text, color)| {
                s.child(div().text_color(color).child(text))
            })
            .child(lines)
    }
}

#[cfg(test)]
mod tests;
