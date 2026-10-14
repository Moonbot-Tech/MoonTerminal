//! Telegram Settings navigation between the terminal bot and the core reader. With a station on a
//! Linux server the station is the only bot host; otherwise the terminal hosts it. Both show
//! the bot, one Chats box and the Mini App. The station itself
//! (install, status, actions, tape window) has its own Settings tab, hosted here as well because
//! its editors are part of [`TelegramEd`].
//!
//! Edits stay on `Backend.preview` and persist through the existing Save transaction. Live
//! service status and pairing come from the live service, independently of unsaved edits.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonAlert, MoonButton, MoonGroupBox, MoonInput, MoonInputEvent, MoonInputState, MoonPalette,
    h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::frame::{PageParts, RemoteBar, SaveBar};
use super::{SettingsView, Tab, common};
use crate::{Backend, design};
use moon_core::config::Secret;

mod access;
mod bot_menu;
mod chat_notify;
mod chat_notify_charts;
mod chat_notify_quotes;
mod core_section;
mod message_layout;
mod qr;
mod server_auto_update;
mod server_bot;
mod server_tape;
mod station_access;
mod station_cores;
mod station_groups;
mod station_overview;

pub(super) use server_tape::plural_key;

/// Password-field width in unscaled pixels, matching the Security tab.
const TOKEN_FIELD_W: f32 = 240.0;

/// Per-window Telegram editors, kept while the Telegram pages change.
pub(super) struct TelegramEd {
    pub(super) token: Entity<MoonInputState>,
    /// The terminal bot's chat editor; one expanded chat keeps long client lists compact.
    chats: access::ChatEd,
    /// History candidates are independent of the draft grants so deselection is reversible.
    history_cores: Vec<(u64, String)>,
    history_loaded: bool,
    history_loading: bool,
    history_failed: bool,
    core: core_section::CoreTelegramEd,
    server: server_bot::ServerBotEd,
    /// The terminal bot's menu tree.
    menu: bot_menu::BotMenuEd,
    /// The terminal's session-only message editor and typed preview cache.
    message_layout: message_layout::MessageLayoutEd,
}

impl TelegramEd {
    /// Show the saved Telegram settings again after a revert.
    ///
    /// The token field takes the saved token. The open chat is closed so its caption field is
    /// seeded from the draft when it is reopened; the menu tree and card editor re-read the draft
    /// on their own (`bot_menu_sync`, the layout preview).
    pub(super) fn reseed(&mut self, token: String, window: &mut Window, cx: &mut App) {
        self.token
            .update(cx, |state, cx| state.set_value(token, window, cx));
        self.chats.active_chat = None;
        self.chats.pending_owner = None;
        self.server.chats.active_chat = None;
        self.server.chats.pending_owner = None;
    }
}

