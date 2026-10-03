//! The station on a Linux server. The Station tab shows its install form, and once it runs the
//! station's own actions; the Telegram bot's segment shows the bot's sections (the bot lives on the
//! server: its token, access, chats with their cores, the Mini App — read and changed through the
//! station's control API).
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

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::config::{Secret, TelegramConfig};
use moon_core::station_api::Access;
use moon_core::telegram::TelegramStatus;
use moon_core::telegram::runtime::mini_app::MiniAppStatus;
use moon_core::update::ReleaseVersion;
use moon_remote::setup::{FirstAccess, Setup, StationBinary};
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

mod progress;

/// Field width in unscaled pixels, as the token field of the terminal's bot.
const FIELD_W: f32 = 260.0;
/// Label column width in unscaled pixels.
const LABEL_W: f32 = 150.0;
/// The SSH port when the address names none.
const SSH_PORT: u16 = 22;

/// The station's editors.
pub(in crate::settings) struct ServerBotEd {
    /// Inline address and destructive-action confirmations, independent of the bot chat draft.
    pub(super) station_access: super::station_access::AccessEd,
    host: Entity<MoonInputState>,
    login: Entity<MoonInputState>,
    password: Entity<MoonInputState>,
    key_path: Entity<MoonInputState>,
    passphrase: Entity<MoonInputState>,
    old_admin: Entity<MoonInputState>,
    /// A token for the bot on the server: a new bot, or another one in place of it.
    server_token: Entity<MoonInputState>,
    /// First login by a key file rather than a password.
    by_key: bool,
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
    seen_restored: u64,
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
    /// The station bot's menu tree, over `access_draft`.
    pub(super) menu: super::bot_menu::BotMenuEd,
}

impl ServerBotEd {
    /// The server's chats, bot settings and chats' notifications as last read.
    pub(super) fn access_seen(&self) -> Option<&Access> {
        self.access_seen.as_ref()
    }

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
        station_access: super::station_access::AccessEd::new(window, cx),
        host: input(window, cx, false),
        login,
        password: input(window, cx, true),
        key_path: input(window, cx, false),
        passphrase: input(window, cx, true),
        old_admin: input(window, cx, true),
        server_token: input(window, cx, true),
        by_key: false,
        known,
        lines_scroll: ScrollHandle::new(),
        seen_line_seq: 0,
        bot_asked: false,
        seen_finished: 0,
        seen_erased: 0,
        seen_restored: 0,
        chats: ChatEd::new(window, cx),
        access_seen: None,
        access_base: None,
        access_draft: None,
        tape: Default::default(),
        menu: super::bot_menu::BotMenuEd::new(cx),
    }
}

/// The first server this terminal set up and closed.
pub(in crate::settings) fn known_server() -> Option<Target> {
    crate::backend::station::known_target()
}

/// `host` or `host:port`; an IPv6 address goes in brackets when it names a port.
pub(super) fn parse_target(text: &str) -> Option<Target> {
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

/// Where the station's service stands against the newest release this terminal knows of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ServiceVersion {
    /// A newer release exists: the one "Update" takes it to (at least this).
    Behind(ReleaseVersion),
    /// The service runs the newest known release (or a newer one).
    Current(ReleaseVersion),
    /// Not known: the service's version is unread or a development build, or no release scan
    /// finished and the terminal's own release does not settle it.
    Unknown,
}

/// The service's version as it reports it (`v0.51.0 (<rev>)`) against the newest known release.
/// The terminal's own release is a floor for the newest: a service older than this terminal is
/// behind even before a release scan finished.
pub(super) fn service_version(
    service: Option<&str>,
    latest: Option<ReleaseVersion>,
    terminal: Option<ReleaseVersion>,
) -> ServiceVersion {
    let Some(service) = service.and_then(|v| ReleaseVersion::parse(v.split_whitespace().next()?))
    else {
        return ServiceVersion::Unknown;
    };
    match (latest.max(terminal), latest) {
        (Some(newest), _) if service < newest => ServiceVersion::Behind(newest),
        (_, Some(latest)) => ServiceVersion::Current(latest.max(service)),
        _ => ServiceVersion::Unknown,
    }
}

