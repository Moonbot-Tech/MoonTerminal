//! The bot's Settings section, the owner's: the buttons of the menu, the report view and basis,
//! the Mini App, this chat's notifications, and the station's status.
//!
//! Every screen is one message under which the buttons sit; a switch saves at once (through the
//! host, so a terminal keeps its open Settings draft) and shows its screen again with the new
//! state. Order and rows of the buttons are edited in the terminal's Settings; numeric thresholds
//! and the cores of trade notices in the Mini App's Settings tab.

use std::sync::mpsc::SyncSender;

use moon_core::config::telegram_menu::{BotSettings, MenuItem, MenuLevel, ReportBasis, ReportView};
use moon_core::telegram::api::{InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup};
use moon_core::telegram::menu_action::{MenuAction, SettingsAction};
use moon_core::telegram::notify::{CoreScope, NotifySettings};
use moon_core::telegram::runtime::Response;
use rust_i18n::t;

use crate::html::escape;
use crate::labels::{button_text, navigation_keyboard};
use crate::{HostKind, TgHost};

/// Minutes a core may stay down before the notice, offered as presets.
const DOWN_PRESETS: [u16; 5] = [1, 5, 15, 30, 60];

/// Answer a Settings button: apply its switch, if any, then show its screen.
pub(super) fn run(
    host: &mut dyn TgHost,
    chat: i64,
    action: SettingsAction,
    reply: &SyncSender<Response>,
) {
    let saved = apply(host, chat, action);
    let screen = match saved {
        Err(text) => {
            crate::labels::answer(reply, text);
            return;
        }
        Ok(screen) => screen,
    };
    let rows = match screen {
        // The station's own answer, which leads back here; a terminal has none to give.
        Screen::Status => {
            if !host.station_status(reply.clone(), true) {
                crate::labels::answer(reply, crate::labels::report_help(host.kind()));
            }
            return;
        }
        Screen::Root => root(host, crate::mini_app::chat_notify(host, chat).is_some()),
        Screen::Buttons => buttons(&host.config().telegram.bot, host.kind()),
        Screen::View => view(host.config().telegram.bot.report_view),
        Screen::Basis => basis(host.config().telegram.bot.period_basis),
        Screen::Notify | Screen::DailyHours => match crate::mini_app::chat_notify(host, chat) {
            Some(notify) if screen == Screen::Notify => notify_screen(&notify, host.report_zone()),
            Some(notify) => daily_hours(notify.daily.hour),
            None => {
                crate::labels::answer(reply, t!("telegram.mini_settings_err_save").to_string());
                return;
            }
        },
    };
    let (title, lines, keyboard) = rows;
    // The keyboard as the change left it: a button hidden here leaves the chat's keyboard now.
    let navigation = navigation_keyboard(host.kind(), true, &host.config().telegram);
    let mut html = format!("<p><b>{}</b></p>", escape(&title));
    for line in lines {
        html.push_str(&format!("<p>{}</p>", escape(&line)));
    }
    let _ = reply.try_send(Response::Rich {
        html,
        keyboard: ReplyMarkup::Inline(InlineKeyboardMarkup::from_rows(keyboard)),
        navigation: (
            t!("telegram.report_navigation_hint").to_string(),
            navigation,
        ),
    });
}

/// Which screen a button leads to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Root,
    Buttons,
    View,
    Basis,
    Notify,
    DailyHours,
    /// The station's status, answered by the station.
    Status,
}

/// A screen: its title, its lines, its buttons.
type Rendered = (String, Vec<String>, Vec<Vec<InlineKeyboardButton>>);

