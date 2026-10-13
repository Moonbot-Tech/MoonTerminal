//! Master-detail frame of Settings -> Telegram на ядре: the list of configured cores with each
//! one's Telegram state tag on the left, the picked core's reader card on the right.
//!
//! Everything here is a free builder over plain data, so the layout probe can draw it without a
//! `SettingsView`. The per-core tag comes only from what the store already holds: a core whose
//! snapshot has not arrived shows "—", never a guessed state.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::feed::{AuthStep, ConnStatus, CoreTelegramState, auth_step};
use moon_ui::{MoonPalette, MoonTag, MoonTone, h_flex, rgba_from, v_flex};
use rust_i18n::t;

use crate::design;
use crate::panels::common::text_tooltip_value;

/// List width beside the card, unscaled; font-scaled at draw time.
pub(super) const LIST_W: f32 = 420.0;
/// Narrowest the list shrinks to beside the card before the two stack, unscaled.
pub(super) const LIST_MIN_W: f32 = 250.0;
/// Narrowest card that keeps the proxy fields and the action row inside it, unscaled.
pub(super) const CARD_MIN_W: f32 = 380.0;
/// Tallest the list grows when stacked over the card before it scrolls on its own, unscaled: a
/// long setup keeps the card in view under it. Beside the card the list fills the page height.
pub(super) const LIST_MAX_H: f32 = 420.0;
/// Gap between the list and the card, unscaled.
pub(super) const SPLIT_GAP: f32 = 14.0;
/// Width of a card row's label column, unscaled.
pub(super) const LABEL_W: f32 = 120.0;
/// Shown in a row whose core's snapshot has not arrived yet.
const UNKNOWN_TAG: &str = "—";

/// Telegram state of one core as its list row and card header show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TgTag {
    /// `ready`: logged in.
    LoggedIn,
    /// Reader on, service up, any other auth step.
    NotLoggedIn,
    /// The service reports a proxy error.
    ProxyError,
    /// The reader is switched off on this core.
    ReaderOff,
    /// The core itself is not connected.
    NoCore,
    /// The service has no recoverable remote login.
    Unsupported,
    /// No current snapshot for this core yet.
    Unknown,
}

/// Map what the store holds for one core onto its tag.
///
/// Args:
///     status: The core's connection status, `None` when the store has no entry.
///     fresh: Whether the held Telegram snapshot is current (the store's `telegram_fresh`).
///     state: The held snapshot, if any.
///
/// Returns:
///     [`TgTag::Unknown`] for a stale or missing snapshot, or a service snapshot that has not
///     arrived — never a guess.
pub(crate) fn tg_tag(
    status: Option<&ConnStatus>,
    fresh: bool,
    state: Option<&CoreTelegramState>,
) -> TgTag {
    if !matches!(status, Some(ConnStatus::Ready)) {
        return TgTag::NoCore;
    }
    let Some(s) = state.filter(|_| fresh) else {
        return TgTag::Unknown;
    };
    if !s.state_supported {
        return TgTag::Unsupported;
    }
    if !s.enabled {
        return TgTag::ReaderOff;
    }
    let Some(service) = s.service.as_ref() else {
        return TgTag::Unknown;
    };
    if service
        .proxy_error
        .as_ref()
        .is_some_and(|e| !e.message.is_empty())
    {
        return TgTag::ProxyError;
    }
    if auth_step(&service.auth_state) == AuthStep::Ready {
        TgTag::LoggedIn
    } else {
        TgTag::NotLoggedIn
    }
}

/// The tag's words in the current language.
pub(crate) fn tag_text(tag: TgTag) -> String {
    match tag {
        TgTag::LoggedIn => t!("telegram_core.tag.logged_in").to_string(),
        TgTag::NotLoggedIn => t!("telegram_core.tag.not_logged_in").to_string(),
        TgTag::ProxyError => t!("telegram_core.tag.proxy_error").to_string(),
        TgTag::ReaderOff => t!("telegram_core.tag.reader_off").to_string(),
        TgTag::NoCore => t!("telegram_core.tag.no_core").to_string(),
        TgTag::Unsupported => t!("telegram_core.tag.unsupported").to_string(),
        TgTag::Unknown => UNKNOWN_TAG.to_string(),
    }
}

