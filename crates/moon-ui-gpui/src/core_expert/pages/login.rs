//! Moonbot's "Логин" page, control for control.
//!
//! Not one control here is live, and none can become live: safe-share carries no secrets, so the
//! API key and secret, the local password, the support identity and the licence state never cross
//! the wire at all. The page is drawn anyway — a trader reads this window beside Moonbot's own
//! dialog, and a page missing from the strip reads as a bug where a dead one reads as a limit.
//!
//! The fields are drawn EMPTY rather than filled with a plausible value. Showing an API key that is
//! not the core's, or a password box that suggests one is set, would be worse than showing nothing.

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use moon_core::feed::CoreConfig;

use crate::design;
use crate::shell::editors::EditorStore;

use super::super::CoreExpertView;
use super::super::widgets::{
    action, caption, columns, dropdown, field, field_masked, flag, group, hint, labeled, link,
    rows, text_block, text_line,
};

/// Nothing on this page reaches the draft; every field stages through this.
const DEAD: fn(&mut CoreConfig, &str) = |_, _| {};

/// See [`super::field_specs`].
///
/// Six fields, all empty and all dead: they exist so the page has Moonbot's shape, not so it can
/// carry a value.
#[allow(clippy::type_complexity)]
pub(super) fn field_specs(
    _draft: &CoreConfig,
) -> Vec<(&'static str, String, fn(&mut CoreConfig, &str))> {
    vec![
        ("exp-log-api-key", String::new(), DEAD),
        ("exp-log-api-secret", String::new(), DEAD),
        ("exp-log-name", String::new(), DEAD),
        ("exp-log-telegram", String::new(), DEAD),
        ("exp-log-password", String::new(), DEAD),
        ("exp-log-password2", String::new(), DEAD),
    ]
}

