//! The bot's Control section, the owner's: commands to the cores from the chat.
//!
//! Every screen is one message under which the buttons sit, edited in place as the owner moves
//! through it: the cores a page at a time, one core's card, the all-cores card. A command runs
//! through the shared command module ([`crate::control`]) — the same owner check, choice of cores
//! and session calls as the Mini App's — and its card comes back with what happened on top. What
//! can lose money or stop trading everywhere asks first: the press shows a confirmation whose
//! button sends the same action confirmed.

use std::sync::mpsc::SyncSender;

use moon_core::session::{CoreId, RunSwitch};
use moon_core::telegram::api::{InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup};
use moon_core::telegram::menu_action::{ControlAction, ControlSwitch, ControlTarget, MenuAction};
use moon_core::telegram::runtime::Response;
use rust_i18n::t;

use crate::TgHost;
use crate::control::{self, Refusal};
use crate::html::escape;
use crate::labels::navigation_keyboard;

/// Cores on one page of the list.
const PAGE: usize = 8;

/// A screen: its title, its lines, its buttons.
type Rendered = (String, Vec<String>, Vec<Vec<InlineKeyboardButton>>);

/// Answer a Control button: run its command, if any, then show the screen it leads to.
pub(super) fn run(
    host: &mut dyn TgHost,
    chat: i64,
    action: ControlAction,
    reply: &SyncSender<Response>,
) {
    let (title, lines, rows) = screen(host, chat, action);
    let navigation = navigation_keyboard(host.kind(), true, &host.config().telegram);
    let mut html = format!("<p><b>{}</b></p>", escape(&title));
    for line in lines {
        html.push_str(&format!("<p>{}</p>", escape(&line)));
    }
    let _ = reply.try_send(Response::Rich {
        html,
        keyboard: ReplyMarkup::Inline(InlineKeyboardMarkup::from_rows(rows)),
        navigation: (
            t!("telegram.report_navigation_hint").to_string(),
            navigation,
        ),
    });
}

/// The screen for `action`, after running its command.
fn screen(host: &mut dyn TgHost, chat: i64, action: ControlAction) -> Rendered {
    use ControlAction as A;
    match action {
        A::Cores(page) => cores(host, usize::from(page), None),
        A::Core(core) => core_card(host, core, None),
        A::All => all_card(host, None),
        A::Run {
            target,
            switch,
            on,
            confirmed,
        } => {
            if target == ControlTarget::All && !confirmed {
                let count = control::owner_cores(host).len();
                let question = match on {
                    true => t!("telegram.control.confirm_start_all", n = count),
                    false => t!("telegram.control.confirm_stop_all", n = count),
                };
                return confirm(question.to_string(), action, ControlAction::All);
            }
            let run_switch = match switch {
                ControlSwitch::Trading => RunSwitch::Trading,
                ControlSwitch::AutoDetect => RunSwitch::AutoDetect,
            };
            match target {
                ControlTarget::Core(core) => {
                    let said = said(control::run_one(host, chat, core, run_switch, on));
                    core_card(host, core, Some(said))
                }
                ControlTarget::All => {
                    let cores: Vec<CoreId> = control::owner_cores(host)
                        .into_iter()
                        .map(|(id, _)| id)
                        .collect();
                    let said = match control::run_many(host, chat, &cores, run_switch, on) {
                        Ok((targets, outcome)) => t!(
                            "telegram.control.scope",
                            sent = outcome.sent.len(),
                            n = targets.len(),
                            already = outcome.already,
                            offline = outcome.offline
                        )
                        .to_string(),
                        Err(refusal) => refusal_text(refusal),
                    };
                    all_card(host, Some(said))
                }
            }
        }
        A::CancelAll { core, confirmed } => {
            if !confirmed {
                let question = t!(
                    "telegram.control.confirm_cancel_all",
                    core = name(host, core)
                );
                return confirm(question.to_string(), action, ControlAction::Core(core));
            }
            let said = said(control::cancel_all(host, chat, core));
            core_card(host, core, Some(said))
        }
        A::PanicAll { target, confirmed } => {
            if !confirmed {
                let (question, back) = match target {
                    ControlTarget::Core(core) => (
                        t!("telegram.control.confirm_panic", core = name(host, core)),
                        ControlAction::Core(core),
                    ),
                    ControlTarget::All => {
                        (t!("telegram.control.confirm_panic_all"), ControlAction::All)
                    }
                };
                return confirm(question.to_string(), action, back);
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
    }
}

/// What a single-core command came to, in the chat's words.
fn said(result: Result<(), Refusal>) -> String {
    match result {
        Ok(()) => t!("telegram.control.sent").to_string(),
        Err(refusal) => refusal_text(refusal),
    }
}

/// A refusal in the chat's words.
fn refusal_text(refusal: Refusal) -> String {
    match refusal {
        Refusal::NotOwner => t!("telegram.refusal"),
        Refusal::NotFound => t!("telegram.mini_cmd_core_not_found"),
        Refusal::Offline => t!("telegram.mini_cmd_offline"),
        Refusal::Unavailable => t!("telegram.control.not_sent"),
    }
    .to_string()
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
    let confirmed = match action {
        ControlAction::Run {
            target, switch, on, ..
        } => ControlAction::Run {
            target,
            switch,
            on,
            confirmed: true,
        },
        ControlAction::CancelAll { core, .. } => ControlAction::CancelAll {
            core,
            confirmed: true,
        },
        ControlAction::PanicAll { target, .. } => ControlAction::PanicAll {
            target,
            confirmed: true,
        },
        other => other,
    };
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
    let positions = control::open_position_markets(host, core).len();
    let mut lines: Vec<String> = said.into_iter().collect();
    lines.push(match state.online {
        true => format!("\u{1f7e2} {}", t!("telegram.control.online")),
        false => format!("\u{1f534} {}", t!("telegram.control.offline")),
    });
    lines.push(format!(
        "{}: {}",
        t!("telegram.control.trading"),
        on_off(state.trading)
    ));
    lines.push(format!(
        "{}: {}",
        t!("telegram.control.autodetect"),
        on_off(state.auto_detect)
    ));
    lines.push(t!("telegram.control.positions", n = positions).to_string());
    let target = ControlTarget::Core(core);
    let run = |switch: ControlSwitch, on: bool| ControlAction::Run {
        target,
        switch,
        on,
        confirmed: false,
    };
    let mut trading_row = Vec::new();
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
    let detect_on = state.auto_detect == Some(true);
    let rows = vec![
        trading_row,
        vec![button(
            format!(
                "{} {}",
                if detect_on { "\u{2705}" } else { "\u{2b1c}" },
                t!("telegram.control.autodetect")
            ),
            run(ControlSwitch::AutoDetect, !detect_on),
        )],
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

#[cfg(test)]
mod tests;
