//! The bot's Control section, the owner's: commands to the cores from the chat.
//!
//! Every screen is one message under which the buttons sit, edited in place as the owner moves
//! through it: the cores a page at a time, one core's card, the all-cores card. A command runs
//! through the shared command module ([`crate::control`]) — the same owner check, choice of cores
//! and session calls as the Mini App's — and its card comes back with what happened on top. What
//! can lose money or stop trading everywhere asks first: the press shows a confirmation whose
//! button sends the same action confirmed — taken once, from this chat, within two minutes of the
//! question, so a stale or second press asks again rather than acting.
//!
//! The section is the owner's and exists only while shown in the bot's menu: hidden, `/control`
//! and its buttons answer that it is off. Every command is logged with the chat that sent it.

use std::sync::mpsc::SyncSender;
use std::time::{Duration, Instant};

use moon_core::config::TempBanSpan;
use moon_core::config::telegram_menu::MenuItem;
use moon_core::session::{CoreId, RunSwitch};
use moon_core::telegram::api::{InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup};
use moon_core::telegram::menu_action::{
    ControlAction, ControlSwitch, ControlTarget, MenuAction, OrderBan,
};
use moon_core::telegram::runtime::Response;
use moon_core::util::fmt;
use rust_i18n::t;

use crate::TgHost;
use crate::control::{self, Refusal};
use crate::html::escape;
use crate::labels::navigation_keyboard;

/// Cores on one page of the list.
const PAGE: usize = 8;

/// Strategies on one page of a core's list.
const STRATEGY_PAGE: usize = 12;

/// A screen: its title, its lines, its buttons.
type Rendered = (String, Vec<String>, Vec<Vec<InlineKeyboardButton>>);

/// How long a question — a coin to type, a command to confirm — stands.
const ASK_FOR: Duration = Duration::from_secs(120);

/// Longest coin a blacklist answer may name, in characters.
const COIN_CHARS: usize = 30;

/// Most characters of the core's blacklist a question shows; Telegram takes 4096 per message.
const BLACKLIST_CHARS: usize = 3000;

/// Answer a Control button: run its command, if any, then show the screen it leads to. Any button
/// but the question itself withdraws a question for a coin.
///
/// `message` is the message the button sits under, which the answer replaces. A press there
/// settles the redraw that message waits for ([`wait::press`]); a command the core still has to
/// confirm leaves a new one.
pub(super) fn run(
    host: &mut dyn TgHost,
    chat: i64,
    action: ControlAction,
    message: Option<i64>,
    reply: &SyncSender<Response>,
) {
    if !matches!(action, ControlAction::AskCoin { .. }) {
        host.state_mut().awaiting_coin.remove(&chat);
    }
    let carried = match message {
        Some(message) => wait::press(host, chat, message, action),
        None => Vec::new(),
    };
    if !shown(host) {
        send(host, hidden(), reply);
        return;
    }
    if is_command(action) {
        log::info!("telegram control: chat {chat} {action:?}");
    }
    let mut asks = Vec::new();
    let rendered = screen(host, chat, action, &mut asks);
    send(host, rendered, reply);
    // Only a press that sent something waits: one answered with a question or a refusal shows
    // no card a redraw could replace without taking the question's buttons away.
    if !asks.is_empty()
        && let Some(screen) = redrawn_as(action)
    {
        asks.extend(carried);
        wait::watch(host, chat, message, screen, asks);
    }
}

/// The screen a command's message is redrawn with once the core answers: the one it answered
/// with. `None` for a press that asks the core nothing.
fn redrawn_as(action: ControlAction) -> Option<ControlAction> {
    use ControlAction as A;
    match action {
        A::Run {
            target: ControlTarget::Core(core),
            ..
        }
        | A::CancelAll { core, .. } => Some(A::Core(core)),
        A::Run {
            target: ControlTarget::All,
            ..
        } => Some(A::All),
        A::StrategyToggle { core, page, .. } => Some(A::Strategies { core, page }),
        _ => None,
    }
}

/// Whether the bot's menu shows the section: hidden, nothing in it runs.
fn shown(host: &dyn TgHost) -> bool {
    host.config().telegram.bot.menu.shows(MenuItem::Control)
}

/// The answer while the section is hidden.
fn hidden() -> Rendered {
    (
        t!("telegram.button_control").to_string(),
        vec![t!("telegram.control.hidden").to_string()],
        Vec::new(),
    )
}

/// Whether `action` changes something on a core, as opposed to showing a screen.
fn is_command(action: ControlAction) -> bool {
    use ControlAction as A;
    !matches!(
        action,
        A::Cores(_)
            | A::Core(_)
            | A::All
            | A::Orders { .. }
            | A::Order { .. }
            | A::Strategies { .. }
            | A::AskCoin { .. }
    )
}

