//! Moonbot's "АвтоПокупка" page, control for control — and live but for the rows named below.
//!
//! The densest page of the dialog: two source groups at the top — the clipboard and Telegram, each
//! with the same three-way search mode — the TradingView webhook row between them, and below it the
//! whole message-filter block in two columns.
//!
//! What is live is `moon_core::feed::AutoBuySettings`: the `signals` section, its `signal_config`
//! sub-record, and the one `trading` field Moonbot files under this page. The two price-approach
//! alert sounds of that same wire section are NOT here — they belong to the Interface page, which
//! is where Moonbot draws them, and one wire field belongs to one area.
//!
//! Two rows stay disabled, and neither is an oversight: the TradingView webhook with its URL and
//! "показать" link, and "не покупать пересланное", whose only plausible neighbour
//! (`trading.dont_buy_forward`) is documented as skipping forward CONTRACTS rather than forwarded
//! messages.
//!
//! Two things about this page were settled against Moonbot's own dialog rather than against the
//! protocol, because the protocol alone points the wrong way on both.
//!
//! The search mode: `SignalsSection::default()` sets `look_full_link_*` and `advanced_filter*`
//! together, which no one-of-three control can express — but that dialog draws them as radio
//! buttons with exactly one selected, so the factory default is a value its own OK normalises. This
//! page mirrors that, and stages nothing on a click that changes no mode; see [`SearchMode`].
//!
//! "Захватывать буфер": `signals.monitor_clipboard` is documented as enabling clipboard monitoring
//! at all, which reads like a title for the group rather than a row inside it — but that dialog
//! puts no checkbox in either frame caption, and this is the only clipboard control left once
//! auto-buy, lowercase and the mode pair have their fields. `signals.do_monitoring`, the master
//! toggle for the whole signal pipeline, has no control on THIS page for the same reason; it most
//! likely belongs to the Telegram tab.

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use moon_core::feed::CoreConfig;

use crate::design;
use crate::shell::editors::EditorStore;

use super::super::CoreExpertView;
use super::super::widgets::{
    caption, columns, field, flag, group, hint, link, radio_live, rows, slider, stepper_live,
    text_line,
};

/// Bounds of the live sliders, all four of which carry a WHOLE number on the wire.
///
/// Wider than Moonbot's own controls where its range is not stated, because the seeded value is
/// clamped into these bounds for display: too narrow a range would show a thumb that disagrees with
/// the number this page would send.
const WORD_GAP: (f32, f32, f32) = (0.0, 100.0, 1.0);
const PRICE_OFFSET_PCT: (f32, f32, f32) = (-100.0, 100.0, 1.0);
const CANCEL_MINUTES: (f32, f32, f32) = (0.0, 1440.0, 1.0);

/// Floor for the message word-count spinner. Zero is a legitimate cap; the flag beside it is what
/// switches the filter off.
const WORDS_FLOOR: i32 = 0;

/// Value shown where Moonbot prints something this terminal has not read — on this page, the
/// TradingView webhook URL.
const NO_VALUE: &str = "—";

/// Moonbot's search mode: one of three, stored as TWO wire flags per source.
///
/// The wire looks like it disagrees — `SignalsSection::default()` sets `look_full_link_*` and
/// `advanced_filter*` together, a pair no three-way choice can express — but Moonbot's own dialog
/// draws these as radio buttons and shows exactly one of them selected, so that default is an
/// unnormalised factory value its dialog collapses, not a state its user can reach. This page
/// mirrors that: it reads the pair with the same precedence, and its OK writes the whole choice.
///
/// Which is why a click on the option already selected must stage NOTHING — see
/// [`super::super::widgets::radio_live`]. Otherwise opening this window on a factory-default core
/// and clicking the mode it already shows would clear the other flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchMode {
    Token,
    Link,
    Special,
}

impl SearchMode {
    /// Read the mode out of the pair of flags, `special` first: that is the order Moonbot's own
    /// dialog resolves them in when a config carries both.
    fn of(full_link: bool, special: bool) -> Self {
        match (special, full_link) {
            (true, _) => Self::Special,
            (false, true) => Self::Link,
            (false, false) => Self::Token,
        }
    }

    /// The pair this mode writes: `(full_link, special)`.
    fn flags(self) -> (bool, bool) {
        match self {
            Self::Token => (false, false),
            Self::Link => (true, false),
            Self::Special => (false, true),
        }
    }
}

/// See [`super::field_specs`].
#[allow(clippy::type_complexity)]
pub(super) fn field_specs(
    draft: &CoreConfig,
) -> Vec<(&'static str, String, fn(&mut CoreConfig, &str))> {
    let b = &draft.auto_buy;
    vec![
        (
            "exp-buy-long-words",
            b.msg_keywords_long.clone(),
            (|d, t| d.auto_buy.msg_keywords_long = t.to_string()) as fn(&mut CoreConfig, &str),
        ),
        (
            "exp-buy-short-words",
            b.msg_keywords_short.clone(),
            |d, t| d.auto_buy.msg_keywords_short = t.to_string(),
        ),
        ("exp-buy-black-words", b.msg_black_words.clone(), |d, t| {
            d.auto_buy.msg_black_words = t.to_string()
        }),
        ("exp-buy-dip-words", b.lower_price_words.clone(), |d, t| {
            d.auto_buy.lower_price_words = t.to_string()
        }),
        ("exp-buy-tags", b.msg_token_tags.clone(), |d, t| {
            d.auto_buy.msg_token_tags = t.to_string()
        }),
    ]
}