/// Every tag the list can show, for the probe's longest-tag pick.
#[cfg(test)]
pub(super) const ALL_TAGS: [TgTag; 7] = [
    TgTag::LoggedIn,
    TgTag::NotLoggedIn,
    TgTag::ProxyError,
    TgTag::ReaderOff,
    TgTag::NoCore,
    TgTag::Unsupported,
    TgTag::Unknown,
];

/// The tag as a chip: filled for the reader's own states, outlined for the core's.
pub(super) fn tag_el(tag: TgTag, selector: String) -> AnyElement {
    let chip = match tag {
        TgTag::LoggedIn => MoonTag::positive(),
        TgTag::NotLoggedIn => MoonTag::warning(),
        TgTag::ProxyError => MoonTag::danger(),
        TgTag::ReaderOff => MoonTag::new(),
        TgTag::NoCore => MoonTag::danger().outline(),
        TgTag::Unsupported | TgTag::Unknown => MoonTag::new().outline(),
    };
    div()
        .debug_selector(move || selector.clone())
        .flex_none()
        .child(chip.compact(true).mono(false).label(tag_text(tag)))
        .into_any_element()
}

/// The tag's short words for a list row, in the current language.
pub(super) fn short_tag_text(tag: TgTag) -> String {
    match tag {
        TgTag::LoggedIn => t!("telegram_core.tag_short.logged_in").to_string(),
        TgTag::NotLoggedIn => t!("telegram_core.tag_short.not_logged_in").to_string(),
        TgTag::ProxyError => t!("telegram_core.tag_short.proxy_error").to_string(),
        TgTag::ReaderOff => t!("telegram_core.tag_short.reader_off").to_string(),
        TgTag::NoCore => t!("telegram_core.tag_short.no_core").to_string(),
        TgTag::Unsupported => t!("telegram_core.tag_short.unsupported").to_string(),
        TgTag::Unknown => UNKNOWN_TAG.to_string(),
    }
}

/// Tone of a tag's words in a list row.
fn tag_tone(tag: TgTag) -> MoonTone {
    match tag {
        TgTag::LoggedIn => MoonTone::Positive,
        TgTag::NotLoggedIn => MoonTone::Warning,
        TgTag::ProxyError | TgTag::NoCore => MoonTone::Negative,
        TgTag::ReaderOff | TgTag::Unsupported | TgTag::Unknown => MoonTone::Muted,
    }
}

/// The tag in a list row: short tone-coloured words, the full meaning on hover.
pub(super) fn list_tag_el(id: u64, tag: TgTag, cx: &App) -> AnyElement {
    div()
        .id(("tgcore-tag", id))
        .debug_selector(move || format!("tgcore-tag-{id}"))
        .flex_none()
        .whitespace_nowrap()
        .font_family(design::ui_font())
        .text_color(rgba_from(tag_tone(tag).color(MoonPalette::active(cx)), 1.0))
        .when(tag != TgTag::Unknown, |el| {
            el.tooltip(text_tooltip_value(tag_text(tag)))
        })
        .child(short_tag_text(tag))
        .into_any_element()
}

/// Connection dot color of a list row.
pub(super) fn dot_color(status: Option<&ConnStatus>, p: MoonPalette) -> u32 {
    match status {
        Some(ConnStatus::Ready) => p.green,
        Some(ConnStatus::Connecting | ConnStatus::Stage(_)) => p.amber,
        _ => p.red,
    }
}

/// One core in the list: dot, name (mono, truncated), state tag.
///
/// Args:
///     id: Core id, for element ids and probe selectors.
///     name: Core name as configured.
///     dot: [`dot_color`].
///     tag: The core's tag.
///     selected: Whether this is the picked core.
///     cx: Application context.
pub(super) fn core_row(
    id: u64,
    name: String,
    dot: u32,
    tag: TgTag,
    selected: bool,
    cx: &App,
) -> Stateful<Div> {
    let p = MoonPalette::active(cx);
    h_flex()
        .id(("tgcore-row", id))
        .debug_selector(move || format!("tgcore-row-{id}"))
        .w_full()
        .gap(design::ui_px(cx, 8.0))
        .items_center()
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 6.0))
        .cursor_pointer()
        .when(selected, |el| el.bg(rgba_from(p.blue, 0.16)))
        .when(!selected, |el| {
            el.hover(|s| s.bg(rgba_from(p.panel_high, 1.0)))
        })
        .child(
            div()
                .debug_selector(move || format!("tgcore-dot-{id}"))
                .flex_none()
                .w(design::ui_px(cx, 8.0))
                .h(design::ui_px(cx, 8.0))
                .rounded_full()
                .bg(rgb(dot)),
        )
        .child(
            div()
                .debug_selector(move || format!("tgcore-name-{id}"))
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(design::mono())
                .child(name),
        )
        .child(list_tag_el(id, tag, cx))
}