/// Build retained editors bound to the Settings draft.
///
/// Args:
///     backend: Settings backend that owns the draft configuration.
///     window: Settings window used to create the input state.
///     cx: Settings context used to install the change subscription.
///
/// Returns:
///     Editor state whose token field writes `AppConfig::telegram.token` on each change.
pub(super) fn build(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> TelegramEd {
    let initial = {
        let b = backend.read(cx);
        let d = b.preview.as_ref().unwrap_or(&b.config);
        d.telegram.token.expose().to_string()
    };
    let token = cx.new(|cx| {
        MoonInputState::new(window, cx)
            .masked(true)
            .default_value(initial)
    });
    token.update(cx, |st, c| st.set_masked(true, window, c));
    cx.subscribe(&token, |this, emitter, ev: &MoonInputEvent, cx| {
        if matches!(ev, MoonInputEvent::Change) {
            let val = emitter.read(cx).value().to_string();
            this.backend.update(cx, |b, bcx| {
                if let Some(p) = b.preview.as_mut()
                    && p.telegram.token.expose() != val
                {
                    p.telegram.token = Secret::new(val);
                    bcx.notify();
                }
            });
        }
    })
    .detach();
    let ed = TelegramEd {
        token,
        chats: access::ChatEd::new(window, cx),
        history_cores: Vec::new(),
        history_loaded: false,
        history_loading: false,
        history_failed: false,
        core: core_section::build(window, cx),
        server: server_bot::build(window, cx, server_bot::known_server()),
        menu: bot_menu::BotMenuEd::new(cx),
        message_layout: message_layout::MessageLayoutEd::default(),
    };
    ed.chats.wire(access::ChatsOf::Terminal, cx);
    ed.server.chats.wire(access::ChatsOf::Station, cx);
    ed
}

/// Which bot the Telegram pages edit: the station's when one is known, else the terminal's.
/// One UI serves both; only where an edit goes differs.
pub(super) fn bot_host(station_known: bool) -> access::ChatsOf {
    if station_known {
        access::ChatsOf::Station
    } else {
        access::ChatsOf::Terminal
    }
}

/// The host line's words: where the bot runs and how many chats it serves.
pub(super) fn host_words(side: access::ChatsOf, chats: usize) -> String {
    let host = match side {
        access::ChatsOf::Terminal => t!("telegram.host.local"),
        access::ChatsOf::Station => t!("telegram.host.station"),
    };
    let n = chats as u64;
    let chats = t!(&plural_key("telegram.host.chats_n", n), n = n);
    format!("{host} · {chats}")
}

/// A disclosure toggle for rows that carry no long help.
pub(super) fn no_more() -> common::Action {
    std::rc::Rc::new(|_: &mut Window, _: &mut App| {})
}

/// Ask before a destructive station action: Cancel, or `label` (red) which runs `on`.
pub(super) fn confirm_danger(
    window: &mut Window,
    cx: &mut App,
    title: String,
    body: String,
    label: String,
    on: common::Action,
) {
    common::confirm(
        window,
        cx,
        common::Confirm {
            title,
            body,
            buttons: vec![
                common::ConfirmBtn {
                    label: t!("dialogs.cancel").to_string(),
                    kind: common::ConfirmKind::Cancel,
                    on: no_more(),
                },
                common::ConfirmBtn {
                    label,
                    kind: common::ConfirmKind::DangerSolid,
                    on,
                },
            ],
        },
    );
}

/// Unscaled gap between a page's cards, across and down.
const CARD_GAP: f32 = 16.0;

/// Unscaled narrowest card column: below two of them and the gap the cards stack in one column.
const CARD_COLUMN_MIN_W: f32 = 460.0;

/// Width of a page column on a page body `width` wide: the body less its two gutters.
///
/// Args:
///     width: The page body's width.
///     cx: Application context for the zoom-scaled gutters.
fn page_column_w(width: f32, cx: &App) -> f32 {
    (width - 2.0 * design::ui_value(cx, super::render::BODY_PAD)).max(0.0)
}

/// How many card columns a page column holds: two when both reach `min_w` beside `gap`, else one.
///
/// Args:
///     column_w: The page column's width.
///     min_w: The narrowest card column, scaled.
///     gap: The gap between the columns, scaled.
fn card_columns(column_w: f32, min_w: f32, gap: f32) -> usize {
    if column_w + 0.5 >= 2.0 * min_w + gap {
        2
    } else {
        1
    }
}

/// A page's independent cards in equal columns across `column_w`: two on a wide page, dealt in
/// turn (first card left, second right, third left...), one column below the breakpoint.
///
/// Each column gets an explicit width from the measured page width rather than `flex_wrap` with
/// `flex_1`, which leaves unequal or overlapping columns (CONTRIBUTING, the flex_wrap trap).
///
/// Args:
///     column_w: The page column's width (see [`page_column_w`]).
///     cards: The cards, in reading order.
///     cx: Application context for scale.
pub(super) fn card_grid_el(column_w: f32, cards: Vec<AnyElement>, cx: &App) -> AnyElement {
    let gap = design::ui_value(cx, CARD_GAP);
    let cols =
        card_columns(column_w, design::font_w(cx, CARD_COLUMN_MIN_W), gap).min(cards.len().max(1));
    let col_w = card_grid_col_w(column_w, cards.len(), cx);
    let mut columns: Vec<Vec<AnyElement>> = (0..cols).map(|_| Vec::new()).collect();
    for (i, card) in cards.into_iter().enumerate() {
        columns[i % cols].push(card);
    }
    h_flex()
        .debug_selector(|| "card-grid".into())
        .w(px(column_w))
        .items_start()
        .gap(px(gap))
        .children(columns.into_iter().enumerate().map(|(i, cards)| {
            v_flex()
                .debug_selector(move || format!("card-col-{i}"))
                .w(px(col_w))
                .flex_none()
                .gap(px(gap))
                .children(cards)
        }))
        .into_any_element()
}

/// Width of one card column in [`card_grid_el`] for `count` cards across `column_w`.
///
/// Args:
///     column_w: The page column's width (see [`page_column_w`]).
///     count: How many cards the grid holds.
///     cx: Application context for scale.
pub(in crate::settings) fn card_grid_col_w(column_w: f32, count: usize, cx: &App) -> f32 {
    let gap = design::ui_value(cx, CARD_GAP);
    let cols = card_columns(column_w, design::font_w(cx, CARD_COLUMN_MIN_W), gap).min(count.max(1));
    ((column_w - gap * (cols - 1) as f32) / cols as f32).max(0.0)
}

/// Prose inside a card on a page `width` wide, its lines capped near 90 characters so a full-width
/// card stays readable. The width is explicit (see `render::scrollable_tab_content`).
///
/// Args:
///     width: The page body's width.
///     text: The prose.
///     cx: Application context for scale and palette.
pub(super) fn card_prose_el(width: f32, text: String, cx: &App) -> AnyElement {
    // The card's padding and border on both sides.
    let inner = page_column_w(width, cx) - 2.0 * design::ui_value(cx, 15.0);
    div()
        .w(px(inner
            .min(design::font_w(cx, common::PROSE_MAX_W))
            .max(0.0)))
        .text_color(rgba_from(MoonPalette::active(cx).text_muted, 1.0))
        .child(text)
        .into_any_element()
}

/// The Station cores page column: the cores card and, under it, the groups line in a card of its
/// own, both across the whole content width like every other Station page. The page body does
/// not scroll (`render`): the column fills its height, the cores card takes what the groups card
/// leaves, and the core table inside it is what scrolls.
///
/// Args:
///     width: The page body's width.
///     cores: The station's cores card.
///     groups: The groups line, `None` when there is nothing to say about groups.
///     cx: Application context for scale.
fn st_cores_column(
    width: f32,
    cores: AnyElement,
    groups: Option<AnyElement>,
    cx: &App,
) -> AnyElement {
    let groups = groups.map(|row| {
        div()
            .debug_selector(|| "station-groups".into())
            .flex_none()
            .child(
                MoonGroupBox::new("station-groups")
                    .padding(14.0)
                    .gap(10.0)
                    .child(row),
            )
            .into_any_element()
    });
    v_flex()
        .w(px(page_column_w(width, cx)))
        .flex_1()
        .min_h(px(0.0))
        .gap(design::ui_px(cx, CARD_GAP))
        .debug_selector(|| "station-cores-column".into())
        .child(cores)
        .children(groups)
        .into_any_element()
}

impl SettingsView {
    /// Whether the bot runs on a known station, which makes its pages apply there.
    pub(super) fn station_known(&self) -> bool {
        self.telegram.server.known().is_some()
    }

    /// Which host's bot the Telegram pages edit.
    fn bot_side(&self) -> access::ChatsOf {
        bot_host(self.station_known())
    }

    /// One column of `width` holding `children`, as every Telegram and Station page lays out: the
    /// whole content width, like the Overview.
    // An explicit width, not `w_full().max_w(..)`: see `render::scrollable_tab_content` — the
    // bottom of a long page would fall outside the scroll extent.
    fn page_column(&self, width: f32, children: Vec<AnyElement>, cx: &Context<Self>) -> AnyElement {
        Self::column_of(width, None, children, cx)
    }

    /// A page column `width` wide less the body gutters, capped at `max` when given.
    fn column_of(width: f32, max: Option<f32>, children: Vec<AnyElement>, cx: &App) -> AnyElement {
        let column_w = page_column_w(width, cx).min(max.unwrap_or(f32::INFINITY));
        v_flex()
            .w(px(column_w))
            .gap(design::ui_px(cx, CARD_GAP))
            .children(children)
            .into_any_element()
    }

    /// The line on top of every bot page: where the bot runs, with how many chats.
    fn bot_host_line(&self, side: access::ChatsOf, cx: &Context<Self>) -> AnyElement {
        let chats = self
            .chats(side, cx)
            .map_or(0, |t| t.authorized_chat_ids.len());
        MoonAlert::success("tg-host-line", host_words(side, chats)).into_any_element()
    }

    /// The bar of a page whose edits go to the station's bot: what is pending there, one apply
    /// for chats, roles, cores and menu alike, and the revert of all of them.
    fn station_access_bar(&self, cx: &Context<Self>) -> SaveBar {
        let station = &self.backend.read(cx).station;
        let pending = self.station_draft_pending();
        let msg = match (&station.applied, station.applying) {
            (_, true) => t!("telegram.server.applying").to_string(),
            (Some(Ok(text)), false) => text.clone(),
            (Some(Err(reason)), false) => reason.clone(),
            (None, false) if pending => t!("settings.savebar.dirty_other").to_string(),
            (None, false) => t!("settings.mode.remote").to_string(),
        };
        let apply: super::frame::OnClick = {
            let weak = cx.entity().downgrade();
            std::rc::Rc::new(move |_: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |this, cx| {
                    this.apply_station_access(cx);
                });
            })
        };
        let revert: super::frame::OnClick = {
            let weak = cx.entity().downgrade();
            std::rc::Rc::new(move |_: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |this, cx| this.revert_station_access(cx));
            })
        };
        SaveBar::Remote(RemoteBar {
            tone: None,
            msg,
            dirty: pending && !station.busy(),
            apply,
            revert: pending.then_some(revert),
            label: t!("telegram.server.access_apply").to_string(),
        })
    }

    /// The bot page: where the bot runs, its token, its chats and the Mini App — the same for the
    /// terminal's bot and the station's.
    pub(super) fn tg_bot_page(&self, width: f32, cx: &Context<Self>) -> PageParts {
        let side = self.bot_side();
        let column_w = page_column_w(width, cx);
        // Both hosts lay three cards whenever the chats card shows: bot, chats and Mini App.
        let card_w = card_grid_col_w(column_w, 3, cx);
        let children = match side {
            access::ChatsOf::Terminal => vec![
                self.bot_host_line(side, cx),
                card_grid_el(column_w, self.terminal_bot_sections(card_w, cx), cx),
            ],
            access::ChatsOf::Station => {
                let mut cards = vec![self.server_bot_block(cx).into_any_element()];
                cards.extend(self.server_bot_sections(card_w, cx));
                vec![
                    self.bot_host_line(side, cx),
                    card_grid_el(column_w, cards, cx),
                    self.server_bot_progress(false, cx).into_any_element(),
                ]
            }
        };
        let body = self.page_column(width, children, cx);
        let mut parts = self.page_parts(Tab::TgBot, body, Vec::new(), cx);
        if side == access::ChatsOf::Station {
            parts.bar = self.station_access_bar(cx);
        }
        parts
    }

    /// The Telegram-on-the-core page: the core's reader, its login and its proxy.
    pub(super) fn tg_core_page(&self, width: f32, cx: &Context<Self>) -> PageParts {
        // The body is bounded (`render::bounded_body`): the list beside the card fills its height.
        let column_w = page_column_w(width, cx);
        let body = v_flex()
            .w(px(column_w))
            .flex_1()
            .min_h(px(0.0))
            .debug_selector(|| "tgcore-column".into())
            .child(self.core_telegram_section(column_w, cx))
            .into_any_element();
        self.page_parts(Tab::TgCore, body, Vec::new(), cx)
    }

    /// The Notifications page: the bot's per-chat notifications, applied as they change.
    pub(super) fn tg_notify_page(&self, width: f32, cx: &Context<Self>) -> PageParts {
        let body = self.page_column(width, vec![self.bot_notify_box(self.bot_side(), cx)], cx);
        let mut parts = self.page_parts(Tab::TgNotify, body, Vec::new(), cx);
        // On the station too every change is sent at once, against the row's revision.
        parts.bar = SaveBar::Live;
        parts
    }

    /// The Menu and messages page: the bot's menu buttons beside its message layout, stacked
    /// when two columns do not fit.
    pub(super) fn tg_messages_page(&self, width: f32, cx: &Context<Self>) -> PageParts {
        let side = self.bot_side();
        let column_w = page_column_w(width, cx);
        let card_w = card_grid_col_w(column_w, 2, cx);
        let body = self.page_column(
            width,
            vec![card_grid_el(
                column_w,
                vec![
                    self.bot_menu_keys_box(side, cx),
                    self.message_layout_box(side, card_w, cx),
                ],
                cx,
            )],
            cx,
        );
        let mut parts = self.page_parts(Tab::TgMessages, body, Vec::new(), cx);
        if side == access::ChatsOf::Station {
            parts.bar = self.station_access_bar(cx);
        }
        parts
    }

    /// The station's cores page.
    pub(super) fn st_cores_page(&self, width: f32, cx: &Context<Self>) -> PageParts {
        let body = st_cores_column(
            width,
            self.station_cores_section(width, cx),
            self.station_groups_row(cx),
            cx,
        );
        let mut parts = self.page_parts(Tab::StCores, body, Vec::new(), cx);
        parts.bar = self.station_cores_bar(cx);
        parts
    }

    /// The station's trade tape page.
    pub(super) fn st_tape_page(&self, width: f32, cx: &Context<Self>) -> PageParts {
        let body = self.page_column(width, vec![self.station_tape_section(width, cx)], cx);
        let mut parts = self.page_parts(Tab::StTape, body, Vec::new(), cx);
        parts.bar = self.station_tape_bar(cx);
        parts
    }

    /// The station's maintenance page: auto-update and the station's access.
    pub(super) fn st_maint_page(&self, width: f32, cx: &Context<Self>) -> PageParts {
        let body = self.page_column(width, vec![self.station_maint_section(width, cx)], cx);
        self.page_parts(Tab::StMaint, body, Vec::new(), cx)
    }

    /// The station overview page.
    pub(super) fn st_overview_page(&self, width: f32, cx: &Context<Self>) -> PageParts {
        let head = if self.station_known() {
            station_overview::tiles_el(self.station_tiles(cx), cx)
        } else {
            let st = &self.backend.read(cx).station;
            station_overview::install_steps_el(station_overview::install_step(
                st.install_probe.is_some(),
                st.busy(),
            ))
        };
        // The overview is a dashboard: its tiles and card take the whole content width.
        let body = Self::column_of(
            width,
            None,
            vec![head, self.server_bot_section(cx).into_any_element()],
            cx,
        );
        self.page_parts(Tab::StOverview, body, Vec::new(), cx)
    }

    /// The terminal's bot: token and where it runs, its chats, the Mini App. Edits go to the
    /// draft and persist with Save; live service status and pairing come from the running bot.
    /// `card_w` is one card's width on the page, which the chats table sizes its cells from.
    fn terminal_bot_sections(&self, card_w: f32, cx: &Context<Self>) -> Vec<AnyElement> {
        let p = MoonPalette::active(cx);
        let cfg = self.backend.read(cx);
        let draft = cfg.preview.as_ref().unwrap_or(&cfg.config);
        let telegram = &draft.telegram;
        let token_changed = telegram.token.expose() != cfg.config.telegram.token.expose();
        let status_text = if token_changed {
            t!("telegram.state.unsaved").to_string()
        } else {
            cfg.telegram_service_status_text()
        };
        let mini_changed =
            token_changed || telegram.mini_app_enabled != cfg.config.telegram.mini_app_enabled;
        let mini_status = if mini_changed {
            t!("telegram.state.unsaved").to_string()
        } else if !telegram.mini_app_enabled {
            t!("telegram.mini_disabled").to_string()
        } else if telegram.token.is_empty() {
            t!("telegram.state.disabled").to_string()
        } else {
            use moon_core::telegram::runtime::mini_app::MiniAppStatus;
            match &cfg.telegram.mini_status {
                MiniAppStatus::Tunneling { .. } => t!("telegram.mini_ready"),
                MiniAppStatus::Starting | MiniAppStatus::Listening { .. } => {
                    t!("telegram.mini_starting")
                }
                MiniAppStatus::Failed { .. } => t!("telegram.mini_failed"),
                MiniAppStatus::Stopped => t!("telegram.stopped"),
            }
            .to_string()
        };
        let mini_url = if telegram.mini_app_enabled && !mini_changed {
            match &cfg.telegram.mini_status {
                moon_core::telegram::runtime::mini_app::MiniAppStatus::Tunneling {
                    url, ..
                } => Some(url.clone()),
                _ => None,
            }
        } else {
            None
        };
        let mini_app = telegram.mini_app_enabled;
        let code = cfg
            .telegram
            .pairing
            .as_ref()
            .filter(|_| !token_changed)
            .map(|(code, _)| code.clone());
        let pairing = self.chat_pairing(
            access::ChatsOf::Terminal,
            access::PairingState {
                code,
                pair_disabled: cfg.telegram.service.is_none() || token_changed,
                unpair_disabled: cfg.config.telegram.authorized_chat_ids.is_empty(),
            },
            cx,
        );
        let onboarding = cfg
            .config
            .telegram
            .token
            .expose()
            .is_empty()
            .then(|| t!("telegram.onboarding").to_string());
        // A bot without a saved token shows how to get one, already unfolded.
        let onboarding_open = onboarding.is_some();
        let token_row = common::hint_row(
            "tg-token",
            t!("telegram.token").to_string(),
            Some(t!("telegram.token_hint").to_string()),
            onboarding,
            div()
                .w(design::font_w_px(cx, TOKEN_FIELD_W))
                .child(
                    MoonInput::new("telegram-token")
                        .state(&self.telegram.token)
                        .size(design::INPUT_SIZE)
                        .mono(true)
                        .mask_toggle(),
                )
                .into_any_element(),
            onboarding_open,
            no_more(),
            cx,
        );
        let where_row = common::hint_row(
            "tg-where",
            t!("telegram.where.label").to_string(),
            Some(t!("telegram.where.local").to_string()),
            None,
            MoonButton::new("tg-where-move")
                .padding_x(12.0)
                .label(t!("telegram.where.move").to_string())
                .on_click(cx.listener(|this, _, _, cx| this.show_page(Tab::StOverview, cx)))
                .render()
                .into_any_element(),
            false,
            no_more(),
            cx,
        );
        let bot = MoonGroupBox::new("telegram-bot-section")
            .child(crate::settings::common::group_title(
                t!("telegram.section_bot").to_string(),
                cx,
            ))
            .padding(14.0)
            .gap(6.0)
            .child(
                div()
                    .debug_selector(|| "tg-bot-status".into())
                    .text_color(rgba_from(p.text, 1.0))
                    .child(status_text),
            )
            .child(token_row)
            .child(where_row);
        let mini_toggle: common::Action = {
            let weak = cx.entity().downgrade();
            std::rc::Rc::new(move |_: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |this, cx| {
                    this.telegram_ui.mini_more = !this.telegram_ui.mini_more;
                    cx.notify();
                });
            })
        };
        let mini_row = common::hint_row(
            "tg-mini",
            t!("telegram.mini_app").to_string(),
            Some(mini_status),
            Some(t!("telegram.mini_app_hint").to_string()),
            h_flex()
                .gap(design::ui_px(cx, 8.0))
                .items_center()
                .child(self.draft_checkbox(cx, "tg-mini-app", mini_app, |p, v| {
                    if p.telegram.mini_app_enabled != v {
                        p.telegram.mini_app_enabled = v;
                        true
                    } else {
                        false
                    }
                }))
                .when_some(mini_url, |row, url| {
                    row.child(
                        MoonButton::new("telegram-copy-url")
                            .label(t!("telegram.mini_copy_url").to_string())
                            .variant(moon_ui::MoonButtonVariant::Neutral)
                            .size(design::CONTROL_TIER)
                            .tooltip(url.clone())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                                this.status =
                                    Some((super::StatusMsg::Key("settings.copied"), false));
                                cx.notify();
                            }))
                            .render(),
                    )
                })
                .into_any_element(),
            self.telegram_ui.mini_more,
            mini_toggle,
            cx,
        );
        let mini = MoonGroupBox::new("telegram-mini-section")
            .child(crate::settings::common::group_title(
                t!("telegram.section_mini_app").to_string(),
                cx,
            ))
            .padding(14.0)
            .gap(6.0)
            .child(mini_row);
        vec![
            bot.into_any_element(),
            self.telegram_chat_access(access::ChatsOf::Terminal, pairing, card_w, cx)
                .into_any_element(),
            mini.into_any_element(),
        ]
    }
}

/// UI state of the Telegram (bot, notifications, messages, Telegram on the core) page that the draft does not hold. Branch-owned: fields are added here,
/// never on `SettingsView`.
#[derive(Default)]
pub(super) struct TelegramUi {
    /// The Mini App row's long help is open.
    mini_more: bool,
}

#[cfg(test)]
mod tests;
