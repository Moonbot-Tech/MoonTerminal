//! Moonbot's "АвтоПокупка" page, control for control.
//!
//! The densest page of the dialog: two source groups at the top — the clipboard and Telegram, each
//! with the same four-way search mode — the TradingView webhook row between them, and below it the
//! whole message-filter block in two columns.
//!
//! None of it is live. The values live in the safe-share `signals` section and its `signal_config`
//! sub-record, which `moon_core::feed::CoreConfig` does not project and
//! `FieldMask::RENDERED_SECTIONS` does not carry; the one part of this page the terminal DOES own —
//! the price-approach alert sounds — sits on the compact popup's General tab rather than here,
//! because Moonbot keeps it elsewhere too.
//!
//! Every text field is empty and every switch unchecked for the reason stated in
//! [`super::login`]: a mirrored control filled with a plausible value states a setting this
//! terminal has not read.

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use moon_core::feed::CoreConfig;

use crate::design;
use crate::shell::editors::EditorStore;

use super::super::CoreExpertView;
use super::super::widgets::{
    caption, columns, field, flag, group, hint, link, radio, rows, slider, stepper, text_line,
};

/// Nothing on this page reaches the draft.
const DEAD_TEXT: fn(&mut CoreConfig, &str) = |_, _| {};
const DEAD_NUM: fn(&mut CoreConfig, f32) = |_, _| {};

/// Bounds of the dead sliders: a range that resembles Moonbot's, on a control that writes nothing.
const DEAD_WORDS: (f32, f32, f32) = (0.0, 50.0, 1.0);
const DEAD_DISCOUNT: (f32, f32, f32) = (-10.0, 0.0, 0.1);
const DEAD_MINUTES: (f32, f32, f32) = (0.0, 600.0, 5.0);

/// Value shown where Moonbot prints a number this terminal has not read.
const NO_VALUE: &str = "—";

/// See [`super::field_specs`].
#[allow(clippy::type_complexity)]
pub(super) fn field_specs(
    _draft: &CoreConfig,
) -> Vec<(&'static str, String, fn(&mut CoreConfig, &str))> {
    vec![
        ("exp-buy-long-words", String::new(), DEAD_TEXT),
        ("exp-buy-short-words", String::new(), DEAD_TEXT),
        ("exp-buy-black-words", String::new(), DEAD_TEXT),
        ("exp-buy-dip-words", String::new(), DEAD_TEXT),
        ("exp-buy-tags", String::new(), DEAD_TEXT),
    ]
}

/// See [`super::slider_specs`].
#[allow(clippy::type_complexity)]
pub(super) fn slider_specs(
    _draft: &CoreConfig,
) -> Vec<(
    &'static str,
    (f32, f32, f32),
    f32,
    fn(&mut CoreConfig, f32),
    Option<&'static str>,
)> {
    vec![
        ("exp-buy-word-gap", DEAD_WORDS, 0.0, DEAD_NUM, None),
        ("exp-buy-market-price", DEAD_DISCOUNT, 0.0, DEAD_NUM, None),
        ("exp-buy-cancel-cheap", DEAD_MINUTES, 0.0, DEAD_NUM, None),
        ("exp-buy-msg-price", DEAD_DISCOUNT, 0.0, DEAD_NUM, None),
    ]
}