/// The list frame: a caption header over the rows.
pub(super) fn core_list(rows: Vec<AnyElement>, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let faint = rgba_from(p.text_muted, 1.0);
    v_flex()
        .w_full()
        .debug_selector(|| "tgcore-list".into())
        .border_1()
        .border_color(rgba_from(p.border, 1.0))
        .rounded(design::r_container(cx))
        .overflow_hidden()
        .child(
            h_flex()
                .w_full()
                .gap(design::ui_px(cx, 8.0))
                .px(design::ui_px(cx, 12.0))
                .py(design::ui_px(cx, 6.0))
                .border_b_1()
                .border_color(rgba_from(p.border, 1.0))
                .font_family(design::ui_font())
                .text_color(faint)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(t!("telegram_core.list_core").to_string()),
                )
                .child(
                    div()
                        .flex_none()
                        .child(t!("telegram_core.list_telegram").to_string()),
                ),
        )
        .children(rows)
        .into_any_element()
}

/// Side by side when the narrowest list and the card's minimum both fit `column_w`; stacked
/// otherwise.
pub(super) fn split_side(column_w: f32, cx: &App) -> bool {
    column_w + 0.5
        >= design::font_w(cx, LIST_MIN_W)
            + design::ui_value(cx, SPLIT_GAP)
            + design::font_w(cx, CARD_MIN_W)
}

/// Place the list and the card in a page column `column_w` wide that takes the page body's
/// height.
///
/// Side by side, the list fills the column's height and scrolls inside it; it gives way from
/// [`LIST_W`] down to [`LIST_MIN_W`] before the card goes under [`CARD_MIN_W`], and the card keeps
/// its natural height, scrolling on its own only when the window is lower than it. Stacked, the
/// list is capped at [`LIST_MAX_H`] and the column scrolls as a whole.
pub(super) fn split(list: AnyElement, card: AnyElement, column_w: f32, cx: &App) -> AnyElement {
    let gap = design::ui_px(cx, SPLIT_GAP);
    if !split_side(column_w, cx) {
        return v_flex()
            .id("tgcore-stack")
            .w(px(column_w))
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .gap(gap)
            .child(
                div()
                    .id("tgcore-list-scroll")
                    .debug_selector(|| "tgcore-list-scroll".into())
                    .flex_none()
                    .w_full()
                    .max_h(design::ui_px(cx, LIST_MAX_H))
                    .overflow_y_scroll()
                    .child(list),
            )
            .child(div().flex_none().w_full().child(card))
            .into_any_element();
    }
    h_flex()
        .w(px(column_w))
        .flex_1()
        .min_h(px(0.0))
        .items_start()
        .gap(gap)
        .child(
            div()
                .id("tgcore-list-scroll")
                .debug_selector(|| "tgcore-list-scroll".into())
                .flex_basis(design::font_w_px(cx, LIST_W))
                .flex_shrink_1()
                .min_w(design::font_w_px(cx, LIST_MIN_W))
                .h_full()
                .overflow_y_scroll()
                .child(list),
        )
        .child(
            div()
                .id("tgcore-card-scroll")
                .flex_1()
                .min_w(design::font_w_px(cx, CARD_MIN_W))
                .max_h_full()
                .overflow_y_scroll()
                .child(card),
        )
        .into_any_element()
}

/// The card's frame.
pub(super) fn card(children: Vec<AnyElement>, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    v_flex()
        .debug_selector(|| "tgcore-card".into())
        .w_full()
        .min_w_0()
        .gap(design::ui_px(cx, 10.0))
        .px(design::ui_px(cx, 16.0))
        .py(design::ui_px(cx, 12.0))
        .border_1()
        .border_color(rgba_from(p.border, 1.0))
        .rounded(design::r_container(cx))
        .children(children)
        .into_any_element()
}

