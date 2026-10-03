//! The bot's inline section menus: the Report section and its custom-period screen.
//!
//! A section is one message with an inline keyboard; pressing a button edits that message in place
//! (the transport edits the message a callback came from), so the section turns into the report it
//! asked for. Which buttons a section shows and in which rows is the bot's configured menu
//! (`TelegramConfig::bot`); a button that opens a report carries the report's own callback, in the
//! bot's default view.

use std::sync::mpsc::SyncSender;

use chrono::NaiveDate;
use moon_core::config::telegram_menu::{MenuItem, MenuLevel, ReportView};
use moon_core::telegram::api::{InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup};
use moon_core::telegram::menu_action::MenuAction;
use moon_core::telegram::report::{Period, ReportRequest};
use moon_core::telegram::runtime::Response;
use moon_core::util::display_time;
use rust_i18n::t;

use crate::TgHost;
use crate::html::escape;
use crate::labels::{button_text, navigation_keyboard};

mod calendar;
mod settings;

/// Answer a section button or one of its screens.
///
/// Args:
///     owner: Whether the chat is the owner, for its navigation keyboard.
pub(crate) fn run(
    host: &mut dyn TgHost,
    chat: i64,
    action: MenuAction,
    owner: bool,
    reply: SyncSender<Response>,
) {
    let telegram = &host.config().telegram;
    let view = telegram.bot.report_view;
    let navigation = navigation_keyboard(host.kind(), owner, telegram);
    let today = today(host);
    match action {
        MenuAction::Report => {
            let rows = report_rows(&telegram.bot.menu, view);
            let _ = reply.try_send(section(
                t!("telegram.button_report").to_string(),
                t!("telegram.menu.report_pick").to_string(),
                rows,
                navigation,
            ));
        }
        MenuAction::Custom { month, from } => {
            let Some(today) = today else {
                crate::labels::answer(&reply, t!("telegram.report_failed").to_string());
                return;
            };
            let hint = match from {
                Some(from) => t!(
                    "telegram.menu.custom_pick_to",
                    from = from.format("%d.%m.%Y").to_string()
                ),
                None => t!("telegram.menu.custom_pick_from"),
            };
            let rows = custom_rows(month, from, today, view);
            let _ = reply.try_send(section(
                t!("telegram.button_custom").to_string(),
                hint.to_string(),
                rows,
                navigation,
            ));
        }
        MenuAction::Preset(preset) => {
            let request = today
                .and_then(|today| preset.dates(today))
                .and_then(|(from, to)| ReportRequest::span(from, to));
            match request {
                Some(request) => crate::report::telegram_report(host, chat, request, reply),
                None => crate::labels::answer(&reply, t!("telegram.report_failed").to_string()),
            }
        }
        // A cell that does nothing: no answer, the screen stays as it is.
        MenuAction::Noop => {}
        MenuAction::Settings(action) => settings::run(host, chat, action, &reply),
    }
}

/// Today in the zone reports are cut in.
fn today(host: &dyn TgHost) -> Option<NaiveDate> {
    let now = i64::try_from(moon_core::util::time::now_unix_secs()).ok()?;
    display_time::at(now, host.report_zone()).map(|at| at.date_naive())
}

/// A section message: its title and hint above its inline buttons.
fn section(
    title: String,
    hint: String,
    rows: Vec<Vec<InlineKeyboardButton>>,
    navigation: ReplyMarkup,
) -> Response {
    Response::Rich {
        html: format!("<p><b>{}</b></p><p>{}</p>", escape(&title), escape(&hint)),
        keyboard: ReplyMarkup::Inline(InlineKeyboardMarkup::from_rows(rows)),
        navigation: (
            t!("telegram.report_navigation_hint").to_string(),
            navigation,
        ),
    }
}

/// The Report section's buttons, as the menu lays them out; a section with nothing shown keeps
/// Today, so it never opens empty.
fn report_rows(
    menu: &moon_core::config::telegram_menu::BotMenu,
    view: ReportView,
) -> Vec<Vec<InlineKeyboardButton>> {
    let mut rows = menu.visible(MenuLevel::Report, |_| true);
    if rows.is_empty() {
        rows = vec![vec![MenuItem::Today]];
    }
    let locale = rust_i18n::locale();
    rows.into_iter()
        .map(|row| {
            row.into_iter()
                .filter_map(|item| {
                    let data = report_callback(item, view)?;
                    Some(InlineKeyboardButton::callback(
                        button_text(item, locale.as_ref()),
                        data,
                    ))
                })
                .collect::<Vec<_>>()
        })
        .filter(|row| !row.is_empty())
        .collect()
}

/// The callback of a Report section item: a report in the bot's view, or the custom screen.
fn report_callback(item: MenuItem, view: ReportView) -> Option<String> {
    let period = match item {
        MenuItem::Today => Period::Today,
        MenuItem::Yesterday => Period::Yesterday,
        MenuItem::Month => Period::Month,
        MenuItem::LastMonth => Period::LastMonth,
        MenuItem::Daily => return Some(ReportRequest::new(Period::Month, true).callback()),
        MenuItem::Custom => {
            return Some(
                MenuAction::Custom {
                    month: None,
                    from: None,
                }
                .callback(),
            );
        }
        _ => return None,
    };
    Some(ReportRequest::preset(period).in_view(view).callback())
}

/// The custom-period screen: presets, the calendar of `month`, and the way back to the section.
fn custom_rows(
    month: Option<NaiveDate>,
    from: Option<NaiveDate>,
    today: NaiveDate,
    view: ReportView,
) -> Vec<Vec<InlineKeyboardButton>> {
    use moon_core::telegram::report::Preset;
    let presets = Preset::ALL
        .into_iter()
        .map(|preset| {
            let key = match preset {
                Preset::Days7 => "telegram.menu.preset_7",
                Preset::Days30 => "telegram.menu.preset_30",
                Preset::LastWeek => "telegram.menu.preset_week",
            };
            InlineKeyboardButton::callback(
                t!(key).to_string(),
                MenuAction::Preset(preset).callback(),
            )
        })
        .collect();
    let mut rows = vec![presets];
    rows.extend(calendar::rows(month, from, today, view));
    rows.push(vec![InlineKeyboardButton::callback(
        format!("\u{2b05}\u{fe0f} {}", t!("telegram.menu.back")),
        MenuAction::Report.callback(),
    )]);
    rows
}

#[cfg(test)]
mod tests;