/// Build the page.
pub(super) fn body(
    view: &Entity<CoreExpertView>,
    store: &EditorStore,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let gap = design::ui_px(cx, 8.0);

    // --- Left: the exchange, its keys, the local password ----------------------------------------
    let left = rows(cx)
        .gap(gap)
        .child(text_line(
            t!("core_expert.log_exchange", v = "—").to_string(),
            p.text,
            true,
            cx,
        ))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .child(dropdown(
                    "exp-log-exchange",
                    t!("core_expert.log_exchange_none").to_string(),
                    false,
                ))
                .child(action(
                    "exp-log-apply",
                    t!("core_expert.log_apply").to_string(),
                    false,
                )),
        )
        // Moonbot's key row carries the signing scheme as two links beside the caption.
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 10.0))
                .child(caption(
                    t!("core_expert.log_api_key").to_string(),
                    false,
                    p,
                    cx,
                ))
                .child(link("exp-log-hmac", "HMAC".to_string(), false))
                .child(link("exp-log-rsa", "RSA".to_string(), false)),
        )
        .children(field(store, "exp-log-api-key", false))
        .child(labeled(
            t!("core_expert.log_api_secret").to_string(),
            field(store, "exp-log-api-secret", false),
            false,
            p,
            cx,
        ))
        .child(
            h_flex()
                .w_full()
                .items_start()
                .gap(design::ui_px(cx, 10.0))
                .child(div().flex_1().min_w_0().child(labeled(
                    t!("core_expert.log_your_name").to_string(),
                    field(store, "exp-log-name", false),
                    false,
                    p,
                    cx,
                )))
                .child(div().flex_1().min_w_0().child(labeled(
                    t!("core_expert.log_telegram").to_string(),
                    field(store, "exp-log-telegram", false),
                    false,
                    p,
                    cx,
                ))),
        )
        .child(hint(t!("core_expert.log_required").to_string(), p, cx))
        .child(action(
            "exp-log-register-api",
            t!("core_expert.log_register_api").to_string(),
            false,
        ))
        .child(action(
            "exp-log-register-other",
            t!("core_expert.log_register_other").to_string(),
            false,
        ))
        .child(labeled(
            t!("core_expert.log_password").to_string(),
            field_masked(store, "exp-log-password", false, true),
            false,
            p,
            cx,
        ))
        .child(labeled(
            t!("core_expert.log_password_again").to_string(),
            field_masked(store, "exp-log-password2", false, true),
            false,
            p,
            cx,
        ))
        .child(action(
            "exp-log-change-password",
            t!("core_expert.log_change_password").to_string(),
            false,
        ))
        .child(flag(
            "exp-log-debug-data",
            t!("core_expert.log_debug_data").to_string(),
            false,
            false,
            view,
            |_, _| {},
        ))
        .child(hint(t!("core_expert.log_debug_hint").to_string(), p, cx));

    // --- Right: licence, support, and what Moonbot sends to its own server -----------------------
    let support = group(
        "exp-log-support",
        t!("core_expert.log_support_frame").to_string(),
    )
    .child(
        rows(cx)
            .gap(gap)
            .child(text_block(
                t!("core_expert.log_support_ask").to_string(),
                p.text,
                true,
                cx,
            ))
            .child(text_block(
                t!("core_expert.log_support_mail").to_string(),
                p.text_soft,
                false,
                cx,
            ))
            .child(text_block(
                t!("core_expert.log_support_pro").to_string(),
                p.text_soft,
                false,
                cx,
            ))
            .child(action(
                "exp-log-support-en",
                t!("core_expert.log_support_en").to_string(),
                false,
            ))
            .child(action(
                "exp-log-support-ru",
                t!("core_expert.log_support_ru").to_string(),
                false,
            )),
    );

    let right = rows(cx)
        .gap(gap)
        // Moonbot's language switch sits at the top of this page; the terminal has its own, in
        // Settings, so this one is a picture of Moonbot's.
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .child(link("exp-log-lang-ru", "RU".to_string(), false))
                .child(link("exp-log-lang-en", "EN".to_string(), false))
                .child(link("exp-log-lang-es", "ES".to_string(), false)),
        )
        // Moonbot prints these in green, which there means "registered, PRO, unlocked". Here the
        // value is unread, and green would assert a licence state this window cannot see.
        .child(text_line(
            t!("core_expert.log_registration_id", v = "—").to_string(),
            p.text_muted,
            true,
            cx,
        ))
        .child(text_line(
            t!("core_expert.log_licence", v = "—").to_string(),
            p.text_muted,
            true,
            cx,
        ))
        .child(action(
            "exp-log-activate-pro",
            t!("core_expert.log_activate_pro").to_string(),
            false,
        ))
        .child(support)
        .child(flag(
            "exp-log-send-trades",
            t!("core_expert.log_send_trades").to_string(),
            false,
            false,
            view,
            |_, _| {},
        ))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 16.0))
                .pl(design::ui_px(cx, 18.0))
                .child(flag(
                    "exp-log-anonymous",
                    t!("core_expert.log_anonymous").to_string(),
                    false,
                    false,
                    view,
                    |_, _| {},
                ))
                .child(flag(
                    "exp-log-no-strat-kind",
                    t!("core_expert.log_no_strat_kind").to_string(),
                    false,
                    false,
                    view,
                    |_, _| {},
                )),
        )
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 10.0))
                .child(flag(
                    "exp-log-online-monitor",
                    t!("core_expert.log_online_monitor").to_string(),
                    false,
                    false,
                    view,
                    |_, _| {},
                ))
                .child(div().flex_1())
                .child(link(
                    "exp-log-online-bot",
                    "@MBOnlineBot".to_string(),
                    false,
                )),
        )
        .child(text_block(
            t!("core_expert.log_stats_note").to_string(),
            p.accent,
            false,
            cx,
        ));

    v_flex()
        .w_full()
        .child(columns(left, right, cx))
        .into_any_element()
}