/// Apply `action`'s switch, saved, and name the screen to show.
///
/// Returns:
///     The screen; the chat's words when the save failed or the switch is not this host's.
fn apply(host: &mut dyn TgHost, chat: i64, action: SettingsAction) -> Result<Screen, String> {
    use SettingsAction as A;
    let bot = |host: &mut dyn TgHost, edit: &dyn Fn(&mut BotSettings) -> bool| {
        let mut bot = host.config().telegram.bot.clone();
        if !edit(&mut bot) || host.save_bot_settings(bot) {
            Ok(())
        } else {
            Err(t!("telegram.settings.not_saved").to_string())
        }
    };
    let notify = |host: &dyn TgHost, edit: &dyn Fn(&mut NotifySettings)| {
        crate::mini_app::save_chat_notify(host, chat, edit).map(|_| ())
    };
    Ok(match action {
        A::Root => Screen::Root,
        A::Buttons => Screen::Buttons,
        A::ShowButton(level, item, show) => {
            // The Settings button is how the chat reaches this; it stays on the keyboard here.
            if item != MenuItem::Settings {
                bot(host, &|bot| bot.menu.set_shown(level, item, show))?;
            }
            Screen::Buttons
        }
        A::View => Screen::View,
        A::SetView(view) => {
            bot(host, &|bot| {
                std::mem::replace(&mut bot.report_view, view) != view
            })?;
            Screen::Root
        }
        A::Basis => Screen::Basis,
        A::SetBasis(basis) => {
            bot(host, &|bot| {
                std::mem::replace(&mut bot.period_basis, basis) != basis
            })?;
            Screen::Root
        }
        A::MiniApp(on) => {
            if host.config().telegram.mini_app_enabled == on {
                return Ok(Screen::Root);
            }
            match host.save_mini_app(on) {
                Some(true) => Screen::Root,
                Some(false) => return Err(t!("telegram.settings.not_saved").to_string()),
                None => return Err(t!("telegram.settings.mini_app_station").to_string()),
            }
        }
        A::Notify => Screen::Notify,
        A::Trades(on) => {
            notify(host, &|n| n.trades.on = on)?;
            Screen::Notify
        }
        A::Down(on) => {
            notify(host, &|n| n.down.on = on)?;
            Screen::Notify
        }
        A::DownAfter(minutes) => {
            notify(host, &|n| n.down.after_minutes = minutes)?;
            Screen::Notify
        }
        A::Daily(on) => {
            notify(host, &|n| n.daily.on = on)?;
            Screen::Notify
        }
        A::DailyHours => Screen::DailyHours,
        A::StationStatus => return Ok(Screen::Status),
        A::DailyHour(hour) => {
            notify(host, &|n| n.daily.hour = hour)?;
            Screen::Notify
        }
    })
}

/// A button for a Settings action.
fn button(text: impl Into<String>, action: SettingsAction) -> InlineKeyboardButton {
    InlineKeyboardButton::callback(text, MenuAction::Settings(action).callback())
}

/// A caption that does nothing when pressed.
fn caption(text: impl Into<String>) -> InlineKeyboardButton {
    InlineKeyboardButton::callback(text, MenuAction::Noop.callback())
}

/// The way back to `to`.
fn back(to: SettingsAction) -> Vec<InlineKeyboardButton> {
    vec![button(
        format!("\u{2b05}\u{fe0f} {}", t!("telegram.menu.back")),
        to,
    )]
}

/// On or off, as a line says it.
fn on_off(on: bool) -> String {
    match on {
        true => t!("telegram.settings.on"),
        false => t!("telegram.settings.off"),
    }
    .to_string()
}

/// A switch's mark.
fn tick(on: bool) -> &'static str {
    if on { "\u{2705}" } else { "\u{2b1c}" }
}

/// A report view's words.
fn view_words(view: ReportView) -> String {
    match view {
        ReportView::Exchanges => t!("telegram.menu_editor.view_exchanges"),
        ReportView::Cores => t!("telegram.menu_editor.view_cores"),
        ReportView::Days => t!("telegram.menu_editor.view_days"),
    }
    .to_string()
}

/// A period basis's words, the terminal Report's own.
fn basis_words(basis: ReportBasis) -> String {
    match basis {
        ReportBasis::Close => t!("report.period_basis.close"),
        ReportBasis::Open => t!("report.period_basis.open"),
    }
    .to_string()
}