/// Take a chat's plain text as the coin a question asked for, and put it on (or take it off) the
/// core's blacklist.
///
/// Returns:
///     Whether a question was standing; an expired one is withdrawn and takes nothing.
pub(super) fn answer_text(
    host: &mut dyn TgHost,
    chat: i64,
    text: &str,
    reply: &SyncSender<Response>,
) -> bool {
    let Some(&(core, lift, until)) = host.state().awaiting_coin.get(&chat) else {
        return false;
    };
    // A question outlived, or asked of a chat that is no longer the owner, or of a section since
    // hidden, takes nothing: the text is answered as any unknown one.
    if Instant::now() >= until || !control::is_owner(host, chat) || !shown(host) {
        host.state_mut().awaiting_coin.remove(&chat);
        return false;
    }
    // Answered, well or not: a question left open would take the next word typed — "cancel",
    // say — as a coin, and an add switches a disabled list on.
    host.state_mut().awaiting_coin.remove(&chat);
    let coin = text.trim();
    if !valid_coin(coin) {
        let rendered = core_card(
            host,
            core,
            Some(t!("telegram.control.bad_coin").to_string()),
        );
        send(host, rendered, reply);
        return true;
    }
    let result = control::core_blacklist(host, chat, core, coin, lift);
    log::info!(
        "telegram control: chat {chat} core {core} blacklist {coin} lift={lift} -> {result:?}"
    );
    let said = match result {
        Ok(true) if lift => t!("telegram.control.coin_lifted", coin = coin).to_string(),
        Ok(true) => t!("telegram.control.coin_banned", coin = coin).to_string(),
        Ok(false) if lift => t!("telegram.control.coin_not_listed", coin = coin).to_string(),
        Ok(false) => t!("telegram.control.already_banned").to_string(),
        Err(refusal) => refusal_text(refusal),
    };
    send(host, core_card(host, core, Some(said)), reply);
    true
}

/// The question for a coin: which one, how to spell it, and the way back.
fn ask_coin(host: &dyn TgHost, core: CoreId, lift: bool, said: Option<String>) -> Rendered {
    let question = match lift {
        true => t!("telegram.control.ask_coin_lift", core = name(host, core)),
        false => t!("telegram.control.ask_coin_ban", core = name(host, core)),
    };
    let mut lines: Vec<String> = said.into_iter().collect();
    lines.push(question.to_string());
    lines.push(t!("telegram.control.ask_coin_hint").to_string());
    lines.push(blacklist_now(host, core));
    (
        t!("telegram.control.blacklist").to_string(),
        lines,
        vec![vec![button(
            format!("\u{274c} {}", t!("telegram.control.cancel")),
            ControlAction::Core(core),
        )]],
    )
}

/// The core's blacklist as it stands, its coins comma-separated as the core keeps them — the list
/// a coin is added to or taken off. Cut at [`BLACKLIST_CHARS`] so a long list cannot push the
/// message past Telegram's limit; the coins left out are counted.
fn blacklist_now(host: &dyn TgHost, core: CoreId) -> String {
    let Some((on, coins)) = control::core_blacklist_state(host, core) else {
        return t!("telegram.control.blacklist_unread").to_string();
    };
    let state = on_off(Some(on));
    if coins.is_empty() {
        return t!("telegram.control.blacklist_empty", state = state).to_string();
    }
    let mut shown = String::new();
    let mut left = coins.len();
    for coin in &coins {
        let next = shown.chars().count() + coin.chars().count() + 2;
        if !shown.is_empty() && next > BLACKLIST_CHARS {
            break;
        }
        if !shown.is_empty() {
            shown.push_str(", ");
        }
        shown.push_str(coin);
        left -= 1;
    }
    if left > 0 {
        shown.push(' ');
        shown.push_str(&t!("telegram.control.blacklist_more", n = left));
    }
    t!(
        "telegram.control.blacklist_now",
        state = state,
        n = coins.len(),
        coins = shown
    )
    .to_string()
}

/// Whether `action` is a confirmed press this chat was asked for, within [`ASK_FOR`]: taken once.
/// An unconfirmed press, a stale one, or one this chat was not asked for is not.
fn take_confirmation(host: &mut dyn TgHost, chat: i64, action: ControlAction) -> bool {
    if confirmed_form(action, false) == action {
        return false;
    }
    match host.state_mut().awaiting_confirm.remove(&chat) {
        Some((asked, until)) => asked == action && Instant::now() < until,
        None => false,
    }
}