/// See [`super::slider_specs`].
#[allow(clippy::type_complexity)]
pub(super) fn slider_specs(
    draft: &CoreConfig,
) -> Vec<(
    &'static str,
    (f32, f32, f32),
    f32,
    fn(&mut CoreConfig, f32),
    Option<&'static str>,
)> {
    let b = &draft.auto_buy;
    vec![
        (
            "exp-buy-word-gap",
            WORD_GAP,
            b.buy_key_dist as f32,
            (|d, v| d.auto_buy.buy_key_dist = v.round() as i32) as fn(&mut CoreConfig, f32),
            None,
        ),
        (
            "exp-buy-market-price",
            PRICE_OFFSET_PCT,
            b.x_lower_price as f32,
            |d, v| d.auto_buy.x_lower_price = v.round() as i32,
            None,
        ),
        (
            "exp-buy-cancel-cheap",
            CANCEL_MINUTES,
            b.auto_cancel_lower_buy as f32,
            |d, v| d.auto_buy.auto_cancel_lower_buy = v.round() as i32,
            None,
        ),
        (
            "exp-buy-msg-price",
            PRICE_OFFSET_PCT,
            b.x_found_price as f32,
            |d, v| d.auto_buy.x_found_price = v.round() as i32,
            None,
        ),
    ]
}