/// The section itself; its notifications only while the bot has a notifications store.
fn root(host: &dyn TgHost, notify: bool) -> Rendered {
    let telegram = &host.config().telegram;
    let station = host.kind() == HostKind::Station;
    let mut lines = vec![
        format!(
            "{}: {}",
            t!("telegram.menu_editor.view"),
            view_words(telegram.bot.report_view)
        ),
        format!(
            "{}: {}",
            t!("telegram.menu_editor.basis"),
            basis_words(telegram.bot.period_basis)
        ),
        format!(
            "Mini App: {}",
            on_off(telegram.mini_app_enabled).to_lowercase()
        ),
    ];
    if station {
        lines.push(t!("telegram.settings.mini_app_station").to_string());
    }
    lines.push(t!("telegram.settings.root_hint").to_string());
    let mut rows = vec![
        vec![button(
            format!("\u{1f518} {}", t!("telegram.settings.buttons")),
            SettingsAction::Buttons,
        )],
        vec![
            button(
                format!("\u{1f4ca} {}", t!("telegram.settings.view")),
                SettingsAction::View,
            ),
            button(
                format!("\u{1f552} {}", t!("telegram.settings.basis")),
                SettingsAction::Basis,
            ),
        ],
    ];
    if !station {
        rows.push(vec![button(
            format!("{} Mini App", tick(telegram.mini_app_enabled)),
            SettingsAction::MiniApp(!telegram.mini_app_enabled),
        )]);
    }
    if notify {
        rows.push(vec![button(
            format!("\u{1f514} {}", t!("telegram.settings.notify")),
            SettingsAction::Notify,
        )]);
    }
    if station {
        rows.push(vec![button(
            format!("\u{1f4e1} {}", t!("telegram.settings.station_status")),
            SettingsAction::StationStatus,
        )]);
    }
    (
        format!("\u{2699}\u{fe0f} {}", t!("telegram.button_settings")),
        lines,
        rows,
    )
}

/// The buttons a chat can show or hide, as ticks: the keyboard's, then the Report section's.
/// Settings itself is not offered (it is how the chat got here), nor Status on a terminal.
fn buttons(bot: &BotSettings, host: HostKind) -> Rendered {
    let locale = rust_i18n::locale();
    let mut rows = Vec::new();
    for (level, title) in [
        (MenuLevel::Keyboard, t!("telegram.menu_editor.keyboard")),
        (MenuLevel::Report, t!("telegram.menu_editor.report")),
    ] {
        rows.push(vec![caption(title.to_string())]);
        let entries: Vec<_> = bot
            .menu
            .rows(level)
            .iter()
            .flatten()
            .filter(|entry| entry.item != MenuItem::Settings)
            .filter(|entry| entry.item != MenuItem::Status || host == HostKind::Station)
            .copied()
            .collect();
        for pair in entries.chunks(2) {
            rows.push(
                pair.iter()
                    .map(|entry| {
                        button(
                            format!(
                                "{} {}",
                                tick(entry.show),
                                button_text(entry.item, locale.as_ref())
                            ),
                            SettingsAction::ShowButton(level, entry.item, !entry.show),
                        )
                    })
                    .collect(),
            );
        }
    }
    rows.push(back(SettingsAction::Root));
    (
        t!("telegram.settings.buttons").to_string(),
        vec![t!("telegram.settings.buttons_hint").to_string()],
        rows,
    )
}

/// The report views, the current one marked.
fn view(current: ReportView) -> Rendered {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = ReportView::ALL
        .into_iter()
        .map(|view| {
            vec![button(
                format!("{} {}", mark(view == current), view_words(view)),
                SettingsAction::SetView(view),
            )]
        })
        .collect();
    rows.push(back(SettingsAction::Root));
    (
        t!("telegram.settings.view").to_string(),
        vec![t!("telegram.settings.view_hint").to_string()],
        rows,
    )
}

/// The period bases, the current one marked.
fn basis(current: ReportBasis) -> Rendered {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = ReportBasis::ALL
        .into_iter()
        .map(|basis| {
            vec![button(
                format!("{} {}", mark(basis == current), basis_words(basis)),
                SettingsAction::SetBasis(basis),
            )]
        })
        .collect();
    rows.push(back(SettingsAction::Root));
    (
        t!("telegram.settings.basis").to_string(),
        vec![t!("telegram.settings.basis_hint").to_string()],
        rows,
    )
}