/// Ask this chat to confirm `action`: remember the confirmed press it may send, and show the
/// question.
fn ask(
    host: &mut dyn TgHost,
    chat: i64,
    question: String,
    action: ControlAction,
    back: ControlAction,
) -> Rendered {
    let confirmed = confirmed_form(action, true);
    host.state_mut()
        .awaiting_confirm
        .insert(chat, (confirmed, Instant::now() + ASK_FOR));
    confirm(question, confirmed, back)
}

/// `action` with its `confirmed` flag set to `value`; an action without one, unchanged.
fn confirmed_form(action: ControlAction, value: bool) -> ControlAction {
    match action {
        ControlAction::Run {
            target, switch, on, ..
        } => ControlAction::Run {
            target,
            switch,
            on,
            confirmed: value,
        },
        ControlAction::CancelAll { core, .. } => ControlAction::CancelAll {
            core,
            confirmed: value,
        },
        ControlAction::PanicAll { target, .. } => ControlAction::PanicAll {
            target,
            confirmed: value,
        },
        ControlAction::OrderPanic { core, uid, .. } => ControlAction::OrderPanic {
            core,
            uid,
            confirmed: value,
        },
        other => other,
    }
}

/// Whether `coin` can be a coin token: one word of letters, digits, `_`, `-` or `.`, at most
/// [`COIN_CHARS`] long — never a list, which would put several coins on the blacklist at once.
fn valid_coin(coin: &str) -> bool {
    !coin.is_empty()
        && coin.chars().count() <= COIN_CHARS
        && coin
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Send a screen as the section's message, with the owner's navigation keyboard.
fn send(host: &dyn TgHost, rendered: Rendered, reply: &SyncSender<Response>) {
    let (html, keyboard) = html(rendered);
    let navigation = navigation_keyboard(host.kind(), true, &host.config().telegram);
    let _ = reply.try_send(Response::Rich {
        html,
        keyboard,
        navigation: (
            t!("telegram.report_navigation_hint").to_string(),
            navigation,
        ),
    });
}

/// A screen as a message: its HTML and its buttons.
fn html(rendered: Rendered) -> (String, ReplyMarkup) {
    let (title, lines, rows) = rendered;
    let mut html = format!("<p><b>{}</b></p>", escape(&title));
    for line in lines {
        html.push_str(&format!("<p>{}</p>", escape(&line)));
    }
    (
        html,
        ReplyMarkup::Inline(InlineKeyboardMarkup::from_rows(rows)),
    )
}

/// `screen` drawn again from the cores' state, with `said` on top: a redraw ([`redrawn_as`]).
fn draw(host: &dyn TgHost, screen: ControlAction, said: String) -> Rendered {
    use ControlAction as A;
    match screen {
        A::Core(core) => core_card(host, core, Some(said)),
        A::Strategies { core, page } => strategies(host, core, usize::from(page), Some(said)),
        _ => all_card(host, Some(said)),
    }
}

/// The screen for `action`, after running its command; what the command asked of a core goes to
/// `asks`.
fn screen(
    host: &mut dyn TgHost,
    chat: i64,
    action: ControlAction,
    asks: &mut Vec<wait::Ask>,
) -> Rendered {
    use ControlAction as A;
    match action {
        A::Cores(page) => cores(host, usize::from(page), None),
        A::Core(core) => core_card(host, core, None),
        A::All => all_card(host, None),
        A::Run {
            target, switch, on, ..
        } => {
            if target == ControlTarget::All && !take_confirmation(host, chat, action) {
                let count = control::owner_cores(host).len();
                let question = match on {
                    true => t!("telegram.control.confirm_start_all", n = count),
                    false => t!("telegram.control.confirm_stop_all", n = count),
                };
                return ask(host, chat, question.to_string(), action, ControlAction::All);
            }
            let run_switch = match switch {
                ControlSwitch::Trading => RunSwitch::Trading,
                ControlSwitch::AutoDetect => RunSwitch::AutoDetect,
            };
            match target {
                ControlTarget::Core(core) => {
                    let said = match control::run_one(host, chat, core, run_switch, on) {
                        Ok(true) => {
                            wait::arm_run(host, core, run_switch, on);
                            asks.push(wait::Ask::Run {
                                core,
                                switch: run_switch,
                                on,
                            });
                            t!("telegram.control.waiting").to_string()
                        }
                        Ok(false) => t!("telegram.control.already").to_string(),
                        Err(refusal) => refusal_text(refusal),
                    };
                    core_card(host, core, Some(said))
                }
                ControlTarget::All => {
                    let cores: Vec<CoreId> = control::owner_cores(host)
                        .into_iter()
                        .map(|(id, _)| id)
                        .collect();
                    let said = match control::run_many(host, chat, &cores, run_switch, on) {
                        Ok((targets, outcome)) => {
                            for &core in &outcome.sent {
                                wait::arm_run(host, core, run_switch, on);
                                asks.push(wait::Ask::Run {
                                    core,
                                    switch: run_switch,
                                    on,
                                });
                            }
                            t!(
                                "telegram.control.scope",
                                sent = outcome.sent.len(),
                                n = targets.len(),
                                already = outcome.already,
                                offline = outcome.offline
                            )
                            .to_string()
                        }
                        Err(refusal) => refusal_text(refusal),
                    };
                    all_card(host, Some(said))
                }
            }
        }
        A::CancelAll { core, .. } => {
            if !take_confirmation(host, chat, action) {
                let question = t!(
                    "telegram.control.confirm_cancel_all",
                    core = name(host, core)
                );
                return ask(
                    host,
                    chat,
                    question.to_string(),
                    action,
                    ControlAction::Core(core),
                );
            }
            let said = match control::cancel_buys(host, chat, core) {
                Ok(0) => t!("telegram.control.buys_none").to_string(),
                Ok(n) => {
                    asks.push(wait::Ask::CancelBuys { core });
                    t!("telegram.control.buys_sent", n = n).to_string()
                }
                Err(refusal) => refusal_text(refusal),
            };
            core_card(host, core, Some(said))
        }
        A::PanicAll { target, .. } => {
            if !take_confirmation(host, chat, action) {
                let (question, back) = match target {
                    ControlTarget::Core(core) => (
                        t!("telegram.control.confirm_panic", core = name(host, core)),
                        ControlAction::Core(core),
                    ),
                    ControlTarget::All => {
                        (t!("telegram.control.confirm_panic_all"), ControlAction::All)
                    }
                };
                return ask(host, chat, question.to_string(), action, back);
            }
            let cores: Vec<CoreId> = match target {
                ControlTarget::Core(core) => vec![core],
                ControlTarget::All => control::owner_cores(host)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect(),
            };
            let said = match control::panic_all(host, chat, &cores) {
                Ok(done) => t!(
                    "telegram.control.panic_done",
                    armed = done.armed,
                    already = done.already,
                    offline = done.offline,
                    refused = done.refused
                )
                .to_string(),
                Err(refusal) => refusal_text(refusal),
            };
            match target {
                ControlTarget::Core(core) => core_card(host, core, Some(said)),
                ControlTarget::All => all_card(host, Some(said)),
            }
        }
        A::Reconnect(core) => {
            let said = match control::reconnect(host, chat, core) {
                Ok(()) => t!("telegram.control.reconnect_sent").to_string(),
                Err(refusal) => refusal_text(refusal),
            };
            core_card(host, core, Some(said))
        }
        A::AskCoin { core, lift } => {
            if !control::is_owner(host, chat) {
                return core_card(host, core, Some(refusal_text(Refusal::NotOwner)));
            }
            host.state_mut()
                .awaiting_coin
                .insert(chat, (core, lift, Instant::now() + ASK_FOR));
            ask_coin(host, core, lift, None)
        }
        A::Strategies { core, page } => strategies(host, core, usize::from(page), None),
        A::StrategyToggle { core, id, on, page } => {
            // The Mini App sends to a core however it is; the chat says plainly that it is not
            // connected rather than showing a mark that later reverts.
            let said = if !host.session().core_run_state(core).online {
                Some(refusal_text(Refusal::Offline))
            } else {
                match control::strategy_toggle(host, chat, core, id, on) {
                    Ok(()) => {
                        asks.push(wait::Ask::Strategy { core, id, on });
                        None
                    }
                    Err(refusal) => Some(refusal_text(refusal)),
                }
            };
            strategies(host, core, usize::from(page), said)
        }
        A::Orders { core, page } => orders(host, core, usize::from(page), None),
        A::Order { core, uid } => order_card(host, core, uid, None),
        A::OrderPanic { core, uid, .. } => {
            if !take_confirmation(host, chat, action) {
                let coin = control::open_order(host, core, uid)
                    .map(|order| order.coin)
                    .unwrap_or_default();
                let question = t!("telegram.control.confirm_order_panic", coin = coin);
                return ask(
                    host,
                    chat,
                    question.to_string(),
                    action,
                    ControlAction::Order { core, uid },
                );
            }
            let said = match control::order_panic(host, chat, core, uid) {
                Ok(()) => t!("telegram.control.order_panic_sent").to_string(),
                Err(refusal) => refusal_text(refusal),
            };
            order_card(host, core, uid, Some(said))
        }
        A::OrderBan { core, uid, ban } => {
            let said = match control::order_ban(host, chat, core, uid, ban) {
                Ok(true) => match ban {
                    OrderBan::Core => t!("telegram.control.banned_core").to_string(),
                    OrderBan::Strategy => t!("telegram.control.banned_strategy").to_string(),
                    OrderBan::Temp(span) => {
                        t!("telegram.control.banned_temp", hours = span.hours()).to_string()
                    }
                },
                Ok(false) => t!("telegram.control.already_banned").to_string(),
                Err(refusal) => refusal_text(refusal),
            };
            order_card(host, core, uid, Some(said))
        }
    }
}

/// A refusal in the chat's words.
fn refusal_text(refusal: Refusal) -> String {
    match refusal {
        Refusal::NotOwner => t!("telegram.refusal"),
        Refusal::NotFound => t!("telegram.mini_cmd_core_not_found"),
        Refusal::Offline => t!("telegram.mini_cmd_offline"),
        Refusal::Unavailable => t!("telegram.control.not_sent"),
        Refusal::NotReady => t!("telegram.control.not_ready"),
        Refusal::NoList => t!("telegram.control.no_list"),
        Refusal::LightStation => t!("telegram.control.light_station"),
    }
    .to_string()
}

/// On a light station, why the cards are empty: it keeps no orders, strategies or run state.
fn light_station() -> Option<String> {
    (moon_core::feed::station::profile() == Some(moon_core::feed::station::Profile::Reports))
        .then(|| t!("telegram.control.light_station").to_string())
}

/// A core's name as the owner sees it, or its id.
fn name(host: &dyn TgHost, core: CoreId) -> String {
    control::owner_cores(host)
        .into_iter()
        .find(|(id, _)| *id == core)
        .map(|(_, name)| name)
        .unwrap_or_else(|| core.to_string())
}

/// A button carrying a Control action.
fn button(text: String, action: ControlAction) -> InlineKeyboardButton {
    InlineKeyboardButton::callback(text, MenuAction::Control(action).callback())
}

/// The confirmation of `action`: the question, and the buttons that send it confirmed or go back.
fn confirm(question: String, action: ControlAction, back: ControlAction) -> Rendered {
    let confirmed = confirmed_form(action, true);
    (
        t!("telegram.control.confirm_title").to_string(),
        vec![question],
        vec![vec![
            button(
                format!("\u{2705} {}", t!("telegram.control.confirm")),
                confirmed,
            ),
            button(format!("\u{274c} {}", t!("telegram.control.cancel")), back),
        ]],
    )
}

/// The cores, `page` of them, with "all cores" on top.
fn cores(host: &dyn TgHost, page: usize, said: Option<String>) -> Rendered {
    let all = control::owner_cores(host);
    let pages = all.len().div_ceil(PAGE).max(1);
    let page = page.min(pages - 1);
    let mut lines: Vec<String> = said.into_iter().collect();
    let mut rows = vec![vec![button(
        format!("\u{1f310} {}", t!("telegram.control.all")),
        ControlAction::All,
    )]];
    if all.is_empty() {
        lines.push(t!("telegram.control.none").to_string());
    } else {
        lines.push(t!("telegram.control.pick").to_string());
    }
    lines.extend(light_station());
    for (core, core_name) in all.iter().skip(page * PAGE).take(PAGE) {
        let state = host.session().core_run_state(*core);
        rows.push(vec![button(
            format!("{} {core_name}", state_marks(&state)),
            ControlAction::Core(*core),
        )]);
    }
    if pages > 1 {
        let to = |page: usize| ControlAction::Cores(u16::try_from(page).unwrap_or(u16::MAX));
        let mut nav = Vec::new();
        if page > 0 {
            nav.push(button("\u{25c0}".into(), to(page - 1)));
        }
        nav.push(InlineKeyboardButton::callback(
            format!("{}/{pages}", page + 1),
            MenuAction::Noop.callback(),
        ));
        if page + 1 < pages {
            nav.push(button("\u{25b6}".into(), to(page + 1)));
        }
        rows.push(nav);
    }
    (t!("telegram.button_control").to_string(), lines, rows)
}

/// A core's link and trading at a glance: green or red, then playing or paused.
fn state_marks(state: &moon_core::session::CoreRunState) -> String {
    let link = if state.online {
        "\u{1f7e2}"
    } else {
        "\u{1f534}"
    };
    let trading = match state.trading {
        Some(true) => "\u{25b6}\u{fe0f}",
        Some(false) => "\u{23f8}\u{fe0f}",
        None => "\u{2754}",
    };
    format!("{link}{trading}")
}

/// A switch's state in the chat's words.
fn on_off(value: Option<bool>) -> String {
    match value {
        Some(true) => t!("telegram.settings.on"),
        Some(false) => t!("telegram.settings.off"),
        None => t!("telegram.control.unknown"),
    }
    .to_string()
}

/// One core's card: its link, switches and open positions, and its commands.
fn core_card(host: &dyn TgHost, core: CoreId, said: Option<String>) -> Rendered {
    let state = host.session().core_run_state(core);
    let positions = control::open_position_markets(host, core, false).len();
    let mut lines: Vec<String> = said.into_iter().collect();
    lines.extend(light_station());
    lines.push(match state.online {
        true => format!("\u{1f7e2} {}", t!("telegram.control.online")),
        false => format!("\u{1f534} {}", t!("telegram.control.offline")),
    });
    // A switch sent and not yet reported shows where it is going, and its button only shows the
    // card again: a second send would race the first.
    let trading_wait = wait::run_waiting(host, core, RunSwitch::Trading);
    let detect_wait = wait::run_waiting(host, core, RunSwitch::AutoDetect);
    let face = |waiting: Option<bool>, now: Option<bool>| match waiting {
        Some(on) => format!("\u{23f3} {}", on_off(Some(on))),
        None => on_off(now),
    };
    lines.push(format!(
        "{}: {}",
        t!("telegram.control.trading"),
        face(trading_wait, state.trading)
    ));
    lines.push(format!(
        "{}: {}",
        t!("telegram.control.autodetect"),
        face(detect_wait, state.auto_detect)
    ));
    lines.push(t!("telegram.control.positions", n = positions).to_string());
    if let Some((on, coins)) = control::core_blacklist_state(host, core) {
        lines.push(format!(
            "{}: {} \u{00b7} {}",
            t!("telegram.control.blacklist"),
            on_off(Some(on)),
            t!("telegram.control.coins", n = coins.len())
        ));
    }
    let target = ControlTarget::Core(core);
    let run = |switch: ControlSwitch, on: bool| ControlAction::Run {
        target,
        switch,
        on,
        confirmed: false,
    };
    let mut trading_row = Vec::new();
    if let Some(on) = trading_wait {
        let label = match on {
            true => t!("telegram.control.start"),
            false => t!("telegram.control.stop"),
        };
        trading_row.push(button(
            format!("\u{23f3} {label}"),
            ControlAction::Core(core),
        ));
    } else {
        if state.trading != Some(true) {
            trading_row.push(button(
                format!("\u{25b6}\u{fe0f} {}", t!("telegram.control.start")),
                run(ControlSwitch::Trading, true),
            ));
        }
        if state.trading != Some(false) {
            trading_row.push(button(
                format!("\u{23f8}\u{fe0f} {}", t!("telegram.control.stop")),
                run(ControlSwitch::Trading, false),
            ));
        }
    }
    // AutoDetect is one button that flips the state it shows: waiting, or with no state reported
    // yet, it only shows the card again rather than guess which way to send.
    let detect = t!("telegram.control.autodetect");
    let detect_button = match (detect_wait, state.auto_detect) {
        (Some(_), _) => button(format!("\u{23f3} {detect}"), ControlAction::Core(core)),
        (None, None) => button(format!("\u{2754} {detect}"), ControlAction::Core(core)),
        (None, Some(on)) => button(
            format!("{} {detect}", if on { "\u{2705}" } else { "\u{2b1c}" }),
            run(ControlSwitch::AutoDetect, !on),
        ),
    };
    let rows = vec![
        trading_row,
        vec![detect_button],
        vec![
            button(
                format!("\u{1f9ef} {}", t!("telegram.control.panic")),
                ControlAction::PanicAll {
                    target,
                    confirmed: false,
                },
            ),
            button(
                format!("\u{1f5d1} {}", t!("telegram.control.cancel_all")),
                ControlAction::CancelAll {
                    core,
                    confirmed: false,
                },
            ),
        ],
        vec![
            button(
                format!("\u{1f4cb} {} ({positions})", t!("telegram.control.orders")),
                ControlAction::Orders { core, page: 0 },
            ),
            button(
                format!("\u{1f9e9} {}", t!("telegram.control.strategies")),
                ControlAction::Strategies { core, page: 0 },
            ),
        ],
        vec![
            button(
                format!("\u{26d4} {}", t!("telegram.control.blacklist_add")),
                ControlAction::AskCoin { core, lift: false },
            ),
            button(
                format!("\u{2705} {}", t!("telegram.control.blacklist_remove")),
                ControlAction::AskCoin { core, lift: true },
            ),
        ],
        vec![button(
            format!("\u{1f504} {}", t!("telegram.control.reconnect")),
            ControlAction::Reconnect(core),
        )],
        vec![button(
            format!("\u{2b05}\u{fe0f} {}", t!("telegram.button_control")),
            ControlAction::Cores(0),
        )],
    ];
    (name(host, core), lines, rows)
}

/// One core's strategies, `page` of them, in the core's order: each a button that checks or
/// unchecks it, marked while the core has not confirmed the change.
fn strategies(host: &dyn TgHost, core: CoreId, page: usize, said: Option<String>) -> Rendered {
    let all: Vec<(u64, String, bool)> = host
        .session()
        .store()
        .core(core)
        .map(|data| {
            data.strategies
                .iter()
                .map(|row| (row.id, row.name.clone(), row.checked))
                .collect()
        })
        .unwrap_or_default();
    let pages = all.len().div_ceil(STRATEGY_PAGE).max(1);
    let page = page.min(pages - 1);
    let checked = all.iter().filter(|(_, _, on)| *on).count();
    let mut lines: Vec<String> = said.into_iter().collect();
    match all.is_empty() {
        true => lines.push(t!("telegram.control.no_strategies").to_string()),
        false => {
            lines.push(
                t!(
                    "telegram.control.strategies_count",
                    on = checked,
                    n = all.len()
                )
                .to_string(),
            );
            lines.push(format!(
                "\u{23f3} \u{2014} {}",
                t!("telegram.control.strategy_waiting")
            ));
        }
    }
    let page16 = u16::try_from(page).unwrap_or(u16::MAX);
    let mut rows: Vec<Vec<InlineKeyboardButton>> = all
        .iter()
        .skip(page * STRATEGY_PAGE)
        .take(STRATEGY_PAGE)
        .map(|(id, strategy, on)| {
            // A row still waiting for the core only shows the page again: its mark may already
            // be the asked state, and a toggle from it would send the switch back.
            if control::strategy_waiting(host, core, *id) {
                return vec![button(
                    format!("\u{23f3} {strategy}"),
                    ControlAction::Strategies { core, page: page16 },
                )];
            }
            let mark = if *on { "\u{2705}" } else { "\u{2b1c}" };
            vec![button(
                format!("{mark} {strategy}"),
                ControlAction::StrategyToggle {
                    core,
                    id: *id,
                    on: !on,
                    page: page16,
                },
            )]
        })
        .collect();
    if pages > 1 {
        let to = |page: usize| ControlAction::Strategies {
            core,
            page: u16::try_from(page).unwrap_or(u16::MAX),
        };
        let mut nav = Vec::new();
        if page > 0 {
            nav.push(button("\u{25c0}".into(), to(page - 1)));
        }
        nav.push(InlineKeyboardButton::callback(
            format!("{}/{pages}", page + 1),
            MenuAction::Noop.callback(),
        ));
        if page + 1 < pages {
            nav.push(button("\u{25b6}".into(), to(page + 1)));
        }
        rows.push(nav);
    }
    rows.push(vec![button(
        format!("\u{2b05}\u{fe0f} {}", name(host, core)),
        ControlAction::Core(core),
    )]);
    (t!("telegram.control.strategies").to_string(), lines, rows)
}

/// One core's open positions, `page` of them, newest first.
fn orders(host: &dyn TgHost, core: CoreId, page: usize, said: Option<String>) -> Rendered {
    let all = control::open_orders(host, core);
    let pages = all.len().div_ceil(PAGE).max(1);
    let page = page.min(pages - 1);
    let mut lines: Vec<String> = said.into_iter().collect();
    lines.push(
        match all.is_empty() {
            true => t!("telegram.control.no_orders"),
            false => t!("telegram.control.pick_order"),
        }
        .to_string(),
    );
    let mut rows: Vec<Vec<InlineKeyboardButton>> = all
        .iter()
        .skip(page * PAGE)
        .take(PAGE)
        .map(|order| {
            vec![button(
                order_caption(order),
                ControlAction::Order {
                    core,
                    uid: order.uid,
                },
            )]
        })
        .collect();
    if pages > 1 {
        let to = |page: usize| ControlAction::Orders {
            core,
            page: u16::try_from(page).unwrap_or(u16::MAX),
        };
        let mut nav = Vec::new();
        if page > 0 {
            nav.push(button("\u{25c0}".into(), to(page - 1)));
        }
        nav.push(InlineKeyboardButton::callback(
            format!("{}/{pages}", page + 1),
            MenuAction::Noop.callback(),
        ));
        if page + 1 < pages {
            nav.push(button("\u{25b6}".into(), to(page + 1)));
        }
        rows.push(nav);
    }
    rows.push(vec![button(
        format!("\u{2b05}\u{fe0f} {}", name(host, core)),
        ControlAction::Core(core),
    )]);
    (t!("telegram.control.orders").to_string(), lines, rows)
}

/// An open position as its list button shows it: coin, side, strategy, emulator mark.
fn order_caption(order: &moon_core::feed::OrderRow) -> String {
    let side = if order.is_short { "S" } else { "L" };
    let strategy = if order.strat_name.is_empty() {
        order.strat.as_str()
    } else {
        order.strat_name.as_str()
    };
    let emulator = if order.emulator { " (E)" } else { "" };
    format!("{} {side} \u{00b7} {strategy}{emulator}", order.coin)
}

/// One open position's card: what it is and where it stands, and its commands. A position that
/// closed meanwhile says so and leads back to the list.
fn order_card(host: &dyn TgHost, core: CoreId, uid: u64, said: Option<String>) -> Rendered {
    let mut lines: Vec<String> = said.into_iter().collect();
    let back = vec![button(
        format!("\u{2b05}\u{fe0f} {}", t!("telegram.control.orders")),
        ControlAction::Orders { core, page: 0 },
    )];
    let Some(order) = control::open_order(host, core, uid) else {
        lines.push(t!("telegram.control.order_gone").to_string());
        return (t!("telegram.control.orders").to_string(), lines, vec![back]);
    };
    let strategy = if order.strat_name.is_empty() {
        order.strat.clone()
    } else {
        order.strat_name.clone()
    };
    lines.push(format!(
        "{} \u{00b7} {} \u{00b7} {}",
        order.market_display,
        if order.is_short { "Short" } else { "Long" },
        strategy
    ));
    lines.push(
        t!(
            "telegram.control.order_prices",
            buy = fmt::compact(order.buy_price, 8),
            sell = fmt::compact(order.sell_price, 8),
            size = fmt::compact(order.size, 8)
        )
        .to_string(),
    );
    if order.emulator {
        lines.push(t!("telegram.notify_emulator").to_string());
    }
    if host.is_panic_armed(core, &order.market) {
        lines.push(t!("telegram.control.panic_armed").to_string());
    }
    let ban = |ban: OrderBan| ControlAction::OrderBan { core, uid, ban };
    let mut ban_row = vec![button(
        format!("\u{26d4} {}", t!("telegram.control.ban_core")),
        ban(OrderBan::Core),
    )];
    if order.strat_id != 0 && host.session().strategy_has_blacklist(core, order.strat_id) {
        ban_row.push(button(
            format!("\u{26d4} {}", t!("telegram.control.ban_strategy")),
            ban(OrderBan::Strategy),
        ));
    }
    let temp_row = TempBanSpan::ALL
        .into_iter()
        .map(|span| {
            button(
                format!(
                    "\u{23f3} {}",
                    t!("telegram.control.hours", hours = span.hours())
                ),
                ban(OrderBan::Temp(span)),
            )
        })
        .collect();
    let rows = vec![
        vec![button(
            format!("\u{1f9ef} {}", t!("telegram.control.order_panic")),
            ControlAction::OrderPanic {
                core,
                uid,
                confirmed: false,
            },
        )],
        ban_row,
        temp_row,
        back,
    ];
    (
        format!("{} \u{00b7} {}", order.coin, name(host, core)),
        lines,
        rows,
    )
}

/// The all-cores card: how many are there, connected and trading, and the commands for all.
fn all_card(host: &dyn TgHost, said: Option<String>) -> Rendered {
    let cores = control::owner_cores(host);
    let states: Vec<_> = cores
        .iter()
        .map(|(id, _)| host.session().core_run_state(*id))
        .collect();
    let online = states.iter().filter(|state| state.online).count();
    let trading = states
        .iter()
        .filter(|state| state.trading == Some(true))
        .count();
    let mut lines: Vec<String> = said.into_iter().collect();
    lines.extend(light_station());
    lines.push(
        t!(
            "telegram.control.cores_count",
            n = cores.len(),
            online = online,
            trading = trading
        )
        .to_string(),
    );
    let run = |on: bool| ControlAction::Run {
        target: ControlTarget::All,
        switch: ControlSwitch::Trading,
        on,
        confirmed: false,
    };
    let rows = vec![
        vec![
            button(
                format!("\u{25b6}\u{fe0f} {}", t!("telegram.control.start_all")),
                run(true),
            ),
            button(
                format!("\u{23f8}\u{fe0f} {}", t!("telegram.control.stop_all")),
                run(false),
            ),
        ],
        vec![button(
            format!("\u{1f9ef} {}", t!("telegram.control.panic")),
            ControlAction::PanicAll {
                target: ControlTarget::All,
                confirmed: false,
            },
        )],
        vec![button(
            format!("\u{2b05}\u{fe0f} {}", t!("telegram.button_control")),
            ControlAction::Cores(0),
        )],
    ];
    (t!("telegram.control.all").to_string(), lines, rows)
}

mod wait;

pub(crate) use wait::{Redraws, tick};

#[cfg(test)]
mod tests;