/// Build the page.
pub(super) fn body(
    view: &Entity<CoreExpertView>,
    store: &EditorStore,
    draft: &CoreConfig,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let gap = design::ui_px(cx, 6.0);
    let b = &draft.auto_buy;
    let cbd_mode = SearchMode::of(b.look_full_link_cbd, b.advanced_filter_clipboard);
    let tlg_mode = SearchMode::of(b.look_full_link_tlg, b.advanced_filter);

    // --- The two sources, each with Moonbot's identical search mode ------------------------------
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
                b.clipboard_auto_buy,
                true,
                view,
                |d, on| d.auto_buy.clipboard_auto_buy = on,
            ))
            .child(radio_live(
                "exp-buy-cbd-token",
                t!("core_expert.buy_by_token").to_string(),
                cbd_mode == SearchMode::Token,
                view,
                |d| {
                    let (link, special) = SearchMode::Token.flags();
                    d.auto_buy.look_full_link_cbd = link;
                    d.auto_buy.advanced_filter_clipboard = special;
                },
            ))
            .child(div().pl(design::ui_px(cx, 18.0)).child(flag(
                "exp-buy-cbd-lower",
                t!("core_expert.buy_lowercase").to_string(),
                b.lower_case_token_cbd,
                true,
                view,
                |d, on| d.auto_buy.lower_case_token_cbd = on,
            )))
            .child(radio_live(
                "exp-buy-cbd-link",
                t!("core_expert.buy_by_link").to_string(),
                cbd_mode == SearchMode::Link,
                view,
                |d| {
                    let (link, special) = SearchMode::Link.flags();
                    d.auto_buy.look_full_link_cbd = link;
                    d.auto_buy.advanced_filter_clipboard = special;
                },
            ))
            .child(radio_live(
                "exp-buy-cbd-special",
                t!("core_expert.buy_special_filter").to_string(),
                cbd_mode == SearchMode::Special,
                view,
                |d| {
                    let (link, special) = SearchMode::Special.flags();
                    d.auto_buy.look_full_link_cbd = link;
                    d.auto_buy.advanced_filter_clipboard = special;
                },
            ))
            .child(flag(
                "exp-buy-cbd-capture",
                t!("core_expert.buy_capture_clipboard").to_string(),
                b.monitor_clipboard,
                true,
                view,
                |d, on| d.auto_buy.monitor_clipboard = on,
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
                b.telegram_auto_buy,
                true,
                view,
                |d, on| d.auto_buy.telegram_auto_buy = on,
            ))
            .child(radio_live(
                "exp-buy-tlg-token",
                t!("core_expert.buy_by_token").to_string(),
                tlg_mode == SearchMode::Token,
                view,
                |d| {
                    let (link, special) = SearchMode::Token.flags();
                    d.auto_buy.look_full_link_tlg = link;
                    d.auto_buy.advanced_filter = special;
                },
            ))
            .child(div().pl(design::ui_px(cx, 18.0)).child(flag(
                "exp-buy-tlg-lower",
                t!("core_expert.buy_lowercase").to_string(),
                b.lower_case_token_tlg,
                true,
                view,
                |d, on| d.auto_buy.lower_case_token_tlg = on,
            )))
            .child(radio_live(
                "exp-buy-tlg-link",
                t!("core_expert.buy_by_link").to_string(),
                tlg_mode == SearchMode::Link,
                view,
                |d| {
                    let (link, special) = SearchMode::Link.flags();
                    d.auto_buy.look_full_link_tlg = link;
                    d.auto_buy.advanced_filter = special;
                },
            ))
            .child(radio_live(
                "exp-buy-tlg-special",
                t!("core_expert.buy_special_filter").to_string(),
                tlg_mode == SearchMode::Special,
                view,
                |d| {
                    let (link, special) = SearchMode::Special.flags();
                    d.auto_buy.look_full_link_tlg = link;
                    d.auto_buy.advanced_filter = special;
                },
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
                        b.dont_buy_reply,
                        true,
                        view,
                        |d, on| d.auto_buy.dont_buy_reply = on,
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
            b.use_keywords,
            true,
            view,
            |d, on| d.auto_buy.use_keywords = on,
        ))
        .children(field(store, "exp-buy-long-words", true))
        .child(caption(
            t!("core_expert.buy_word_gap", v = b.buy_key_dist.to_string()).to_string(),
            true,
            p,
            cx,
        ))
        .children(slider(store, "exp-buy-word-gap", true))
        .child(flag(
            "exp-buy-black-on",
            t!("core_expert.buy_black_words").to_string(),
            b.use_black_words,
            true,
            view,
            |d, on| d.auto_buy.use_black_words = on,
        ))
        .children(field(store, "exp-buy-black-words", true))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .child(flag(
                    "exp-buy-count-on",
                    t!("core_expert.buy_word_count").to_string(),
                    b.use_words_count,
                    true,
                    view,
                    |d, on| d.auto_buy.use_words_count = on,
                ))
                .child(div().flex_1())
                .child(stepper_live(
                    "exp-buy-word-count",
                    b.words_count,
                    WORDS_FLOOR,
                    view,
                    |d, v| d.auto_buy.words_count = v,
                )),
        )
        .child(flag(
            "exp-buy-dip-on",
            t!("core_expert.buy_dip_words").to_string(),
            b.use_lower_price_words,
            true,
            view,
            |d, on| d.auto_buy.use_lower_price_words = on,
        ))
        .children(field(store, "exp-buy-dip-words", true))
        .child(caption(
            t!(
                "core_expert.buy_market_price",
                v = b.x_lower_price.to_string()
            )
            .to_string(),
            true,
            p,
            cx,
        ))
        .children(slider(store, "exp-buy-market-price", true))
        .child(caption(
            t!(
                "core_expert.buy_cancel_cheap",
                v = b.auto_cancel_lower_buy.to_string()
            )
            .to_string(),
            true,
            p,
            cx,
        ))
        .children(slider(store, "exp-buy-cancel-cheap", true));

    let filter_right = rows(cx)
        .gap(gap)
        .child(caption(
            t!("core_expert.buy_short_words").to_string(),
            true,
            p,
            cx,
        ))
        .children(field(store, "exp-buy-short-words", true))
        .child(flag(
            "exp-buy-tags-on",
            t!("core_expert.buy_tags").to_string(),
            b.use_token_tags,
            true,
            view,
            |d, on| d.auto_buy.use_token_tags = on,
        ))
        .children(field(store, "exp-buy-tags", true))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 12.0))
                .child(flag(
                    "exp-buy-no-tags",
                    t!("core_expert.buy_tokens_without_tags").to_string(),
                    b.tokens_no_tags,
                    true,
                    view,
                    |d, on| d.auto_buy.tokens_no_tags = on,
                ))
                .child(flag(
                    "exp-buy-links",
                    t!("core_expert.buy_links").to_string(),
                    b.token_links,
                    true,
                    view,
                    |d, on| d.auto_buy.token_links = on,
                ))
                .child(flag(
                    "exp-buy-special",
                    t!("core_expert.buy_special").to_string(),
                    b.special_formats,
                    true,
                    view,
                    |d, on| d.auto_buy.special_formats = on,
                )),
        )
        .child(flag(
            "exp-buy-one-token",
            t!("core_expert.buy_single_token").to_string(),
            b.only_1_token,
            true,
            view,
            |d, on| d.auto_buy.only_1_token = on,
        ))
        .child(flag(
            "exp-buy-price-required",
            t!("core_expert.buy_price_required").to_string(),
            b.buy_if_price_found,
            true,
            view,
            |d, on| d.auto_buy.buy_if_price_found = on,
        ))
        .child(flag(
            "exp-buy-price-from-msg",
            t!("core_expert.buy_price_from_message").to_string(),
            b.use_price,
            true,
            view,
            |d, on| d.auto_buy.use_price = on,
        ))
        .child(caption(
            t!("core_expert.buy_msg_price", v = b.x_found_price.to_string()).to_string(),
            true,
            p,
            cx,
        ))
        .children(slider(store, "exp-buy-msg-price", true))
        .child(flag(
            "exp-buy-stops-from-msg",
            t!("core_expert.buy_stops_from_message").to_string(),
            b.use_stops,
            true,
            view,
            |d, on| d.auto_buy.use_stops = on,
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