/// Card header: name, tag, then the reader toggle pushed to the right when given.
pub(super) fn card_header(
    name: String,
    tag: TgTag,
    toggle: Option<AnyElement>,
    cx: &App,
) -> AnyElement {
    h_flex()
        .debug_selector(|| "tgcore-head".into())
        .w_full()
        .flex_wrap()
        .items_center()
        .gap(design::ui_px(cx, 10.0))
        .child(
            div()
                .debug_selector(|| "tgcore-head-name".into())
                .min_w_0()
                .truncate()
                .font_family(design::mono())
                .font_weight(FontWeight::SEMIBOLD)
                .child(name),
        )
        .child(tag_el(tag, "tgcore-head-tag".into()))
        .child(div().flex_1())
        .children(toggle.map(|t| {
            div()
                .debug_selector(|| "tgcore-head-toggle".into())
                .flex_none()
                .child(t)
                .into_any_element()
        }))
        .into_any_element()
}

/// One labelled row of the card body: caption column, then the value, a hairline above.
pub(super) fn card_row(
    key: &'static str,
    label: String,
    value: AnyElement,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    h_flex()
        .debug_selector(move || format!("tgcore-r-{key}"))
        .w_full()
        .flex_wrap()
        .items_start()
        .gap_x(design::ui_px(cx, 16.0))
        .gap_y(design::ui_px(cx, 4.0))
        .py(design::ui_px(cx, 8.0))
        .border_t_1()
        .border_color(rgba_from(p.border, 0.6))
        .child(
            div()
                .debug_selector(move || format!("tgcore-l-{key}"))
                .flex_none()
                .w(design::font_w_px(cx, LABEL_W))
                .font_family(design::ui_font())
                .child(label),
        )
        .child(
            div()
                .debug_selector(move || format!("tgcore-v-{key}"))
                .flex_1()
                .min_w(design::font_w_px(cx, 180.0))
                .child(value),
        )
        .into_any_element()
}

/// Centred empty state of a disconnected core, with its reconnect button.
pub(super) fn disconnected(reconnect: AnyElement, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    v_flex()
        .debug_selector(|| "tgcore-empty".into())
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .px(design::ui_px(cx, 16.0))
        .py(design::ui_px(cx, 28.0))
        .border_1()
        .border_dashed()
        .border_color(rgba_from(p.border, 1.0))
        .rounded(design::r_container(cx))
        .font_family(design::ui_font())
        .child(
            div()
                .debug_selector(|| "tgcore-empty-title".into())
                .font_weight(FontWeight::SEMIBOLD)
                .child(t!("telegram_core.disconnected").to_string()),
        )
        .child(
            div()
                .debug_selector(|| "tgcore-empty-hint".into())
                .w_full()
                .text_center()
                .text_color(rgba_from(p.text_muted, 1.0))
                .child(t!("telegram_core.disconnected_hint").to_string()),
        )
        .child(reconnect)
        .into_any_element()
}

/// A plain bordered note: the unsupported-service text, or "pick a core".
pub(super) fn note_box(selector: &'static str, text: String, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    div()
        .debug_selector(move || selector.into())
        .w_full()
        .px(design::ui_px(cx, 16.0))
        .py(design::ui_px(cx, 14.0))
        .border_1()
        .border_color(rgba_from(p.border, 1.0))
        .rounded(design::r_container(cx))
        .font_family(design::ui_font())
        .text_color(rgba_from(p.text_muted, 1.0))
        .child(text)
        .into_any_element()
}

/// The red banner at the top of the card: a bold lead, then the message.
pub(super) fn error_banner(
    selector: String,
    lead: String,
    message: String,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    div()
        .debug_selector(move || selector.clone())
        .w_full()
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 10.0))
        .border_1()
        .border_color(rgba_from(p.red, 1.0))
        .bg(rgba_from(p.red, 0.12))
        .rounded(design::r_container(cx))
        .font_family(design::ui_font())
        .child(
            div()
                .text_color(rgba_from(p.red, 1.0))
                .font_weight(FontWeight::SEMIBOLD)
                .child(format!("{lead}:")),
        )
        .child(div().font_family(design::mono()).child(message))
        .into_any_element()
}

#[cfg(test)]
mod tests;
