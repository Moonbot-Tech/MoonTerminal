//! Navigation, labels and service status in the user's language.
//!
//! The transport never invents prose: every button, page label and status line it shows comes from
//! here, through this crate's dictionary.

use std::borrow::Cow;
use std::sync::mpsc::SyncSender;

use moon_core::telegram::TelegramStatus;
use moon_core::telegram::api::{KeyboardButton, ReplyKeyboardMarkup, ReplyMarkup};
use moon_core::telegram::runtime::Response;
use moon_core::venue::{CoreVenue, caption};
use rust_i18n::t;

/// Translate one caption key through this crate's dictionary.
fn tr(key: &str) -> Cow<'_, str> {
    t!(key)
}

/// Exchange section caption in the user's language, as every terminal core list spells it.
pub(crate) fn section_label(venue: Option<&CoreVenue>) -> String {
    caption::venue_section_label(venue, &tr)
}

/// Nonblocking localized response; a disconnected requester cannot stall the owner thread.
pub(crate) fn answer(reply: &SyncSender<Response>, text: String) {
    let _ = reply.try_send(Response::Text {
        text,
        keyboard: None,
    });
}
/// The period and command help a chat gets for a request it cannot run; what the bot depends on
/// is `host`'s.
pub(crate) fn report_help(host: crate::HostKind) -> String {
    match host {
        crate::HostKind::Terminal => t!("telegram.report_help"),
        crate::HostKind::Station => t!("telegram.report_help_station"),
    }
    .to_string()
}

/// Render service health through the Telegram locale domain.
pub fn status_text(status: &TelegramStatus) -> String {
    match status {
        TelegramStatus::Stopping => t!("telegram.stopping"),
        TelegramStatus::Disabled => t!("telegram.state.disabled"),
        TelegramStatus::Starting => t!("telegram.state.starting"),
        TelegramStatus::Unpaired | TelegramStatus::Paired { .. } => t!("telegram.state.connected"),
        TelegramStatus::RateLimited { retry_after_secs } => {
            t!("telegram.rate_limited", seconds = retry_after_secs)
        }
        TelegramStatus::Unavailable => t!("telegram.state.unavailable"),
        TelegramStatus::Conflict => t!("telegram.state.conflict"),
        TelegramStatus::Stopped => t!("telegram.stopped"),
    }
    .to_string()
}

/// Global navigation owns periods and help — and on a station its status; report actions remain
/// inline.
pub(crate) fn navigation_keyboard(host: crate::HostKind) -> ReplyMarkup {
    let locale = rust_i18n::locale();
    let button = |(name, icon): (&str, &str)| {
        let key = format!("telegram.button_{name}");
        KeyboardButton {
            text: format!("{icon} {}", t!(&key, locale = locale.as_ref())),
            style: None,
        }
    };
    let [today, yesterday, month, lastmonth, help] = navigation_buttons().map(button);
    let mut months = vec![month, lastmonth];
    if host == crate::HostKind::Station {
        months.push(button(STATION_STATUS_BUTTON));
    }
    ReplyMarkup::Reply(ReplyKeyboardMarkup {
        keyboard: vec![vec![today, yesterday, help], months],
        resize_keyboard: true,
        is_persistent: true,
    })
}

/// The station's own navigation button: its status in the chat.
const STATION_STATUS_BUTTON: (&str, &str) = ("status", "\u{1f4e1}");

/// Stable glyphs are kept outside localization dictionaries and shared by rendering and aliases.
fn navigation_buttons() -> [(&'static str, &'static str); 5] {
    [
        ("today", "\u{1f4c5}"),
        ("yesterday", "\u{23ee}"),
        ("month", "\u{1f5d3}"),
        ("lastmonth", "\u{1f4c6}"),
        ("help", "\u{2139}\u{fe0f}"),
    ]
}

/// Page and transport keys whose text names what the bot depends on: a station has its own
/// wording, under the same key with `_station` appended.
const STATION_WORDED: &[&str] = &[
    "mini_shell_denied",
    "mini_shell_unreachable",
    "mini_error_busy",
    "report_delivery_failed",
];

/// The report-delivery failure a chat is shown; what the bot depends on is `host`'s.
pub(crate) fn report_delivery_failed(host: crate::HostKind) -> String {
    match host {
        crate::HostKind::Terminal => t!("telegram.report_delivery_failed"),
        crate::HostKind::Station => t!("telegram.report_delivery_failed_station"),
    }
    .to_string()
}