/// Build the page.
pub(super) fn body(
    view: &Entity<CoreExpertView>,
    store: &EditorStore,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let gap = design::ui_px(cx, 6.0);

    // --- The two sources, each with Moonbot's identical four-way search mode ----------------------
    let clipboard = group(
        "exp-buy-clipboard",
        t!("core_expert.buy_clipboard_frame").to_string(),
    )
    .child(
        rows(cx)
            .gap(gap)
            .child(flag(
                "exp-buy-cbd-on",
                t!("core_expert.buy_clipboard_auto").to_string(),
                false,
                false,
                view,
                |_, _| {},
            ))
            .child(radio(
                "exp-buy-cbd-token",
                t!("core_expert.buy_by_token").to_string(),
                false,
                false,
            ))
            .child(div().pl(design::ui_px(cx, 18.0)).child(flag(
                "exp-buy-cbd-lower",
                t!("core_expert.buy_lowercase").to_string(),
                false,
                false,
                view,
                |_, _| {},
            )))
            .child(radio(
                "exp-buy-cbd-link",
                t!("core_expert.buy_by_link").to_string(),
                false,
                false,
            ))
            .child(radio(
                "exp-buy-cbd-special",
                t!("core_expert.buy_special_filter").to_string(),
                false,
                false,
            ))
            .child(flag(
                "exp-buy-cbd-capture",
                t!("core_expert.buy_capture_clipboard").to_string(),
                false,
                false,
                view,
                |_, _| {},
            )),
    );

    let telegram = group(
        "exp-buy-telegram",
        t!("core_expert.buy_telegram_frame").to_string(),
    )
    .child(
        rows(cx)
            .gap(gap)
            .child(flag(
                "exp-buy-tlg-on",
                t!("core_expert.buy_telegram_auto").to_string(),
                false,
                false,
                view,
                |_, _| {},
            ))
            .child(radio(
                "exp-buy-tlg-token",
                t!("core_expert.buy_by_token").to_string(),
                false,
                false,
            ))
            .child(div().pl(design::ui_px(cx, 18.0)).child(flag(
                "exp-buy-tlg-lower",
                t!("core_expert.buy_lowercase").to_string(),
                false,
                false,
                view,
                |_, _| {},
            )))
            .child(radio(
                "exp-buy-tlg-link",
                t!("core_expert.buy_by_link").to_string(),
                false,
                false,
            ))
            .child(radio(
                "exp-buy-tlg-special",
                t!("core_expert.buy_special_filter").to_string(),
                false,
                false,
            ))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 12.0))
                    .child(flag(
                        "exp-buy-tlg-no-forward",
                        t!("core_expert.buy_no_forwarded").to_string(),
                        false,
                        false,
                        view,
                        |_, _| {},
                    ))
                    .child(flag(
                        "exp-buy-tlg-no-reply",
                        t!("core_expert.buy_no_reply").to_string(),
                        false,
                        false,
                        view,
                        |_, _| {},
                    )),
            ),
    );

    // --- The TradingView webhook row that sits between the sources and the filter block ----------
    let webhook = h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 10.0))
        .child(flag(
            "exp-buy-tv-webhook",
            t!("core_expert.buy_tv_webhook").to_string(),
            false,
            false,
            view,
            |_, _| {},
        ))
        .child(div().flex_1().min_w_0().child(text_line(
            t!("core_expert.buy_tv_url", v = NO_VALUE).to_string(),
            p.accent,
            false,
            cx,
        )))
        .child(link(
            "exp-buy-tv-show",
            t!("core_expert.buy_tv_show").to_string(),
            false,
        ));

    // --- The message filter, in Moonbot's own two columns -----------------------------------------
    let filter_left = rows(cx)
        .gap(gap)
        .child(flag(
            "exp-buy-long-on",
            t!("core_expert.buy_long_words").to_string(),
            false,
            false,
            view,
            |_, _| {},
        ))
        .children(field(store, "exp-buy-long-words", false))
        .child(caption(
            t!("core_expert.buy_word_gap", v = NO_VALUE).to_string(),
            false,
            p,
            cx,
        ))
        .children(slider(store, "exp-buy-word-gap", false))
        .child(flag(
            "exp-buy-black-on",
            t!("core_expert.buy_black_words").to_string(),
            false,
            false,
            view,
            |_, _| {},
        ))
        .children(field(store, "exp-buy-black-words", false))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .child(flag(
                    "exp-buy-count-on",
                    t!("core_expert.buy_word_count").to_string(),
                    false,
                    false,
                    view,
                    |_, _| {},
                ))
                .child(div().flex_1())
                .child(stepper("exp-buy-word-count", 0.0, false)),
        )
        .child(flag(
            "exp-buy-dip-on",
            t!("core_expert.buy_dip_words").to_string(),
            false,
            false,
            view,
            |_, _| {},
        ))
        .children(field(store, "exp-buy-dip-words", false))
        .child(caption(
            t!("core_expert.buy_market_price", v = NO_VALUE).to_string(),
            false,
            p,
            cx,
        ))
        .children(slider(store, "exp-buy-market-price", false))
        .child(caption(
            t!("core_expert.buy_cancel_cheap", v = NO_VALUE).to_string(),
            false,
            p,
            cx,
        ))
        .children(slider(store, "exp-buy-cancel-cheap", false));

    let filter_right = rows(cx)
        .gap(gap)
        .child(caption(
            t!("core_expert.buy_short_words").to_string(),
            false,
            p,
            cx,
        ))
        .children(field(store, "exp-buy-short-words", false))
        .child(flag(
            "exp-buy-tags-on",
            t!("core_expert.buy_tags").to_string(),
            false,
            false,
            view,
            |_, _| {},
        ))
        .children(field(store, "exp-buy-tags", false))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 12.0))
                .child(flag(
                    "exp-buy-no-tags",
                    t!("core_expert.buy_tokens_without_tags").to_string(),
                    false,
                    false,
                    view,
                    |_, _| {},
                ))
                .child(flag(
                    "exp-buy-links",
                    t!("core_expert.buy_links").to_string(),
                    false,
                    false,
                    view,
                    |_, _| {},
                ))
                .child(flag(
                    "exp-buy-special",
                    t!("core_expert.buy_special").to_string(),
                    false,
                    false,
                    view,
                    |_, _| {},
                )),
        )
        .child(flag(
            "exp-buy-one-token",
            t!("core_expert.buy_single_token").to_string(),
            false,
            false,
            view,
            |_, _| {},
        ))
        .child(flag(
            "exp-buy-price-required",
            t!("core_expert.buy_price_required").to_string(),
            false,
            false,
            view,
            |_, _| {},
        ))
        .child(flag(
            "exp-buy-price-from-msg",
            t!("core_expert.buy_price_from_message").to_string(),
            false,
            false,
            view,
            |_, _| {},
        ))
        .child(caption(
            t!("core_expert.buy_msg_price", v = NO_VALUE).to_string(),
            false,
            p,
            cx,
        ))
        .children(slider(store, "exp-buy-msg-price", false))
        .child(flag(
            "exp-buy-stops-from-msg",
            t!("core_expert.buy_stops_from_message").to_string(),
            false,
            false,
            view,
            |_, _| {},
        ))
        .child(hint(t!("core_expert.buy_example").to_string(), p, cx))
        .child(h_flex().w_full().justify_end().child(text_line(
            t!("core_expert.gen_need_help").to_string(),
            design::positive_color(p),
            false,
            cx,
        )));

    let filter = group(
        "exp-buy-filter",
        t!("core_expert.buy_filter_frame").to_string(),
    )
    .child(columns(filter_left, filter_right, cx));

    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 10.0))
        .child(columns(clipboard, telegram, cx))
        .child(webhook)
        .child(filter)
        .into_any_element()
}