/// A Telegram configuration holding only `access`: what the chat and menu editors work on.
fn draft_of(access: &Access) -> TelegramConfig {
    let mut telegram = TelegramConfig::default();
    access.apply_to(&mut telegram);
    telegram
}

/// The change `draft` asks of a station whose access read `base`: the chats, and the bot's
/// settings only when the station knows them. Never the zone — it is pushed on its own, and an
/// edit begun before a push must not take the station's zone back.
pub(super) fn draft_access(draft: &TelegramConfig, base: &Access) -> Access {
    let mut access = Access::of(draft);
    if base.bot.is_none() {
        access.bot = None;
    }
    access
}

/// Whether the station's draft differs from what it was taken from — the zone and the chats'
/// notifications aside: those are saved on their own.
pub(super) fn access_edited(draft: Option<&TelegramConfig>, base: Option<&Access>) -> bool {
    match (draft, base) {
        (Some(draft), Some(base)) => {
            draft_access(draft, base)
                != Access {
                    zone: None,
                    notify: None,
                    ..base.clone()
                }
        }
        (None, None) => false,
        _ => true,
    }
}

impl SettingsView {
    /// After each job end: forget submitted secrets, refresh the token input after hand-over
    /// erasure or return, re-read which server is set up, and keep the newest progress
    /// line in view.
    pub(in crate::settings) fn server_bot_sync(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (finished, erased, line_seq, bot_unknown, new_access, bot_gone) = {
            let b = self.backend.read(cx);
            let access = b.station.bot.as_ref().and_then(|s| s.access.as_ref());
            (
                b.station.finished,
                b.station.erased,
                b.station.line_seq,
                b.station.bot.is_none() && b.station.bot_error.is_none(),
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
            let edited = access_edited(ed.access_draft.as_ref(), ed.access_base.as_ref());
            match &access {
                // Untouched, or just applied (the station now holds exactly the draft).
                Some(read) if !edited || !access_edited(ed.access_draft.as_ref(), Some(read)) => {
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
        // A window opened straight on this tab never "activated" it: the first render asks.
        let ask = self.active == super::super::Tab::Telegram
            && ed.known.is_some()
            && bot_unknown
            && !ed.bot_asked;
        ed.bot_asked |= ask;
        if finished != ed.seen_finished {
            ed.seen_finished = finished;
            let known = known_server();
            if ed.known.as_ref().map(Target::addr) != known.as_ref().map(Target::addr) {
                ed.station_access.clear();
                ed.bot_asked = false;
                ed.access_seen = None;
                ed.access_base = None;
                ed.access_draft = None;
                ed.chats.close();
            }
            ed.known = known;
            if ed.known.is_none() {
                ed.station_access.clear();
            }
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
            self.telegram
                .token
                .update(cx, |st, c| st.set_value("", window, c));
        }
        let restored = self.backend.read(cx).station.restored;
        if restored != self.telegram.server.seen_restored {
            self.telegram.server.seen_restored = restored;
            let token = self.backend.read(cx).config.telegram.token.clone();
            self.telegram
                .token
                .update(cx, |st, c| st.set_value(token.expose(), window, c));
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

    /// The Mini App for a bot set up on the server from here: the terminal's own switch. Once the
    /// bot runs there, its own Mini App section changes it.
    fn server_mini_choice(&self, cx: &App) -> bool {
        self.backend.read(cx).config.telegram.mini_app_enabled
    }

    /// The terminal's bot handed to the station: its saved token and chats. `None` when the
    /// terminal has no saved bot.
    fn server_bot_transfer(&self, cx: &App) -> Option<BotPlan> {
        let saved = &self.backend.read(cx).config.telegram;
        if saved.token.is_empty() {
            return None;
        }
        let change = self.server_bot_change(Some(self.server_mini_choice(cx)), cx);
        Some(BotPlan::Transfer {
            token: saved.token.clone(),
            // The bot's menu goes with its chats, and its zone is the header clock's.
            pairing: Box::new(Access {
                zone: change.zone.clone(),
                ..Access::of(saved)
            }),
            change,
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
            .map_err(|e| crate::backend::station::text::error(&e))
    }

    /// "Install": prepare the server, install the station, send every active core, move the bot.
    /// `host_key` is the fingerprint the user confirmed after the probe.
    fn server_bot_install_job(&self, host_key: String, cx: &App) -> Result<Job, String> {
        let ed = &self.telegram.server;
        let target = parse_target(&text(&ed.host, cx))
            .ok_or_else(|| t!("telegram.server.need_host").to_string())?;
        // The terminal's bot always moves with the install (decided 2026-09-30).
        let bot = self.server_bot_transfer(cx).unwrap_or(BotPlan::Keep);
        Ok(Job::Install {
            setup: Setup {
                target,
                first: self.server_bot_first_access(cx)?,
                legacy_admin_password: secret(&ed.old_admin, cx),
                // Only from the release (STATION.md §1 п. 21): no file to choose.
                station: StationBinary::Release,
                host_key: Some(host_key),
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
        Ok(Job::Resetup {
            setup: Setup {
                target,
                first: FirstAccess::Password {
                    user: "root".into(),
                    password: Zeroizing::new(String::new()),
                },
                legacy_admin_password: secret(&ed.old_admin, cx),
                // The installed station stays; "Update the service" moves it to the release.
                station: StationBinary::Keep,
                host_key: None,
            },
        })
    }

    /// Forget only the exact host explicitly confirmed inline. Refuse a changed record or an
    /// ongoing job or unresolved bot recovery, retaining access and cached state on a failed save.
    pub(super) fn server_bot_forget(
        &mut self,
        source: moon_remote::hosts::Host,
        cx: &mut Context<Self>,
    ) {
        if self.backend.read(cx).station.running {
            return;
        }
        if let Some(reason) = self.backend.read(cx).station.access_refusal() {
            self.server_bot_run(Err(reason), cx);
            return;
        }
        let path = moon_remote::hosts::Hosts::path();
        let forgotten = moon_remote::hosts::Hosts::load(&path).and_then(|mut hosts| {
            if hosts.get(&source.addr) != Some(&source) {
                return Err(moon_remote::hosts::HostEditError::Changed.into());
            }
            hosts.forget(&source.addr);
            hosts.save(&path)
        });
        let outcome = match forgotten {
            Ok(()) => {
                self.telegram.server.known = known_server();
                Ok(t!("telegram.server.forgotten", addr = source.addr).to_string())
            }
            Err(e) => {
                self.server_bot_run(Err(crate::backend::station::text::error(&e)), cx);
                return;
            }
        };
        let ed = &mut self.telegram.server;
        ed.bot_asked = false;
        ed.access_seen = None;
        ed.access_base = None;
        ed.access_draft = None;
        ed.chats.close();
        ed.station_access.clear();
        self.backend.update(cx, |b, bcx| {
            b.station_forgotten(outcome);
            bcx.notify();
        });
        cx.notify();
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
                                (None, _) => Err(t!("telegram.server.no_known_server").to_string()),
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
                    .when(has_token || b.station.has_returned_bot(), |row| {
                        row.child(
                            button("server-bot-off", t!("telegram.server.bot_off").to_string())
                                .tooltip(t!("telegram.server.bot_off_hint").to_string())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(target) = known_server() {
                                        this.server_bot_run(
                                            Ok(Job::BotOff {
                                                target,
                                                restore: true,
                                                recovered: None,
                                            }),
                                            cx,
                                        );
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
    }

    /// Render one Chats box with pairing and role actions, then the station's Mini App.
    /// Sections appear once the station has reported its bot and access state.
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
        let seen = state.access.clone().unwrap_or_default();
        let paired = match seen.authorized_chat_ids.len() {
            0 => t!("telegram.paired_none").to_string(),
            count => t!("telegram.paired_count", count = count).to_string(),
        };
        let code = state.pairing_code().map(str::to_owned);

        let pairing =
            v_flex()
                .gap(design::ui_px(cx, 8.0))
                .child(div().text_color(muted).child(paired))
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
                                                edits: false,
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

        let apply_row = self.server_access_actions("server-access", cx);

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
            self.telegram_chat_access(
                ChatsOf::Station,
                pairing.into_any_element(),
                Some(apply_row),
                cx,
            )
            .into_any_element(),
            self.bot_menu_box(ChatsOf::Station, cx),
            mini.into_any_element(),
        ]
    }

    /// "Apply on the server" and "Revert" for the station's draft — chats and bot menu alike;
    /// shown under each box that edits it, `id` keeping each pair's elements apart.
    pub(super) fn server_access_actions(&self, id: &str, cx: &Context<Self>) -> AnyElement {
        let station = &self.backend.read(cx).station;
        let busy = station.busy();
        let p = MoonPalette::active(cx);
        // How the last "Apply on the server" came out, here beside its buttons.
        let outcome = match (&station.applied, station.applying) {
            (_, true) => Some((t!("telegram.server.applying").to_string(), p.text_muted)),
            (Some(Ok(text)), false) => Some((text.clone(), p.text_muted)),
            (Some(Err(reason)), false) => Some((reason.clone(), p.red_text)),
            (None, false) => None,
        };
        let ed = &self.telegram.server;
        let edited = access_edited(ed.access_draft.as_ref(), ed.access_base.as_ref());
        v_flex()
            .gap(design::ui_px(cx, 8.0))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 8.0))
                    .child(
                        MoonButton::new(SharedString::from(format!("{id}-apply")))
                            .primary()
                            .padding_x(12.0)
                            .label(t!("telegram.server.access_apply").to_string())
                            .disabled(busy || !edited)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let ed = &this.telegram.server;
                                let edit = ed.access_base.clone().zip(ed.access_draft.as_ref());
                                if let (Some(target), Some((base, draft))) = (known_server(), edit)
                                {
                                    let access = draft_access(draft, &base);
                                    this.server_bot_run(
                                        Ok(Job::Access {
                                            target,
                                            base,
                                            access,
                                            edits: true,
                                        }),
                                        cx,
                                    );
                                }
                            }))
                            .render(),
                    )
                    .child(
                        MoonButton::new(SharedString::from(format!("{id}-revert")))
                            .ghost()
                            .padding_x(12.0)
                            .label(t!("telegram.server.access_revert").to_string())
                            .disabled(!edited)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let ed = &mut this.telegram.server;
                                ed.access_draft = ed.access_seen.as_ref().map(draft_of);
                                ed.access_base = ed.access_seen.clone();
                                ed.chats.close();
                                this.backend.update(cx, |b, bcx| {
                                    b.station.applied = None;
                                    b.station.revision = b.station.revision.wrapping_add(1);
                                    bcx.notify();
                                });
                                cx.notify();
                            }))
                            .render(),
                    ),
            )
            .when_some(outcome, |s, (text, color)| {
                s.child(div().text_color(rgba_from(color, 1.0)).child(text))
            })
            .into_any_element()
    }

    /// The station section: the install form, or the station's own actions, and the last job.
    pub(in crate::settings) fn server_bot_section(&self, cx: &Context<Self>) -> impl IntoElement {
        let st = &self.backend.read(cx).station;
        let show_progress =
            st.outcome.is_some() || st.status.is_some() || !st.lines.is_empty() || st.busy();
        let section = MoonGroupBox::new("telegram-station-section")
            .title(t!("telegram.server.section").to_string())
            .padding(14.0)
            .gap(10.0)
            .child(self.server_bot_hint("telegram.server.intro", cx));
        let section = match self.telegram.server.known.clone() {
            Some(target) => self.server_bot_known(section, &target, cx),
            None => self.server_bot_new(section, cx),
        };
        section.when(show_progress, |s| {
            s.child(self.server_bot_progress(true, cx))
        })
    }

    /// A labelled, manually editable provider credential field.
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

    /// Keep manual key-path entry beside a native single-file picker, wrapping at narrow widths.
    fn server_bot_key_field(&self, cx: &Context<Self>) -> impl IntoElement {
        let busy = self.backend.read(cx).station.busy();
        h_flex()
            .flex_wrap()
            .items_center()
            .gap(design::ui_px(cx, 10.0))
            .child(self.server_bot_field(
                "server-key",
                t!("telegram.server.key_path").to_string(),
                &self.telegram.server.key_path,
                false,
                cx,
            ))
            .child(
                MoonButton::new("server-key-browse")
                    .size(design::CONTROL_TIER)
                    .label(t!("telegram.server.key_browse").to_string())
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.server_bot_browse_key(window, cx);
                    }))
                    .render(),
            )
    }

    /// Apply a selected file through a weak view handle; cancellation and a busy form keep edits.
    fn server_bot_browse_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.backend.read(cx).station.busy() {
            return;
        }
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("telegram.server.key_path").to_string().into()),
        });
        cx.spawn_in(window, async move |view, cx| match selection.await {
            Ok(Ok(Some(paths))) => {
                if let Some(path) = paths.into_iter().next() {
                    let _ = view.update_in(cx, |this, window, cx| {
                        if !this.backend.read(cx).station.busy() {
                            this.telegram.server.key_path.update(cx, |state, cx| {
                                state.set_value(path.to_string_lossy().into_owned(), window, cx);
                            });
                            cx.notify();
                        }
                    });
                }
            }
            Ok(Err(error)) => log::warn!("key file dialog failed: {error}"),
            _ => {}
        })
        .detach();
    }

    fn server_bot_hint(&self, key: &str, cx: &Context<Self>) -> impl IntoElement {
        self.server_bot_note(t!(key).to_string(), cx)
    }

    /// Already localized text in the hint's style.
    fn server_bot_note(&self, text: String, cx: &Context<Self>) -> impl IntoElement {
        div()
            .text_color(rgba_from(MoonPalette::active(cx).text_muted, 1.0))
            .child(text)
    }

    /// The probed key of the server in the form, with what the install changes there: the
    /// install starts only from here, with exactly the fingerprint shown.
    fn server_bot_install_review(&self, section: MoonGroupBox, cx: &Context<Self>) -> MoonGroupBox {
        let ed = &self.telegram.server;
        let host = parse_target(&text(&ed.host, cx)).map(|t| t.addr());
        let station = &self.backend.read(cx).station;
        let busy = station.busy();
        let Some(probe) = station
            .install_probe
            .clone()
            .filter(|key| Some(key.target.addr()) == host)
        else {
            return section;
        };
        let fingerprint = probe.fingerprint.clone();
        let p = MoonPalette::active(cx);
        let bullet = |key: &str| self.server_bot_note(format!("\u{2022} {}", t!(key)), cx);
        section
            .child(
                div().child(
                    t!(
                        "telegram.server.install_fingerprint",
                        addr = probe.target.addr(),
                        fingerprint = probe.fingerprint.clone()
                    )
                    .to_string(),
                ),
            )
            .child(self.server_bot_hint("telegram.server.install_changes_title", cx))
            .child(bullet("telegram.server.install_change_ssh"))
            .child(bullet("telegram.server.install_change_firewall"))
            .child(bullet("telegram.server.install_change_admin"))
            .child(bullet("telegram.server.install_change_extras"))
            .when(!ed.by_key, |s| {
                s.child(
                    div()
                        .text_color(rgba_from(p.red_text, 1.0))
                        .child(t!("telegram.server.install_password_warning").to_string()),
                )
            })
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 8.0))
                    .child(
                        MoonButton::new("server-install-confirm")
                            .primary()
                            .size(design::CONTROL_TIER)
                            .label(t!("telegram.server.install_confirm").to_string())
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                // The captured key is exactly the fingerprint displayed above;
                                // starting the job clears the probe.
                                let job = this.server_bot_install_job(fingerprint.clone(), cx);
                                this.server_bot_run(job, cx);
                            }))
                            .render(),
                    )
                    .child(
                        MoonButton::new("server-install-cancel")
                            .size(design::CONTROL_TIER)
                            .label(t!("telegram.server.cancel").to_string())
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.server_bot_clear_install_probe(cx);
                            }))
                            .render(),
                    ),
            )
    }

    /// Drop the probed key: a later install reads it again.
    fn server_bot_clear_install_probe(&mut self, cx: &mut Context<Self>) {
        self.backend.update(cx, |b, bcx| {
            b.station.install_probe = None;
            b.station.outcome = None;
            b.station.revision = b.station.revision.wrapping_add(1);
            bcx.notify();
        });
        cx.notify();
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

    /// A server not set up yet: what the provider gave, the bot, "Install" — the station comes from
    /// the latest release.
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
                s.child(self.server_bot_key_field(cx))
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
            .child(self.server_bot_hint("telegram.server.release_hint", cx))
            .when(has_bot, |s| {
                s.child(self.server_bot_hint("telegram.server.transfer_hint", cx))
            })
            .child(self.server_bot_hint("telegram.server.cores_hint", cx))
            .child(
                h_flex().child(
                    MoonButton::new("server-install")
                        .primary()
                        .size(design::CONTROL_TIER)
                        .label(t!("telegram.server.install").to_string())
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            // Nothing is sent before the user confirms the key this reads.
                            let job = parse_target(&text(&this.telegram.server.host, cx))
                                .map(|target| Job::InstallProbe { target })
                                .ok_or_else(|| t!("telegram.server.need_host").to_string());
                            this.server_bot_run(job, cx);
                        }))
                        .render(),
                ),
            )
            .map(|s| self.server_bot_install_review(s, cx))
    }

    /// Group a known station's everyday and maintenance actions into wrapping button rows.
    /// Destructive actions and their unchanged confirmations live in the separated access block.
    fn server_bot_known(
        &self,
        section: MoonGroupBox,
        target: &Target,
        cx: &Context<Self>,
    ) -> MoonGroupBox {
        let p = MoonPalette::active(cx);
        let busy = self.backend.read(cx).station.busy();
        let (service, version) = self.server_versions(cx);
        let button = |id: &'static str, label: String| {
            MoonButton::new(id)
                .size(design::CONTROL_TIER)
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
            .w_full()
            .min_w(px(0.0))
            .flex_wrap()
            .gap(design::ui_px(cx, 8.0))
            .child(
                button("server-status", t!("telegram.server.status").to_string())
                    .on_click(on(|_, target, _| Ok(Job::Status { target })))
                    .render(),
            )
            .child(
                button("server-logs", t!("telegram.server.logs").to_string())
                    .on_click(on(|_, target, _| Ok(Job::Logs { target })))
                    .render(),
            )
            .child({
                let update = match version {
                    ServiceVersion::Behind(newest) => button(
                        "server-update",
                        t!("telegram.server.update_to", version = newest.to_string()).to_string(),
                    )
                    .primary(),
                    _ => button("server-update", t!("telegram.server.update").to_string()),
                };
                update
                    .tooltip(t!("telegram.server.update_hint").to_string())
                    .on_click(on(|_, target, _| Ok(Job::Update { target })))
                    .render()
            });
        let maintenance = h_flex()
            .w_full()
            .min_w(px(0.0))
            .flex_wrap()
            .gap(design::ui_px(cx, 8.0))
            .child(
                button("server-resetup", t!("telegram.server.resetup").to_string())
                    .tooltip(t!("telegram.server.resetup_hint").to_string())
                    .on_click(on(|this, target, cx| {
                        this.server_bot_resetup_job(target, cx)
                    }))
                    .render(),
            )
            .child(
                button(
                    "server-change-address",
                    t!("telegram.server.change_address").to_string(),
                )
                .disabled(self.backend.read(cx).station.running)
                .on_click(cx.listener(|this, _, window, cx| this.station_address_begin(window, cx)))
                .render(),
            );
        let section = section.child(
            div()
                .text_color(rgba_from(p.text, 1.0))
                .child(t!("telegram.server.known", addr = target.addr()).to_string()),
        );
        let section = section.child(self.server_version_block(service, version, cx));
        // A server set up already updates from the release: no file to choose.
        self.server_bot_old_admin(section, cx)
            .child(actions)
            .child(maintenance)
            .child(self.station_access_block(target, cx))
            .child(self.server_auto_update_block(target, cx))
            .child(self.server_tape_block(target, cx))
    }

    /// The service's version as last read — `Err` with the muted word while it is being read or
    /// could not be — and where it stands against the newest known release.
    fn server_versions(&self, cx: &App) -> (Result<String, String>, ServiceVersion) {
        let b = self.backend.read(cx);
        let st = &b.station;
        let reported = st
            .bot
            .as_ref()
            .and_then(|bot| bot.station.as_ref())
            .map(|status| status.station_version.clone());
        let latest = b.updater.read(cx).latest_release();
        let version = service_version(
            reported.as_deref(),
            latest,
            crate::update::terminal_release(),
        );
        let service = match reported {
            Some(reported) => Ok(reported
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_owned()),
            None if st.bot.is_none() && st.bot_error.is_none() => {
                Err(t!("telegram.server.version_checking").to_string())
            }
            None => Err(t!("telegram.server.version_unknown").to_string()),
        };
        (service, version)
    }

    /// "On the server: vX" and "Latest: vY" — versions in mono, unknown states as muted words.
    fn server_version_block(
        &self,
        service: Result<String, String>,
        version: ServiceVersion,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let caption = |text: String| {
            div()
                .font_family(design::ui_font())
                .text_color(rgba_from(p.text_muted, 1.0))
                .child(text)
        };
        let value = |value: Result<String, String>| match value {
            Ok(text) => div()
                .font_family(design::mono())
                .text_color(rgba_from(p.text, 1.0))
                .child(text),
            Err(text) => caption(text),
        };
        let latest = match version {
            ServiceVersion::Behind(newest) => value(Ok(newest.to_string())).into_any_element(),
            ServiceVersion::Current(newest) => h_flex()
                .gap(design::ui_px(cx, 6.0))
                .child(value(Ok(newest.to_string())))
                .child(caption(t!("telegram.server.version_current").to_string()))
                .into_any_element(),
            ServiceVersion::Unknown => {
                caption(t!("telegram.server.version_unknown").to_string()).into_any_element()
            }
        };
        h_flex()
            .flex_wrap()
            .gap(design::ui_px(cx, 6.0))
            .child(caption(t!("telegram.server.version_server").to_string()))
            .child(value(service))
            .child(div().w(design::ui_px(cx, 10.0)))
            .child(caption(t!("telegram.server.version_latest").to_string()))
            .child(latest)
    }

    /// Show shared busy/outcome feedback; only Station requests status facts and the journal.
    /// `station_details` retains Station's full view and newest-line scroll cursor when true.
    pub(in crate::settings) fn server_bot_progress(
        &self,
        station_details: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let st = &self.backend.read(cx).station;
        let outcome = st.outcome.as_ref().map(|o| match o {
            Ok(text) => (text.clone(), rgba_from(p.text, 1.0)),
            Err(text) => (text.clone(), rgba_from(p.red_text, 1.0)),
        });
        v_flex()
            .w_full()
            .min_w(px(0.0))
            .gap(design::ui_px(cx, 8.0))
            .when(st.busy(), |s| {
                s.child(div().child(t!("telegram.server.running").to_string()))
            })
            .when_some(outcome, |s, (text, color)| {
                s.child(div().text_color(color).child(text))
            })
            .when(station_details, |s| {
                s.child(progress::StationProgress {
                    status: st.status.clone(),
                    lines: st.lines.clone(),
                    scroll: self.telegram.server.lines_scroll.clone(),
                })
            })
    }
}

#[cfg(test)]
mod tests;