/// Compose Mini App page and shell labels, plus reply-button aliases in the UI locale domain.
///
/// Args:
///     host: Which process runs the bot; a station swaps in its own wording ([`STATION_WORDED`]).
pub(crate) fn telegram_labels(host: crate::HostKind) -> std::collections::BTreeMap<String, String> {
    let mut labels: std::collections::BTreeMap<String, String> = [
        ("menu_miniapp".to_string(), t!("telegram.open").to_string()),
        (
            "mini_shell_checking".to_string(),
            t!("telegram.mini_shell_checking").to_string(),
        ),
        (
            "mini_shell_denied".to_string(),
            t!("telegram.mini_shell_denied").to_string(),
        ),
        (
            "mini_shell_unreachable".to_string(),
            t!("telegram.mini_shell_unreachable").to_string(),
        ),
        ("refusal".to_string(), t!("telegram.refusal").to_string()),
        ("locale".to_string(), rust_i18n::locale().to_string()),
    ]
    .into_iter()
    .collect();
    labels.insert(
        "report_delivery_failed".into(),
        t!("telegram.report_delivery_failed").to_string(),
    );
    // Keep old keyboard labels usable after the desktop locale changes.
    for locale in ["ru", "en", "es"] {
        for (name, icon) in [("home", "\u{1f4ca}"), ("help", "\u{2753}")] {
            let key = format!("telegram.button_{name}");
            labels.insert(
                format!("button_{name}_legacy_emoji_{locale}"),
                format!("{icon} {}", t!(&key, locale = locale)),
            );
        }
        for (name, icon) in navigation_buttons() {
            let key = format!("telegram.button_{name}");
            labels.insert(
                format!("button_{name}_emoji_{locale}"),
                format!("{icon} {}", t!(&key, locale = locale)),
            );
        }
        for name in ["today", "yesterday", "month", "lastmonth", "daily", "home"] {
            let key = format!("telegram.button_{name}");
            labels.insert(
                format!("button_{name}_{locale}"),
                t!(&key, locale = locale).to_string(),
            );
        }
        labels.insert(
            format!("button_miniapp_{locale}"),
            t!("telegram.mini_open", locale = locale).to_string(),
        );
        labels.insert(
            format!("button_help_{locale}"),
            t!("telegram.button_help", locale = locale).to_string(),
        );
    }
    for key in MINI_LABEL_KEYS {
        let path = format!("telegram.{key}");
        labels.insert((*key).to_string(), t!(&path).to_string());
    }
    for (kind, panel_key) in moon_core::feed::fault_keys::FAULT_KIND_SHORT_KEYS {
        let path = (*panel_key).to_string();
        labels.insert(format!("mini_fault_{kind}"), t!(&path).to_string());
    }
    if host == crate::HostKind::Station {
        for key in STATION_WORDED {
            let path = format!("telegram.{key}_station");
            labels.insert((*key).to_string(), t!(&path).to_string());
        }
        // Only a station's bot knows its status button: a terminal's never parses it.
        let (name, icon) = STATION_STATUS_BUTTON;
        for locale in ["ru", "en", "es"] {
            let text = t!("telegram.button_status", locale = locale).to_string();
            labels.insert(
                format!("button_{name}_emoji_{locale}"),
                format!("{icon} {text}"),
            );
            labels.insert(format!("button_{name}_{locale}"), text);
        }
    }
    labels
}

/// Mini App page keys. Values live in the locale files; this list only wires the lookup.
const MINI_LABEL_KEYS: &[&str] = &[
    "mini_title",
    "mini_tab_report",
    "mini_tab_cores",
    "mini_tab_balances",
    "mini_period_today",
    "mini_period_yesterday",
    "mini_period_month",
    "mini_period_lastmonth",
    "mini_report_total",
    "mini_report_by_exchange",
    "mini_report_by_core",
    "report_days",
    "report_date",
    "report_trades",
    "mini_report_orders",
    "mini_unvalued",
    "mini_core_ready",
    "mini_core_connecting",
    "mini_core_stage",
    "mini_core_failed",
    "mini_core_disconnected",
    "mini_cores_online",
    "mini_ping",
    "mini_exch_ping",
    "mini_cpu",
    "mini_unit_ms",
    "mini_unit_pct",
    "mini_fault",
    "mini_balance_stale",
    "mini_balance_awaiting",
    "mini_balance_unpriced",
    "mini_total",
    "mini_free",
    "mini_partial",
    "mini_orders_qty",
    "mini_orders_entry",
    "mini_orders_to_entry",
    "mini_empty_report",
    "mini_empty_cores",
    "mini_empty_balances",
    "mini_empty_orders",
    "mini_error_read",
    "mini_error_busy",
    "mini_error_stale_session",
    "mini_error_network",
    "mini_retry",
    "mini_loading",
    "mini_updated",
    "mini_updated_now",
    "mini_updated_secs",
    "mini_updated_mins",
    "mini_refresh",
    "mini_stale_data",
    "mini_report_core_orders",
    "mini_report_unvalued_n",
    "mini_show_all",
    "mini_cores_problems",
    "mini_online",
    "mini_orders_summary",
    "mini_cancel",
    "mini_cancel_confirm",
    "mini_panic_sell",
    "mini_panic_off",
    "mini_panic_confirm",
    "mini_panic_off_confirm",
    "mini_cmd_failed",
    "mini_cmd_sent",
    "mini_cmd_not_found",
    "mini_cmd_unknown",
    "mini_trading",
    "mini_autodetect",
    "mini_cancel_all",
    "mini_all_cores",
    "mini_cores_trading_on_confirm",
    "mini_cores_trading_off_confirm",
    "mini_cores_auto_on_confirm",
    "mini_cores_auto_off_confirm",
    "mini_cmd_partial",
    "mini_cmd_core_not_found",
    "mini_tab_trades",
    "mini_deals_open",
    "mini_deals_closed",
    "mini_tab_strategies",
    "mini_empty_trades",
    "mini_trades_shown_one",
    "mini_trades_shown_few",
    "mini_trades_shown_many",
    "mini_empty_strategies",
    "mini_trade_entry",
    "mini_trade_exit",
    "mini_trade_qty",
    "mini_trade_duration",
    "mini_trade_strategy",
    "mini_trade_manual",
    "mini_trade_closed",
    "mini_duration_dh",
    "mini_duration_hm",
    "mini_duration_ms",
    "mini_strategy_root",
    "mini_strategy_pending",
    "mini_strategy_timed_out",
    "mini_version",
    "mini_memory",
    "mini_free_memory",
    "mini_unit_mb",
    "mini_reconnect",
    "mini_back",
    "mini_close",
    "mini_balances_show",
    "mini_balances_hide",
    "mini_start_all",
    "mini_stop_all",
    "mini_on_of",
    "mini_orders_n_one",
    "mini_orders_n_few",
    "mini_orders_n_many",
];

#[cfg(test)]
mod tests;