/// The current choice's mark.
fn mark(current: bool) -> &'static str {
    if current { "\u{1f518}" } else { "\u{26aa}" }
}

/// This chat's notifications: three switches, the down delay, the summary's hour.
fn notify_screen(notify: &NotifySettings, zone: chrono_tz::Tz) -> Rendered {
    let trades = &notify.trades;
    let mut filters = Vec::new();
    if matches!(trades.cores, CoreScope::Only(_)) {
        filters.push(t!("telegram.settings.trades_some_cores").to_string());
    }
    for (key, value) in [
        ("telegram.mini_settings_min_volume", trades.min_volume_usd),
        (
            "telegram.mini_settings_profit_at_least",
            trades.profit_at_least_usd,
        ),
        (
            "telegram.mini_settings_loss_at_least",
            trades.loss_at_least_usd,
        ),
    ] {
        if let Some(value) = value {
            filters.push(format!("{} {value}$", t!(key)));
        }
    }
    let trades_line = match filters.is_empty() {
        true => on_off(trades.on),
        false => format!("{} ({})", on_off(trades.on), filters.join(", ")),
    };
    let lines = vec![
        format!("{}: {trades_line}", t!("telegram.mini_settings_trades")),
        format!(
            "{}: {}",
            t!("telegram.mini_settings_down"),
            match notify.down.on {
                true => t!(
                    "telegram.settings.down_after",
                    minutes = notify.down.after_minutes
                )
                .to_string(),
                false => on_off(false),
            }
        ),
        format!(
            "{}: {}",
            t!("telegram.mini_settings_daily"),
            match notify.daily.on {
                true => format!(
                    "{:02}:{:02} ({})",
                    notify.daily.hour, notify.daily.minute, zone
                ),
                false => on_off(false),
            }
        ),
        t!("telegram.settings.notify_hint").to_string(),
    ];
    let mut rows = vec![
        vec![button(
            format!(
                "{} {}",
                tick(trades.on),
                t!("telegram.mini_settings_trades")
            ),
            SettingsAction::Trades(!trades.on),
        )],
        vec![button(
            format!(
                "{} {}",
                tick(notify.down.on),
                t!("telegram.mini_settings_down")
            ),
            SettingsAction::Down(!notify.down.on),
        )],
        DOWN_PRESETS
            .into_iter()
            .map(|minutes| {
                let current = minutes == notify.down.after_minutes;
                button(
                    t!("telegram.settings.minutes", minutes = minutes).to_string()
                        + if current { " \u{2022}" } else { "" },
                    SettingsAction::DownAfter(minutes),
                )
            })
            .collect(),
        vec![
            button(
                format!(
                    "{} {}",
                    tick(notify.daily.on),
                    t!("telegram.mini_settings_daily")
                ),
                SettingsAction::Daily(!notify.daily.on),
            ),
            button(
                format!(
                    "\u{1f552} {:02}:{:02}",
                    notify.daily.hour, notify.daily.minute
                ),
                SettingsAction::DailyHours,
            ),
        ],
    ];
    rows.push(back(SettingsAction::Root));
    (
        format!("\u{1f514} {}", t!("telegram.settings.notify")),
        lines,
        rows,
    )
}

/// The hours of the day for the summary, the current one marked; minutes stay as they are.
fn daily_hours(current: u8) -> Rendered {
    let hours: Vec<u8> = (0..24).collect();
    let mut rows: Vec<Vec<InlineKeyboardButton>> = hours
        .chunks(6)
        .map(|chunk| {
            chunk
                .iter()
                .map(|&hour| {
                    let text = match hour == current {
                        true => format!("\u{2022}{hour:02}"),
                        false => format!("{hour:02}"),
                    };
                    button(text, SettingsAction::DailyHour(hour))
                })
                .collect()
        })
        .collect();
    rows.push(back(SettingsAction::Notify));
    (
        t!("telegram.mini_settings_daily").to_string(),
        vec![t!("telegram.settings.daily_hours_hint").to_string()],
        rows,
    )
}

#[cfg(test)]
mod tests;
